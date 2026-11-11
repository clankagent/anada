//! Read-only spectrum IPC probe. The host must already be running.
//! Does not start a host, open audio devices, or change Windows settings.
use anada_app::{
    model::{Command, Spectrum},
    transport,
};
use std::time::{Duration, Instant};

fn valid_spectrum(spectrum: &Spectrum, warmup: bool) -> bool {
    if spectrum.frequencies_hz.is_empty() && spectrum.dbfs.is_empty() {
        return warmup;
    }
    (44_100..=192_000).contains(&spectrum.sample_rate)
        && spectrum.frequencies_hz.len() == 128
        && spectrum.dbfs.len() == 128
        && spectrum
            .frequencies_hz
            .iter()
            .all(|f| f.is_finite() && (19.99..=20_001.0).contains(f))
        && spectrum
            .frequencies_hz
            .windows(2)
            .all(|pair| pair[0] < pair[1])
        && spectrum
            .dbfs
            .iter()
            .all(|db| db.is_finite() && (-100.0..=60.0).contains(db))
}

fn strongest(spectrum: &Spectrum) -> Option<serde_json::Value> {
    let (index, dbfs) = spectrum
        .dbfs
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))?;
    Some(serde_json::json!({"frequency_hz": spectrum.frequencies_hz.get(index)?, "dbfs": dbfs}))
}

fn run() -> Result<bool, String> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    if arguments.len() > 1 {
        return Err("Usage: check_spectrum [channel_id=gaming]".into());
    }
    let channel_id = arguments.first().map_or("gaming", String::as_str);
    let before = transport::request(Command::Snapshot)?;
    let host_pid = before.host_pid;
    let interval = Duration::from_millis(50);
    let started = Instant::now();
    let mut next = started;
    let mut all_valid = true;
    let mut all_running = true;
    let mut maximum_request_ms = 0.0_f64;
    let mut channel_max_dbfs = -100.0_f32;
    let mut output_max_dbfs = -100.0_f32;
    let mut samples = Vec::with_capacity(12);
    for index in 0..12 {
        let request_started = Instant::now();
        let state = transport::request(Command::SnapshotMeters)?;
        let request_ms = request_started.elapsed().as_secs_f64() * 1000.0;
        maximum_request_ms = maximum_request_ms.max(request_ms);
        let meters = &state.audio_diagnostics.meters;
        let channel = meters
            .channels
            .iter()
            .find(|channel| channel.channel_id == channel_id)
            .ok_or_else(|| format!("No active channel meter for {channel_id}"))?;
        let valid = state.host_pid == host_pid
            && valid_spectrum(&channel.spectrum, index < 2)
            && valid_spectrum(&meters.output_spectrum, index < 2)
            && (channel.spectrum.dbfs.is_empty()
                || meters.output_spectrum.dbfs.is_empty()
                || (channel.spectrum.sample_rate == meters.output_spectrum.sample_rate
                    && channel.spectrum.frequencies_hz == meters.output_spectrum.frequencies_hz))
            && channel
                .peak
                .iter()
                .all(|peak| peak.is_finite() && *peak >= 0.0)
            && meters
                .output_peak
                .iter()
                .all(|peak| peak.is_finite() && (0.0..=1.0).contains(peak));
        all_valid &= valid;
        all_running &= meters.running;
        for db in &channel.spectrum.dbfs {
            channel_max_dbfs = channel_max_dbfs.max(*db);
        }
        for db in &meters.output_spectrum.dbfs {
            output_max_dbfs = output_max_dbfs.max(*db);
        }
        samples.push(serde_json::json!({
            "index": index, "request_ms": request_ms, "running": meters.running, "valid": valid,
            "channel_sample_rate": channel.spectrum.sample_rate,
            "output_sample_rate": meters.output_spectrum.sample_rate,
            "channel_bins": channel.spectrum.dbfs.len(), "output_bins": meters.output_spectrum.dbfs.len(),
            "channel_peak_linear": channel.peak, "output_peak_linear": meters.output_peak,
            "channel_strongest": strongest(&channel.spectrum),
            "output_strongest": strongest(&meters.output_spectrum),
        }));
        if index < 11 {
            next += interval;
            if next < Instant::now() {
                next = Instant::now() + interval;
            }
            std::thread::sleep(next.saturating_duration_since(Instant::now()));
        }
    }
    let after = transport::request(Command::Snapshot)?;
    let faults_before = before.audio_diagnostics.stream_faults;
    let faults_after = after.audio_diagnostics.stream_faults;
    let stable_host = after.host_pid == host_pid;
    let no_new_stream_faults = faults_after == faults_before;
    let passed = all_valid && all_running && stable_host && no_new_stream_faults;
    println!("{}", serde_json::to_string_pretty(&serde_json::json!({
        "passed": passed, "count": samples.len(), "channel_id": channel_id, "host_pid": host_pid,
        "elapsed_seconds": started.elapsed().as_secs_f64(),
        "maximum_request_ms": maximum_request_ms, "all_valid": all_valid, "all_running": all_running,
        "stable_host": stable_host, "no_new_stream_faults": no_new_stream_faults,
        "stream_faults_before": faults_before, "stream_faults_after": faults_after,
        "output_frames_before": before.audio_diagnostics.output_frames,
        "output_frames_after": after.audio_diagnostics.output_frames,
        "channel_max_spectrum_dbfs": channel_max_dbfs, "output_max_spectrum_dbfs": output_max_dbfs,
        "samples": samples,
        "note": "Read-only IPC structure and latency check. Signal levels and strongest frequencies are informational; VAC trial voice or other audio can contribute. Output precedes Windows endpoint volume/mute."
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
