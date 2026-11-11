use crate::audio::AudioEngine;
use crate::model::{
    Command, Config, HostState, PROTOCOL_VERSION, Response, automatic_output, availability,
    discover_outputs,
};
use crate::storage::ConfigStore;
use anada_windows::{AudioSystem, WindowsSnapshot};
use std::time::{Duration, Instant};

pub struct Controller {
    system: AudioSystem,
    store: ConfigStore,
    state: HostState,
    audio: AudioEngine,
    last_scan: Instant,
    startup_guard: bool,
}

fn reconcile_outputs(
    state: &mut HostState,
    windows: WindowsSnapshot,
    mut save: impl FnMut(&Config) -> Result<(), String>,
) -> Result<(), String> {
    let changed = availability(&windows) != availability(&state.windows);
    let mut config = state.config.clone();
    let discovered = discover_outputs(&mut config, &windows, state.selected_output_id.as_deref());
    let result = if discovered { save(&config) } else { Ok(()) };
    if discovered && result.is_ok() {
        state.config = config;
        state.revision += 1;
    }
    // Observed availability remains authoritative even if settings cannot be saved.
    if changed {
        state.manual_output = false;
    }
    state.windows = windows;
    if !state.manual_output {
        state.selected_output_id = automatic_output(&state.config, &state.windows);
    }
    result
}

fn channel_playback<'a>(
    config: &'a Config,
    windows: &WindowsSnapshot,
    channel_id: &str,
) -> Result<&'a str, String> {
    let channel = config
        .channels
        .iter()
        .find(|channel| channel.id == channel_id)
        .ok_or_else(|| "Unknown Anada channel".to_owned())?;
    let playback = channel
        .endpoint_id
        .as_deref()
        .ok_or_else(|| "Bind the channel playback endpoint first".to_owned())?;
    let capture = channel
        .capture_id
        .as_deref()
        .ok_or_else(|| "Bind the channel capture endpoint first".to_owned())?;
    if !windows
        .outputs
        .iter()
        .any(|endpoint| endpoint.id == playback && endpoint.is_virtual)
        || !windows
            .inputs
            .iter()
            .any(|endpoint| endpoint.id == capture && endpoint.is_virtual)
    {
        return Err("The channel needs an available virtual playback and capture pair".into());
    }
    Ok(playback)
}

fn validate_application(state: &HostState, process_id: u32) -> Result<(), String> {
    if process_id == 0 || process_id == state.host_pid {
        return Err("Anada cannot route its own audio or a system session".into());
    }
    if !state
        .windows
        .sessions
        .iter()
        .any(|session| session.process_id == process_id)
    {
        return Err(
            "The application is no longer present in Windows audio sessions. Refresh and retry"
                .into(),
        );
    }
    Ok(())
}

fn validated_process_ids(state: &HostState, mut process_ids: Vec<u32>) -> Result<Vec<u32>, String> {
    if process_ids.is_empty() || process_ids.len() > 1024 {
        return Err("Choose an application with 1-1024 audio processes".into());
    }
    process_ids.sort_unstable();
    process_ids.dedup();
    for &process_id in &process_ids {
        validate_application(state, process_id)?;
    }
    Ok(process_ids)
}

impl Controller {
    pub fn new() -> Result<Self, String> {
        Self::with_directory(ConfigStore::default_directory()?)
    }

    fn with_directory(directory: std::path::PathBuf) -> Result<Self, String> {
        let system = AudioSystem::new()?;
        let windows = system.snapshot()?;
        let (store, mut config, warnings) = ConfigStore::open(directory)?;
        let current = automatic_output(&config, &windows);
        let mut discovered = discover_outputs(&mut config, &windows, current.as_deref());
        let unnamed: Vec<_> = config
            .output_priorities
            .iter()
            .filter(|id| !config.output_names.contains_key(*id))
            .cloned()
            .collect();
        if !unnamed.is_empty()
            && let Ok(names) = system.output_names(&unnamed)
        {
            discovered |= !names.is_empty();
            config.output_names.extend(names);
        }
        if discovered {
            store.save(&config)?;
        }
        let selected_output_id = automatic_output(&config, &windows);
        let state = HostState {
            audio_diagnostics: Default::default(),
            protocol_version: PROTOCOL_VERSION,
            revision: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|error| error.to_string())?
                .as_micros() as u64,
            config,
            windows,
            selected_output_id,
            manual_output: false,
            audio_running: false,
            audio_status: "No virtual channels configured".into(),
            warnings,
            host_pid: std::process::id(),
            quiet: Default::default(),
        };
        let mut controller = Self {
            system,
            store,
            state,
            audio: AudioEngine::new(),
            last_scan: Instant::now(),
            startup_guard: true,
        };
        controller.refresh_audio();
        Ok(controller)
    }

    /// End Quiet for removed devices, outside volume changes and switched-away outputs.
    fn end_expired_quiet(&mut self) {
        for (id, restore) in crate::model::expired_quiet(&self.state) {
            self.state.quiet.remove(&id);
            if let Some(volume) = restore {
                let result = self
                    .system
                    .set_endpoint_volume(&id, volume)
                    .and_then(|()| self.refresh_endpoint(&id));
                if let Err(error) = result {
                    let warning =
                        format!("Quiet could not restore the previous output volume: {error}");
                    if self.state.warnings.len() < 32 && !self.state.warnings.contains(&warning) {
                        self.state.warnings.push(warning);
                    }
                }
            }
        }
    }

    fn set_quiet(&mut self, id: &str, on: bool) -> Result<(), String> {
        self.controlled_endpoint(id)?;
        if !on {
            if let Some(entry) = self.state.quiet.remove(id) {
                self.system.set_endpoint_volume(id, entry.restore)?;
                self.refresh_endpoint(id)?;
            }
            return Ok(());
        }
        if self.state.quiet.contains_key(id) {
            return Ok(());
        }
        let current = self
            .state
            .windows
            .outputs
            .iter()
            .find(|endpoint| endpoint.id == id)
            .map(|endpoint| endpoint.volume)
            .filter(|volume| (0.0..=1.0).contains(volume))
            .ok_or("Cannot read a valid Windows volume for this endpoint")?;
        let target = crate::model::quiet_target(current, self.state.config.quiet_volume_percent);
        if target < current {
            self.system.set_endpoint_volume(id, target)?;
        }
        self.refresh_endpoint(id)?;
        let applied = self
            .state
            .windows
            .outputs
            .iter()
            .find(|endpoint| endpoint.id == id)
            .map(|endpoint| endpoint.volume)
            .ok_or("The controlled endpoint is no longer available")?;
        self.state.quiet.insert(
            id.to_owned(),
            crate::model::QuietEntry {
                restore: current,
                applied,
            },
        );
        Ok(())
    }

    fn refresh_audio(&mut self) {
        self.end_expired_quiet();
        if let Err(error) = self.prepare_quiet_levels() {
            self.audio.stop();
            self.state.audio_running = false;
            self.state.audio_status = format!("Audio stopped: {error}");
            self.state.audio_diagnostics = self.audio.diagnostics();
            return;
        }
        let (running, status) = self.audio.reconcile(
            &self.state.config,
            &self.state.windows,
            self.state.selected_output_id.as_deref(),
        );
        self.state.audio_running = running;
        self.state.audio_status = status;
        self.state.audio_diagnostics = self.audio.diagnostics();
    }

    fn prepare_quiet_levels(&mut self) -> Result<(), String> {
        use crate::quiet_start::{QuietTarget, plan_endpoints};
        let endpoints: Vec<_> = self
            .state
            .windows
            .outputs
            .iter()
            .filter(|endpoint| {
                (endpoint.is_virtual
                    && self.state.config.channels.iter().any(|channel| {
                        channel.endpoint_id.as_deref() == Some(endpoint.id.as_str())
                            && channel.capture_id.is_some()
                    }))
                    || (!endpoint.is_virtual
                        && self.state.selected_output_id.as_deref() == Some(endpoint.id.as_str()))
            })
            .cloned()
            .collect();
        // Only a host's first setup with no saved history may lower levels.
        // Device switches, reconnects and binding edits never write volume.
        let targets = plan_endpoints(
            &endpoints,
            &self.state.config.quiet_initialized_ids,
            self.startup_guard && self.state.config.quiet_initialized_ids.is_empty(),
        )?;
        for (id, target) in &targets {
            match *target {
                QuietTarget::Decibels(db) => self.system.set_endpoint_volume_db(id, db)?,
                QuietTarget::Scalar(volume) => self.system.set_endpoint_volume(id, volume)?,
            }
        }
        if !targets.is_empty() {
            let observed = self.system.snapshot()?;
            for (id, target) in &targets {
                let endpoint = observed
                    .outputs
                    .iter()
                    .find(|endpoint| &endpoint.id == id)
                    .ok_or("Quiet start could not verify the Windows endpoint")?;
                let safe = match *target {
                    QuietTarget::Decibels(db) => endpoint.level_db.as_ref().is_some_and(|level| {
                        level.current.is_finite() && level.current <= db + 0.1
                    }),
                    QuietTarget::Scalar(value) => {
                        endpoint.volume.is_finite() && endpoint.volume <= value + 0.001
                    }
                };
                if !safe {
                    return Err(format!(
                        "Quiet start could not verify {} at a quiet level",
                        endpoint.name
                    ));
                }
            }
            self.state.windows = observed;
        }
        let mut config = self.state.config.clone();
        for endpoint in endpoints {
            if !config.quiet_initialized_ids.contains(&endpoint.id) {
                if config.quiet_initialized_ids.len() >= 4096 {
                    return Err("Quiet-start device history is full".into());
                }
                config.quiet_initialized_ids.push(endpoint.id);
            }
        }
        if config.quiet_initialized_ids != self.state.config.quiet_initialized_ids {
            self.store.save(&config)?;
            self.state.config = config;
        }
        self.startup_guard = false;
        Ok(())
    }

    fn refresh(&mut self, force: bool) -> Result<(), String> {
        if !force && self.last_scan.elapsed() < Duration::from_millis(750) {
            // Inventory scans are throttled; measurement counters must still be
            // sampled at every request rather than including old warmup data.
            self.state.audio_diagnostics = self.audio.diagnostics();
            return Ok(());
        }
        let windows = self.system.snapshot()?;
        let discovery =
            reconcile_outputs(&mut self.state, windows, |config| self.store.save(config));
        self.last_scan = Instant::now();
        self.refresh_audio();
        discovery
    }

    fn validate_bindings(&self, config: &Config) -> Result<(), String> {
        for channel in &config.channels {
            let previous = self
                .state
                .config
                .channels
                .iter()
                .find(|previous| previous.id == channel.id);
            if channel.endpoint_id != previous.and_then(|previous| previous.endpoint_id.clone())
                && let Some(id) = &channel.endpoint_id
                && !self
                    .state
                    .windows
                    .outputs
                    .iter()
                    .any(|endpoint| endpoint.id == *id && endpoint.is_virtual)
            {
                return Err("A channel must use an available virtual playback endpoint".into());
            }
            if channel.capture_id != previous.and_then(|previous| previous.capture_id.clone())
                && let Some(id) = &channel.capture_id
                && !self
                    .state
                    .windows
                    .inputs
                    .iter()
                    .any(|endpoint| endpoint.id == *id && endpoint.is_virtual)
            {
                return Err(
                    "A channel must use an available virtual capture endpoint, not a microphone"
                        .into(),
                );
            }
        }
        if config.output_priorities.iter().any(|id| {
            self.state
                .windows
                .outputs
                .iter()
                .any(|endpoint| endpoint.id == *id && endpoint.is_virtual)
        }) {
            return Err("Listening outputs cannot be virtual channel endpoints".into());
        }
        Ok(())
    }

    fn controlled_endpoint(&self, id: &str) -> Result<(), String> {
        if self.state.selected_output_id.as_deref() == Some(id)
            && self
                .state
                .windows
                .outputs
                .iter()
                .any(|endpoint| endpoint.id == id && !endpoint.is_virtual)
        {
            return Ok(());
        }
        if self
            .state
            .config
            .channels
            .iter()
            .any(|channel| channel.endpoint_id.as_deref() == Some(id))
            && self
                .state
                .windows
                .outputs
                .iter()
                .any(|endpoint| endpoint.id == id && endpoint.is_virtual)
        {
            Ok(())
        } else {
            Err("Volume controls can change only a configured virtual channel or the selected listening output".into())
        }
    }

    fn assign_applications(
        &mut self,
        process_ids: Vec<u32>,
        channel_id: &str,
    ) -> Result<(), String> {
        let process_ids = validated_process_ids(&self.state, process_ids)?;
        let endpoint =
            channel_playback(&self.state.config, &self.state.windows, channel_id)?.to_owned();
        let mut changed = Vec::new();
        for process_id in process_ids {
            if let Err(error) = self.system.set_process_playback(process_id, &endpoint) {
                return Err(format!(
                    "Routing preference failed for PID {process_id}: {error}. Preferences already changed for PIDs {changed:?}; the failed PID may have a partially changed Windows role. Existing streams may remain on their previous endpoint until the application reopens audio."
                ));
            }
            changed.push(process_id);
        }
        // Policy writes do not reliably move existing WASAPI streams. Keep the
        // observed inventory and allow the regular throttled scan to update it.
        Ok(())
    }

    fn refresh_endpoint(&mut self, id: &str) -> Result<(), String> {
        let observed = self.system.endpoint_state(id)?;
        let endpoint = self
            .state
            .windows
            .outputs
            .iter_mut()
            .find(|endpoint| endpoint.id == id)
            .ok_or("The controlled endpoint is no longer available")?;
        *endpoint = observed;
        Ok(())
    }

    fn apply(&mut self, command: Command) -> Result<(), String> {
        if matches!(command, Command::SnapshotMeters) {
            self.state.audio_diagnostics.meters = self.audio.meters();
            return Ok(());
        }
        self.refresh(false)?;
        match command {
            Command::Snapshot | Command::SnapshotMeters | Command::StopHost => Ok(()),
            Command::RepairDefaults => {
                let gaming =
                    channel_playback(&self.state.config, &self.state.windows, "gaming")?.to_owned();
                let chat =
                    channel_playback(&self.state.config, &self.state.windows, "chat")?.to_owned();
                self.system.set_default_playback(&gaming)?;
                self.system.set_default_communications_playback(&chat).map_err(|error|
                    format!("Gaming is now the ordinary default, but the Chat communications default could not be set: {error}"))?;
                self.refresh(true)
            }
            Command::AssignApplication {
                process_id,
                channel_id,
            } => self.assign_applications(vec![process_id], &channel_id),
            Command::AssignApplications {
                process_ids,
                channel_id,
            } => self.assign_applications(process_ids, &channel_id),
            Command::SaveConfig {
                mut config,
                expected_revision,
            } => {
                if expected_revision != self.state.revision {
                    return Err("Settings changed in another window. Revert edits to load the current settings, then retry.".into());
                }
                config.validate()?;
                self.validate_bindings(&config)?;
                // Discovery history is host-owned. A stale client cannot erase it.
                config.known_output_ids = self.state.config.known_output_ids.clone();
                config.output_names = self.state.config.output_names.clone();
                config.quiet_initialized_ids = self.state.config.quiet_initialized_ids.clone();
                config.output_discovery_initialized =
                    self.state.config.output_discovery_initialized;
                self.store.save(&config)?;
                self.state.config = config;
                self.state.revision += 1;
                if !self.state.manual_output {
                    self.state.selected_output_id =
                        automatic_output(&self.state.config, &self.state.windows);
                }
                self.refresh_audio();
                Ok(())
            }
            Command::SelectOutput { id } => {
                if !self
                    .state
                    .windows
                    .outputs
                    .iter()
                    .any(|endpoint| endpoint.id == id && !endpoint.is_virtual)
                {
                    return Err("Choose an available physical listening output".into());
                }
                self.state.selected_output_id = Some(id);
                self.state.manual_output = true;
                self.refresh_audio();
                Ok(())
            }
            Command::SetVolume {
                endpoint_id,
                volume,
            } => {
                self.controlled_endpoint(&endpoint_id)?;
                self.system.set_endpoint_volume(&endpoint_id, volume)?;
                // A manual level replaces Quiet; turning it off later restores nothing.
                self.state.quiet.remove(&endpoint_id);
                self.refresh_endpoint(&endpoint_id)
            }
            Command::SetVolumeDb {
                endpoint_id,
                level_db,
            } => {
                self.controlled_endpoint(&endpoint_id)?;
                self.system.set_endpoint_volume_db(&endpoint_id, level_db)?;
                self.state.quiet.remove(&endpoint_id);
                self.refresh_endpoint(&endpoint_id)
            }
            Command::SetQuiet { endpoint_id, on } => self.set_quiet(&endpoint_id, on),
            Command::SetMute { endpoint_id, muted } => {
                self.controlled_endpoint(&endpoint_id)?;
                self.system.set_endpoint_mute(&endpoint_id, muted)?;
                self.refresh_endpoint(&endpoint_id)
            }
        }
    }

    pub fn handle(&mut self, command: Command) -> Response {
        let error = self.apply(command).err();
        Response {
            state: Some(self.state.clone()),
            error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn routing_fixture() -> HostState {
        let mut config = Config::default();
        config.channels[0].endpoint_id = Some("playback".into());
        config.channels[0].capture_id = Some("capture".into());
        let endpoint = |id: &str| anada_windows::Endpoint {
            level_db: None,
            id: id.into(),
            name: id.into(),
            volume: 1.0,
            muted: false,
            is_virtual: true,
        };
        HostState {
            audio_diagnostics: Default::default(),
            protocol_version: PROTOCOL_VERSION,
            revision: 1,
            config,
            windows: WindowsSnapshot {
                outputs: vec![endpoint("playback")],
                inputs: vec![endpoint("capture")],
                sessions: vec![anada_windows::AudioSession {
                    executable_path: None,
                    process_id: 42,
                    name: "Test app".into(),
                    endpoint_id: "playback".into(),
                    active: true,
                }],
                default_output_id: None,
                communications_output_id: None,
            },
            selected_output_id: None,
            manual_output: false,
            audio_running: false,
            audio_status: String::new(),
            warnings: Vec::new(),
            host_pid: 99,
            quiet: Default::default(),
        }
    }

    fn physical_snapshot(ids: &[&str]) -> WindowsSnapshot {
        let mut windows = routing_fixture().windows;
        windows.outputs = ids
            .iter()
            .map(|id| anada_windows::Endpoint {
                level_db: None,
                id: (*id).into(),
                name: "Same name".into(),
                volume: 1.0,
                muted: false,
                is_virtual: false,
            })
            .collect();
        windows
    }

    #[test]
    fn discovery_persists_revision_and_manual_choice_expires_on_observed_change() {
        let mut state = routing_fixture();
        state.windows = physical_snapshot(&["a", "b"]);
        state.config.output_priorities = vec!["a".into(), "b".into()];
        state.config.known_output_ids = state.config.output_priorities.clone();
        state.config.output_names = [
            ("a".into(), "Same name".into()),
            ("b".into(), "Same name".into()),
        ]
        .into();
        state.selected_output_id = Some("b".into());
        state.manual_output = true;
        let old_revision = state.revision;
        let mut writes = 0;
        reconcile_outputs(&mut state, physical_snapshot(&["a", "b"]), |_| {
            writes += 1;
            Ok(())
        })
        .unwrap();
        assert!(state.manual_output);
        reconcile_outputs(
            &mut state,
            physical_snapshot(&["a", "b", "new"]),
            |config| {
                writes += 1;
                assert_eq!(config.output_priorities, ["a", "b", "new"]);
                Ok(())
            },
        )
        .unwrap();
        assert!(!state.manual_output);
        assert_eq!(state.selected_output_id.as_deref(), Some("a"));
        assert_eq!(state.revision, old_revision + 1);
        assert_eq!(writes, 1);
        // Returning to the original set cannot resurrect manual selection.
        reconcile_outputs(&mut state, physical_snapshot(&["a", "b"]), |_| {
            panic!("No write needed")
        })
        .unwrap();
        assert!(!state.manual_output);
        assert_eq!(state.selected_output_id.as_deref(), Some("a"));
    }

    #[test]
    fn discovery_leaves_unchanged_destination_and_failed_save_preserves_intent() {
        let mut state = routing_fixture();
        state.windows = physical_snapshot(&["a"]);
        state.config.output_priorities = vec!["a".into()];
        state.config.known_output_ids = vec!["a".into()];
        state.selected_output_id = Some("a".into());
        let revision = state.revision;
        state.manual_output = true;
        assert!(
            reconcile_outputs(&mut state, physical_snapshot(&["a", "new"]), |_| Err(
                "disk full".into()
            ))
            .is_err()
        );
        assert!(!state.manual_output);
        assert_eq!(state.revision, revision);
        assert_eq!(state.config.output_priorities, ["a"]);
        assert_eq!(state.selected_output_id.as_deref(), Some("a"));
        reconcile_outputs(&mut state, physical_snapshot(&["a", "new"]), |_| Ok(())).unwrap();
        assert_eq!(state.revision, revision + 1);
        assert_eq!(state.selected_output_id.as_deref(), Some("a"));
        // AudioEngine's existing same_streams comparison therefore sees the same
        // endpoint topology and does not reopen streams for discovery alone.
        reconcile_outputs(&mut state, physical_snapshot(&[]), |_| {
            panic!("Known IDs retained")
        })
        .unwrap();
        assert_eq!(state.selected_output_id, None);
        reconcile_outputs(&mut state, physical_snapshot(&["new", "a"]), |_| {
            panic!("Reconnect is known")
        })
        .unwrap();
        assert_eq!(state.selected_output_id.as_deref(), Some("a"));
    }

    #[test]
    fn group_routing_validates_every_pid_before_writes_and_deduplicates() {
        let state = routing_fixture();
        assert_eq!(validated_process_ids(&state, vec![42, 42]).unwrap(), [42]);
        for ids in [
            vec![],
            vec![42, 0],
            vec![42, 99],
            vec![42, 123],
            vec![42; 1025],
        ] {
            assert!(validated_process_ids(&state, ids).is_err());
        }
    }

    #[test]
    fn routing_requires_a_complete_available_virtual_pair() {
        let mut state = routing_fixture();
        assert_eq!(
            channel_playback(&state.config, &state.windows, "gaming").unwrap(),
            "playback"
        );
        state.windows.inputs[0].is_virtual = false;
        assert!(channel_playback(&state.config, &state.windows, "gaming").is_err());
        state.windows.inputs[0].is_virtual = true;
        state.config.channels[0].capture_id = None;
        assert!(channel_playback(&state.config, &state.windows, "gaming").is_err());
        assert!(channel_playback(&state.config, &state.windows, "unknown").is_err());
    }

    #[test]
    fn routing_rejects_host_system_and_disappeared_applications() {
        let state = routing_fixture();
        assert!(validate_application(&state, 42).is_ok());
        for process_id in [0, 99, 123] {
            assert!(validate_application(&state, process_id).is_err());
        }
    }

    #[test]
    #[ignore = "Reads real Windows audio inventory; writes only isolated temporary settings"]
    fn saves_rejects_stale_edits_and_recovers_without_ui() {
        let directory =
            std::env::temp_dir().join(format!("anada-controller-{}", std::process::id()));
        let mut controller = Controller::with_directory(directory.clone()).unwrap();
        let before = controller.handle(Command::Snapshot).state.unwrap();
        let mut config = before.config.clone();
        config.channels[0].name = "QA Games".into();
        config.channels[0].eq.bands[4].gain_db = 3.0;
        let saved = controller.handle(Command::SaveConfig {
            config: config.clone(),
            expected_revision: before.revision,
        });
        assert!(saved.error.is_none());
        let saved = saved.state.unwrap();
        assert!(saved.revision > before.revision);
        assert_eq!(saved.config.channels[0].name, "QA Games");
        let stale = controller.handle(Command::SaveConfig {
            config,
            expected_revision: before.revision,
        });
        assert!(stale.error.unwrap().contains("another window"));
        assert!(!saved.audio_running);
        let rejected = controller.handle(Command::SetVolume {
            endpoint_id: "not-a-virtual-channel".into(),
            volume: 0.0,
        });
        assert!(rejected.error.is_some());
        drop(controller);
        let mut reopened = Controller::with_directory(directory.clone()).unwrap();
        let recovered = reopened.handle(Command::Snapshot).state.unwrap();
        assert_eq!(recovered.config.channels[0].name, "QA Games");
        assert_eq!(recovered.config.channels[0].eq.bands[4].gain_db, 3.0);
        assert_ne!(recovered.revision, saved.revision);
        drop(reopened);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
