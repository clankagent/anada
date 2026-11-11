#![forbid(unsafe_code)]

use biquad::{Biquad, Coefficients, DirectForm2Transposed, ToHertz, Type};
use serde::{Deserialize, Serialize};
use std::f64::consts::TAU;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct EqSettings {
    pub bypass: bool,
    pub preamp_db: f32,
    pub bands: Vec<EqBand>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct EqBand {
    pub enabled: bool,
    pub kind: FilterKind,
    pub frequency_hz: f32,
    pub gain_db: f32,
    pub q: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum FilterKind {
    Bell,
    LowShelf,
    HighShelf,
}

impl Default for EqSettings {
    fn default() -> Self {
        Self {
            bypass: false,
            preamp_db: 0.0,
            bands: [
                31.25, 62.5, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0,
            ]
            .into_iter()
            .map(|frequency_hz| EqBand {
                enabled: true,
                kind: FilterKind::Bell,
                frequency_hz,
                gain_db: 0.0,
                q: std::f32::consts::FRAC_1_SQRT_2,
            })
            .collect(),
        }
    }
}

impl EqSettings {
    pub fn validate(&self, sample_rate: u32) -> Result<(), String> {
        if sample_rate < 41 {
            return Err("Sample rate must be greater than 40 Hz".into());
        }
        if !self.preamp_db.is_finite() || !(-24.0..=12.0).contains(&self.preamp_db) {
            return Err("Preamp must be finite and within -24..12 dB".into());
        }
        if self.bands.len() > 10 {
            return Err("EQ supports at most ten bands".into());
        }
        for (index, band) in self.bands.iter().enumerate() {
            if !band.frequency_hz.is_finite()
                || !(20.0..=20000.0).contains(&band.frequency_hz)
                || f64::from(band.frequency_hz) >= f64::from(sample_rate) * 0.5
            {
                return Err(format!(
                    "Band {index}: frequency must be within 20..20000 Hz and below Nyquist"
                ));
            }
            if !band.gain_db.is_finite() || !(-24.0..=24.0).contains(&band.gain_db) {
                return Err(format!(
                    "Band {index}: gain must be finite and within -24..24 dB"
                ));
            }
            if !band.q.is_finite() || !(0.1..=20.0).contains(&band.q) {
                return Err(format!("Band {index}: Q must be finite and within 0.1..20"));
            }
        }
        Ok(())
    }
}

fn coefficients(band: &EqBand, sample_rate: u32) -> Result<Coefficients<f64>, String> {
    let gain = f64::from(band.gain_db);
    let kind = match band.kind {
        FilterKind::Bell => Type::PeakingEQ(gain),
        FilterKind::LowShelf => Type::LowShelf(gain),
        FilterKind::HighShelf => Type::HighShelf(gain),
    };
    let result = Coefficients::<f64>::from_params(
        kind,
        f64::from(sample_rate).hz(),
        f64::from(band.frequency_hz).hz(),
        f64::from(band.q),
    )
    .map_err(|error| format!("Cannot build EQ coefficients: {error:?}"))?;
    if [result.b0, result.b1, result.b2, result.a1, result.a2]
        .iter()
        .any(|value| !value.is_finite())
    {
        return Err("EQ coefficients must be finite".into());
    }
    Ok(result)
}

pub struct StereoEq {
    filters: Vec<[DirectForm2Transposed<f64>; 2]>,
    preamp: f64,
    passthrough: bool,
    next_channel: usize,
}

impl StereoEq {
    pub fn new(settings: &EqSettings, sample_rate: u32) -> Result<Self, String> {
        settings.validate(sample_rate)?;
        let mut filters = Vec::with_capacity(settings.bands.len());
        if !settings.bypass {
            for band in settings
                .bands
                .iter()
                .filter(|band| band.enabled && band.gain_db != 0.0)
            {
                let coefficient = coefficients(band, sample_rate)?;
                filters.push([
                    DirectForm2Transposed::new(coefficient),
                    DirectForm2Transposed::new(coefficient),
                ]);
            }
        }
        let passthrough = settings.bypass || (filters.is_empty() && settings.preamp_db == 0.0);
        Ok(Self {
            filters,
            preamp: if settings.bypass {
                1.0
            } else {
                10.0_f64.powf(f64::from(settings.preamp_db) / 20.0)
            },
            passthrough,
            next_channel: 0,
        })
    }

    pub fn process_interleaved(&mut self, samples: &mut [f32]) {
        if self.passthrough {
            self.next_channel ^= samples.len() & 1;
            return;
        }
        for sample in samples {
            let mut value = if sample.is_finite() {
                f64::from(*sample) * self.preamp
            } else {
                0.0
            };
            for filter in &mut self.filters {
                value = filter[self.next_channel].run(value);
                if !value.is_finite() {
                    filter[self.next_channel].reset_state();
                    value = 0.0;
                }
            }
            *sample = finite_f32(value);
            self.next_channel ^= 1;
        }
    }

    pub fn reset(&mut self) {
        for filter in &mut self.filters {
            for channel in filter {
                channel.reset_state();
            }
        }
        self.next_channel = 0;
    }
}

pub fn response_db(
    settings: &EqSettings,
    frequency_hz: f32,
    sample_rate: u32,
) -> Result<f32, String> {
    settings.validate(sample_rate)?;
    if !frequency_hz.is_finite()
        || frequency_hz < 0.0
        || f64::from(frequency_hz) > f64::from(sample_rate) * 0.5
    {
        return Err("Response frequency must be finite and within 0..Nyquist".into());
    }
    if settings.bypass {
        return Ok(0.0);
    }
    let angle = TAU * f64::from(frequency_hz) / f64::from(sample_rate);
    let mut decibels = f64::from(settings.preamp_db);
    for band in settings
        .bands
        .iter()
        .filter(|band| band.enabled && band.gain_db != 0.0)
    {
        let coefficient = coefficients(band, sample_rate)?;
        let magnitude_squared = |constant: f64, first: f64, second: f64| {
            let real = constant + first * angle.cos() + second * (2.0 * angle).cos();
            let imaginary = -first * angle.sin() - second * (2.0 * angle).sin();
            real * real + imaginary * imaginary
        };
        let numerator = magnitude_squared(coefficient.b0, coefficient.b1, coefficient.b2);
        let denominator = magnitude_squared(1.0, coefficient.a1, coefficient.a2);
        if numerator <= 0.0 || denominator <= 0.0 {
            return Err("Response magnitude cannot be evaluated at this frequency".into());
        }
        decibels += 10.0 * (numerator / denominator).log10();
    }
    if !decibels.is_finite() {
        return Err("Response magnitude is not finite".into());
    }
    Ok(decibels as f32)
}

fn finite_f32(value: f64) -> f32 {
    if value.is_nan() {
        0.0
    } else {
        value.clamp(-f64::from(f32::MAX), f64::from(f32::MAX)) as f32
    }
}

pub fn mix_stereo(inputs: &[&[f32]], output: &mut [f32]) -> Result<(), String> {
    if !output.len().is_multiple_of(2) {
        return Err("Stereo mix requires complete interleaved frames".into());
    }
    if inputs.iter().any(|input| input.len() != output.len()) {
        return Err("All mix buffers must have equal lengths".into());
    }
    for (index, sample) in output.iter_mut().enumerate() {
        let sum = inputs
            .iter()
            .map(|input| input[index])
            .filter(|value| value.is_finite())
            .fold(0.0_f64, |sum, value| sum + f64::from(value));
        *sample = finite_f32(sum);
    }
    Ok(())
}

pub fn sanitize_output(samples: &mut [f32]) -> usize {
    let mut changed = 0;
    for sample in samples {
        if !sample.is_finite() {
            *sample = 0.0;
            changed += 1;
        } else if *sample > 1.0 || *sample < -1.0 {
            *sample = sample.clamp(-1.0, 1.0);
            changed += 1;
        }
    }
    changed
}

#[cfg(test)]
mod tests;
