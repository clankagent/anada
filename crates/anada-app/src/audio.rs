//! Explicit-endpoint WASAPI bridge. Adaptive band-limited sinc resampling uses
//! preallocated 64-frame chunks. Queues hold 200 ms per input and prime at 50 ms.
//! Protection is a clamp, not a limiter.

use crate::model::Config;
use anada_dsp::{EqSettings, StereoEq, sanitize_output};
use anada_windows::WindowsSnapshot;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rtrb::{Consumer, Producer, RingBuffer};
use rubato::{
    Resampler, SincFixedOut, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

type Frame = [f32; 2];
const RESAMPLE_CHUNK: usize = 64;
const RETRY_DELAY: Duration = Duration::from_secs(2);

#[derive(Clone, PartialEq)]
struct Binding {
    channel_id: String,
    playback: String,
    capture: String,
    eq: EqSettings,
}

#[derive(Clone, PartialEq)]
struct Plan {
    output: String,
    output_eq: EqSettings,
    inputs: Vec<Binding>,
}

impl Plan {
    fn same_streams(&self, other: &Self) -> bool {
        self.output == other.output
            && self.inputs.len() == other.inputs.len()
            && self.inputs.iter().zip(&other.inputs).all(|(a, b)| {
                a.channel_id == b.channel_id && a.playback == b.playback && a.capture == b.capture
            })
    }
}

// Prepared on the control thread; returned there for destruction. The callback
// only moves owned banks through bounded SPSC queues and runs existing filters.
struct EqBank {
    inputs: Vec<StereoEq>,
    output: StereoEq,
}

impl EqBank {
    fn new(plan: &Plan, rate: u32) -> Result<Self, String> {
        Ok(Self {
            inputs: plan
                .inputs
                .iter()
                .map(|input| StereoEq::new(&input.eq, rate))
                .collect::<Result<_, _>>()?,
            output: StereoEq::new(&plan.output_eq, rate)?,
        })
    }
}

struct EqControl {
    pending: Producer<EqBank>,
    retired: Consumer<EqBank>,
}

impl EqControl {
    fn update(&mut self, plan: &Plan, rate: u32) -> Result<bool, String> {
        while let Ok(retired) = self.retired.pop() {
            drop(retired);
        }
        if self.pending.is_full() {
            return Ok(false);
        }
        Ok(self.pending.push(EqBank::new(plan, rate)?).is_ok())
    }
}

struct LiveEq {
    current: EqBank,
    previous: Option<EqBank>,
    pending: Consumer<EqBank>,
    retired: Producer<EqBank>,
    remaining: usize,
    duration: usize,
}

impl LiveEq {
    fn prepare(&mut self) {
        if self.remaining == 0 && self.previous.is_some() {
            if self.retired.is_full() {
                return;
            }
            let previous = self.previous.take().expect("previous bank exists");
            if let Err(rtrb::PushError::Full(previous)) = self.retired.push(previous) {
                self.previous = Some(previous);
                return;
            }
        }
        if self.previous.is_none()
            && let Ok(next) = self.pending.pop()
        {
            self.previous = Some(std::mem::replace(&mut self.current, next));
            self.remaining = self.duration;
        }
    }

    fn reset_input(&mut self, index: usize) {
        self.current.inputs[index].reset();
        if let Some(previous) = &mut self.previous {
            previous.inputs[index].reset();
        }
    }

    fn input(&mut self, index: usize, frame: Frame) -> (Frame, Frame) {
        let mut current = frame;
        self.current.inputs[index].process_interleaved(&mut current);
        let mut old = frame;
        if self.remaining > 0
            && let Some(previous) = &mut self.previous
        {
            previous.inputs[index].process_interleaved(&mut old);
        }
        (current, old)
    }

    fn output(&mut self, mut current: Frame, mut old: Frame) -> Frame {
        self.current.output.process_interleaved(&mut current);
        if self.remaining == 0 {
            return current;
        }
        if let Some(previous) = &mut self.previous {
            previous.output.process_interleaved(&mut old);
        }
        let weight = 1.0 - self.remaining as f64 / self.duration as f64;
        self.remaining -= 1;
        interpolate(old, current, weight)
    }
}

fn plan(config: &Config, windows: &WindowsSnapshot, output: Option<&str>) -> Result<Plan, String> {
    config.validate()?;
    let output = output.ok_or("No listening output is selected; audio is stopped")?;
    if !windows
        .outputs
        .iter()
        .any(|endpoint| endpoint.id == output && !endpoint.is_virtual)
    {
        return Err("The selected physical output is unavailable; audio is stopped".into());
    }
    let mut inputs = Vec::new();
    for channel in &config.channels {
        let (playback, capture) = match (&channel.endpoint_id, &channel.capture_id) {
            (None, None) => continue,
            (Some(playback), Some(capture)) => (playback, capture),
            _ => {
                return Err(format!(
                    "{} needs an explicit virtual playback/capture pair",
                    channel.name
                ));
            }
        };
        if !windows
            .outputs
            .iter()
            .any(|endpoint| endpoint.id == *playback && endpoint.is_virtual)
            || !windows
                .inputs
                .iter()
                .any(|endpoint| endpoint.id == *capture && endpoint.is_virtual)
        {
            return Err(format!(
                "{}: bound virtual playback/capture endpoint is unavailable; audio is stopped",
                channel.name
            ));
        }
        inputs.push(Binding {
            channel_id: channel.id.clone(),
            playback: playback.clone(),
            capture: capture.clone(),
            eq: channel.eq.clone(),
        });
    }
    if inputs.is_empty() {
        return Err(
            "No virtual playback/capture pairs are bound; no audio streams are open".into(),
        );
    }
    Ok(Plan {
        output: output.into(),
        output_eq: config.output_eq.get(output).cloned().unwrap_or_default(),
        inputs,
    })
}

#[derive(Default)]
struct Health {
    fault: AtomicU32,
    backend_xruns: AtomicU64,
    dropped: AtomicU64,
    underruns: AtomicU64,
    clamped: AtomicU64,
    invalid: AtomicU64,
    output_frames: AtomicU64,
    callback_overruns: AtomicU64,
    callback_max_ns: AtomicU64,
}

impl Health {
    fn fail(&self, code: u32) {
        let _ = self
            .fault
            .compare_exchange(0, code, Ordering::Relaxed, Ordering::Relaxed);
    }

    fn error(&self, error: cpal::Error) {
        // CPAL 0.18.2 WASAPI reports capture DATA_DISCONTINUITY here, then
        // continues delivering usable packets on the same stream. Do not turn
        // that recoverable glitch (including on an idle input) into a global
        // fault and the two-second full-mixer retry. Existing bounded queues
        // and per-input underrun recovery handle any actual missing samples.
        if error.kind() == cpal::ErrorKind::Xrun {
            self.backend_xruns.fetch_add(1, Ordering::Relaxed);
            return;
        }
        self.fail(match error.kind() {
            cpal::ErrorKind::DeviceNotAvailable => 1,
            cpal::ErrorKind::StreamInvalidated => 2,
            cpal::ErrorKind::PermissionDenied => 3,
            _ => 5,
        });
    }
}

struct Running {
    output: cpal::Stream,
    inputs: Vec<cpal::Stream>,
    health: Arc<Health>,
    rate: u32,
    eq: EqControl,
    meters: Arc<SignalMeters>,
    spectra: Vec<crate::spectrum::Analyzer>,
    output_spectrum: crate::spectrum::Analyzer,
}

#[derive(Default)]
struct AtomicPeak([AtomicU32; 2]);

impl AtomicPeak {
    fn publish(&self, peak: Frame) {
        for (value, sample) in self.0.iter().zip(peak) {
            // Nonnegative finite IEEE floats have the same ordering as u32.
            value.fetch_max(sample.to_bits(), Ordering::Relaxed);
        }
    }

    fn take(&self) -> Frame {
        self.0
            .each_ref()
            .map(|value| f32::from_bits(value.swap(0, Ordering::Relaxed)))
    }
}

struct SignalMeters {
    channels: Vec<(String, AtomicPeak)>,
    output: AtomicPeak,
}

impl SignalMeters {
    fn new(plan: &Plan) -> Self {
        Self {
            channels: plan
                .inputs
                .iter()
                .map(|binding| (binding.channel_id.clone(), AtomicPeak::default()))
                .collect(),
            output: AtomicPeak::default(),
        }
    }

    fn take(&self) -> crate::model::AudioMeters {
        crate::model::AudioMeters {
            running: true,
            channels: self
                .channels
                .iter()
                .map(|(id, peak)| crate::model::ChannelMeter {
                    channel_id: id.clone(),
                    peak: peak.take(),
                    spectrum: Default::default(),
                })
                .collect(),
            output_peak: self.output.take(),
            output_spectrum: Default::default(),
        }
    }
}

fn accumulate_peak(peak: &mut Frame, frame: Frame) {
    for (peak, sample) in peak.iter_mut().zip(frame) {
        if sample.is_finite() {
            *peak = peak.max(sample.abs());
        }
    }
}

pub struct AudioEngine {
    running: Option<Running>,
    requested: Option<Plan>,
    retry_at: Option<Instant>,
    last_error: String,
    stream_opens: u64,
    stream_faults: u64,
}

impl Default for AudioEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioEngine {
    pub fn meters(&mut self) -> crate::model::AudioMeters {
        self.running
            .as_mut()
            .filter(|running| running.health.fault.load(Ordering::Relaxed) == 0)
            .map(|running| {
                let mut meters = running.meters.take();
                for (meter, analyzer) in meters.channels.iter_mut().zip(&mut running.spectra) {
                    meter.spectrum = analyzer.snapshot();
                }
                meters.output_spectrum = running.output_spectrum.snapshot();
                meters
            })
            .unwrap_or_default()
    }
    pub fn diagnostics(&self) -> crate::model::AudioDiagnostics {
        let mut diagnostics = crate::model::AudioDiagnostics {
            stream_opens: self.stream_opens,
            stream_faults: self.stream_faults,
            ..Default::default()
        };
        if let Some(running) = &self.running {
            let health = &running.health;
            diagnostics.sample_rate = running.rate;
            diagnostics.output_frames = health.output_frames.load(Ordering::Relaxed);
            diagnostics.backend_xruns = health.backend_xruns.load(Ordering::Relaxed);
            diagnostics.dropped_frames = health.dropped.load(Ordering::Relaxed);
            diagnostics.underruns = health.underruns.load(Ordering::Relaxed);
            diagnostics.clamped_samples = health.clamped.load(Ordering::Relaxed);
            diagnostics.invalid_inputs = health.invalid.load(Ordering::Relaxed);
            diagnostics.callback_overruns = health.callback_overruns.load(Ordering::Relaxed);
            diagnostics.callback_max_ns = health.callback_max_ns.load(Ordering::Relaxed);
        }
        diagnostics
    }
    pub fn new() -> Self {
        Self {
            running: None,
            requested: None,
            retry_at: None,
            last_error: String::new(),
            stream_opens: 0,
            stream_faults: 0,
        }
    }

    pub(crate) fn stop(&mut self) {
        if let Some(running) = self.running.take() {
            running.health.fail(6);
            drop(running);
        }
    }

    pub fn reconcile(
        &mut self,
        config: &Config,
        windows: &WindowsSnapshot,
        selected_output: Option<&str>,
    ) -> (bool, String) {
        let requested = match plan(config, windows, selected_output) {
            Ok(requested) => requested,
            Err(error) => {
                self.stop();
                self.requested = None;
                self.retry_at = None;
                return (false, error);
            }
        };
        if self
            .requested
            .as_ref()
            .is_none_or(|previous| !previous.same_streams(&requested))
        {
            self.stop();
            self.requested = Some(requested.clone());
            self.retry_at = None;
        }
        if let Some(running) = &mut self.running {
            while let Ok(retired) = running.eq.retired.pop() {
                drop(retired);
            }
            if self.requested.as_ref() != Some(&requested) {
                match running.eq.update(&requested, running.rate) {
                    Ok(true) => self.requested = Some(requested.clone()),
                    Ok(false) => {} // Retry the latest configuration on reconciliation.
                    Err(error) => {
                        return (true, format!("Audio running; EQ update rejected: {error}"));
                    }
                }
            }
            let fault = running.health.fault.load(Ordering::Relaxed);
            if fault == 0 {
                let health = &running.health;
                return (
                    true,
                    format!(
                        "{} bound virtual input(s), stereo {} Hz; streams started (not an end-to-end signal check). Adaptive band-limited sinc resampling; hard safety clamp, not a limiter. Frames {}; dropped {}; underruns {}; backend xruns {}; clamped samples {}; invalid input samples {}; stream opens {}; stream faults {}; callback overruns {}; max callback elapsed {:.3} ms",
                        running.inputs.len(),
                        running.rate,
                        health.output_frames.load(Ordering::Relaxed),
                        health.dropped.load(Ordering::Relaxed),
                        health.underruns.load(Ordering::Relaxed),
                        health.backend_xruns.load(Ordering::Relaxed),
                        health.clamped.load(Ordering::Relaxed),
                        health.invalid.load(Ordering::Relaxed),
                        self.stream_opens,
                        self.stream_faults,
                        health.callback_overruns.load(Ordering::Relaxed),
                        health.callback_max_ns.load(Ordering::Relaxed) as f64 / 1_000_000.0,
                    ),
                );
            }
            self.last_error = match fault {
                1 => "An audio endpoint was lost",
                2 => "An audio stream was invalidated",
                3 => "Audio endpoint access was denied",
                7 => "The audio backend returned an incomplete frame",
                _ => "An audio stream failed",
            }
            .into();
            self.stream_faults += 1;
            self.stop();
            self.retry_at = Some(Instant::now() + RETRY_DELAY);
        }
        if self.retry_at.is_some_and(|retry| Instant::now() < retry) {
            return (
                false,
                format!("{}; audio stopped, retry pending", self.last_error),
            );
        }
        match open(&requested) {
            Ok(running) => {
                self.stream_opens += 1;
                self.running = Some(running);
                self.requested = Some(requested.clone());
                self.retry_at = None;
                self.reconcile(config, windows, selected_output)
            }
            Err(error) => {
                self.last_error = error;
                self.retry_at = Some(Instant::now() + RETRY_DELAY);
                (
                    false,
                    format!(
                        "{}; no audio streams are open, retry pending",
                        self.last_error
                    ),
                )
            }
        }
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        self.stop();
    }
}

fn exact_device(
    devices: impl Iterator<Item = cpal::Device>,
    raw_id: &str,
) -> Result<cpal::Device, String> {
    let mut matching = devices.filter(|device| device.id().is_ok_and(|id| id.id() == raw_id));
    let device = matching
        .next()
        .ok_or_else(|| format!("Bound audio endpoint {raw_id} is unavailable in CPAL"))?;
    if matching.next().is_some() {
        return Err("Audio endpoint identity is ambiguous; refusing to open streams".into());
    }
    Ok(device)
}

#[cfg(windows)]
fn audio_host() -> Result<cpal::Host, String> {
    cpal::host_from_id(cpal::HostId::Wasapi).map_err(|error| error.to_string())
}

#[cfg(not(windows))]
fn audio_host() -> Result<cpal::Host, String> {
    Err("The endpoint-ID bridge currently supports Windows WASAPI only".into())
}

fn open(plan: &Plan) -> Result<Running, String> {
    let host = audio_host()?;
    let output = exact_device(
        host.output_devices().map_err(|error| error.to_string())?,
        &plan.output,
    )?;
    let output_config = output
        .default_output_config()
        .map_err(|error| format!("Output format: {error}"))?;
    let output_rate = output_config.sample_rate();
    if output_config.channels() != 2 || !(44_100..=192_000).contains(&output_rate) {
        return Err("Selected output must have a native stereo format at 44.1-192 kHz".into());
    }
    let health = Arc::new(Health::default());
    let mut streams = Vec::with_capacity(plan.inputs.len());
    let mut mixer_inputs = Vec::with_capacity(plan.inputs.len());
    for binding in &plan.inputs {
        let device = exact_device(
            host.input_devices().map_err(|error| error.to_string())?,
            &binding.capture,
        )?;
        let config = device
            .default_input_config()
            .map_err(|error| format!("Capture format: {error}"))?;
        let rate = config.sample_rate();
        if !(1..=2).contains(&config.channels()) || !(8_000..=192_000).contains(&rate) {
            return Err("Bound virtual capture must have one or two channels at 8-192 kHz".into());
        }
        let (producer, consumer) = RingBuffer::new(rate as usize / 5);
        let reset = Arc::new(AtomicBool::new(false));
        streams.push(capture_stream(
            &device,
            config,
            producer,
            reset.clone(),
            health.clone(),
        )?);
        mixer_inputs.push(MixerInput {
            resampler: Interpolator::new(consumer, rate, output_rate, rate as usize / 20)?,
            reset,
        });
    }
    let (pending, updates) = RingBuffer::new(1);
    let (retired, returned) = RingBuffer::new(1);
    let meters = Arc::new(SignalMeters::new(plan));
    let (spectrum_feeds, spectra) = (0..plan.inputs.len())
        .map(|_| crate::spectrum::pair(output_rate))
        .unzip();
    let (output_spectrum_feed, output_spectrum) = crate::spectrum::pair(output_rate);
    let mixer = Mixer {
        spectrum_feeds,
        output_spectrum_feed,
        meters: meters.clone(),
        input_peaks: vec![[0.0; 2]; plan.inputs.len()],
        output_peak: [0.0; 2],
        inputs: mixer_inputs,
        eq: LiveEq {
            current: EqBank::new(plan, output_rate)?,
            previous: None,
            pending: updates,
            retired,
            remaining: 0,
            duration: (output_rate as usize / 50).max(1),
        },
        health: health.clone(),
    };
    let output = output_stream(&output, output_config, mixer)?;
    let running = Running {
        spectra,
        output_spectrum,
        meters,
        output,
        inputs: streams,
        health,
        rate: output_rate,
        eq: EqControl {
            pending,
            retired: returned,
        },
    };
    for stream in &running.inputs {
        stream
            .play()
            .map_err(|error| format!("Starting virtual capture: {error}"))?;
    }
    running
        .output
        .play()
        .map_err(|error| format!("Starting selected output: {error}"))?;
    Ok(running)
}

fn capture_stream(
    device: &cpal::Device,
    config: cpal::SupportedStreamConfig,
    producer: Producer<Frame>,
    reset: Arc<AtomicBool>,
    health: Arc<Health>,
) -> Result<cpal::Stream, String> {
    macro_rules! capture {
        ($sample:ty) => {
            build_capture::<$sample>(device, config.config(), producer, reset, health)
        };
    }
    match config.sample_format() {
        cpal::SampleFormat::F32 => capture!(f32),
        cpal::SampleFormat::F64 => capture!(f64),
        cpal::SampleFormat::I8 => capture!(i8),
        cpal::SampleFormat::I16 => capture!(i16),
        cpal::SampleFormat::I24 => capture!(cpal::I24),
        cpal::SampleFormat::I32 => capture!(i32),
        cpal::SampleFormat::I64 => capture!(i64),
        cpal::SampleFormat::U8 => capture!(u8),
        cpal::SampleFormat::U16 => capture!(u16),
        cpal::SampleFormat::U32 => capture!(u32),
        cpal::SampleFormat::U64 => capture!(u64),
        format => Err(format!(
            "Unsupported virtual capture sample format: {format:?}"
        )),
    }
}

fn input_sample<T: cpal::Sample>(sample: T) -> f32
where
    f32: cpal::FromSample<T>,
{
    let sample = <f32 as cpal::FromSample<T>>::from_sample_(sample);
    if sample.is_finite() { sample } else { 0.0 }
}

fn build_capture<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mut producer: Producer<Frame>,
    reset: Arc<AtomicBool>,
    health: Arc<Health>,
) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    let channels = config.channels as usize;
    let errors = health.clone();
    device
        .build_input_stream(
            config,
            move |data: &[T], _| {
                if health.fault.load(Ordering::Relaxed) != 0 {
                    return;
                }
                if !data.len().is_multiple_of(channels) {
                    health.fail(7);
                    return;
                }
                let mut dropped = 0;
                let mut invalid = 0;
                for frame in data.chunks_exact(channels) {
                    let left = <f32 as cpal::FromSample<T>>::from_sample_(frame[0]);
                    let right = <f32 as cpal::FromSample<T>>::from_sample_(frame[channels - 1]);
                    invalid += u64::from(!left.is_finite()) + u64::from(!right.is_finite());
                    if producer
                        .push([input_sample::<f32>(left), input_sample::<f32>(right)])
                        .is_err()
                    {
                        dropped += 1;
                    }
                }
                if dropped != 0 {
                    health.dropped.fetch_add(dropped, Ordering::Relaxed);
                    reset.store(true, Ordering::Release);
                }
                health.invalid.fetch_add(invalid, Ordering::Relaxed);
            },
            move |error| errors.error(error),
            Some(Duration::from_secs(2)),
        )
        .map_err(|error| format!("Opening bound virtual capture: {error}"))
}

struct Drift {
    nominal: f64,
    filtered_error: f64,
    target: usize,
    output_rate: u32,
}

impl Drift {
    fn new(input_rate: u32, output_rate: u32, target: usize) -> Self {
        Self {
            nominal: f64::from(input_rate) / f64::from(output_rate),
            filtered_error: 0.0,
            target,
            output_rate,
        }
    }

    fn update(&mut self, occupancy: usize, output_frames: usize) -> f64 {
        let seconds = output_frames as f64 / f64::from(self.output_rate);
        let alpha = 1.0 - (-seconds / 0.5).exp();
        let error = (occupancy as f64 - self.target as f64) / self.target.max(1) as f64;
        self.filtered_error += alpha * (error - self.filtered_error);
        self.nominal * (1.0 + (self.filtered_error * 0.01).clamp(-0.005, 0.005))
    }
}

struct Interpolator {
    consumer: Consumer<Frame>,
    drift: Drift,
    sinc: SincFixedOut<f32>,
    input: Vec<Vec<f32>>,
    output: Vec<Vec<f32>>,
    cursor: usize,
    ready: usize,
    step: f64,
    primed: bool,
}

impl Interpolator {
    fn new(
        consumer: Consumer<Frame>,
        input_rate: u32,
        output_rate: u32,
        target: usize,
    ) -> Result<Self, String> {
        if !(8_000..=192_000).contains(&input_rate) || !(44_100..=192_000).contains(&output_rate) {
            return Err("Resampler rates are outside the supported range".into());
        }
        let drift = Drift::new(input_rate, output_rate, target);
        // Scale filter length when downsampling to preserve transition width.
        // 0.95 leaves margin for the full +/-5000 ppm correction, whose ratio
        // changes never rebuild rubato's anti-alias filter.
        let taps = 256 * (drift.nominal.ceil() as usize).max(1);
        let sinc = SincFixedOut::<f32>::new(
            1.0 / drift.nominal,
            1.01,
            SincInterpolationParameters {
                sinc_len: taps,
                f_cutoff: 0.95,
                interpolation: SincInterpolationType::Cubic,
                oversampling_factor: 256,
                window: WindowFunction::BlackmanHarris2,
            },
            RESAMPLE_CHUNK,
            2,
        )
        .map_err(|error| format!("Preparing sinc resampler: {error}"))?;
        let input = sinc.input_buffer_allocate(true);
        let output = sinc.output_buffer_allocate(true);
        Ok(Self {
            consumer,
            step: drift.nominal,
            drift,
            sinc,
            input,
            output,
            cursor: 0,
            ready: 0,
            primed: false,
        })
    }

    fn reset(&mut self, discard: bool) {
        if discard {
            for _ in 0..self.consumer.slots() {
                let _ = self.consumer.pop();
            }
        }
        self.primed = false;
        self.cursor = 0;
        self.ready = 0;
        self.sinc.reset();
        self.drift.filtered_error = 0.0;
        self.step = self.drift.nominal;
    }

    fn prepare(&mut self, frames: usize) {
        if self.primed {
            // Include queued output's source-frame equivalent in occupancy.
            let queued = ((self.ready - self.cursor) as f64 * self.step) as usize;
            self.step = self.drift.update(self.consumer.slots() + queued, frames);
        }
    }

    fn next(&mut self) -> (Frame, bool) {
        if self.cursor == self.ready {
            if self.sinc.set_resample_ratio(1.0 / self.step, true).is_err() {
                self.reset(true);
                return ([0.0; 2], true);
            }
            let needed = self.sinc.input_frames_next();
            if !self.primed && self.consumer.slots() < self.drift.target.max(needed) {
                return ([0.0; 2], false);
            }
            if self.consumer.slots() < needed {
                self.reset(false);
                return ([0.0; 2], true);
            }
            for index in 0..needed {
                // This is the only consumer, and availability was checked above.
                let frame = self.consumer.pop().unwrap_or([0.0; 2]);
                self.input[0][index] = input_sample(frame[0]);
                self.input[1][index] = input_sample(frame[1]);
            }
            let result = self
                .sinc
                .process_into_buffer(&self.input, &mut self.output, None);
            match result {
                Ok((_, ready)) if ready > 0 && ready <= RESAMPLE_CHUNK => {
                    self.cursor = 0;
                    self.ready = ready;
                    self.primed = true;
                }
                _ => {
                    self.reset(true);
                    return ([0.0; 2], true);
                }
            }
        }
        let frame = [self.output[0][self.cursor], self.output[1][self.cursor]];
        self.cursor += 1;
        (frame.map(input_sample), false)
    }
}
fn interpolate(left: Frame, right: Frame, phase: f64) -> Frame {
    std::array::from_fn(|channel| {
        (f64::from(left[channel]) * (1.0 - phase) + f64::from(right[channel]) * phase) as f32
    })
}

struct MixerInput {
    resampler: Interpolator,
    reset: Arc<AtomicBool>,
}

struct Mixer {
    spectrum_feeds: Vec<crate::spectrum::Feed>,
    output_spectrum_feed: crate::spectrum::Feed,
    meters: Arc<SignalMeters>,
    input_peaks: Vec<Frame>,
    output_peak: Frame,
    inputs: Vec<MixerInput>,
    eq: LiveEq,
    health: Arc<Health>,
}

impl Mixer {
    fn render<T: cpal::SizedSample + cpal::FromSample<f32>>(
        &mut self,
        data: &mut [T],
        sample_rate: u32,
    ) {
        let callback_started = Instant::now();
        let silence = <T as cpal::FromSample<f32>>::from_sample_(0.0);
        if !data.len().is_multiple_of(2) {
            self.health.fail(7);
        }
        if self.health.fault.load(Ordering::Relaxed) != 0 {
            data.fill(silence);
            return;
        }
        self.prepare(data.len() / 2);
        for output in data.as_chunks_mut::<2>().0 {
            if self.health.fault.load(Ordering::Relaxed) != 0 {
                output.fill(silence);
            } else {
                let frame = self.next();
                output[0] = <T as cpal::FromSample<f32>>::from_sample_(frame[0]);
                output[1] = <T as cpal::FromSample<f32>>::from_sample_(frame[1]);
            }
        }
        self.publish_meters();
        self.health
            .output_frames
            .fetch_add((data.len() / 2) as u64, Ordering::Relaxed);
        let elapsed_ns = callback_started
            .elapsed()
            .as_nanos()
            .min(u128::from(u64::MAX)) as u64;
        self.health
            .callback_max_ns
            .fetch_max(elapsed_ns, Ordering::Relaxed);
        let budget_ns = (data.len() / 2) as u64 * 1_000_000_000 / u64::from(sample_rate);
        if budget_ns > 0 && elapsed_ns > budget_ns {
            self.health
                .callback_overruns
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    fn publish_meters(&mut self) {
        for ((_, meter), peak) in self.meters.channels.iter().zip(&mut self.input_peaks) {
            meter.publish(*peak);
            *peak = [0.0; 2];
        }
        self.meters.output.publish(self.output_peak);
        self.output_peak = [0.0; 2];
    }
    fn prepare(&mut self, frames: usize) {
        self.eq.prepare();
        for (index, input) in self.inputs.iter_mut().enumerate() {
            if input.reset.swap(false, Ordering::AcqRel) {
                input.resampler.reset(true);
                self.eq.reset_input(index);
            }
            input.resampler.prepare(frames);
        }
    }

    fn next(&mut self) -> Frame {
        let mut sum = [0.0_f64; 2];
        let mut old_sum = [0.0_f64; 2];
        for (index, input) in self.inputs.iter_mut().enumerate() {
            let (frame, underrun) = input.resampler.next();
            let (frame, old) = self.eq.input(index, frame);
            let audible = if self.eq.remaining > 0 {
                interpolate(
                    old,
                    frame,
                    1.0 - self.eq.remaining as f64 / self.eq.duration as f64,
                )
            } else {
                frame
            };
            accumulate_peak(&mut self.input_peaks[index], audible);
            self.spectrum_feeds[index].push(audible);
            if underrun {
                self.eq.reset_input(index);
                self.health.underruns.fetch_add(1, Ordering::Relaxed);
            }
            for channel in 0..2 {
                sum[channel] += f64::from(frame[channel]);
                old_sum[channel] += f64::from(old[channel]);
            }
        }
        let mut frame =
            sum.map(|sample| sample.clamp(-f64::from(f32::MAX), f64::from(f32::MAX)) as f32);
        let old =
            old_sum.map(|sample| sample.clamp(-f64::from(f32::MAX), f64::from(f32::MAX)) as f32);
        frame = self.eq.output(frame, old);
        let changed = sanitize_output(&mut frame);
        accumulate_peak(&mut self.output_peak, frame);
        self.output_spectrum_feed.push(frame);
        self.health
            .clamped
            .fetch_add(changed as u64, Ordering::Relaxed);
        frame
    }
}

fn output_stream(
    device: &cpal::Device,
    config: cpal::SupportedStreamConfig,
    mixer: Mixer,
) -> Result<cpal::Stream, String> {
    macro_rules! output {
        ($sample:ty) => {
            build_output::<$sample>(device, config.config(), mixer)
        };
    }
    match config.sample_format() {
        cpal::SampleFormat::F32 => output!(f32),
        cpal::SampleFormat::F64 => output!(f64),
        cpal::SampleFormat::I8 => output!(i8),
        cpal::SampleFormat::I16 => output!(i16),
        cpal::SampleFormat::I24 => output!(cpal::I24),
        cpal::SampleFormat::I32 => output!(i32),
        cpal::SampleFormat::I64 => output!(i64),
        cpal::SampleFormat::U8 => output!(u8),
        cpal::SampleFormat::U16 => output!(u16),
        cpal::SampleFormat::U32 => output!(u32),
        cpal::SampleFormat::U64 => output!(u64),
        format => Err(format!(
            "Unsupported physical output sample format: {format:?}"
        )),
    }
}

fn build_output<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mut mixer: Mixer,
) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let errors = mixer.health.clone();
    let sample_rate = config.sample_rate;
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _| {
                mixer.render(data, sample_rate);
            },
            move |error| errors.error(error),
            Some(Duration::from_secs(2)),
        )
        .map_err(|error| format!("Opening selected physical output: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use anada_windows::Endpoint;

    #[test]
    fn conversion_preserves_pcm_scale_and_rejects_nonfinite() {
        assert_eq!(input_sample(i16::MIN), -1.0);
        assert_eq!(input_sample(0_i16), 0.0);
        assert!(input_sample(i16::MAX) < 1.0);
        assert_eq!(input_sample(32_768_u16), 0.0);
        assert_eq!(input_sample(f32::NAN), 0.0);
        assert_eq!(input_sample(f64::INFINITY), 0.0);
        assert_eq!(input_sample(2.0_f32), 2.0);
        let packed = <cpal::I24 as cpal::FromSample<f32>>::from_sample_(-1.0);
        assert_eq!(input_sample(packed), -1.0);
    }

    #[test]
    fn linear_interpolation_preserves_stereo_and_endpoints() {
        assert_eq!(interpolate([0.0, 1.0], [1.0, -1.0], 0.25), [0.25, 0.5]);
        assert_eq!(interpolate([-1.0, 1.0], [1.0, -1.0], 0.0), [-1.0, 1.0]);
        assert_eq!(interpolate([-1.0, 1.0], [1.0, -1.0], 1.0), [1.0, -1.0]);
    }

    fn resampled_sine(input_rate: u32, output_rate: u32, frequency: f64) -> (Vec<Frame>, usize) {
        let count = input_rate as usize;
        let (mut producer, consumer) = RingBuffer::new(count);
        for index in 0..count {
            let sample = (std::f64::consts::TAU * frequency * index as f64 / f64::from(input_rate))
                .sin() as f32
                * 0.25;
            producer.push([sample, -sample]).unwrap();
        }
        let mut converter = Interpolator::new(consumer, input_rate, output_rate, 2).unwrap();
        let delay = converter.sinc.output_delay();
        let result = (0..output_rate / 2)
            .map(|_| {
                let (frame, underrun) = converter.next();
                assert!(!underrun);
                assert!(frame.iter().all(|sample| sample.is_finite()));
                assert!((frame[0] + frame[1]).abs() < 1e-6);
                frame
            })
            .collect();
        (result, delay)
    }

    fn rms(frames: &[Frame]) -> f64 {
        (frames
            .iter()
            .map(|frame| f64::from(frame[0]).powi(2))
            .sum::<f64>()
            / frames.len() as f64)
            .sqrt()
    }

    #[test]
    fn sinc_preserves_passband_and_stereo_across_supported_rates() {
        for (input_rate, output_rate, frequency) in [
            (8_000, 192_000, 1_000.0),
            (44_100, 48_000, 1_000.0),
            (48_000, 44_100, 20_000.0),
            (96_000, 48_000, 20_000.0),
            (192_000, 44_100, 20_000.0),
            (48_000, 192_000, 20_000.0),
        ] {
            let (samples, delay) = resampled_sine(input_rate, output_rate, frequency);
            let amplitude = rms(&samples[output_rate as usize / 10..]);
            let reference = 0.25 / 2.0_f64.sqrt();
            let gain_db = 20.0 * (amplitude / reference).log10();
            println!(
                "sinc {input_rate}->{output_rate} {frequency}Hz passband {gain_db:.4}dB; filter delay {delay} output frames, chunk {RESAMPLE_CHUNK}"
            );
            assert!(gain_db.abs() < 0.2, "passband {gain_db}dB");
        }
    }

    #[test]
    fn sinc_rejects_aliases_where_linear_folds_them_into_audible_band() {
        for (input_rate, output_rate, frequency) in [
            (48_000, 44_100, 23_000.0),
            (96_000, 48_000, 30_000.0),
            (192_000, 44_100, 23_500.0),
            (192_000, 48_000, 70_000.0),
        ] {
            let (samples, _) = resampled_sine(input_rate, output_rate, frequency);
            let start = output_rate as usize / 10;
            let sinc_rms = rms(&samples[start..]);
            // Independent direct evaluation of the previous two-point algorithm.
            let linear: Vec<Frame> = (start..samples.len())
                .map(|n| {
                    let position = n as f64 * f64::from(input_rate) / f64::from(output_rate);
                    let base = position.floor();
                    let sample = |index: f64| {
                        (std::f64::consts::TAU * frequency * index / f64::from(input_rate)).sin()
                            as f32
                            * 0.25
                    };
                    interpolate([sample(base); 2], [sample(base + 1.0); 2], position - base)
                })
                .collect();
            let linear_rms = rms(&linear);
            let improvement_db = 20.0 * (linear_rms / sinc_rms.max(1e-20)).log10();
            let rejection_db = 20.0 * (sinc_rms.max(1e-20) / (0.25 / 2.0_f64.sqrt())).log10();
            println!(
                "alias {input_rate}->{output_rate} {frequency}Hz: linear RMS {linear_rms:.8}, sinc RMS {sinc_rms:.8}, improvement {improvement_db:.2}dB, source rejection {rejection_db:.2}dB"
            );
            assert!(linear_rms > 0.02);
            assert!(
                improvement_db > 60.0,
                "only {improvement_db}dB alias improvement"
            );
            assert!(rejection_db < -60.0);
        }
    }

    #[test]
    fn sinc_drift_extremes_keep_preallocated_buffer_bounds_and_reject_aliases() {
        for correction in [0.995, 1.005] {
            let (mut producer, consumer) = RingBuffer::new(32_768);
            for n in 0..32_768 {
                let sample =
                    (std::f64::consts::TAU * 30_000.0 * n as f64 / 96_000.0).sin() as f32 * 0.25;
                producer.push([sample, -sample]).unwrap();
            }
            let mut converter = Interpolator::new(consumer, 96_000, 48_000, 2).unwrap();
            let input_pointers: Vec<_> = converter
                .input
                .iter()
                .map(|channel| (channel.as_ptr(), channel.len(), channel.capacity()))
                .collect();
            let output_pointers: Vec<_> = converter
                .output
                .iter()
                .map(|channel| (channel.as_ptr(), channel.len(), channel.capacity()))
                .collect();
            converter.step *= correction;
            let mut energy = 0.0_f64;
            for n in 0..8_192 {
                let (frame, underrun) = converter.next();
                assert!(!underrun);
                assert!(frame.iter().all(|sample| sample.is_finite()));
                if n >= 4_096 {
                    energy += f64::from(frame[0]).powi(2);
                }
            }
            let amplitude = (energy / 4_096.0).sqrt();
            assert!(amplitude < 0.0001, "alias RMS {amplitude} at {correction}");
            converter.reset(false);
            for (channel, before) in converter.input.iter().zip(input_pointers) {
                assert_eq!(
                    (channel.as_ptr(), channel.len(), channel.capacity()),
                    before
                );
            }
            for (channel, before) in converter.output.iter().zip(output_pointers) {
                assert_eq!(
                    (channel.as_ptr(), channel.len(), channel.capacity()),
                    before
                );
            }
        }
    }
    #[cfg(windows)]
    #[test]
    fn raw_windows_id_is_not_the_prefixed_display_id() {
        let raw = "{0.0.1.00000000}.{00000000-0000-0000-0000-000000000001}";
        let id = cpal::DeviceId::new(cpal::HostId::Wasapi, raw);
        assert_eq!(id.id(), raw);
        assert_ne!(id.to_string(), raw);
    }

    #[test]
    fn drift_is_directional_and_bounded() {
        let mut drift = Drift::new(44_100, 48_000, 2_205);
        let nominal = 44_100.0 / 48_000.0;
        assert_eq!(drift.update(2_205, 480), nominal);
        for _ in 0..1_000 {
            assert!(drift.update(100_000, 480) <= nominal * 1.005);
        }
        assert!(drift.update(100_000, 480) > nominal);
        let mut drift = Drift::new(48_000, 48_000, 2_400);
        for _ in 0..1_000 {
            assert!(drift.update(0, 480) >= 0.995);
        }
        assert!(drift.update(0, 480) < 1.0);
    }

    #[test]
    fn resampler_primes_restarts_and_never_replays_stale_frames() {
        let (mut producer, consumer) = RingBuffer::new(8_192);
        let mut resampler = Interpolator::new(consumer, 48_000, 48_000, 256).unwrap();
        assert_eq!(resampler.next(), ([0.0; 2], false));
        for _ in 0..4_096 {
            producer.push([9.0; 2]).unwrap();
        }
        resampler.reset(true);
        assert_eq!(resampler.consumer.slots(), 0);
        assert_eq!(resampler.next(), ([0.0; 2], false));
        for _ in 0..4_096 {
            producer.push([0.25; 2]).unwrap();
        }
        for _ in 0..1_024 {
            resampler.next();
        }
        assert!((resampler.next().0[0] - 0.25).abs() < 1e-5);
        let mut underruns = 0;
        for _ in 0..8_192 {
            underruns += usize::from(resampler.next().1);
        }
        assert_eq!(underruns, 1);
        assert!(!resampler.primed);
        assert_eq!(resampler.next(), ([0.0; 2], false));
        for _ in 0..4_096 {
            producer.push([f32::NAN, f32::INFINITY]).unwrap();
        }
        for _ in 0..1_024 {
            assert_eq!(resampler.next().0, [0.0; 2]);
        }
    }
    #[test]
    fn drift_tracks_independent_clocks_without_overflow_or_underflow() {
        for ppm in [-1_000.0, -100.0, 100.0, 1_000.0] {
            let (mut producer, consumer) = RingBuffer::new(9_600);
            for _ in 0..2_402 {
                producer.push([0.25, -0.25]).unwrap();
            }
            let mut resampler = Interpolator::new(consumer, 48_000, 48_000, 2_400).unwrap();
            let mut fractional = 0.0;
            // Ten seconds per independent clock is enough to exercise the
            // 0.5-second drift-control time constant repeatedly.
            for block in 0..1_000 {
                fractional += 480.0 * (1.0 + ppm / 1_000_000.0);
                let frames = fractional as usize;
                fractional -= frames as f64;
                for _ in 0..frames {
                    producer.push([0.25, -0.25]).unwrap();
                }
                resampler.prepare(480);
                for _ in 0..480 {
                    let (frame, underrun) = resampler.next();
                    assert!(!underrun, "ppm {ppm}");
                    assert!(frame.iter().all(|sample| sample.is_finite()));
                    assert!((frame[0] + frame[1]).abs() < 1e-6);
                    if block > 1 {
                        assert!((frame[0] - 0.25).abs() < 1e-5);
                    }
                }
            }
            assert!(
                (1_500..3_300).contains(&resampler.consumer.slots()),
                "ppm {ppm}"
            );
        }
    }

    fn endpoint(id: &str, virtual_device: bool) -> Endpoint {
        Endpoint {
            level_db: None,
            id: id.into(),
            name: "Not used for identity".into(),
            volume: 0.5,
            muted: false,
            is_virtual: virtual_device,
        }
    }

    fn fixture() -> (Config, WindowsSnapshot) {
        let mut config = Config::default();
        config.channels[0].endpoint_id = Some("virtual-playback".into());
        config.channels[0].capture_id = Some("virtual-capture".into());
        let windows = WindowsSnapshot {
            outputs: vec![
                endpoint("physical", false),
                endpoint("virtual-playback", true),
            ],
            inputs: vec![endpoint("virtual-capture", true), endpoint("mic", false)],
            sessions: Vec::new(),
            default_output_id: Some("physical".into()),
            communications_output_id: None,
        };
        (config, windows)
    }

    #[test]
    fn synthetic_mixer_meters_are_stereo_isolated_post_eq_and_post_clamp() {
        let (config, windows) = fixture();
        let mut plan = plan(&config, &windows, Some("physical")).unwrap();
        plan.inputs[0].eq = EqSettings {
            bypass: true,
            ..Default::default()
        };
        let mut second = plan.inputs[0].clone();
        second.channel_id = "chat".into();
        plan.inputs.push(second);
        plan.output_eq = EqSettings {
            bypass: true,
            ..Default::default()
        };
        let meters = Arc::new(SignalMeters::new(&plan));
        let (_control, eq) = live_fixture(&plan);
        let inputs = [[0.5, -0.25], [0.75, 0.0]]
            .into_iter()
            .map(|frame| {
                let (_producer, consumer) = RingBuffer::new(4096);
                let mut resampler = Interpolator::new(consumer, 48_000, 48_000, 2400).unwrap();
                // Supply prepared resampler output, so this exercises the same mixer
                // path as the callback without opening an audio device.
                resampler.output[0][0] = frame[0];
                resampler.output[1][0] = frame[1];
                resampler.ready = 1;
                MixerInput {
                    resampler,
                    reset: Arc::new(AtomicBool::new(false)),
                }
            })
            .collect();
        let mut mixer = Mixer {
            spectrum_feeds: (0..2).map(|_| crate::spectrum::pair(48_000).0).collect(),
            output_spectrum_feed: crate::spectrum::pair(48_000).0,
            inputs,
            eq,
            health: Arc::new(Health::default()),
            meters: meters.clone(),
            input_peaks: vec![[0.0; 2]; 2],
            output_peak: [0.0; 2],
        };
        assert_eq!(mixer.next(), [1.0, -0.25]);
        mixer.publish_meters();
        let measured = meters.take();
        assert_eq!(measured.channels[0].channel_id, "gaming");
        assert_eq!(measured.channels[0].peak, [0.5, 0.25]);
        assert_eq!(measured.channels[1].peak, [0.75, 0.0]);
        assert_eq!(measured.output_peak, [1.0, 0.25]);
        assert_eq!(meters.take().output_peak, [0.0; 2]);
        assert_eq!(meters.take().channels[0].peak, [0.0; 2]);
        // An underrun/empty prepared buffer emits silence and cannot replay a peak.
        assert_eq!(mixer.next(), [0.0; 2]);
        mixer.publish_meters();
        assert_eq!(meters.take().channels[0].peak, [0.0; 2]);
        // New stream generations own fresh atomics, never retired signal peaks.
        meters.output.publish([1.0; 2]);
        assert_eq!(SignalMeters::new(&plan).take().output_peak, [0.0; 2]);
        assert!(!AudioEngine::new().meters().running);
    }

    #[test]
    fn capture_discontinuities_do_not_silence_the_mix_but_device_loss_does() {
        let (config, windows) = fixture();
        let mut plan = plan(&config, &windows, Some("physical")).unwrap();
        let mut idle = plan.inputs[0].clone();
        idle.channel_id = "idle".into();
        plan.inputs.push(idle);
        let (_control, eq) = live_fixture(&plan);
        let meters = Arc::new(SignalMeters::new(&plan));
        let health = Arc::new(Health::default());
        let mut producers = Vec::new();
        let inputs = (0..2)
            .map(|_| {
                let (producer, consumer) = RingBuffer::new(9_600);
                producers.push(producer);
                MixerInput {
                    resampler: Interpolator::new(consumer, 48_000, 48_000, 2_400).unwrap(),
                    reset: Arc::new(AtomicBool::new(false)),
                }
            })
            .collect();
        let mut mixer = Mixer {
            spectrum_feeds: (0..2).map(|_| crate::spectrum::pair(48_000).0).collect(),
            output_spectrum_feed: crate::spectrum::pair(48_000).0,
            meters: meters.clone(),
            input_peaks: vec![[0.0; 2]; 2],
            output_peak: [0.0; 2],
            inputs,
            eq,
            health: health.clone(),
        };
        for (producer, signal) in producers.iter_mut().zip([[0.25, -0.125], [0.0; 2]]) {
            for _ in 0..2_880 {
                producer.push(signal).unwrap();
            }
        }
        let mut output = [0.0_f32; 960];
        mixer.render(&mut output, 48_000);
        let _ = meters.take();
        for block in 0..20 {
            for (producer, signal) in producers.iter_mut().zip([[0.25, -0.125], [0.0; 2]]) {
                for _ in 0..480 {
                    producer.push(signal).unwrap();
                }
            }
            if block % 2 == 0 {
                // CPAL reports this before delivering the next usable capture
                // packet. Even a report from an idle input used to kill all audio.
                health.error(cpal::ErrorKind::Xrun.into());
            }
            mixer.render(&mut output, 48_000);
            assert!(
                output.as_chunks::<2>().0.iter().all(|frame| {
                    (frame[0] - 0.25).abs() < 1e-5 && (frame[1] + 0.125).abs() < 1e-5
                }),
                "a recoverable capture discontinuity silenced output block {block}"
            );
            let measured = meters.take();
            assert!(measured.output_peak[0] > 0.24);
            assert!(measured.channels[0].peak[0] > 0.24);
            assert_eq!(measured.channels[1].peak, [0.0; 2]);
        }
        assert_eq!(health.fault.load(Ordering::Relaxed), 0);
        assert_eq!(health.backend_xruns.load(Ordering::Relaxed), 10);
        assert_eq!(health.underruns.load(Ordering::Relaxed), 0);
        health.error(cpal::ErrorKind::DeviceNotAvailable.into());
        health.error(cpal::ErrorKind::Xrun.into());
        output.fill(1.0);
        mixer.render(&mut output, 48_000);
        assert_eq!(output, [0.0; 960]);
        assert_eq!(health.fault.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn peak_accumulator_ignores_nonfinite_and_preserves_short_transients() {
        let mut peak = [0.0; 2];
        accumulate_peak(&mut peak, [0.5, -0.25]);
        accumulate_peak(&mut peak, [f32::NAN, f32::INFINITY]);
        let meter = AtomicPeak::default();
        meter.publish(peak);
        meter.publish([0.1, 0.1]);
        assert_eq!(meter.take(), [0.5, 0.25]);
        assert_eq!(meter.take(), [0.0; 2]);
        assert!((20.0 * 0.5_f32.log10() + 6.0206).abs() < 0.0001);
    }

    #[test]
    fn bindings_fail_closed_without_default_or_microphone_fallback() {
        let (mut config, windows) = fixture();
        assert!(plan(&config, &windows, Some("physical")).is_ok());
        assert!(plan(&config, &windows, None).is_err());
        assert!(plan(&config, &windows, Some("virtual-playback")).is_err());
        config.channels[0].capture_id = Some("mic".into());
        assert!(plan(&config, &windows, Some("physical")).is_err());
        config.channels[0].capture_id = None;
        assert!(plan(&config, &windows, Some("physical")).is_err());
        config.channels[0].endpoint_id = None;
        assert!(plan(&config, &windows, Some("physical")).is_err());
    }

    #[test]
    fn endpoint_volume_and_mute_do_not_change_the_stream_plan() {
        let (mut config, mut windows) = fixture();
        let original = plan(&config, &windows, Some("physical")).unwrap();
        windows.outputs[0].volume = 0.0;
        windows.outputs[0].muted = true;
        windows.inputs[0].volume = 0.25;
        config.eq_graph_range_db = 12;
        assert!(original == plan(&config, &windows, Some("physical")).unwrap());
    }

    #[test]
    fn eq_edits_keep_stream_topology_but_device_changes_do_not() {
        let (mut config, windows) = fixture();
        let original = plan(&config, &windows, Some("physical")).unwrap();
        config.channels[0].eq.preamp_db = -12.0;
        config.output_eq.insert(
            "physical".into(),
            EqSettings {
                bypass: true,
                ..Default::default()
            },
        );
        let changed = plan(&config, &windows, Some("physical")).unwrap();
        assert!(original != changed);
        assert!(original.same_streams(&changed));
        let mut moved = changed;
        moved.inputs[0].capture = "different".into();
        assert!(!original.same_streams(&moved));
        moved.output = "different-output".into();
        assert!(!original.same_streams(&moved));
    }

    fn live_fixture(plan: &Plan) -> (EqControl, LiveEq) {
        let (pending, updates) = RingBuffer::new(1);
        let (retired, returned) = RingBuffer::new(1);
        (
            EqControl {
                pending,
                retired: returned,
            },
            LiveEq {
                current: EqBank::new(plan, 48_000).unwrap(),
                previous: None,
                pending: updates,
                retired,
                remaining: 0,
                duration: 960,
            },
        )
    }

    fn process_eq(eq: &mut LiveEq) -> Frame {
        let (current, old) = eq.input(0, [0.25, -0.25]);
        eq.output(current, old)
    }

    #[test]
    fn prepared_eq_crossfade_is_bounded_finite_and_retires_off_callback() {
        let (config, windows) = fixture();
        let mut plan = plan(&config, &windows, Some("physical")).unwrap();
        let (mut control, mut live) = live_fixture(&plan);
        plan.inputs[0].eq.preamp_db = -12.0;
        plan.output_eq.preamp_db = -6.0;
        assert!(control.update(&plan, 48_000).unwrap());
        live.prepare();
        assert_eq!(process_eq(&mut live), [0.25, -0.25]);
        let mut previous = 0.25;
        for _ in 1..960 {
            let value = process_eq(&mut live);
            assert!(value.iter().all(|sample| sample.is_finite()));
            assert!(value[0] <= previous && previous - value[0] < 0.001);
            previous = value[0];
        }
        assert!(live.previous.is_some()); // Completion does not destroy the old bank.
        assert_eq!(control.retired.slots(), 0);
        live.prepare();
        assert!(live.previous.is_none());
        assert_eq!(control.retired.slots(), 1); // Control owns destruction.
        let expected = 0.25 * 10.0_f32.powf(-18.0 / 20.0);
        assert!((process_eq(&mut live)[0] - expected).abs() < 0.000001);
    }

    #[test]
    fn rapid_eq_updates_retry_latest_without_dropping_retired_banks() {
        let (config, windows) = fixture();
        let mut plan = plan(&config, &windows, Some("physical")).unwrap();
        let (mut control, mut live) = live_fixture(&plan);
        plan.inputs[0].eq.preamp_db = -3.0;
        assert!(control.update(&plan, 48_000).unwrap());
        plan.inputs[0].eq.preamp_db = -6.0;
        assert!(!control.update(&plan, 48_000).unwrap());
        live.prepare();
        assert!(control.update(&plan, 48_000).unwrap());
        for _ in 0..960 {
            process_eq(&mut live);
        }
        live.prepare(); // Return first bank, begin second transition.
        for _ in 0..960 {
            process_eq(&mut live);
        }
        live.prepare(); // Return queue is full: retain ownership, never drop here.
        assert!(live.previous.is_some());
        plan.inputs[0].eq.preamp_db = -24.0;
        assert!(control.update(&plan, 48_000).unwrap()); // Drains old banks.
        live.prepare();
        for _ in 0..960 {
            assert!(process_eq(&mut live)[0].is_finite());
        }
        let expected = 0.25 * 10.0_f32.powf(-24.0 / 20.0);
        assert!((process_eq(&mut live)[0] - expected).abs() < 0.000001);
        live.prepare();
        assert!(live.previous.is_some()); // Still held until control drains.
    }

    #[test]
    fn filter_kind_bypass_and_high_q_updates_stay_finite() {
        let (config, windows) = fixture();
        let mut plan = plan(&config, &windows, Some("physical")).unwrap();
        let (mut control, mut live) = live_fixture(&plan);
        for kind in [
            anada_dsp::FilterKind::Bell,
            anada_dsp::FilterKind::LowShelf,
            anada_dsp::FilterKind::HighShelf,
        ] {
            plan.inputs[0].eq.bands[0].kind = kind;
            plan.inputs[0].eq.bands[0].q = 20.0;
            plan.inputs[0].eq.bands[0].gain_db = 24.0;
            plan.inputs[0].eq.bands[0].frequency_hz = 20.0;
            plan.output_eq = plan.inputs[0].eq.clone();
            assert!(control.update(&plan, 48_000).unwrap());
            live.prepare();
            for n in 0..2_000 {
                let input = if n == 0 { [1.0, -1.0] } else { [0.0; 2] };
                let (new, old) = live.input(0, input);
                assert!(
                    live.output(new, old)
                        .iter()
                        .all(|sample| sample.is_finite())
                );
            }
            live.prepare();
        }
        plan.inputs[0].eq.bypass = true;
        plan.output_eq.bypass = true;
        assert!(control.update(&plan, 48_000).unwrap());
        live.prepare();
        for _ in 0..960 {
            process_eq(&mut live);
        }
        assert_eq!(process_eq(&mut live), [0.25, -0.25]);
    }
}
