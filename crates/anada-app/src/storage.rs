use crate::model::Config;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use windows::Win32::Storage::FileSystem::{
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
};
use windows::core::PCWSTR;

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const MAX_CONFIG_BYTES: u64 = 1_048_576;

pub struct ConfigStore {
    directory: PathBuf,
}

impl ConfigStore {
    pub fn default_directory() -> Result<PathBuf, String> {
        if let Some(directory) = std::env::var_os("ANADA_CONFIG_DIR") {
            return Ok(PathBuf::from(directory));
        }
        std::env::var_os("LOCALAPPDATA")
            .map(|directory| PathBuf::from(directory).join("Anada"))
            .ok_or_else(|| "Windows LOCALAPPDATA is not set".into())
    }

    pub fn open(directory: PathBuf) -> Result<(Self, Config, Vec<String>), String> {
        fs::create_dir_all(&directory)
            .map_err(|error| format!("Create settings folder: {error}"))?;
        let store = Self { directory };
        let primary = store.directory.join("settings.json");
        let backup = store.directory.join("settings.backup.json");
        match read_config(&primary) {
            Ok(config) => Ok((store, config, Vec::new())),
            Err(primary_error) if primary.exists() || backup.exists() => {
                let config = read_config(&backup).map_err(|backup_error| {
                    format!("Settings need repair. Original files were preserved. Primary: {primary_error}. Backup: {backup_error}")
                })?;
                Ok((
                    store,
                    config,
                    vec![format!(
                        "Recovered settings from backup. Original settings file is unchanged: {primary_error}"
                    )],
                ))
            }
            Err(_) => Ok((store, Config::default(), Vec::new())),
        }
    }

    pub fn save(&self, config: &Config) -> Result<(), String> {
        config.validate()?;
        let primary = self.directory.join("settings.json");
        let backup = self.directory.join("settings.backup.json");
        let encoded = serde_json::to_vec_pretty(config).map_err(|error| error.to_string())?;
        if encoded.len() as u64 > MAX_CONFIG_BYTES {
            return Err("Settings exceed the 1 MiB limit".into());
        }
        if primary.exists() {
            match read_config(&primary) {
                Ok(previous) => {
                    let previous =
                        serde_json::to_vec_pretty(&previous).map_err(|error| error.to_string())?;
                    atomic_write(&backup, &previous)?;
                }
                Err(_) => {
                    let preserved = self.directory.join(format!(
                        "settings.damaged.{}.{}.json",
                        std::process::id(),
                        TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
                    ));
                    fs::copy(&primary, &preserved)
                        .map_err(|error| format!("Preserve damaged settings: {error}"))?;
                }
            }
        }
        atomic_write(&primary, &encoded)
    }
}

fn read_config(path: &Path) -> Result<Config, String> {
    let metadata = fs::metadata(path).map_err(|error| error.to_string())?;
    if metadata.len() > MAX_CONFIG_BYTES {
        return Err("Settings exceed the 1 MiB limit".into());
    }
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    let config: Config = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    config.validate()?;
    Ok(config)
}

pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    let temporary = path.with_extension(format!(
        "{}.{}.tmp",
        std::process::id(),
        TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| format!("Create settings transaction: {error}"))?;
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|error| format!("Write settings transaction: {error}"))?;
        drop(file);
        let source: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
        let destination: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        unsafe {
            MoveFileExW(
                PCWSTR(source.as_ptr()),
                PCWSTR(destination.as_ptr()),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        }
        .map_err(|error| format!("Commit settings transaction: {error}"))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_directory() -> PathBuf {
        std::env::temp_dir().join(format!(
            "anada-storage-{}-{}",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn saves_and_recovers_previous_valid_settings() {
        let directory = test_directory();
        let (store, mut config, _) = ConfigStore::open(directory.clone()).unwrap();
        store.save(&config).unwrap();
        config.channels[0].name = "My game".into();
        config.eq_graph_range_db = 12;
        store.save(&config).unwrap();
        let (_, loaded, warnings) = ConfigStore::open(directory.clone()).unwrap();
        assert_eq!(loaded.channels[0].name, "My game");
        assert_eq!(loaded.eq_graph_range_db, 12);
        assert!(warnings.is_empty());
        fs::write(directory.join("settings.json"), b"damaged").unwrap();
        let (_, recovered, warnings) = ConfigStore::open(directory.clone()).unwrap();
        assert_eq!(recovered.channels[0].name, "Gaming");
        assert_eq!(warnings.len(), 1);
        assert_eq!(
            fs::read(directory.join("settings.json")).unwrap(),
            b"damaged"
        );
        store.save(&recovered).unwrap();
        assert!(fs::read_dir(&directory).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains("damaged.")
        }));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn malformed_settings_do_not_silently_reset() {
        let directory = test_directory();
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("settings.json"), b"{invalid").unwrap();
        assert!(ConfigStore::open(directory.clone()).is_err());
        assert_eq!(
            fs::read(directory.join("settings.json")).unwrap(),
            b"{invalid"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn invalid_edits_do_not_replace_saved_settings() {
        let directory = test_directory();
        let (store, mut config, _) = ConfigStore::open(directory.clone()).unwrap();
        store.save(&config).unwrap();
        config.channels.clear();
        assert!(store.save(&config).is_err());
        let (_, loaded, _) = ConfigStore::open(directory.clone()).unwrap();
        assert_eq!(loaded.channels.len(), 4);
        fs::remove_dir_all(directory).unwrap();
    }
}
