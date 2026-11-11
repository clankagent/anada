use std::f64::consts::TAU;

const SIGNAL_PEAK: f64 = 0.03162277660168379;

fn validate_host_interval(
    before: &anada_app::model::HostState,
    after: &anada_app::model::HostState,
    playback: &str,
    physical: &str,
) -> Result<(), String> {
    for state in [before, after] {
        if !state.audio_running
            || state.selected_output_id.as_deref() != Some(physical)
            || !state.config.channels.iter().any(|channel| {
                channel.endpoint_id.as_deref() == Some(playback) && channel.capture_id.is_some()
            })
        {
            return Err(
                "Host path was stopped, selected another output, or lacked the source binding"
                    .into(),
            );
        }
    }
    let a = &before.audio_diagnostics;
    let b = &after.audio_diagnostics;
    if before.host_pid != after.host_pid
        || a.stream_opens != b.stream_opens
        || a.stream_faults != b.stream_faults
        || a.sample_rate == 0
        || a.sample_rate != b.sample_rate
        || b.output_frames <= a.output_frames
    {
        return Err("Host restarted, reopened/faulted its streams, changed format, or made no output progress".into());
    }
    for (label, start, end) in [
        ("dropped frames", a.dropped_frames, b.dropped_frames),
        ("underruns", a.underruns, b.underruns),
        (
            "callback overruns",
            a.callback_overruns,
            b.callback_overruns,
        ),
        ("invalid inputs", a.invalid_inputs, b.invalid_inputs),
    ] {
        if start != end {
            return Err(format!(
                "Host {label} changed during measurement: {start} -> {end}"
            ));
        }
    }
    Ok(())
}

#[derive(Default)]
struct SignalWindow {
    squares: f64,
    frames: u64,
    first: Option<f64>,
    last: Option<f64>,
    crossings: u64,
}

#[derive(Default)]
struct Measurement {
    frames: u64,
    sum_squares: f64,
    peak: f64,
    matrix: [[f64; 3]; 3],
    vector: [f64; 3],
    previous_sample: f64,
    crossings: u64,
    first_crossing: Option<f64>,
    last_crossing: Option<f64>,
    windows: Vec<SignalWindow>,
}

impl Measurement {
    fn add(&mut self, sample: f64, phase: f64) {
        let window = self.frames as usize / 4096;
        if window == self.windows.len() {
            self.windows.push(SignalWindow::default());
        }
        self.windows[window].squares += sample * sample;
        self.windows[window].frames += 1;
        if self.frames > 0 && self.previous_sample <= 0.0 && sample > 0.0 {
            let crossing =
                (self.frames - 1) as f64 - self.previous_sample / (sample - self.previous_sample);
            self.first_crossing.get_or_insert(crossing);
            self.last_crossing = Some(crossing);
            self.crossings += 1;
            let window = &mut self.windows[window];
            window.first.get_or_insert(crossing);
            window.last = Some(crossing);
            window.crossings += 1;
        }
        self.previous_sample = sample;
        let basis = [phase.sin(), phase.cos(), 1.0];
        self.frames += 1;
        self.sum_squares += sample * sample;
        self.peak = self.peak.max(sample.abs());
        for row in 0..3 {
            self.vector[row] += basis[row] * sample;
            for column in 0..3 {
                self.matrix[row][column] += basis[row] * basis[column];
            }
        }
    }

    fn tone_peak(&self) -> Result<f64, String> {
        if self.frames == 0 {
            return Err("No loopback samples were measured".into());
        }
        let mut matrix = self.matrix;
        let mut vector = self.vector;
        for column in 0..3 {
            let pivot = (column..3)
                .max_by(|left, right| {
                    matrix[*left][column]
                        .abs()
                        .total_cmp(&matrix[*right][column].abs())
                })
                .unwrap();
            matrix.swap(column, pivot);
            vector.swap(column, pivot);
            let divisor = matrix[column][column];
            if divisor.abs() < self.frames as f64 * 1e-12 {
                return Err("Tone fit is singular; use a higher frequency".into());
            }
            for entry in &mut matrix[column][column..] {
                *entry /= divisor;
            }
            vector[column] /= divisor;
            for row in 0..3 {
                if row == column {
                    continue;
                }
                let factor = matrix[row][column];
                let pivot_row = matrix[column];
                for (entry, pivot_entry) in
                    matrix[row][column..].iter_mut().zip(&pivot_row[column..])
                {
                    *entry -= factor * pivot_entry;
                }
                vector[row] -= factor * vector[column];
            }
        }
        Ok(vector[0].hypot(vector[1]))
    }

    fn rms(&self) -> f64 {
        (self.sum_squares / self.frames as f64).sqrt()
    }

    fn observed_frequency(&self, sample_rate: u32) -> Option<f64> {
        let span = self.last_crossing? - self.first_crossing?;
        (self.crossings > 1 && span > 0.0)
            .then(|| (self.crossings - 1) as f64 * f64::from(sample_rate) / span)
    }

    fn signal_validity(&self, rate: u32, expected: f64) -> Result<(), String> {
        let observed = self
            .observed_frequency(rate)
            .ok_or("Silence or insufficient crossings; gain is unknown")?;
        if (observed / expected - 1.0).abs() > 0.01 {
            return Err(format!(
                "Observed frequency {observed:.3} Hz differs from requested {expected:.3} Hz"
            ));
        }
        let rms = self.rms();
        for window in self.windows.iter().filter(|window| window.frames >= 2048) {
            let local_rms = (window.squares / window.frames as f64).sqrt();
            if (local_rms / rms - 1.0).abs() > 0.10 {
                return Err("Nonstationary signal: window RMS varies by more than 10%".into());
            }
            if window.crossings >= 4 {
                let local = (window.crossings - 1) as f64 * f64::from(rate)
                    / (window.last.unwrap() - window.first.unwrap());
                if (local / observed - 1.0).abs() > 0.01 {
                    return Err(
                        "Nonstationary signal: window frequency varies by more than 1%".into(),
                    );
                }
            }
        }
        Ok(())
    }
}

#[cfg(windows)]
mod windows_probe {
    use super::{Measurement, SIGNAL_PEAK, TAU, validate_host_interval};
    use anada_app::{model::Command, transport};
    use anada_windows::AudioSystem;
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use rtrb::{Producer, RingBuffer};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    const MAX_CHANNELS: usize = 8;
    const MAX_RATE: u32 = 384_000;
    const DEADLINE: Duration = Duration::from_secs(4);

    struct Frame {
        index: u64,
        samples: [f32; MAX_CHANNELS],
    }

    #[derive(Default)]
    struct Health {
        armed: AtomicBool,
        output_frames: AtomicU64,
        capture_frames: AtomicU64,
        output_errors: AtomicU64,
        capture_errors: AtomicU64,
        capture_error_kind: AtomicU64,
        warmup_xruns: AtomicU64,
        dropped_frames: AtomicU64,
        malformed_buffers: AtomicU64,
    }

    impl Health {
        fn check(&self) -> Result<(), String> {
            let output = self.output_errors.load(Ordering::Relaxed);
            let capture = self.capture_errors.load(Ordering::Relaxed);
            let dropped = self.dropped_frames.load(Ordering::Relaxed);
            let malformed = self.malformed_buffers.load(Ordering::Relaxed);
            let capture_kind = self.capture_error_kind.load(Ordering::Relaxed);
            if output + capture + dropped + malformed != 0 {
                return Err(format!(
                    "Audio measurement invalid: output stream errors={output}, physical loopback stream errors={capture} (kind {capture_kind}: 1=xrun, 2=device unavailable, 3=invalidated, 4=other), queue dropped frames={dropped}, malformed callback buffers={malformed}. Check endpoint availability and supported shared-mode formats; no capture fallback was attempted."
                ));
            }
            Ok(())
        }
    }

    fn find_output(host: &cpal::Host, raw_id: &str) -> Result<cpal::Device, String> {
        for device in host
            .output_devices()
            .map_err(|error| format!("Enumerate WASAPI outputs: {error}"))?
        {
            if device
                .id()
                .map_err(|error| format!("Read WASAPI output ID: {error}"))?
                .id()
                == raw_id
            {
                return Ok(device);
            }
        }
        Err(format!(
            "No WASAPI output matches exact raw Windows endpoint ID {raw_id:?}"
        ))
    }

    fn validate_format(config: &cpal::SupportedStreamConfig, label: &str) -> Result<(), String> {
        if !matches!(
            config.sample_format(),
            cpal::SampleFormat::F32 | cpal::SampleFormat::I16 | cpal::SampleFormat::U16
        ) {
            return Err(format!(
                "{label}: unsupported default sample format {:?}; supported formats are f32, i16, u16",
                config.sample_format()
            ));
        }
        if !(8_000..=MAX_RATE).contains(&config.sample_rate()) {
            return Err(format!("{label}: sample rate must be 8000..={MAX_RATE} Hz"));
        }
        Ok(())
    }

    fn output_stream<T>(
        device: &cpal::Device,
        config: cpal::StreamConfig,
        frequency: f64,
        health: Arc<Health>,
    ) -> Result<cpal::Stream, String>
    where
        T: cpal::SizedSample + cpal::FromSample<f32>,
    {
        let rate = config.sample_rate as u64;
        let maximum_frames = rate * 3;
        let phase_step = TAU * frequency / rate as f64;
        let mut next_frame = 0_u64;
        let errors = health.clone();
        device
            .build_output_stream(
                config,
                move |data: &mut [T], _| {
                    if !data.len().is_multiple_of(2) {
                        health.malformed_buffers.fetch_add(1, Ordering::Relaxed);
                    }
                    data.fill(T::from_sample(0.0));
                    let mut generated = 0;
                    for frame in data.as_chunks_mut::<2>().0 {
                        if next_frame >= maximum_frames {
                            break;
                        }
                        let ramp = (next_frame as f64 / (rate as f64 * 0.005))
                            .min(1.0)
                            .min((maximum_frames - next_frame) as f64 / (rate as f64 * 0.005));
                        let sample =
                            (SIGNAL_PEAK * ramp * (next_frame as f64 * phase_step).sin()) as f32;
                        frame.fill(T::from_sample(sample));
                        next_frame += 1;
                        generated += 1;
                    }
                    health.output_frames.fetch_add(generated, Ordering::Relaxed);
                    if generated != 0 {
                        health.armed.store(true, Ordering::Release);
                    }
                },
                move |_| {
                    errors.output_errors.fetch_add(1, Ordering::Relaxed);
                },
                Some(DEADLINE),
            )
            .map_err(|error| format!("Build explicit virtual playback stream: {error}"))
    }

    fn capture_stream<T>(
        device: &cpal::Device,
        config: cpal::StreamConfig,
        mut producer: Producer<Frame>,
        health: Arc<Health>,
    ) -> Result<cpal::Stream, String>
    where
        T: cpal::SizedSample,
        f32: cpal::FromSample<T>,
    {
        let channels = config.channels as usize;
        let maximum_frames = config.sample_rate as u64 * 3 / 2;
        let warmup_frames = config.sample_rate as u64 / 2;
        let mut next_frame = 0_u64;
        let errors = health.clone();
        device.build_input_stream(config, move |data: &[T], _| {
            if !health.armed.load(Ordering::Acquire) { return; }
            if !data.len().is_multiple_of(channels) {
                health.malformed_buffers.fetch_add(1, Ordering::Relaxed);
            }
            let mut captured = 0;
            let mut dropped = 0;
            for input in data.chunks_exact(channels) {
                if next_frame >= maximum_frames { break; }
                let mut frame = Frame { index: next_frame, samples: [0.0; MAX_CHANNELS] };
                for (target, sample) in frame.samples.iter_mut().zip(input.iter()) {
                    *target = <f32 as cpal::FromSample<T>>::from_sample_(*sample);
                }
                if producer.push(frame).is_err() { dropped += 1; }
                next_frame += 1;
                captured += 1;
            }
            health.capture_frames.fetch_add(captured, Ordering::Relaxed);
            health.dropped_frames.fetch_add(dropped, Ordering::Relaxed);
        }, move |error| {
            if error.kind() == cpal::ErrorKind::Xrun
                && errors.capture_frames.load(Ordering::Relaxed) < warmup_frames {
                errors.warmup_xruns.fetch_add(1, Ordering::Relaxed);
                return;
            }
            errors.capture_error_kind.store(match error.kind() {
                cpal::ErrorKind::Xrun => 1,
                cpal::ErrorKind::DeviceNotAvailable => 2,
                cpal::ErrorKind::StreamInvalidated => 3,
                _ => 4,
            }, Ordering::Relaxed);
            errors.capture_errors.fetch_add(1, Ordering::Relaxed);
        }, Some(DEADLINE))
            .map_err(|error| format!("Build WASAPI loopback on explicit physical output (no microphone fallback): {error}"))
    }

    fn dbfs(amplitude: f64) -> Option<f64> {
        (amplitude > 0.0).then(|| 20.0 * amplitude.log10())
    }

    pub fn run() -> Result<(), String> {
        let arguments: Vec<String> = std::env::args().skip(1).collect();
        if !(2..=3).contains(&arguments.len()) {
            return Err("Usage: measure_path RAW_PLAYBACK_ENDPOINT_ID RAW_PHYSICAL_OUTPUT_ID [frequency_hz=1000]".into());
        }
        let frequency = arguments
            .get(2)
            .map_or(Ok(1000.0), |value| value.parse::<f64>())
            .map_err(|error| format!("Invalid frequency_hz: {error}"))?;
        if !frequency.is_finite() || frequency < 1.0 {
            return Err("frequency_hz must be finite and at least 1 Hz".into());
        }
        let audio = AudioSystem::new()?;
        let snapshot = audio.snapshot()?;
        let playback = snapshot
            .outputs
            .iter()
            .find(|endpoint| endpoint.id == arguments[0] && endpoint.is_virtual)
            .ok_or("Requested playback ID is not an available virtual Windows output")?;
        let physical = snapshot
            .outputs
            .iter()
            .find(|endpoint| endpoint.id == arguments[1] && !endpoint.is_virtual)
            .ok_or("Requested physical ID is not an available non-virtual Windows output")?;
        let host = cpal::host_from_id(cpal::HostId::Wasapi)
            .map_err(|error| format!("Open WASAPI host: {error}"))?;
        let playback_device = find_output(&host, &playback.id)?;
        let physical_device = find_output(&host, &physical.id)?;
        let playback_config = playback_device
            .default_output_config()
            .map_err(|error| format!("Virtual default output config: {error}"))?;
        let capture_config = physical_device
            .default_output_config()
            .map_err(|error| format!("Physical default output config for loopback: {error}"))?;
        validate_format(&playback_config, "Virtual playback")?;
        validate_format(&capture_config, "Physical loopback")?;
        if playback_config.channels() != 2 {
            return Err("Virtual playback default config must be stereo; no automatic config changes are made".into());
        }
        let channels = capture_config.channels() as usize;
        if !(1..=MAX_CHANNELS).contains(&channels) {
            return Err(format!(
                "Physical loopback supports 1..={MAX_CHANNELS} channels, got {channels}"
            ));
        }
        let playback_rate = playback_config.sample_rate();
        let capture_rate = capture_config.sample_rate();
        if frequency >= f64::from(playback_rate.min(capture_rate)) / 2.0 {
            return Err("frequency_hz must be below Nyquist at both endpoint sample rates".into());
        }
        let warmup_frames = u64::from(capture_rate) / 2;
        let target_frames = u64::from(capture_rate);
        let (producer, mut consumer) = RingBuffer::new(capture_rate as usize / 2);
        let health = Arc::new(Health::default());
        let mut measurements: Vec<Measurement> =
            (0..channels).map(|_| Measurement::default()).collect();
        macro_rules! capture {
            ($sample:ty) => {
                capture_stream::<$sample>(
                    &physical_device,
                    capture_config.config(),
                    producer,
                    health.clone(),
                )?
            };
        }
        let capture = match capture_config.sample_format() {
            cpal::SampleFormat::F32 => capture!(f32),
            cpal::SampleFormat::I16 => capture!(i16),
            cpal::SampleFormat::U16 => capture!(u16),
            _ => unreachable!(),
        };
        macro_rules! output {
            ($sample:ty) => {
                output_stream::<$sample>(
                    &playback_device,
                    playback_config.config(),
                    frequency,
                    health.clone(),
                )?
            };
        }
        let output = match playback_config.sample_format() {
            cpal::SampleFormat::F32 => output!(f32),
            cpal::SampleFormat::I16 => output!(i16),
            cpal::SampleFormat::U16 => output!(u16),
            _ => unreachable!(),
        };
        capture
            .play()
            .map_err(|error| format!("Start explicit physical loopback: {error}"))?;
        let started = Instant::now();
        output
            .play()
            .map_err(|error| format!("Start explicit virtual playback: {error}"))?;
        let mut host_before = None;
        let mut host_after = None;
        let result = (|| -> Result<(), String> {
            let mut expected_index = 0;
            while measurements[0].frames < target_frames {
                health.check()?;
                if started.elapsed() >= DEADLINE {
                    return Err(format!(
                        "Physical loopback timed out: received {} frames, measured {}/{} frames. Check Anada routing and endpoint activity; no microphone/default capture fallback was attempted.",
                        health.capture_frames.load(Ordering::Relaxed),
                        measurements[0].frames,
                        target_frames
                    ));
                }
                let available = consumer.slots();
                for _ in 0..available {
                    let frame = consumer
                        .pop()
                        .map_err(|_| "Loopback queue unexpectedly empty")?;
                    if frame.index != expected_index {
                        return Err("Loopback frame discontinuity; measurement rejected".into());
                    }
                    expected_index += 1;
                    if frame.index < warmup_frames {
                        continue;
                    }
                    if host_before.is_none() {
                        host_before =
                            Some(transport::request(Command::Snapshot).map_err(|error| {
                                format!("Host health unavailable at measurement start: {error}")
                            })?);
                    }
                    let phase = TAU * frequency * (frame.index - warmup_frames) as f64
                        / f64::from(capture_rate);
                    for (measurement, sample) in measurements.iter_mut().zip(frame.samples.iter()) {
                        if !sample.is_finite() {
                            return Err("Physical loopback supplied a non-finite sample; measurement rejected".into());
                        }
                        measurement.add(f64::from(*sample), phase);
                    }
                    if measurements[0].frames == target_frames {
                        break;
                    }
                }
                if available == 0 {
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
            host_after =
                Some(transport::request(Command::Snapshot).map_err(|error| {
                    format!("Host health unavailable at measurement end: {error}")
                })?);
            Ok(())
        })();
        drop(output);
        drop(capture);
        result?;
        health.check()?;
        let before = host_before.ok_or("Host health start was not captured")?;
        let after = host_after.ok_or("Host health end was not captured")?;
        let host_validity = validate_host_interval(&before, &after, &playback.id, &physical.id);
        let required_generated_frames = ((warmup_frames + target_frames)
            * u64::from(playback_rate))
        .div_ceil(u64::from(capture_rate));
        let generated_frames = health.output_frames.load(Ordering::Relaxed);
        if generated_frames < required_generated_frames {
            return Err(format!(
                "Insufficient virtual playback progress: generated {generated_frames}/{required_generated_frames} frames needed to cover warmup and measurement; silence cannot be interpreted as a muted path"
            ));
        }
        let per_channel: Result<Vec<_>, String> = measurements
            .iter()
            .enumerate()
            .map(|(channel, measurement)| {
                let amplitude = measurement.tone_peak()?;
                let validity = measurement.signal_validity(capture_rate, frequency);
                let valid_gain = validity.is_ok() && host_validity.is_ok();
                Ok(serde_json::json!({
                    "channel_index": channel,
                    "frames": measurement.frames,
                    "tone_peak_amplitude": amplitude,
                    "tone_peak_dbfs": dbfs(amplitude),
                    "tone_gain_db": if valid_gain { dbfs(amplitude / SIGNAL_PEAK) } else { None },
                    "rms": measurement.rms(),
                    "rms_gain_db": if valid_gain { dbfs(measurement.rms() / (SIGNAL_PEAK / std::f64::consts::SQRT_2)) } else { None },
                    "gain_valid": valid_gain,
                    "signal_validity_error": validity.err(),
                    "observed_zero_crossing_frequency_hz": measurement.observed_frequency(capture_rate),
                    "peak": measurement.peak
                }))
            })
            .collect();
        let report = serde_json::json!({
            "path_health_valid": host_validity.is_ok(),
            "path_health_error": host_validity.as_ref().err(),
            "host_before": { "pid": before.host_pid, "diagnostics": before.audio_diagnostics },
            "host_after": { "pid": after.host_pid, "diagnostics": after.audio_diagnostics },
            "playback_endpoint_id": playback.id,
            "physical_output_id": physical.id,
            "capture_mode": "wasapi_output_loopback",
            "frequency_hz": frequency,
            "gain_method": "RMS gain assumes an isolated sine. Nominal-frequency tone fit can under-read with adaptive clock correction; inspect observed frequency and RMS together.",
            "generated_peak_amplitude": SIGNAL_PEAK,
            "generated_peak_dbfs": -30.0,
            "playback_sample_rate_hz": playback_rate,
            "capture_sample_rate_hz": capture_rate,
            "playback_sample_format": format!("{:?}", playback_config.sample_format()),
            "capture_sample_format": format!("{:?}", capture_config.sample_format()),
            "playback_channels": 2,
            "capture_channels": channels,
            "warmup_duration_seconds": warmup_frames as f64 / f64::from(capture_rate),
            "measurement_duration_seconds": target_frames as f64 / f64::from(capture_rate),
            "elapsed_wall_seconds": started.elapsed().as_secs_f64(),
            "maximum_generated_duration_seconds": 3.0,
            "generated_frames": health.output_frames.load(Ordering::Relaxed),
            "captured_frames": health.capture_frames.load(Ordering::Relaxed),
            "measured_frames": target_frames,
            "stream_errors": { "output": health.output_errors.load(Ordering::Relaxed), "capture": health.capture_errors.load(Ordering::Relaxed) },
            "queue_dropped_frames": health.dropped_frames.load(Ordering::Relaxed),
            "warmup_xruns": health.warmup_xruns.load(Ordering::Relaxed),
            "channels": per_channel?
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&report)
                .map_err(|error| format!("Serialize measurement: {error}"))?
        );
        host_validity?;
        Ok(())
    }
}

fn main() {
    #[cfg(windows)]
    let result = windows_probe::run();
    #[cfg(not(windows))]
    let result: Result<(), String> =
        Err("measure_path requires Windows WASAPI output loopback".into());
    if let Err(error) = result {
        eprintln!("measure_path: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host_state() -> anada_app::model::HostState {
        let mut config = anada_app::model::Config::default();
        config.channels[0].endpoint_id = Some("source".into());
        config.channels[0].capture_id = Some("capture".into());
        anada_app::model::HostState {
            audio_diagnostics: anada_app::model::AudioDiagnostics {
                sample_rate: 48_000,
                output_frames: 100,
                stream_opens: 1,
                ..Default::default()
            },
            protocol_version: anada_app::model::PROTOCOL_VERSION,
            revision: 1,
            config,
            windows: anada_windows::WindowsSnapshot {
                outputs: Vec::new(),
                inputs: Vec::new(),
                sessions: Vec::new(),
                default_output_id: None,
                communications_output_id: None,
            },
            selected_output_id: Some("physical".into()),
            manual_output: false,
            audio_running: true,
            audio_status: String::new(),
            warnings: Vec::new(),
            host_pid: 42,
            quiet: Default::default(),
        }
    }

    #[test]
    fn host_guard_rejects_internal_faults_even_when_probe_streams_are_healthy() {
        let before = host_state();
        let mut after = before.clone();
        after.audio_diagnostics.output_frames += 48_000;
        assert!(validate_host_interval(&before, &after, "source", "physical").is_ok());
        for mutation in 0..8 {
            let mut bad = after.clone();
            match mutation {
                0 => bad.host_pid += 1,
                1 => bad.audio_diagnostics.stream_opens += 1,
                2 => bad.audio_diagnostics.stream_faults += 1,
                3 => bad.audio_diagnostics.underruns += 1,
                4 => bad.audio_diagnostics.dropped_frames += 1,
                5 => bad.audio_diagnostics.callback_overruns += 1,
                6 => bad.audio_diagnostics.invalid_inputs += 1,
                _ => bad.selected_output_id = Some("wrong".into()),
            }
            assert!(validate_host_interval(&before, &bad, "source", "physical").is_err());
        }
        after.config.channels[0].capture_id = None;
        assert!(validate_host_interval(&before, &after, "source", "physical").is_err());
    }

    #[test]
    fn signal_guard_accepts_stable_drift_and_eq_gain_but_rejects_nonstationary_audio() {
        for gain in [0.1, 1.0, 4.0] {
            let mut stable = Measurement::default();
            let mut interrupted = Measurement::default();
            let mut shifted = Measurement::default();
            for frame in 0..48_000 {
                let phase = TAU * 1000.0 * f64::from(frame) / 48_000.0;
                stable.add(gain * SIGNAL_PEAK * (phase * 1.004).sin(), phase);
                interrupted.add(
                    if (16_000..24_000).contains(&frame) {
                        0.0
                    } else {
                        gain * SIGNAL_PEAK * phase.sin()
                    },
                    phase,
                );
                shifted.add(
                    gain * SIGNAL_PEAK * (phase * if frame < 24_000 { 1.0 } else { 1.2 }).sin(),
                    phase,
                );
            }
            assert!(stable.signal_validity(48_000, 1000.0).is_ok());
            assert!(interrupted.signal_validity(48_000, 1000.0).is_err());
            assert!(shifted.signal_validity(48_000, 1000.0).is_err());
        }
        let mut silence = Measurement::default();
        for _ in 0..48_000 {
            silence.add(0.0, 0.0);
        }
        assert!(silence.signal_validity(48_000, 1000.0).is_err());
        assert_eq!(silence.rms(), 0.0);
    }

    #[test]
    fn tone_fit_is_phase_independent_with_dc_and_fractional_cycles() {
        for phase_offset in [0.0, 0.7, 2.9] {
            let mut measurement = Measurement::default();
            for frame in 0..48_000 {
                let phase = TAU * 1000.37 * f64::from(frame) / 48_000.0;
                measurement.add(SIGNAL_PEAK * (phase + phase_offset).sin() + 0.017, phase);
            }
            assert!((measurement.tone_peak().unwrap() - SIGNAL_PEAK).abs() < 1e-10);
        }
    }

    #[test]
    fn rms_and_crossing_frequency_remain_valid_when_nominal_fit_drifts() {
        let mut measurement = Measurement::default();
        for frame in 0..48_000 {
            let actual_phase = TAU * 1001.0 * f64::from(frame) / 48_000.0;
            let nominal_phase = TAU * 1000.0 * f64::from(frame) / 48_000.0;
            measurement.add(SIGNAL_PEAK * actual_phase.sin(), nominal_phase);
        }
        assert!((measurement.rms() - SIGNAL_PEAK / 2.0_f64.sqrt()).abs() < 1e-10);
        assert!((measurement.observed_frequency(48_000).unwrap() - 1001.0).abs() < 0.01);
        assert!(measurement.tone_peak().unwrap() < SIGNAL_PEAK * 0.01);
    }

    #[test]
    fn silent_samples_measure_zero() {
        let mut measurement = Measurement::default();
        for frame in 0..48_000 {
            measurement.add(0.0, TAU * 1000.0 * f64::from(frame) / 48_000.0);
        }
        assert_eq!(measurement.tone_peak().unwrap(), 0.0);
        assert_eq!(measurement.rms(), 0.0);
        assert_eq!(measurement.peak, 0.0);
    }

    #[test]
    fn empty_and_singular_fits_are_rejected() {
        let mut measurement = Measurement::default();
        assert!(measurement.tone_peak().is_err());
        measurement.add(1.0, 0.0);
        assert!(measurement.tone_peak().is_err());
    }

    #[test]
    fn unity_sine_has_expected_rms_and_peak() {
        let mut measurement = Measurement::default();
        for frame in 0..48_000 {
            let phase = TAU * 1000.0 * f64::from(frame) / 48_000.0;
            measurement.add(SIGNAL_PEAK * phase.sin(), phase);
        }
        assert!((measurement.rms() - SIGNAL_PEAK / 2.0_f64.sqrt()).abs() < 1e-10);
        assert!((measurement.peak - SIGNAL_PEAK).abs() < 1e-10);
    }
}
