//! Sign-in startup through the per-user Windows Run key, the entry that Windows lists
//! under Startup apps. Reading the status never writes.
use std::path::Path;

#[cfg(windows)]
const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
/// Windows records here when the user turns an entry off under Startup apps.
#[cfg(windows)]
const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
#[cfg(windows)]
const VALUE: &str = "Anada";

#[cfg(any(windows, test))]
fn command(executable: &Path) -> Result<String, String> {
    if !executable.is_absolute() {
        return Err("Startup requires an absolute host executable path".into());
    }
    let executable = executable
        .to_str()
        .ok_or("Startup path is not valid Unicode")?;
    Ok(format!("\"{executable}\""))
}

/// The first byte of a StartupApproved entry is even while the entry is enabled.
#[cfg(any(windows, test))]
fn approved(state: Option<&[u8]>) -> bool {
    state
        .and_then(|state| state.first())
        .is_none_or(|flags| flags & 1 == 0)
}

#[cfg(any(windows, test))]
fn verify_change(change: Option<bool>, enabled: bool) -> Result<bool, String> {
    match change {
        Some(true) if !enabled => Err(
            "Windows did not enable Anada sign-in startup. Refresh its status, then retry.".into(),
        ),
        Some(false) if enabled => Err(
            "Windows did not remove Anada sign-in startup. Refresh its status, then retry.".into(),
        ),
        _ => Ok(enabled),
    }
}

#[cfg(windows)]
mod registry {
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR};
    use windows::Win32::System::Registry::{
        HKEY_CURRENT_USER, REG_SZ, RRF_RT_ANY, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
    };
    use windows::core::HSTRING;

    fn check(status: WIN32_ERROR, action: &str) -> Result<(), String> {
        if status == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(format!(
                "Cannot {action} Anada startup: {}",
                status.to_hresult().message()
            ))
        }
    }

    pub fn read(key: &str, name: &str) -> Result<Option<Vec<u8>>, String> {
        let (key, name) = (HSTRING::from(key), HSTRING::from(name));
        let mut size = 0u32;
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                &key,
                &name,
                RRF_RT_ANY,
                None,
                None,
                Some(&mut size),
            )
        };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        check(status, "read")?;
        let mut data = vec![0u8; size as usize];
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                &key,
                &name,
                RRF_RT_ANY,
                None,
                Some(data.as_mut_ptr().cast()),
                Some(&mut size),
            )
        };
        check(status, "read")?;
        data.truncate(size as usize);
        Ok(Some(data))
    }

    pub fn read_string(key: &str, name: &str) -> Result<Option<String>, String> {
        Ok(read(key, name)?.map(|data| {
            let wide: Vec<u16> = data
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u16::from_le_bytes(*pair))
                .collect();
            String::from_utf16_lossy(&wide)
                .trim_end_matches('\0')
                .to_owned()
        }))
    }

    pub fn write_string(key: &str, name: &str, value: &str) -> Result<(), String> {
        let wide: Vec<u16> = value.encode_utf16().chain([0]).collect();
        let status = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                &HSTRING::from(key),
                &HSTRING::from(name),
                REG_SZ.0,
                Some(wide.as_ptr().cast()),
                (wide.len() * 2) as u32,
            )
        };
        check(status, "update")
    }

    pub fn delete(key: &str, name: &str) -> Result<(), String> {
        let status = unsafe {
            RegDeleteKeyValueW(HKEY_CURRENT_USER, &HSTRING::from(key), &HSTRING::from(name))
        };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        check(status, "update")
    }
}

#[cfg(windows)]
fn apply(name: &str, executable: &Path, change: Option<bool>) -> Result<bool, String> {
    let command = command(executable)?;
    match change {
        Some(true) => {
            if !executable.is_file() {
                return Err("The Anada host executable is missing.".into());
            }
            registry::write_string(RUN, name, &command)?;
            registry::delete(APPROVED, name)?;
        }
        Some(false) => {
            registry::delete(RUN, name)?;
            registry::delete(APPROVED, name)?;
        }
        None => {}
    }
    let registered =
        registry::read_string(RUN, name)?.is_some_and(|value| value.eq_ignore_ascii_case(&command));
    let enabled = registered && approved(registry::read(APPROVED, name)?.as_deref());
    verify_change(change, enabled)
}

fn invoke(change: Option<bool>) -> Result<bool, String> {
    #[cfg(not(windows))]
    {
        let _ = change;
        Err("Anada startup requires Windows".into())
    }
    #[cfg(windows)]
    {
        if std::env::var_os("ANADA_CONFIG_DIR").is_some()
            || std::env::var_os("ANADA_INSTANCE").is_some()
        {
            return Err("Startup is unavailable in an isolated development instance".into());
        }
        let executable = std::env::current_exe()
            .map_err(|e| e.to_string())?
            .with_file_name("anada-host.exe");
        apply(VALUE, &executable, change)
    }
}

pub fn enabled() -> Result<bool, String> {
    invoke(None)
}
pub fn set_enabled(enabled: bool) -> Result<bool, String> {
    invoke(Some(enabled))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn startup_change_reports_unapplied_requests_instead_of_silent_reversion() {
        assert_eq!(verify_change(None, false), Ok(false));
        assert_eq!(verify_change(None, true), Ok(true));
        assert_eq!(verify_change(Some(true), true), Ok(true));
        assert_eq!(verify_change(Some(false), false), Ok(false));
        assert!(
            verify_change(Some(true), false)
                .unwrap_err()
                .contains("did not enable")
        );
        assert!(
            verify_change(Some(false), true)
                .unwrap_err()
                .contains("did not remove")
        );
    }
    #[test]
    fn startup_command_quotes_an_absolute_path() {
        assert_eq!(
            command(Path::new(r"C:\Program Files\O'Brien\anada-host.exe")).unwrap(),
            r#""C:\Program Files\O'Brien\anada-host.exe""#
        );
        assert!(command(Path::new("anada-host.exe")).is_err());
    }
    #[test]
    fn an_entry_turned_off_in_windows_startup_apps_reads_as_disabled() {
        assert!(approved(None));
        assert!(approved(Some(&[])));
        assert!(approved(Some(&[2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0])));
        assert!(approved(Some(&[6, 0, 0, 0])));
        assert!(!approved(Some(&[3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0])));
    }
    #[cfg(windows)]
    #[test]
    #[ignore = "Writes then removes a uniquely named per-user QA startup entry"]
    fn live_startup_entry_round_trip() {
        let executable = std::env::current_exe().unwrap();
        let name = format!(
            "Anada.StartupTest.{}.{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        struct Cleanup(String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = registry::delete(RUN, &self.0);
                let _ = registry::delete(APPROVED, &self.0);
            }
        }
        let cleanup = Cleanup(name.clone());
        assert!(!apply(&name, &executable, None).unwrap());
        assert!(apply(&name, &executable, Some(true)).unwrap());
        assert!(apply(&name, &executable, None).unwrap());
        // Windows writes this entry when the user turns the app off under Startup apps.
        let off = [3u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let status = unsafe {
            use windows::Win32::System::Registry::{
                HKEY_CURRENT_USER, REG_BINARY, RegSetKeyValueW,
            };
            use windows::core::HSTRING;
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                &HSTRING::from(APPROVED),
                &HSTRING::from(name.as_str()),
                REG_BINARY.0,
                Some(off.as_ptr().cast()),
                off.len() as u32,
            )
        };
        assert!(status.is_ok());
        assert!(!apply(&name, &executable, None).unwrap());
        assert!(apply(&name, &executable, Some(true)).unwrap());
        assert!(!apply(&name, &executable, Some(false)).unwrap());
        assert_eq!(registry::read(RUN, &name).unwrap(), None);
        drop(cleanup);
    }
}
