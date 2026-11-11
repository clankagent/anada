use super::*;

fn single_band(kind: FilterKind, gain_db: f32) -> EqSettings {
    EqSettings {
        bypass: false,
        preamp_db: 0.0,
        bands: vec![EqBand {
            enabled: true,
            kind,
            frequency_hz: 1000.0,
            gain_db,
            q: std::f32::consts::FRAC_1_SQRT_2,
        }],
    }
}

fn close(actual: f64, expected: f64, tolerance: f64) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "actual {actual}, expected {expected}"
    );
}

fn measured_tone_db(settings: &EqSettings, frequency: f64) -> f64 {
    let mut samples: Vec<f32> = (0..48000)
        .flat_map(|frame| {
            let sample = (0.02 * (TAU * frequency * f64::from(frame) / 48000.0).sin()) as f32;
            [sample, sample]
        })
        .collect();
    let input_power: f64 = samples[48000..]
        .iter()
        .map(|sample| f64::from(*sample).powi(2))
        .sum();
    StereoEq::new(settings, 48000)
        .unwrap()
        .process_interleaved(&mut samples);
    let output_power: f64 = samples[48000..]
        .iter()
        .map(|sample| f64::from(*sample).powi(2))
        .sum();
    10.0 * (output_power / input_power).log10()
}

#[test]
fn defaults_are_ten_enabled_log_spaced_flat_bands() {
    let settings = EqSettings::default();
    assert_eq!(settings.bands.len(), 10);
    assert_eq!(settings.preamp_db, 0.0);
    assert!(!settings.bypass);
    assert!(
        settings
            .bands
            .iter()
            .all(|band| band.enabled && band.gain_db == 0.0)
    );
    for pair in settings.bands.windows(2) {
        assert_eq!(pair[1].frequency_hz / pair[0].frequency_hz, 2.0);
    }
    settings.validate(48000).unwrap();
}

#[test]
fn flat_and_bypass_preserve_every_bit_including_preamp_bypass() {
    let input = [
        0.0,
        -0.0,
        0.125,
        -0.8,
        f32::MIN_POSITIVE,
        f32::MAX,
        f32::from_bits(0x7fc01234),
    ];
    let flat = EqSettings::default();
    let mut bypass = single_band(FilterKind::Bell, 24.0);
    bypass.preamp_db = 12.0;
    bypass.bypass = true;
    for settings in [flat, bypass] {
        let mut actual = input;
        StereoEq::new(&settings, 48000)
            .unwrap()
            .process_interleaved(&mut actual);
        assert_eq!(actual.map(f32::to_bits), input.map(f32::to_bits));
        assert_eq!(response_db(&settings, 1000.0, 48000).unwrap(), 0.0);
    }
}

#[test]
fn bell_center_gain_matches_analytic_gain_and_measured_tones() {
    for gain_db in [-24.0, -6.0, 6.0, 24.0] {
        let settings = single_band(FilterKind::Bell, gain_db);
        close(
            f64::from(response_db(&settings, 1000.0, 48000).unwrap()),
            f64::from(gain_db),
            0.0001,
        );
        close(
            measured_tone_db(&settings, 1000.0),
            f64::from(gain_db),
            0.001,
        );
        close(
            f64::from(response_db(&settings, 0.0, 48000).unwrap()),
            0.0,
            0.0001,
        );
        close(
            f64::from(response_db(&settings, 24000.0, 48000).unwrap()),
            0.0,
            0.0001,
        );
    }
}

#[test]
fn bell_off_center_matches_independent_bilinear_analytic_response() {
    let mut settings = single_band(FilterKind::Bell, 9.0);
    settings.bands[0].q = 1.3;
    let amplitude = 10.0_f64.powf(9.0 / 40.0);
    for frequency in [100.0_f32, 600.0, 1800.0, 12000.0] {
        let ratio = (std::f64::consts::PI * f64::from(frequency) / 48000.0).tan()
            / (std::f64::consts::PI * 1000.0 / 48000.0).tan();
        let real_squared = (1.0 - ratio * ratio).powi(2);
        let width = ratio / f64::from(settings.bands[0].q);
        let expected = 10.0
            * ((real_squared + (amplitude * width).powi(2))
                / (real_squared + (width / amplitude).powi(2)))
            .log10();
        close(
            f64::from(response_db(&settings, frequency, 48000).unwrap()),
            expected,
            0.0001,
        );
        close(
            measured_tone_db(&settings, f64::from(frequency)),
            expected,
            0.002,
        );
    }
}

#[test]
fn shelves_have_analytic_endpoints_midpoint_and_measured_plateaus() {
    for kind in [FilterKind::LowShelf, FilterKind::HighShelf] {
        for gain_db in [-12.0, 12.0] {
            let settings = single_band(kind.clone(), gain_db);
            let low_shelf = kind == FilterKind::LowShelf;
            let dc = if low_shelf { gain_db } else { 0.0 };
            let nyquist = if low_shelf { 0.0 } else { gain_db };
            close(
                f64::from(response_db(&settings, 0.0, 48000).unwrap()),
                f64::from(dc),
                0.0001,
            );
            close(
                f64::from(response_db(&settings, 24000.0, 48000).unwrap()),
                f64::from(nyquist),
                0.0001,
            );
            close(
                f64::from(response_db(&settings, 1000.0, 48000).unwrap()),
                f64::from(gain_db) / 2.0,
                0.0001,
            );
            close(measured_tone_db(&settings, 50.0), f64::from(dc), 0.05);
            close(
                measured_tone_db(&settings, 18000.0),
                f64::from(nyquist),
                0.05,
            );
        }
    }
}

#[test]
fn preamp_and_disabled_bands_have_explicit_gain() {
    let mut settings = single_band(FilterKind::Bell, 12.0);
    settings.bands[0].enabled = false;
    settings.preamp_db = -6.0;
    close(measured_tone_db(&settings, 1000.0), -6.0, 0.0001);
    close(
        f64::from(response_db(&settings, 1000.0, 48000).unwrap()),
        -6.0,
        0.0001,
    );
}

#[test]
fn stereo_channels_are_independent_across_odd_chunks_and_reset() {
    let settings = single_band(FilterKind::Bell, 12.0);
    let mut input = vec![0.0; 2048];
    input[0] = 0.5;
    let mut whole = input.clone();
    let mut split = input.clone();
    StereoEq::new(&settings, 48000)
        .unwrap()
        .process_interleaved(&mut whole);
    let mut equalizer = StereoEq::new(&settings, 48000).unwrap();
    for chunk in split.chunks_mut(7) {
        equalizer.process_interleaved(chunk);
    }
    assert_eq!(whole, split);
    assert!(whole.iter().skip(1).step_by(2).all(|sample| *sample == 0.0));
    equalizer.process_interleaved(&mut [0.1]);
    equalizer.reset();
    equalizer.process_interleaved(&mut input);
    assert_eq!(whole, input);
}

#[test]
fn validation_rejects_all_malformed_parameters_even_when_bypassed() {
    let base = single_band(FilterKind::Bell, 0.0);
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -24.01, 12.01] {
        let mut settings = base.clone();
        settings.preamp_db = bad;
        assert!(StereoEq::new(&settings, 48000).is_err());
    }
    for field in 0..3 {
        let invalid = match field {
            0 => vec![f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 19.0, 20001.0],
            1 => vec![f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -24.01, 24.01],
            _ => vec![f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.09, 20.01],
        };
        for bad in invalid {
            let mut settings = base.clone();
            settings.bypass = true;
            settings.bands[0].enabled = false;
            match field {
                0 => settings.bands[0].frequency_hz = bad,
                1 => settings.bands[0].gain_db = bad,
                _ => settings.bands[0].q = bad,
            }
            assert!(StereoEq::new(&settings, 48000).is_err());
            assert!(response_db(&settings, 1000.0, 48000).is_err());
        }
    }
    let mut settings = base.clone();
    settings.bands = vec![base.bands[0].clone(); 11];
    assert!(settings.validate(48000).is_err());
    assert!(base.validate(0).is_err());
    assert!(base.validate(40).is_err());
    assert!(base.validate(2000).is_err());
    for frequency in [-1.0, 24001.0, f32::NAN, f32::INFINITY] {
        assert!(response_db(&base, frequency, 48000).is_err());
    }
}

#[test]
fn parameter_boundaries_produce_finite_processing_and_response() {
    for kind in [
        FilterKind::Bell,
        FilterKind::LowShelf,
        FilterKind::HighShelf,
    ] {
        for frequency in [20.0, 20000.0] {
            for gain in [-24.0, 24.0] {
                for quality in [0.1, 20.0] {
                    let mut settings = single_band(kind.clone(), gain);
                    settings.bands[0].frequency_hz = frequency;
                    settings.bands[0].q = quality;
                    for preamp in [-24.0, 12.0] {
                        settings.preamp_db = preamp;
                        let mut samples = [0.1; 2048];
                        StereoEq::new(&settings, 48000)
                            .unwrap()
                            .process_interleaved(&mut samples);
                        assert!(samples.iter().all(|sample| sample.is_finite()));
                        for probe in [0.0, 20.0, 1000.0, 20000.0, 24000.0] {
                            assert!(response_db(&settings, probe, 48000).unwrap().is_finite());
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn mixing_sums_without_normalization_or_double_endpoint_gain() {
    let already_endpoint_scaled = [0.25, -0.125, 0.75, -0.75];
    let second = [0.25, 0.125, 0.75, -0.75];
    let mut output = [0.0; 4];
    mix_stereo(&[&already_endpoint_scaled], &mut output).unwrap();
    assert_eq!(output, already_endpoint_scaled);
    mix_stereo(&[&already_endpoint_scaled, &second], &mut output).unwrap();
    assert_eq!(output, [0.5, 0.0, 1.5, -1.5]);
    mix_stereo(&[], &mut output).unwrap();
    assert_eq!(output, [0.0; 4]);
    assert!(mix_stereo(&[&second[..2]], &mut output).is_err());
    assert_eq!(output, [0.0; 4]);
    let mut odd = [0.5; 3];
    assert!(mix_stereo(&[], &mut odd).is_err());
    assert_eq!(odd, [0.5; 3]);
}

#[test]
fn nonfinite_samples_and_large_sums_are_safe() {
    let invalid = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, f32::MAX];
    let valid = [0.25, -0.25, 0.125, f32::MAX];
    let mut output = [0.0; 4];
    mix_stereo(&[&invalid, &valid], &mut output).unwrap();
    assert_eq!(output, [0.25, -0.25, 0.125, f32::MAX]);
    let settings = single_band(FilterKind::Bell, 12.0);
    let mut samples = [f32::NAN, f32::INFINITY, 0.1, 0.2, 0.0, 0.0];
    StereoEq::new(&settings, 48000)
        .unwrap()
        .process_interleaved(&mut samples);
    assert!(samples.iter().all(|sample| sample.is_finite()));
    assert_ne!(samples[2], 0.0);
    assert_ne!(samples[3], 0.0);
}

#[test]
fn saturation_protection_counts_only_changed_samples() {
    let mut samples = [
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        1.5,
        -2.0,
        1.0,
        -1.0,
        -0.0,
        0.5,
    ];
    assert_eq!(sanitize_output(&mut samples), 5);
    assert_eq!(samples, [0.0, 0.0, 0.0, 1.0, -1.0, 1.0, -1.0, -0.0, 0.5]);
    assert_eq!(samples[7].to_bits(), (-0.0_f32).to_bits());
    assert_eq!(sanitize_output(&mut samples), 0);
}
