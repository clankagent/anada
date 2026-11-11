use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

macro_rules! permissions {
    ($( $name:ident => ($label:literal, $group:literal, $description:literal)),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum Permission { $($name),+ }
        impl Permission {
            pub const ALL: [Self; 25] = [$(Self::$name),+];
            pub fn label(self) -> &'static str { match self { $(Self::$name => $label),+ } }
            pub fn group(self) -> &'static str { match self { $(Self::$name => $group),+ } }
            pub fn description(self) -> &'static str { match self { $(Self::$name => $description),+ } }
        }
    }
}
permissions! {
 ReadStatus => ("Status", "Read", "Read host health and connection status."),
 ReadDevices => ("Devices", "Read", "Read audio device names and identities."),
 ReadApplications => ("Applications", "Read", "Read application names and routing."),
 ReadEq => ("Equalizers", "Read", "Read channel and output equalizer settings."),
 ReadLevels => ("Signal levels", "Read", "Read channel and output signal meters."),
 ReadSpectrum => ("Spectrum", "Read", "Read frequency spectrum data."),
 ReadPresets => ("Presets", "Read", "Read saved equalizer presets."),
 ReadChannelSettings => ("Channel settings", "Read", "Read channel names, visibility and bindings."),
 ReadOutputSettings => ("Output settings", "Read", "Read output selection and priorities."),
 WriteChannelEq => ("Channel EQ", "Equalizers and presets", "Change channel EQ bands, preamp and bypass; reset EQ or recall permitted presets."),
 WriteOutputEq => ("Output EQ", "Equalizers and presets", "Change output EQ bands, preamp and bypass; reset EQ or recall permitted presets."),
 SavePresets => ("Save presets", "Equalizers and presets", "Create or replace equalizer presets."),
 DeletePresets => ("Delete presets", "Equalizers and presets", "Delete saved equalizer presets."),
 ChannelVolume => ("Channel volume", "Volume and mute", "Change Windows channel endpoint volume."),
 OutputVolume => ("Output volume", "Volume and mute", "Change Windows output endpoint volume."),
 Mute => ("Mute", "Volume and mute", "Mute or unmute channel and output endpoints."),
 RouteApplications => ("Application routing", "Routing", "Assign applications to channels."),
 SelectOutput => ("Select output", "Routing", "Change the listening output."),
 OutputPriorities => ("Output priorities", "Routing", "Change automatic output priorities."),
 ChannelNames => ("Channel names", "Channels", "Rename channels."),
 ChannelVisibility => ("Channel visibility", "Channels", "Show or hide channels."),
 ChannelBindings => ("Channel bindings", "Channels", "Change channel endpoint bindings."),
 RepairWindowsDefaults => ("Repair Windows defaults", "System", "Explicitly repair Windows default audio routing."),
 Startup => ("Sign-in startup", "System", "Change Anada sign-in startup."),
 StopHost => ("Stop audio host", "System", "Stop Anada's audio host.")
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Preferences {
    pub granted: BTreeSet<Permission>,
    pub duration_minutes: u16,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            granted: BTreeSet::new(),
            duration_minutes: 30,
        }
    }
}
impl Preferences {
    pub fn validate(&self) -> Result<(), String> {
        if ![15, 30, 60, 120].contains(&self.duration_minutes) {
            return Err("Choose a 15, 30, 60 or 120 minute session".into());
        }
        Ok(())
    }
    pub fn load() -> Result<Self, String> {
        let path = crate::storage::ConfigStore::default_directory()?.join("mcp-preferences.json");
        if !path.exists() {
            return Ok(Self::default());
        }
        if std::fs::metadata(&path).map_err(|e| e.to_string())?.len() > 65536 {
            return Err("MCP preferences are too large".into());
        }
        let value: Self = serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        value.validate()?;
        Ok(value)
    }
    pub fn save(&self) -> Result<(), String> {
        self.validate()?;
        let dir = crate::storage::ConfigStore::default_directory()?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        crate::storage::atomic_write(
            &dir.join("mcp-preferences.json"),
            &serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?,
        )
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActivityRecord {
    pub tool: String,
    pub success: bool,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PublicSessionStatus {
    pub active: bool,
    pub remaining_seconds: u64,
    pub granted: BTreeSet<Permission>,
    pub activity: Vec<ActivityRecord>,
}
