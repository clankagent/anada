# Anada DSP

Pure stereo DSP. No audio I/O, endpoint gain, driver integration, or unsafe code.

- Default settings create ten enabled, flat bell bands from 31.25 Hz to 16 kHz.
- EqSettings::validate checks every parameter, including disabled and bypassed bands. Band frequencies must be strictly below Nyquist.
- StereoEq prepares independent left/right f64 biquads. Processing uses no allocation. Samples are f32, interleaved left then right. Odd-length chunks continue the same stream; reset clears history and restores left-first alignment.
- Flat EQ and whole-EQ bypass preserve input bits, including signed zero. Bypass includes preamp. Active processing treats nonfinite input as silence and prevents nonfinite filter history from persisting. Final output sanitation remains the caller's responsibility, including in bypass.
- response_db evaluates the actual filter coefficients. Probe frequencies include DC and Nyquist. The result includes preamp unless bypassed.
- mix_stereo sums equal-length, complete stereo frames with f64 accumulation. It does not normalize, apply endpoint gain, or clip to unity. It ignores nonfinite input samples. An error leaves output unchanged. Zero inputs produce silence. Overflow beyond f32 range saturates at finite f32 limits.
- sanitize_output is final saturation protection, **not a transparent limiter**. It hard-clips finite samples to [-1, 1], replaces nonfinite samples with silence, and returns the number of changed samples. A transparent limiter remains pending.

Use EQ preamps deliberately to preserve headroom. Neither EQ nor mixing implicitly invokes saturation protection.
