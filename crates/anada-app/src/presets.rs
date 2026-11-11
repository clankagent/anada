use anada_dsp::{EqSettings, StereoEq};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::os::windows::fs::OpenOptionsExt;
use std::path::PathBuf;

const MAX_BYTES: u64 = 1_048_576;
const MAX_PRESETS: usize = 128;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EqPreset {
    pub name: String,
    pub eq: EqSettings,
}

#[derive(Default, Serialize, Deserialize)]
struct Library {
    schema_version: u32,
    presets: Vec<EqPreset>,
}

pub struct PresetStore {
    directory: PathBuf,
}

impl PresetStore {
    pub fn default_store() -> Result<Self, String> {
        Ok(Self::new(crate::storage::ConfigStore::default_directory()?))
    }

    pub fn new(directory: PathBuf) -> Self {
        Self { directory }
    }

    fn read(&self) -> Result<Library, String> {
        let path = self.directory.join("eq-presets.json");
        let file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Library {
                    schema_version: 1,
                    presets: Vec::new(),
                });
            }
            Err(error) => return Err(format!("Open EQ presets: {error}")),
        };
        let mut bytes = Vec::new();
        file.take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("Read EQ presets: {error}"))?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err("EQ preset library exceeds 1 MiB; original file preserved".into());
        }
        let library: Library = serde_json::from_slice(&bytes).map_err(|error| {
            format!("EQ preset library needs repair; original file preserved: {error}")
        })?;
        validate_library(&library)?;
        Ok(library)
    }

    pub fn list(&self) -> Result<Vec<EqPreset>, String> {
        Ok(self.read()?.presets)
    }

    pub fn remove(&self, name: &str) -> Result<Vec<EqPreset>, String> {
        validate_name(name)?;
        // Match save's exclusive transaction so another window cannot lose a preset.
        fs::create_dir_all(&self.directory)
            .map_err(|error| format!("Create preset folder: {error}"))?;
        let _lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .share_mode(0)
            .open(self.directory.join("eq-presets.lock"))
            .map_err(|error| format!("Another window may be saving presets; retry: {error}"))?;
        let mut library = self.read()?;
        let index = library
            .presets
            .iter()
            .position(|preset| preset.name == name)
            .ok_or("This preset no longer exists. Refresh the library")?;
        library.presets.remove(index);
        let bytes = serde_json::to_vec_pretty(&library).map_err(|error| error.to_string())?;
        crate::storage::atomic_write(&self.directory.join("eq-presets.json"), &bytes)?;
        Ok(library.presets)
    }

    pub fn save(
        &self,
        name: &str,
        eq: &EqSettings,
        replace: bool,
    ) -> Result<Vec<EqPreset>, String> {
        validate_name(name)?;
        StereoEq::new(eq, 48_000)?;
        fs::create_dir_all(&self.directory)
            .map_err(|error| format!("Create preset folder: {error}"))?;
        let _lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .share_mode(0)
            .open(self.directory.join("eq-presets.lock"))
            .map_err(|error| format!("Another window may be saving presets; retry: {error}"))?;
        let mut library = self.read()?;
        let preset = EqPreset {
            name: name.to_owned(),
            eq: eq.clone(),
        };
        if let Some(existing) = library.presets.iter_mut().find(|saved| saved.name == name) {
            if !replace {
                return Err(
                    "A preset has this name. Use Replace preset to replace it explicitly".into(),
                );
            }
            *existing = preset;
        } else {
            if library.presets.len() == MAX_PRESETS {
                return Err("The local library already contains 128 presets".into());
            }
            library.presets.push(preset);
        }
        library
            .presets
            .sort_by(|left, right| left.name.cmp(&right.name));
        validate_library(&library)?;
        let bytes = serde_json::to_vec_pretty(&library).map_err(|error| error.to_string())?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err("EQ preset library exceeds 1 MiB; no changes saved".into());
        }
        crate::storage::atomic_write(&self.directory.join("eq-presets.json"), &bytes)?;
        Ok(library.presets)
    }
}

fn validate_name(name: &str) -> Result<(), String> {
    if name.trim() != name
        || name.is_empty()
        || name.chars().count() > 64
        || name.chars().any(char::is_control)
    {
        return Err(
            "Preset names need 1–64 characters, no surrounding spaces or control characters".into(),
        );
    }
    Ok(())
}

fn validate_library(library: &Library) -> Result<(), String> {
    if library.schema_version != 1 || library.presets.len() > MAX_PRESETS {
        return Err("Unsupported or oversized EQ preset library; original file preserved".into());
    }
    let mut names = std::collections::HashSet::new();
    for preset in &library.presets {
        validate_name(&preset.name)?;
        if !names.insert(&preset.name) {
            return Err("Duplicate EQ preset names; original file preserved".into());
        }
        StereoEq::new(&preset.eq, 48_000)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn directory() -> PathBuf {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "anada-presets-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn save_recall_and_explicit_replace_preserve_all_eq_settings() {
        let directory = directory();
        let store = PresetStore::new(directory.clone());
        assert!(store.list().unwrap().is_empty());
        let mut eq = EqSettings {
            bypass: true,
            preamp_db: -6.0,
            ..Default::default()
        };
        eq.bands[2].gain_db = 4.0;
        store.save("Voice", &eq, false).unwrap();
        assert_eq!(store.list().unwrap()[0].eq, eq);
        assert!(store.save("Voice", &EqSettings::default(), false).is_err());
        assert_eq!(store.list().unwrap()[0].eq, eq);
        store.save("Voice", &EqSettings::default(), true).unwrap();
        assert_eq!(store.list().unwrap()[0].eq, EqSettings::default());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn malformed_library_is_not_overwritten() {
        let directory = directory();
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("eq-presets.json");
        fs::write(&path, b"broken original").unwrap();
        let store = PresetStore::new(directory.clone());
        assert!(store.save("New", &EqSettings::default(), false).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"broken original");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn invalid_names_and_eq_cannot_change_existing_library() {
        let directory = directory();
        let store = PresetStore::new(directory.clone());
        for name in ["", " leading", "trailing ", "line\nbreak"] {
            assert!(store.save(name, &EqSettings::default(), false).is_err());
        }
        let invalid = EqSettings {
            preamp_db: f32::NAN,
            ..Default::default()
        };
        assert!(store.save("Invalid", &invalid, false).is_err());
        assert!(!directory.exists());
    }

    #[test]
    fn stores_read_current_library_and_lock_rejects_concurrent_writer() {
        let directory = directory();
        let first = PresetStore::new(directory.clone());
        let second = PresetStore::new(directory.clone());
        first.save("First", &EqSettings::default(), false).unwrap();
        second
            .save("Second", &EqSettings::default(), false)
            .unwrap();
        assert_eq!(first.list().unwrap().len(), 2);
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0)
            .open(directory.join("eq-presets.lock"))
            .unwrap();
        assert!(first.save("Third", &EqSettings::default(), false).is_err());
        drop(lock);
        assert_eq!(first.list().unwrap().len(), 2);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn removal_preserves_other_presets_and_rejects_stale_or_damaged_library() {
        let directory = directory();
        let store = PresetStore::new(directory.clone());
        store.save("First", &EqSettings::default(), false).unwrap();
        store.save("Second", &EqSettings::default(), false).unwrap();
        let remaining = store.remove("First").unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].name, "Second");
        let path = directory.join("eq-presets.json");
        let saved = fs::read(&path).unwrap();
        assert!(store.remove("First").is_err());
        assert_eq!(fs::read(&path).unwrap(), saved);
        fs::write(&path, b"damaged").unwrap();
        assert!(store.remove("Second").is_err());
        assert_eq!(fs::read(&path).unwrap(), b"damaged");
        fs::remove_dir_all(directory).unwrap();
    }
}
