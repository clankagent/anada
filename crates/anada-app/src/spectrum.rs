//! Bounded, lossy telemetry. Only `Feed::push` runs in the audio callback.
//! All allocation, windowing and FFT work runs on the host meter-request thread.
use crate::model::Spectrum;
use rtrb::{Consumer, Producer, RingBuffer};
use rustfft::{Fft, FftPlanner, num_complex::Complex32};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

const N: usize = 4096;
const CAPACITY: usize = 8192;
const BINS: usize = 128;
const FLOOR: f32 = -100.0;
const INTERVAL: Duration = Duration::from_millis(50);
const IDLE: Duration = Duration::from_millis(150);

pub(crate) struct Feed {
    queue: Producer<(u64, [f32; 2])>,
    produced: Arc<AtomicU64>,
    sequence: u64,
}
impl Feed {
    pub fn push(&mut self, frame: [f32; 2]) {
        self.sequence = self.sequence.wrapping_add(1);
        let _ = self.queue.push((self.sequence, frame));
        self.produced.store(self.sequence, Ordering::Relaxed);
    }
}

pub(crate) struct Analyzer {
    queue: Consumer<(u64, [f32; 2])>,
    produced: Arc<AtomicU64>,
    rate: u32,
    frames: Vec<[f32; 2]>,
    cursor: usize,
    count: usize,
    previous: Option<u64>,
    observed: u64,
    activity: Instant,
    calculated: Option<Instant>,
    fft: Arc<dyn Fft<f32>>,
    buffer: Vec<Complex32>,
    scratch: Vec<Complex32>,
    window: Vec<f32>,
    normalization: f32,
    powers: Vec<f32>,
    result: Spectrum,
}

pub(crate) fn pair(rate: u32) -> (Feed, Analyzer) {
    let (queue, consumer) = RingBuffer::new(CAPACITY);
    let produced = Arc::new(AtomicU64::new(0));
    let fft = FftPlanner::<f32>::new().plan_fft_forward(N);
    let window: Vec<f32> = (0..N)
        .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / N as f32).cos())
        .collect();
    let normalization = 2.0 / window.iter().sum::<f32>();
    let frequencies_hz = (0..BINS)
        .map(|i| 20.0 * (1000.0_f32).powf(i as f32 / (BINS - 1) as f32))
        .collect();
    let scratch = vec![Complex32::default(); fft.get_inplace_scratch_len()];
    (
        Feed {
            queue,
            produced: produced.clone(),
            sequence: 0,
        },
        Analyzer {
            queue: consumer,
            produced,
            rate,
            frames: vec![[0.0; 2]; N],
            cursor: 0,
            count: 0,
            previous: None,
            observed: 0,
            activity: Instant::now(),
            calculated: None,
            fft,
            buffer: vec![Complex32::default(); N],
            scratch,
            window,
            normalization,
            powers: vec![0.0; N / 2 + 1],
            result: Spectrum {
                sample_rate: rate,
                frequencies_hz,
                dbfs: vec![FLOOR; BINS],
            },
        },
    )
}

impl Analyzer {
    fn reset(&mut self) {
        self.count = 0;
        self.cursor = 0;
        self.previous = None;
        self.result.dbfs.fill(FLOOR);
    }
    pub fn snapshot(&mut self) -> Spectrum {
        self.snapshot_at(Instant::now())
    }
    fn snapshot_at(&mut self, now: Instant) -> Spectrum {
        if self
            .calculated
            .is_some_and(|last| now.duration_since(last) < INTERVAL)
        {
            return self.result.clone();
        }
        // Even a queue that stopped filling before it overflowed can contain
        // old audio. A resumed UI discards that backlog before showing signal.
        let resumed = now.duration_since(self.calculated.unwrap_or(self.activity))
            > Duration::from_millis(200);
        self.calculated = Some(now);
        let produced = self.produced.load(Ordering::Relaxed);
        if produced != self.observed {
            self.observed = produced;
            self.activity = now;
        }
        // A fixed drain bound prevents a producer from extending this request.
        let available = self.queue.slots().min(CAPACITY);
        let mut last = None;
        for _ in 0..available {
            if let Ok((sequence, frame)) = self.queue.pop() {
                if self
                    .previous
                    .is_some_and(|previous| sequence != previous.wrapping_add(1))
                {
                    self.reset();
                }
                self.previous = Some(sequence);
                last = Some(sequence);
                self.frames[self.cursor] =
                    frame.map(|sample| if sample.is_finite() { sample } else { 0.0 });
                self.cursor = (self.cursor + 1) % N;
                self.count = (self.count + 1).min(N);
            }
        }
        // After a long period without requests the full queue holds obsolete
        // audio. Reject it; the next request can consume fresh contiguous data.
        let stale =
            last.is_some_and(|last| produced.saturating_sub(last) > u64::from(self.rate) / 5);
        if resumed || stale || now.duration_since(self.activity) >= IDLE {
            self.reset();
        } else if self.count == N && last.is_some() {
            self.calculate();
        } else if self.count < N {
            self.result.dbfs.fill(FLOOR);
        }
        self.result.clone()
    }
    fn calculate(&mut self) {
        self.powers.fill(0.0);
        for channel in 0..2 {
            for i in 0..N {
                self.buffer[i] = Complex32::new(
                    self.frames[(self.cursor + i) % N][channel] * self.window[i],
                    0.0,
                );
            }
            self.fft
                .process_with_scratch(&mut self.buffer, &mut self.scratch);
            for (power, value) in self.powers.iter_mut().zip(&self.buffer) {
                *power += value.norm_sqr() * self.normalization * self.normalization * 0.5;
            }
        }
        // Peak power in each log band preserves narrow sine amplitude. Bands
        // narrower than one FFT bin use the nearest bin; this is not an RTA
        // integral and low-frequency resolution is still limited by N/rate.
        for i in 0..BINS {
            let center = self.result.frequencies_hz[i];
            let ratio = 1000.0_f32.powf(0.5 / (BINS - 1) as f32);
            let lo = ((center / ratio * N as f32 / self.rate as f32).ceil() as usize).max(1);
            let hi = (center * ratio * N as f32 / self.rate as f32).floor() as usize;
            let nearest = (center * N as f32 / self.rate as f32).round() as usize;
            let power = if center > self.rate as f32 * 0.5 {
                0.0
            } else if lo <= hi {
                self.powers[lo.min(N / 2)..=hi.min(N / 2)]
                    .iter()
                    .copied()
                    .fold(0.0_f32, f32::max)
            } else {
                self.powers[nearest.clamp(1, N / 2)]
            };
            self.result.dbfs[i] = if power.is_finite() {
                (10.0 * power.max(1.0e-10).log10()).clamp(FLOOR, 60.0)
            } else {
                FLOOR
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sine(feed: &mut Feed, start: usize, len: usize, amplitude: f32, opposite: bool) {
        for i in start..start + len {
            let x = amplitude * (std::f32::consts::TAU * 85.0 * i as f32 / N as f32).sin();
            feed.push([x, if opposite { -x } else { x }]);
        }
    }
    #[test]
    fn hann_calibration_frequency_and_opposite_stereo() {
        for opposite in [false, true] {
            let (mut feed, mut analyzer) = pair(48_000);
            sine(&mut feed, 0, N, 0.25, opposite);
            let result = analyzer.snapshot();
            let (index, db) = result
                .dbfs
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .unwrap();
            let expected = 85.0 * 48_000.0 / N as f32;
            assert!((result.frequencies_hz[index] / expected).ln().abs() < 0.06);
            assert!((*db - 20.0 * 0.25_f32.log10()).abs() < 0.01, "{db}");
        }
    }
    #[test]
    fn silence_replaces_signal_and_idle_clears() {
        let (mut feed, mut analyzer) = pair(48_000);
        let now = Instant::now();
        sine(&mut feed, 0, N, 0.5, false);
        assert!(analyzer.snapshot_at(now).dbfs.iter().any(|db| *db > -10.0));
        for _ in 0..N {
            feed.push([0.0; 2]);
        }
        assert!(
            analyzer
                .snapshot_at(now + INTERVAL)
                .dbfs
                .iter()
                .all(|db| *db == FLOOR)
        );
        sine(&mut feed, 0, N, 0.5, false);
        analyzer.snapshot_at(now + INTERVAL * 2);
        assert!(
            analyzer
                .snapshot_at(now + IDLE + INTERVAL * 2)
                .dbfs
                .iter()
                .all(|db| *db == FLOOR)
        );
    }
    #[test]
    fn partial_windows_gaps_and_closed_ui_are_bounded() {
        let (mut feed, mut analyzer) = pair(48_000);
        let now = Instant::now();
        sine(&mut feed, 0, N / 2, 0.5, false);
        assert!(analyzer.snapshot_at(now).dbfs.iter().all(|db| *db == FLOOR));
        sine(&mut feed, N / 2, N / 2, 0.5, false);
        assert!(
            analyzer
                .snapshot_at(now + INTERVAL)
                .dbfs
                .iter()
                .any(|db| *db > -10.0)
        );
        sine(&mut feed, 0, CAPACITY + 48_000, 0.5, false);
        assert!(
            analyzer
                .snapshot_at(now + INTERVAL * 2)
                .dbfs
                .iter()
                .all(|db| *db == FLOOR)
        );
        sine(&mut feed, 0, N / 2, 0.5, false);
        assert!(
            analyzer
                .snapshot_at(now + INTERVAL * 3)
                .dbfs
                .iter()
                .all(|db| *db == FLOOR)
        );
        sine(&mut feed, N / 2, N / 2, 0.5, false);
        assert!(
            analyzer
                .snapshot_at(now + INTERVAL * 4)
                .dbfs
                .iter()
                .any(|db| *db > -10.0)
        );
    }

    #[test]
    fn high_rate_overflow_recovers_and_throttles_without_nonfinite_json() {
        let (mut feed, mut analyzer) = pair(192_000);
        let now = Instant::now();
        sine(&mut feed, 0, 9600, 0.25, true);
        let first = analyzer.snapshot_at(now);
        assert_eq!(first.sample_rate, 192_000);
        assert!(first.dbfs.iter().any(|db| *db > -13.0));
        // Overflow leaves a sequence gap. The next full window still recovers.
        sine(&mut feed, 9600, 9600, 0.25, true);
        assert_eq!(analyzer.snapshot_at(now).dbfs, first.dbfs);
        assert!(
            analyzer
                .snapshot_at(now + INTERVAL)
                .dbfs
                .iter()
                .any(|db| *db > -13.0)
        );
        for _ in 0..N {
            feed.push([f32::NAN, f32::INFINITY]);
        }
        let result = analyzer.snapshot_at(now + INTERVAL * 2);
        assert!(result.dbfs.iter().all(|db| *db == FLOOR));
        assert!(!serde_json::to_string(&result).unwrap().contains("null"));
    }

    #[test]
    fn reopened_ui_discards_old_audio_even_without_queue_overflow() {
        let (mut feed, mut analyzer) = pair(48_000);
        let now = Instant::now();
        analyzer.snapshot_at(now);
        sine(&mut feed, 0, N, 0.5, false);
        let resumed = now + Duration::from_secs(2);
        assert!(
            analyzer
                .snapshot_at(resumed)
                .dbfs
                .iter()
                .all(|db| *db == FLOOR)
        );
        sine(&mut feed, 0, N, 0.5, false);
        assert!(
            analyzer
                .snapshot_at(resumed + INTERVAL)
                .dbfs
                .iter()
                .any(|db| *db > -10.0)
        );
    }
}
