//! Read-only live meter check. Start this collector, then run measure_path once
//! against the requested channel. Leave at least one second after the tone ends.
//! This program neither starts a host nor opens or changes an audio device.
use anada_app::{model::Command, transport};
use std::time::{Duration, Instant};

fn dbfs(peak: f32) -> Option<f32> {
    (peak > 0.0).then(|| 20.0 * peak.log10())
}

fn run() -> Result<bool, String> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    if !(1..=2).contains(&arguments.len()) {
        return Err("Usage: check_meters CHANNEL_ID [collection_seconds=6]; start collector before one measure_path tone; close the mixer UI so it does not consume peaks".into());
    }
    let seconds = arguments
        .get(1)
        .map_or(Ok(6.0), |s| s.parse::<f64>())
        .map_err(|error| error.to_string())?;
    if !seconds.is_finite() || !(2.0..=30.0).contains(&seconds) {
        return Err("Collection duration must be 2-30 seconds".into());
    }
    let interval = Duration::from_millis(50);
    let duration = Duration::from_secs_f64(seconds);
    let started = Instant::now();
    let mut next = started;
    let mut channel_peak = [0.0_f32; 2];
    let mut output_peak = [0.0_f32; 2];
    let mut other_peak = 0.0_f32;
    let mut trailing_peak = 0.0_f32;
    let mut samples = 0_u32;
    let mut trailing_samples = 0_u32;
    let mut maximum_request_ms = 0.0_f64;
    let mut host_pid = None;
    let mut all_running = true;
    // Clear any peak retained before collection starts.
    let initial = transport::request(Command::SnapshotMeters)?;
    if !initial
        .audio_diagnostics
        .meters
        .channels
        .iter()
        .any(|m| m.channel_id == arguments[0])
    {
        return Err(
            "Requested channel has no active meter; host must already be running with that binding"
                .into(),
        );
    }
    eprintln!(
        "Collector ready: {} seconds at 20 Hz. Start one tone now.",
        seconds
    );
    while started.elapsed() < duration {
        let request_started = Instant::now();
        let state = transport::request(Command::SnapshotMeters)?;
        maximum_request_ms =
            maximum_request_ms.max(request_started.elapsed().as_secs_f64() * 1000.0);
        if host_pid.is_some_and(|pid| pid != state.host_pid) {
            return Err("Host changed during meter collection".into());
        }
        host_pid = Some(state.host_pid);
        let meters = state.audio_diagnostics.meters;
        all_running &= meters.running;
        let channel = meters
            .channels
            .iter()
            .find(|m| m.channel_id == arguments[0])
            .ok_or("Channel meter disappeared during collection")?;
        let mut current_peak = 0.0_f32;
        for (index, peak) in channel.peak.into_iter().enumerate() {
            if !peak.is_finite() || peak < 0.0 {
                return Err("Channel meter returned an invalid peak".into());
            }
            channel_peak[index] = channel_peak[index].max(peak);
            current_peak = current_peak.max(peak);
        }
        for (index, peak) in meters.output_peak.into_iter().enumerate() {
            if !peak.is_finite() || !(0.0..=1.0).contains(&peak) {
                return Err("Post-clamp output meter returned an invalid peak".into());
            }
            output_peak[index] = output_peak[index].max(peak);
            current_peak = current_peak.max(peak);
        }
        for other in meters
            .channels
            .iter()
            .filter(|m| m.channel_id != arguments[0])
        {
            for peak in other.peak {
                if !peak.is_finite() || peak < 0.0 {
                    return Err("Other channel meter returned an invalid peak".into());
                }
                other_peak = other_peak.max(peak);
                current_peak = current_peak.max(peak);
            }
        }
        if started.elapsed() >= duration - Duration::from_millis(500) {
            trailing_peak = trailing_peak.max(current_peak);
            trailing_samples += 1;
        }
        samples += 1;
        next += interval;
        // Skip missed deadlines rather than issuing burst requests.
        if next < Instant::now() {
            next = Instant::now() + interval;
        }
        std::thread::sleep(next.saturating_duration_since(Instant::now()));
    }
    let detected = channel_peak.into_iter().all(|peak| peak > 0.00001)
        && output_peak.into_iter().all(|peak| peak > 0.00001);
    let isolated = other_peak <= 0.00001;
    let silence = trailing_samples >= 5 && trailing_peak <= 0.00001;
    let passed = all_running && detected && isolated && silence;
    println!("{}", serde_json::to_string_pretty(&serde_json::json!({
        "passed": passed,
        "channel_id": arguments[0],
        "host_pid": host_pid,
        "samples": samples,
        "elapsed_seconds": started.elapsed().as_secs_f64(),
        "maximum_request_ms": maximum_request_ms,
        "all_running": all_running,
        "channel_peak_linear": channel_peak,
        "channel_peak_dbfs": channel_peak.map(dbfs),
        "output_peak_linear": output_peak,
        "output_peak_dbfs": output_peak.map(dbfs),
        "other_channels_peak_linear": other_peak,
        "isolated_below_minus_100_dbfs": isolated,
        "signal_detected_both_sides": detected,
        "last_500_ms_samples": trailing_samples,
        "last_500_ms_peak_linear": trailing_peak,
        "silence_below_minus_100_dbfs": silence,
        "measurement": "sample peaks; output before Windows physical endpoint volume/mute",
        "note": "Requires one external stereo tone followed by silence. VAC trial reminders or other apps can fail isolation/silence. This does not verify physical loudness."
    })).map_err(|error| error.to_string())?);
    Ok(passed)
}

fn main() {
    match run() {
        Ok(true) => {}
        Ok(false) => std::process::exit(1),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    }
}
