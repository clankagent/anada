use anada_dsp::EqSettings;
use anada_windows::WindowsSnapshot;
use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;
pub const EQ_GRAPH_RANGES_DB: [u8; 6] = [6, 12, 16, 24, 36, 48];

fn default_eq_graph_range_db() -> u8 {
    16
}

fn default_quiet_volume_percent() -> f32 {
    20.0
}

/// An endpoint the explicit Quiet toggle lowered. `restore` is the Windows scalar
/// volume before Quiet; `applied` is the scalar read back after lowering it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct QuietEntry {
    pub restore: f32,
    pub applied: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Channel {
    pub id: String,
    pub name: String,
    #[serde(default = "default_visible")]
    pub visible: bool,
    pub endpoint_id: Option<String>,
    pub capture_id: Option<String>,
    pub eq: EqSettings,
}

fn default_visible() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    pub schema_version: u32,
    #[serde(default = "default_eq_graph_range_db")]
    pub eq_graph_range_db: u8,
    /// Windows volume (percent) the Quiet toggle lowers a channel or output to.
    #[serde(default = "default_quiet_volume_percent")]
    pub quiet_volume_percent: f32,
    pub channels: Vec<Channel>,
    pub output_priorities: Vec<String>,
    #[serde(default)]
    pub output_names: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub quiet_initialized_ids: Vec<String>,
    /// Discovery history distinguishes a removed priority from a new device.
    #[serde(default)]
    pub known_output_ids: Vec<String>,
    #[serde(default)]
    pub output_discovery_initialized: bool,
    pub output_eq: std::collections::BTreeMap<String, EqSettings>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: 1,
            eq_graph_range_db: default_eq_graph_range_db(),
            quiet_volume_percent: default_quiet_volume_percent(),
            channels: ["Gaming", "Chat", "Media", "Aux"]
                .into_iter()
                .map(|name| Channel {
                    id: name.to_lowercase(),
                    name: name.into(),
                    visible: true,
                    endpoint_id: None,
                    capture_id: None,
                    eq: EqSettings::default(),
                })
                .collect(),
            output_priorities: Vec::new(),
            output_names: Default::default(),
            quiet_initialized_ids: Vec::new(),
            known_output_ids: Vec::new(),
            output_discovery_initialized: true,
            output_eq: Default::default(),
        }
    }
}

impl Config {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1 || self.channels.len() != 4 {
            return Err("Unsupported configuration schema or channel count".into());
        }
        if !EQ_GRAPH_RANGES_DB.contains(&self.eq_graph_range_db) {
            return Err("Unsupported EQ graph range".into());
        }
        if !(1.0..=100.0).contains(&self.quiet_volume_percent) {
            return Err("The quiet level must be 1-100%".into());
        }
        let mut ids = std::collections::HashSet::new();
        let mut endpoints = std::collections::HashSet::new();
        let mut captures = std::collections::HashSet::new();
        for channel in &self.channels {
            if !["gaming", "chat", "media", "aux"].contains(&channel.id.as_str()) {
                return Err("Channel identities must remain gaming, chat, media and aux".into());
            }
            if !ids.insert(&channel.id)
                || channel.id.is_empty()
                || channel.name.trim().is_empty()
                || channel.name.chars().count() > 48
            {
                return Err(
                    "Channel IDs must be unique and names must contain 1-48 characters".into(),
                );
            }
            if let Some(endpoint) = &channel.endpoint_id
                && !endpoints.insert(endpoint)
            {
                return Err("A playback endpoint cannot belong to two channels".into());
            }
            if let Some(capture) = &channel.capture_id
                && !captures.insert(capture)
            {
                return Err("A capture endpoint cannot belong to two channels".into());
            }
            anada_dsp::StereoEq::new(&channel.eq, 48_000)?;
        }
        for settings in self.output_eq.values() {
            anada_dsp::StereoEq::new(settings, 48_000)?;
        }
        if self.output_priorities.len() > 64 {
            return Err("Too many output priorities".into());
        }
        if self.output_names.len() > 4096
            || self.output_names.values().any(|name| name.len() > 2048)
            || self.quiet_initialized_ids.len() > 4096
            || self
                .quiet_initialized_ids
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
                != self.quiet_initialized_ids.len()
        {
            return Err("Endpoint history is oversized or invalid".into());
        }
        let known: std::collections::HashSet<_> = self.known_output_ids.iter().collect();
        if self.known_output_ids.len() > 4096 || known.len() != self.known_output_ids.len() {
            return Err("Output discovery history is oversized or contains duplicates".into());
        }
        let unique: std::collections::HashSet<_> = self.output_priorities.iter().collect();
        if unique.len() != self.output_priorities.len() {
            return Err("Output priorities contain duplicates".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HostState {
    #[serde(default)]
    pub audio_diagnostics: AudioDiagnostics,
    pub protocol_version: u32,
    pub revision: u64,
    pub config: Config,
    pub windows: WindowsSnapshot,
    pub selected_output_id: Option<String>,
    pub manual_output: bool,
    pub audio_running: bool,
    pub audio_status: String,
    pub warnings: Vec<String>,
    pub host_pid: u32,
    /// Endpoints currently lowered by the Quiet toggle, by endpoint ID. Host memory only.
    #[serde(default)]
    pub quiet: std::collections::BTreeMap<String, QuietEntry>,
}

/// Quiet never raises volume: an endpoint already below the level stays where it is.
pub fn quiet_target(current: f32, level_percent: f32) -> f32 {
    current.min(level_percent / 100.0)
}

/// Quiet entries that must end, with the volume to restore when it still makes sense.
/// A missing device or a volume changed elsewhere ends Quiet without a write.
/// A listening output that is no longer selected is restored, then released.
pub fn expired_quiet(state: &HostState) -> Vec<(String, Option<f32>)> {
    state
        .quiet
        .iter()
        .filter_map(|(id, entry)| {
            let Some(endpoint) = state.windows.outputs.iter().find(|e| e.id == *id) else {
                return Some((id.clone(), None));
            };
            if (endpoint.volume - entry.applied).abs() > 0.005 {
                return Some((id.clone(), None));
            }
            (!endpoint.is_virtual && state.selected_output_id.as_deref() != Some(id))
                .then(|| (id.clone(), Some(entry.restore)))
        })
        .collect()
}

#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub struct AudioDiagnostics {
    #[serde(default)]
    pub meters: AudioMeters,
    pub sample_rate: u32,
    pub output_frames: u64,
    pub dropped_frames: u64,
    pub underruns: u64,
    /// Recoverable backend discontinuity reports in the current stream generation.
    #[serde(default)]
    pub backend_xruns: u64,
    pub clamped_samples: u64,
    pub invalid_inputs: u64,
    pub callback_overruns: u64,
    pub callback_max_ns: u64,
    pub stream_opens: u64,
    pub stream_faults: u64,
}

/// Linear sample peaks since the last meter request (not intersample true peaks).
/// Channels are measured after channel EQ, including EQ crossfades. Output is
/// measured after output EQ and the clamp, before Windows endpoint volume/mute.
/// Zero means digital silence; convert positive values with 20*log10 for dBFS.
#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub struct AudioMeters {
    pub running: bool,
    pub channels: Vec<ChannelMeter>,
    pub output_peak: [f32; 2],
    #[serde(default)]
    pub output_spectrum: Spectrum,
}

#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub struct ChannelMeter {
    pub channel_id: String,
    pub peak: [f32; 2],
    #[serde(default)]
    pub spectrum: Spectrum,
}

/// Post-EQ stereo energy spectrum. dBFS uses sine peak amplitude, averaged as
/// L/R power (opposite polarity cannot cancel). Empty arrays mean unavailable.
#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub struct Spectrum {
    pub sample_rate: u32,
    pub frequencies_hz: Vec<f32>,
    pub dbfs: Vec<f32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Command {
    Snapshot,
    /// Read signal peaks without enumerating Windows devices or sessions.
    SnapshotMeters,
    RepairDefaults,
    AssignApplication {
        process_id: u32,
        channel_id: String,
    },
    AssignApplications {
        process_ids: Vec<u32>,
        channel_id: String,
    },
    SaveConfig {
        config: Config,
        expected_revision: u64,
    },
    SelectOutput {
        id: String,
    },
    SetVolume {
        endpoint_id: String,
        volume: f32,
    },
    SetVolumeDb {
        endpoint_id: String,
        level_db: f32,
    },
    SetMute {
        endpoint_id: String,
        muted: bool,
    },
    /// Turn Quiet on (lower to the configured level) or off (restore the earlier volume).
    SetQuiet {
        endpoint_id: String,
        on: bool,
    },
    StopHost,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Request {
    pub protocol_version: u32,
    pub command: Command,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Response {
    pub state: Option<HostState>,
    pub error: Option<String>,
}

pub fn automatic_output(config: &Config, windows: &WindowsSnapshot) -> Option<String> {
    config
        .output_priorities
        .iter()
        .find(|id| {
            windows
                .outputs
                .iter()
                .any(|endpoint| endpoint.id == **id && !endpoint.is_virtual)
        })
        .cloned()
}

/// Register new physical devices immediately below the current destination.
/// Existing priorities and deliberately unlisted known devices retain their intent.
pub fn discover_outputs(
    config: &mut Config,
    windows: &WindowsSnapshot,
    current: Option<&str>,
) -> bool {
    let before_priorities = config.output_priorities.clone();
    let before_known = config.known_output_ids.clone();
    let before_names = config.output_names.clone();
    for endpoint in windows
        .outputs
        .iter()
        .filter(|endpoint| !endpoint.is_virtual)
    {
        if config.output_names.len() < 4096 || config.output_names.contains_key(&endpoint.id) {
            config
                .output_names
                .insert(endpoint.id.clone(), endpoint.name.clone());
        }
    }
    if !config.output_discovery_initialized {
        // Existing files have no record of why a visible output was unlisted.
        // Preserve that exclusion on upgrade. Only truly new setup auto-adds all.
        for id in config.output_priorities.iter().chain(
            windows
                .outputs
                .iter()
                .filter(|endpoint| !endpoint.is_virtual)
                .map(|endpoint| &endpoint.id),
        ) {
            if !config.known_output_ids.contains(id) && config.known_output_ids.len() < 4096 {
                config.known_output_ids.push(id.clone());
            }
        }
        config.output_discovery_initialized = true;
        return true;
    }
    // Migrate older settings: even absent saved devices are already known.
    for id in &config.output_priorities {
        if !config.known_output_ids.contains(id) && config.known_output_ids.len() < 4096 {
            config.known_output_ids.push(id.clone());
        }
    }
    let mut insertion = current
        .and_then(|id| {
            config
                .output_priorities
                .iter()
                .position(|saved| saved == id)
        })
        .map_or(config.output_priorities.len(), |index| index + 1);
    for endpoint in windows
        .outputs
        .iter()
        .filter(|endpoint| !endpoint.is_virtual)
    {
        if config.known_output_ids.contains(&endpoint.id)
            || config.output_priorities.contains(&endpoint.id)
        {
            continue;
        }
        // A full list cannot accept discovery. Leave it unknown so a later scan
        // can retry after the user makes room, without discarding saved intent.
        if config.output_priorities.len() == 64 || config.known_output_ids.len() == 4096 {
            break;
        }
        config
            .output_priorities
            .insert(insertion, endpoint.id.clone());
        insertion += 1;
        config.known_output_ids.push(endpoint.id.clone());
    }
    before_priorities != config.output_priorities
        || before_known != config.known_output_ids
        || before_names != config.output_names
}

pub fn availability(windows: &WindowsSnapshot) -> Vec<String> {
    let mut result: Vec<_> = windows
        .outputs
        .iter()
        .map(|endpoint| endpoint.id.clone())
        .collect();
    result.sort();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inventory(ids: &[&str]) -> WindowsSnapshot {
        WindowsSnapshot {
            outputs: ids
                .iter()
                .map(|id| anada_windows::Endpoint {
                    level_db: None,
                    id: (*id).into(),
                    name: "Duplicate display name".into(),
                    volume: 1.0,
                    muted: false,
                    is_virtual: *id == "virtual",
                })
                .collect(),
            inputs: Vec::new(),
            sessions: Vec::new(),
            default_output_id: None,
            communications_output_id: None,
        }
    }

    #[test]
    fn discovery_inserts_below_current_without_reordering_known_devices() {
        for current in ["a", "b"] {
            let mut config = Config {
                output_priorities: vec!["a".into(), "b".into(), "c".into()],
                ..Default::default()
            };
            let windows = inventory(&["a", "b", "c", "new-one", "new-two", "virtual"]);
            assert!(discover_outputs(&mut config, &windows, Some(current)));
            let expected = if current == "a" {
                vec!["a", "new-one", "new-two", "b", "c"]
            } else {
                vec!["a", "b", "new-one", "new-two", "c"]
            };
            assert_eq!(config.output_priorities, expected);
            assert!(!discover_outputs(&mut config, &windows, Some(current)));
            assert!(!config.known_output_ids.iter().any(|id| id == "virtual"));
        }
    }

    #[test]
    fn first_setup_discovers_by_id_and_known_exclusions_survive_reconnect() {
        let mut config = Config::default();
        let windows = inventory(&["one", "two", "one", "virtual"]);
        discover_outputs(&mut config, &windows, None);
        assert_eq!(config.output_priorities, ["one", "two"]);
        assert_eq!(automatic_output(&config, &windows).as_deref(), Some("one"));
        config.output_priorities.clear();
        let mut restarted: Config =
            serde_json::from_slice(&serde_json::to_vec(&config).unwrap()).unwrap();
        assert!(!discover_outputs(&mut restarted, &inventory(&[]), None));
        assert!(!discover_outputs(&mut restarted, &windows, None));
        assert_eq!(automatic_output(&restarted, &windows), None);
    }

    #[test]
    fn legacy_upgrade_preserves_visible_exclusions_and_later_discovers_new_devices() {
        let config = Config {
            output_priorities: vec!["disconnected-headset".into()],
            ..Default::default()
        };
        let mut legacy = serde_json::to_value(config).unwrap();
        legacy.as_object_mut().unwrap().remove("known_output_ids");
        legacy
            .as_object_mut()
            .unwrap()
            .remove("output_discovery_initialized");
        let mut migrated: Config = serde_json::from_value(legacy).unwrap();
        let visible = inventory(&["excluded-speakers", "virtual"]);
        assert!(discover_outputs(&mut migrated, &visible, None));
        assert_eq!(migrated.output_priorities, ["disconnected-headset"]);
        assert_eq!(automatic_output(&migrated, &visible), None);
        assert!(
            migrated
                .known_output_ids
                .contains(&"excluded-speakers".into())
        );
        assert!(!discover_outputs(&mut migrated, &visible, None));
        let new = inventory(&["excluded-speakers", "new-usb", "virtual"]);
        assert!(discover_outputs(&mut migrated, &new, None));
        assert_eq!(
            migrated.output_priorities,
            ["disconnected-headset", "new-usb"]
        );
        assert_eq!(
            automatic_output(&migrated, &new).as_deref(),
            Some("new-usb")
        );
        let mut restarted: Config =
            serde_json::from_slice(&serde_json::to_vec(&migrated).unwrap()).unwrap();
        assert!(!discover_outputs(&mut restarted, &new, None));
    }

    #[test]
    fn discovery_without_current_appends_and_retains_absent_saved_rank() {
        let mut config = Config {
            output_priorities: vec!["absent".into()],
            ..Default::default()
        };
        discover_outputs(&mut config, &inventory(&["new"]), None);
        assert_eq!(config.output_priorities, ["absent", "new"]);
        discover_outputs(&mut config, &inventory(&["absent", "new"]), Some("new"));
        assert_eq!(config.output_priorities, ["absent", "new"]);
        assert_eq!(
            automatic_output(&config, &inventory(&["absent", "new"])).as_deref(),
            Some("absent")
        );
    }

    #[test]
    fn legacy_visibility_defaults_true_and_hiding_preserves_binding() {
        let mut config = Config::default();
        config.channels[0].endpoint_id = Some("playback".into());
        config.channels[0].capture_id = Some("capture".into());
        let mut value = serde_json::to_value(&config).unwrap();
        for channel in value["channels"].as_array_mut().unwrap() {
            channel.as_object_mut().unwrap().remove("visible");
        }
        let mut legacy: Config = serde_json::from_value(value).unwrap();
        assert!(legacy.channels.iter().all(|channel| channel.visible));
        legacy.channels[0].visible = false;
        assert!(legacy.validate().is_ok());
        assert_eq!(legacy.channels[0].id, "gaming");
        assert_eq!(legacy.channels[0].endpoint_id.as_deref(), Some("playback"));
        assert_eq!(legacy.channels[0].capture_id.as_deref(), Some("capture"));
    }

    #[test]
    fn discovery_history_rejects_duplicates_and_is_bounded() {
        let mut config = Config {
            known_output_ids: vec!["same".into(), "same".into()],
            ..Default::default()
        };
        assert!(config.validate().is_err());
        config.known_output_ids = (0..4097).map(|index| index.to_string()).collect();
        assert!(config.validate().is_err());
    }

    #[test]
    fn quiet_never_raises_and_ends_on_device_switch_removal_or_outside_change() {
        assert_eq!(quiet_target(0.8, 20.0), 0.2);
        assert_eq!(quiet_target(0.1, 20.0), 0.1);
        let mut windows = inventory(&["a", "b", "virtual"]);
        for endpoint in &mut windows.outputs {
            endpoint.volume = 0.2;
        }
        let entry = QuietEntry {
            restore: 0.8,
            applied: 0.2,
        };
        let mut state = HostState {
            audio_diagnostics: Default::default(),
            protocol_version: PROTOCOL_VERSION,
            revision: 1,
            config: Config::default(),
            windows,
            selected_output_id: Some("a".into()),
            manual_output: false,
            audio_running: false,
            audio_status: String::new(),
            warnings: Vec::new(),
            host_pid: 1,
            quiet: [("a".into(), entry), ("virtual".into(), entry)].into(),
        };
        assert!(
            expired_quiet(&state).is_empty(),
            "selected output and channels stay quiet"
        );
        state.selected_output_id = Some("b".into());
        assert_eq!(expired_quiet(&state), [("a".to_owned(), Some(0.8))]);
        state.selected_output_id = Some("a".into());
        state.windows.outputs[0].volume = 0.5;
        assert_eq!(expired_quiet(&state), [("a".to_owned(), None)]);
        state.windows.outputs.remove(0);
        state.windows.outputs.pop();
        // BTreeMap order: endpoint IDs sort alphabetically.
        assert_eq!(
            expired_quiet(&state),
            [("a".to_owned(), None), ("virtual".to_owned(), None)]
        );
    }

    #[test]
    fn quiet_level_defaults_for_older_settings_and_is_bounded() {
        let mut json = serde_json::to_value(Config::default()).unwrap();
        json.as_object_mut().unwrap().remove("quiet_volume_percent");
        let mut migrated: Config = serde_json::from_value(json).unwrap();
        assert_eq!(migrated.quiet_volume_percent, 20.0);
        for bad in [0.0, 101.0, f32::NAN] {
            migrated.quiet_volume_percent = bad;
            assert!(migrated.validate().is_err());
        }
    }

    #[test]
    fn defaults_have_four_distinct_channels() {
        let config = Config::default();
        assert_eq!(config.channels.len(), 4);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn older_settings_use_sixteen_db_graph_range_without_changing_eq() {
        let mut original = Config::default();
        original.channels[0].eq.bands[4].gain_db = 24.0;
        let mut json = serde_json::to_value(&original).unwrap();
        json.as_object_mut().unwrap().remove("eq_graph_range_db");
        let migrated: Config = serde_json::from_value(json).unwrap();
        assert_eq!(migrated.eq_graph_range_db, 16);
        assert_eq!(migrated.channels[0].eq, original.channels[0].eq);
        assert!(migrated.validate().is_ok());
    }

    #[test]
    fn duplicate_endpoint_is_rejected() {
        let mut config = Config::default();
        config.channels[0].endpoint_id = Some("same".into());
        config.channels[1].endpoint_id = Some("same".into());
        assert!(config.validate().is_err());
    }

    #[test]
    fn malformed_settings_are_rejected() {
        let mut config = Config::default();
        config.channels[0].eq.preamp_db = f32::NAN;
        assert!(config.validate().is_err());
        config.channels[0].eq.preamp_db = 0.0;
        config.channels[1].name = " ".into();
        assert!(config.validate().is_err());
    }

    #[test]
    fn rename_and_reorder_keep_channel_identity() {
        let mut config = Config::default();
        config.channels[0].name = "Games".into();
        config.channels.swap(0, 3);
        assert!(config.validate().is_ok());
        assert_eq!(config.channels[3].id, "gaming");
        config.channels[3].id = "replacement".into();
        assert!(config.validate().is_err());
    }

    #[test]
    fn automatic_selection_never_falls_back_to_unlisted_or_virtual_output() {
        use anada_windows::Endpoint;
        let windows = WindowsSnapshot {
            outputs: vec![
                Endpoint {
                    level_db: None,
                    id: "cable".into(),
                    name: "Virtual Audio Cable".into(),
                    volume: 1.0,
                    muted: false,
                    is_virtual: true,
                },
                Endpoint {
                    level_db: None,
                    id: "physical".into(),
                    name: "Headset".into(),
                    volume: 1.0,
                    muted: false,
                    is_virtual: false,
                },
            ],
            inputs: Vec::new(),
            sessions: Vec::new(),
            default_output_id: Some("physical".into()),
            communications_output_id: None,
        };
        let mut config = Config::default();
        assert_eq!(automatic_output(&config, &windows), None);
        config.output_priorities = vec!["missing".into(), "cable".into()];
        assert_eq!(automatic_output(&config, &windows), None);
        config.output_priorities.push("physical".into());
        assert_eq!(
            automatic_output(&config, &windows).as_deref(),
            Some("physical")
        );
    }
}
