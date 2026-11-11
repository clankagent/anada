//! Demo mode with sample data. No audio, Windows changes, startup task or preset file writes.
use super::*;

/// Explicit stress fixture for native layout review; never used for real audio.
pub(super) fn stress(host: &mut HostState) {
    host.config.channels[0].name = "Default".into();
    for (endpoint, volume) in host
        .windows
        .outputs
        .iter_mut()
        .zip([0.23, 0.339, 0.385, 0.194, 0.76, 0.3])
    {
        endpoint.volume = volume;
        if endpoint.id == "demo-headphones" {
            endpoint.name = "Headphones (MOMENTUM 5 Wireless — Bluetooth Audio)".into();
        }
    }
    for (index, name) in ["Spotify", "steam", "steamwebhelper", "T3 Code (Beta)"]
        .into_iter()
        .enumerate()
    {
        host.windows.sessions.push(anada_windows::AudioSession {
            executable_path: Some(format!("C:/Demo/Outside/{name}.exe")),
            process_id: 1000 + index as u32,
            name: name.into(),
            endpoint_id: "demo-headphones".into(),
            active: false,
        });
    }
}
/// Six connected outputs and eleven saved offline devices, matching the reported workload.
pub(super) fn priority_stress(host: &mut HostState) {
    let template = host
        .windows
        .outputs
        .iter()
        .find(|e| e.id == "demo-headphones")
        .unwrap()
        .clone();
    host.windows.outputs.retain(|e| e.is_virtual);
    host.config.output_priorities.clear();
    let names = [
        "Headphones (MOMENTUM 5)",
        "Speakers (Realtek(R) Audio)",
        "Headphones (Arctis Nova Pro Wireless)",
        "Speakers (Steam Streaming Microphone)",
        "Headphones (Elgato Wave Neo)",
        "Speakers (Steam Streaming Speakers)",
    ];
    for (index, name) in names.into_iter().enumerate() {
        let id = format!("priority-{index}");
        host.windows.outputs.push(Endpoint {
            id: id.clone(),
            name: name.into(),
            ..template.clone()
        });
        host.config.output_priorities.push(id.clone());
        host.config.output_names.insert(id, name.into());
        if index < 5 {
            let offline = format!("offline-{index}");
            host.config.output_priorities.push(offline.clone());
            host.config
                .output_names
                .insert(offline, format!("Studio output {}", index + 1));
        }
    }
    for index in 5..11 {
        let id = format!("offline-{index}");
        host.config.output_priorities.push(id.clone());
        host.config
            .output_names
            .insert(id, format!("Studio output {}", index + 1));
    }
    host.config.known_output_ids = host.config.output_priorities.clone();
    for index in 0..9 {
        let id = format!("unlisted-{index}");
        let name = format!("Unused playback device {}", index + 1);
        host.config.known_output_ids.push(id.clone());
        host.config.output_names.insert(id.clone(), name.clone());
        if index < 6 {
            host.windows.outputs.push(Endpoint {
                id,
                name,
                ..template.clone()
            });
        }
    }
    host.selected_output_id = Some("priority-0".into());
}
pub(super) fn state() -> HostState {
    let mut config = Config::default();
    let mut outputs = Vec::new();
    let mut inputs = Vec::new();
    for (index, c) in config.channels.iter_mut().enumerate() {
        let id = format!("demo-{}", c.id);
        let capture = format!("demo-capture-{}", c.id);
        c.endpoint_id = Some(id.clone());
        c.capture_id = Some(capture.clone());
        let endpoint = Endpoint {
            level_db: Some(anada_windows::EndpointLevelDb {
                current: [-9.0, -6.0, -12.0, -20.0][index],
                min: -40.0,
                max: 12.0,
                step: 0.5,
            }),
            id,
            name: format!("Line {} (Virtual Audio Cable)", index + 1),
            volume: 0.5,
            muted: false,
            is_virtual: true,
        };
        outputs.push(endpoint.clone());
        inputs.push(Endpoint {
            id: capture,
            ..endpoint
        });
    }
    outputs.push(Endpoint {
        level_db: Some(anada_windows::EndpointLevelDb {
            current: -12.0,
            min: -96.0,
            max: 0.0,
            step: 0.5,
        }),
        id: "demo-headphones".into(),
        name: "Headphones · USB Audio".into(),
        volume: 0.5,
        muted: false,
        is_virtual: false,
    });
    outputs.push(Endpoint {
        level_db: Some(anada_windows::EndpointLevelDb {
            current: -18.0,
            min: -96.0,
            max: 0.0,
            step: 0.5,
        }),
        id: "demo-speakers".into(),
        name: "Speakers · Realtek Audio".into(),
        volume: 0.3,
        muted: false,
        is_virtual: false,
    });
    config.output_priorities = vec!["demo-headphones".into(), "demo-speakers".into()];
    config.known_output_ids = config.output_priorities.clone();
    config.channels[0].eq.bands[3].gain_db = -3.0;
    config.channels[0].eq.bands[7].gain_db = 2.0;
    let session = |pid, name: &str, endpoint: &str| anada_windows::AudioSession {
        executable_path: Some(format!("C:/Demo/{name}/{name}.exe")),
        process_id: pid,
        name: name.into(),
        endpoint_id: endpoint.into(),
        active: true,
    };
    HostState {
        audio_diagnostics: Default::default(),
        protocol_version: crate::model::PROTOCOL_VERSION,
        revision: 1,
        config,
        windows: WindowsSnapshot {
            outputs,
            inputs,
            sessions: vec![
                session(42, "Chrome", "demo-gaming"),
                session(42, "Chrome", "demo-gaming"),
                session(43, "Chrome", "demo-gaming"),
                session(43, "Chrome", "demo-gaming"),
                session(50, "Discord", "demo-chat"),
                session(60, "Spotify", "demo-media"),
                session(70, "Game", "demo-gaming"),
                session(80, "Edge", "demo-headphones"),
            ],
            default_output_id: Some("demo-gaming".into()),
            communications_output_id: Some("demo-chat".into()),
        },
        selected_output_id: Some("demo-headphones".into()),
        manual_output: false,
        audio_running: false,
        audio_status: "Demo · no audio or Windows changes".into(),
        warnings: vec![],
        host_pid: 99999,
        quiet: Default::default(),
    }
}
pub(super) fn command(host: &mut HostState, command: Command) -> Result<(), String> {
    match command {
        Command::Snapshot => {}
        Command::SnapshotMeters => {
            host.audio_diagnostics.meters = Default::default();
        }
        Command::SaveConfig {
            config,
            expected_revision,
        } => {
            if expected_revision != host.revision {
                return Err("Settings changed in another window".into());
            }
            config.validate()?;
            host.config = config;
            host.revision += 1;
            if !host.manual_output {
                host.selected_output_id =
                    crate::model::automatic_output(&host.config, &host.windows);
            }
            end_expired_quiet(host);
        }
        Command::AssignApplication {
            process_id,
            channel_id,
        } => assign(host, &[process_id], &channel_id)?,
        Command::AssignApplications {
            process_ids,
            channel_id,
        } => assign(host, &process_ids, &channel_id)?,
        Command::RepairDefaults => {
            host.windows.default_output_id = host
                .config
                .channels
                .iter()
                .find(|c| c.id == "gaming")
                .and_then(|c| c.endpoint_id.clone());
            host.windows.communications_output_id = host
                .config
                .channels
                .iter()
                .find(|c| c.id == "chat")
                .and_then(|c| c.endpoint_id.clone());
        }
        Command::SelectOutput { id } => {
            host.selected_output_id = Some(id);
            host.manual_output = true;
            end_expired_quiet(host);
        }
        Command::SetQuiet { endpoint_id, on } => {
            let level = host.config.quiet_volume_percent;
            let e = host
                .windows
                .outputs
                .iter_mut()
                .find(|e| e.id == endpoint_id)
                .ok_or("Unknown demo endpoint")?;
            if !on {
                if let Some(entry) = host.quiet.remove(&endpoint_id) {
                    e.volume = entry.restore;
                }
            } else if let std::collections::btree_map::Entry::Vacant(slot) =
                host.quiet.entry(endpoint_id)
            {
                let restore = e.volume;
                e.volume = crate::model::quiet_target(restore, level);
                slot.insert(crate::model::QuietEntry {
                    restore,
                    applied: e.volume,
                });
            }
        }
        Command::SetVolumeDb {
            endpoint_id,
            level_db,
        } => {
            let e = host
                .windows
                .outputs
                .iter_mut()
                .find(|e| e.id == endpoint_id)
                .ok_or("Unknown demo endpoint")?;
            if let Some(db) = &mut e.level_db {
                db.current = level_db.clamp(db.min, db.max);
            }
            host.quiet.remove(&endpoint_id);
        }
        Command::SetVolume {
            endpoint_id,
            volume,
        } => {
            let e = host
                .windows
                .outputs
                .iter_mut()
                .find(|e| e.id == endpoint_id)
                .ok_or("Unknown demo endpoint")?;
            e.volume = volume;
            host.quiet.remove(&endpoint_id);
        }
        Command::SetMute { endpoint_id, muted } => {
            let e = host
                .windows
                .outputs
                .iter_mut()
                .find(|e| e.id == endpoint_id)
                .ok_or("Unknown demo endpoint")?;
            e.muted = muted;
        }
        Command::StopHost => {}
    }
    Ok(())
}

fn end_expired_quiet(host: &mut HostState) {
    for (id, restore) in crate::model::expired_quiet(host) {
        host.quiet.remove(&id);
        if let Some(volume) = restore
            && let Some(e) = host.windows.outputs.iter_mut().find(|e| e.id == id)
        {
            e.volume = volume;
        }
    }
}
fn assign(host: &mut HostState, pids: &[u32], target: &str) -> Result<(), String> {
    let endpoint = host
        .config
        .channels
        .iter()
        .find(|c| c.id == target)
        .and_then(|c| c.endpoint_id.clone())
        .ok_or("Unbound demo channel")?;
    for s in &mut host.windows.sessions {
        if pids.contains(&s.process_id) {
            s.endpoint_id = endpoint.clone();
        }
    }
    Ok(())
}
