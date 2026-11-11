//! A UI-owned, temporary capability broker. This module never starts the audio host.
use crate::mcp_types::{ActivityRecord, Permission, Preferences, PublicSessionStatus};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    fs::{self, OpenOptions},
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};
const MAX_BYTES: usize = 1024 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(1);
#[derive(Serialize, Deserialize)]
struct Discovery {
    port: u16,
    token: String,
    session_id: String,
}
#[derive(Serialize, Deserialize)]
struct Request {
    token: String,
    op: String,
    #[serde(default)]
    tool: String,
    #[serde(default)]
    arguments: Value,
}
#[derive(Serialize, Deserialize)]
struct Reply {
    success: bool,
    result: Option<Value>,
    error: Option<String>,
}
struct State {
    enabled: bool,
    deadline: Instant,
    granted: BTreeSet<Permission>,
    activity: Vec<ActivityRecord>,
}
impl State {
    fn status(&self) -> PublicSessionStatus {
        let active = self.enabled && Instant::now() < self.deadline;
        PublicSessionStatus {
            active,
            remaining_seconds: if active {
                self.deadline
                    .saturating_duration_since(Instant::now())
                    .as_secs()
            } else {
                0
            },
            granted: if active {
                self.granted.clone()
            } else {
                BTreeSet::new()
            },
            activity: self.activity.clone(),
        }
    }
}
pub struct Session {
    state: Arc<Mutex<State>>,
    discovery: Option<PathBuf>,
}
impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}
impl Session {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                enabled: false,
                deadline: Instant::now(),
                granted: BTreeSet::new(),
                activity: Vec::new(),
            })),
            discovery: None,
        }
    }
    pub fn start(
        &mut self,
        preferences: Preferences,
        execute: impl Fn(&str, Value, &BTreeSet<Permission>) -> Result<Value, String>
        + Send
        + Sync
        + 'static,
    ) -> Result<(), String> {
        preferences.validate()?;
        self.start_at(
            crate::storage::ConfigStore::default_directory()?,
            preferences.clone(),
            Duration::from_secs(u64::from(preferences.duration_minutes) * 60),
            execute,
        )
    }
    fn start_at(
        &mut self,
        directory: PathBuf,
        preferences: Preferences,
        duration: Duration,
        execute: impl Fn(&str, Value, &BTreeSet<Permission>) -> Result<Value, String>
        + Send
        + Sync
        + 'static,
    ) -> Result<(), String> {
        if self.status().active {
            return Err("An MCP session is already active".into());
        }
        fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(directory.join("mcp-session.lock"))
            .map_err(|e| e.to_string())?;
        lock.try_lock().map_err(|_| "Another Anada window owns this MCP session. Stop it or wait for its requests to finish.".to_string())?;
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|e| e.to_string())?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let discovery = Discovery {
            port: listener.local_addr().map_err(|e| e.to_string())?.port(),
            token: random_token()?,
            session_id: random_token()?,
        };
        let path = directory.join("mcp-session.json");
        // The lock excludes all cooperating brokers, including stale discovery replacement.
        let _ = fs::remove_file(&path);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| e.to_string())?;
        if let Err(error) = protect_file(&path) {
            let _ = fs::remove_file(&path);
            return Err(error);
        }
        file.write_all(&serde_json::to_vec(&discovery).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        let state = Arc::new(Mutex::new(State {
            enabled: true,
            deadline: Instant::now() + duration,
            granted: preferences.granted,
            activity: Vec::new(),
        }));
        self.state = state.clone();
        self.discovery = Some(path.clone());
        thread::spawn(move || {
            let _lock = lock;
            loop {
                if !state.lock().unwrap().status().active {
                    break;
                }
                match listener.accept() {
                    Ok((mut stream, peer)) if peer.ip().is_loopback() => {
                        let result = handle(&mut stream, &discovery.token, &state, &execute);
                        let reply = match result {
                            Ok(value) => Reply {
                                success: true,
                                result: Some(value),
                                error: None,
                            },
                            Err(error) => Reply {
                                success: false,
                                result: None,
                                error: Some(error),
                            },
                        };
                        let _ = write_frame(&mut stream, &reply);
                    }
                    Ok(_) => (),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10))
                    }
                    Err(_) => break,
                }
            }
            state.lock().unwrap().enabled = false;
            let _ = fs::remove_file(path);
        });
        Ok(())
    }
    pub fn stop(&mut self) {
        // Hold the state guard through removal. While enabled, the worker still owns
        // the OS lock. An expired, already-cleaned owner must not delete a newer
        // window's discovery file when this old UI is eventually closed.
        let mut state = self.state.lock().unwrap();
        let owned = state.enabled;
        state.enabled = false;
        if let Some(path) = self.discovery.take()
            && owned
        {
            let _ = fs::remove_file(path);
        }
    }
    pub fn status(&self) -> PublicSessionStatus {
        self.state.lock().unwrap().status()
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.stop();
    }
}
fn handle(
    stream: &mut TcpStream,
    token: &str,
    state: &Mutex<State>,
    execute: &impl Fn(&str, Value, &BTreeSet<Permission>) -> Result<Value, String>,
) -> Result<Value, String> {
    let request: Request = read_frame(stream)?;
    // Check both authentication and revocation after reading the complete request.
    if request.token != token {
        return Err("MCP authentication failed".into());
    }
    let grants = {
        let current = state.lock().unwrap();
        let status = current.status();
        if !status.active {
            return Err("MCP session is disabled or expired".into());
        }
        if request.op == "status" {
            return serde_json::to_value(status).map_err(|e| e.to_string());
        }
        if request.op != "call" {
            return Err("Unknown MCP operation".into());
        }
        current.granted.clone()
    };
    let result = execute(&request.tool, request.arguments, &grants);
    // Store only known catalog names, never arbitrary client input or error details.
    let all_grants = Permission::ALL.into_iter().collect();
    let safe_name = if crate::mcp_tools::catalog(&all_grants)
        .iter()
        .any(|tool| tool["name"].as_str() == Some(request.tool.as_str()))
    {
        request.tool
    } else {
        "unknown_tool".into()
    };
    let mut current = state.lock().unwrap();
    current.activity.push(ActivityRecord {
        tool: safe_name,
        success: result.is_ok(),
    });
    if current.activity.len() > 30 {
        current.activity.remove(0);
    }
    result
}
fn read_frame<T: DeserializeOwned>(stream: &mut TcpStream) -> Result<T, String> {
    read_frame_with_timeout(stream, IO_TIMEOUT)
}
fn read_frame_with_timeout<T: DeserializeOwned>(
    stream: &mut TcpStream,
    timeout: Duration,
) -> Result<T, String> {
    stream
        .set_read_timeout(Some(IO_TIMEOUT))
        .map_err(|e| e.to_string())?;
    let deadline = Instant::now() + timeout;
    let mut header = [0; 4];
    read_until(stream, &mut header, deadline)?;
    let length = u32::from_le_bytes(header) as usize;
    if length == 0 || length > MAX_BYTES {
        return Err("Invalid MCP frame size".into());
    }
    let mut bytes = vec![0; length];
    read_until(stream, &mut bytes, deadline)?;
    serde_json::from_slice(&bytes).map_err(|_| "Invalid MCP request".into())
}
fn read_until(
    stream: &mut TcpStream,
    mut bytes: &mut [u8],
    deadline: Instant,
) -> Result<(), String> {
    while !bytes.is_empty() {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or("MCP read timed out")?;
        stream
            .set_read_timeout(Some(remaining))
            .map_err(|e| e.to_string())?;
        let count = stream
            .read(bytes)
            .map_err(|_| "MCP read failed or timed out")?;
        if count == 0 {
            return Err("MCP connection closed".into());
        }
        bytes = &mut bytes[count..];
    }
    Ok(())
}
fn write_frame(stream: &mut TcpStream, value: &impl Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    if bytes.len() > MAX_BYTES {
        return Err("MCP response exceeds 1 MiB".into());
    }
    let deadline = Instant::now() + IO_TIMEOUT;
    let header = (bytes.len() as u32).to_le_bytes();
    for mut part in [header.as_slice(), bytes.as_slice()] {
        while !part.is_empty() {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or("MCP write timed out")?;
            stream
                .set_write_timeout(Some(remaining))
                .map_err(|e| e.to_string())?;
            let count = stream
                .write(part)
                .map_err(|_| "MCP write failed or timed out")?;
            if count == 0 {
                return Err("MCP connection closed".into());
            }
            part = &part[count..];
        }
    }
    Ok(())
}
fn exchange_at(directory: &Path, op: &str, tool: &str, arguments: Value) -> Result<Value, String> {
    let path = directory.join("mcp-session.json");
    if fs::metadata(&path)
        .map_err(|_| "MCP is disabled. Enable a temporary session in Anada.")?
        .len()
        > 4096
    {
        return Err("Invalid MCP discovery file".into());
    }
    let discovery: Discovery =
        serde_json::from_slice(&fs::read(path).map_err(|_| "Cannot read MCP discovery file")?)
            .map_err(|_| "Invalid MCP discovery file")?;
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, discovery.port));
    let mut stream = TcpStream::connect_timeout(&address, IO_TIMEOUT)
        .map_err(|_| "MCP session is unavailable. Enable a new session in Anada.")?;
    write_frame(
        &mut stream,
        &Request {
            token: discovery.token,
            op: op.into(),
            tool: tool.into(),
            arguments,
        },
    )?;
    // Host tools may perform two or three separately bounded 3-second IPC calls.
    // The client allows their reply to finish; inbound request frames remain 1 second.
    let reply: Reply = read_frame_with_timeout(&mut stream, Duration::from_secs(10)).map_err(|error| {
        if op == "call" { format!("{error}. Tool outcome is unknown; do not automatically retry a change. Check the current state first.") } else { error }
    })?;
    match (reply.success, reply.result, reply.error) {
        (true, value, None) => Ok(value.unwrap_or(Value::Null)),
        (false, _, Some(error)) => Err(error),
        _ => Err("Invalid MCP response".into()),
    }
}
pub fn local_status() -> Result<PublicSessionStatus, String> {
    serde_json::from_value(exchange_at(
        &crate::storage::ConfigStore::default_directory()?,
        "status",
        "",
        Value::Null,
    )?)
    .map_err(|_| "Invalid MCP status".into())
}
pub fn call_local(tool: &str, arguments: Value) -> Result<Value, String> {
    exchange_at(
        &crate::storage::ConfigStore::default_directory()?,
        "call",
        tool,
        arguments,
    )
}
fn random_token() -> Result<String, String> {
    use windows::Win32::Security::Cryptography::{
        BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom,
    };
    let mut bytes = [0u8; 32];
    unsafe { BCryptGenRandom(None, &mut bytes, BCRYPT_USE_SYSTEM_PREFERRED_RNG) }
        .ok()
        .map_err(|e| e.to_string())?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
fn protect_file(path: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::{
        Win32::{
            Foundation::{HLOCAL, LocalFree},
            Security::{
                Authorization::{
                    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
                },
                DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
                PSECURITY_DESCRIPTOR, SetFileSecurityW,
            },
        },
        core::PCWSTR,
    };
    let descriptor: Vec<u16> = "D:P(A;;FA;;;OW)(A;;FA;;;SY)"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut security = PSECURITY_DESCRIPTOR::default();
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(descriptor.as_ptr()),
            SDDL_REVISION_1,
            &mut security,
            None,
        )
    }
    .map_err(|e| e.to_string())?;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let result = unsafe {
        SetFileSecurityW(
            PCWSTR(wide.as_ptr()),
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            security,
        )
    }
    .ok()
    .map_err(|e| e.to_string());
    unsafe {
        let _ = LocalFree(Some(HLOCAL(security.0)));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn dir() -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("anada-mcp-test-{}", random_token().unwrap()));
        fs::create_dir_all(&directory).unwrap();
        directory
    }
    fn start(directory: &Path, duration: Duration) -> Session {
        let mut session = Session::new();
        session
            .start_at(
                directory.to_owned(),
                Preferences::default(),
                duration,
                |name, _, _| Ok(Value::String(name.into())),
            )
            .unwrap();
        session
    }
    fn wait_disabled(directory: &Path) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while directory.join("mcp-session.json").exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
    }
    #[test]
    fn new_is_disabled_and_preferences_never_enable() {
        let session = Session::new();
        assert!(!session.status().active);
        assert!(Preferences::default().granted.is_empty());
        assert_eq!(Preferences::default().duration_minutes, 30);
    }
    #[test]
    fn ephemeral_broker_authentication_stop_and_exclusive_lock() {
        let directory = dir();
        fs::write(directory.join("mcp-session.json"), "stale").unwrap();
        let mut session = start(&directory, Duration::from_secs(10));
        assert!(session.status().active);
        assert_eq!(
            exchange_at(&directory, "call", "test_tool", Value::Null).unwrap(),
            "test_tool"
        );
        let mut second = Session::new();
        assert!(
            second
                .start_at(
                    directory.clone(),
                    Preferences::default(),
                    Duration::from_secs(10),
                    |_, _, _| Ok(Value::Null)
                )
                .is_err()
        );
        let discovery: Discovery =
            serde_json::from_slice(&fs::read(directory.join("mcp-session.json")).unwrap()).unwrap();
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, discovery.port)).unwrap();
        write_frame(
            &mut stream,
            &Request {
                token: "wrong".into(),
                op: "call".into(),
                tool: "test_tool".into(),
                arguments: Value::Null,
            },
        )
        .unwrap();
        let reply: Reply = read_frame(&mut stream).unwrap();
        assert!(reply.error.unwrap().contains("authentication"));
        session.stop();
        assert!(!session.status().active);
        assert!(exchange_at(&directory, "status", "", Value::Null).is_err());
        assert_eq!(session.status().activity.len(), 1);
        thread::sleep(Duration::from_millis(50));
        let _ = fs::remove_dir_all(directory);
    }
    #[test]
    fn expiry_and_ui_owner_drop_remove_discovery() {
        let directory = dir();
        let expired_owner = start(&directory, Duration::from_millis(40));
        thread::sleep(Duration::from_millis(70));
        assert!(!expired_owner.status().active);
        wait_disabled(&directory);
        assert!(!directory.join("mcp-session.json").exists());
        let session = start(&directory, Duration::from_secs(10));
        // Closing an old, expired UI must not revoke a different window's session.
        drop(expired_owner);
        assert!(exchange_at(&directory, "status", "", Value::Null).is_ok());
        drop(session);
        assert!(!directory.join("mcp-session.json").exists());
        thread::sleep(Duration::from_millis(40));
        let _ = fs::remove_dir_all(directory);
    }
    #[test]
    fn oversized_and_stalled_frames_are_bounded() {
        let directory = dir();
        let session = start(&directory, Duration::from_secs(10));
        let discovery: Discovery =
            serde_json::from_slice(&fs::read(directory.join("mcp-session.json")).unwrap()).unwrap();
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, discovery.port)).unwrap();
        stream
            .write_all(&((MAX_BYTES + 1) as u32).to_le_bytes())
            .unwrap();
        let reply: Reply = read_frame(&mut stream).unwrap();
        assert!(reply.error.unwrap().contains("size"));
        let _stalled = TcpStream::connect((Ipv4Addr::LOCALHOST, discovery.port)).unwrap();
        thread::sleep(Duration::from_millis(1150));
        assert!(exchange_at(&directory, "status", "", Value::Null).is_ok());
        drop(session);
        thread::sleep(Duration::from_millis(30));
        let _ = fs::remove_dir_all(directory);
    }
    #[test]
    fn activity_is_bounded_and_excludes_unknown_input_and_arguments() {
        let directory = dir();
        let session = start(&directory, Duration::from_secs(10));
        for _ in 0..35 {
            exchange_at(
                &directory,
                "call",
                "secret_in_unknown_tool",
                serde_json::json!({"secret":"never-store"}),
            )
            .unwrap();
        }
        let status = session.status();
        assert_eq!(status.activity.len(), 30);
        let serialized = serde_json::to_string(&status).unwrap();
        assert!(!serialized.contains("secret"));
        assert!(!serialized.contains("token"));
        assert!(
            status
                .activity
                .iter()
                .all(|record| record.tool == "unknown_tool")
        );
        drop(session);
        thread::sleep(Duration::from_millis(40));
        let _ = fs::remove_dir_all(directory);
    }
    #[test]
    fn stop_is_fast_and_blocks_queued_calls_while_dispatch_finishes() {
        let directory = dir();
        let mut session = Session::new();
        let (begun_tx, begun_rx) = std::sync::mpsc::channel();
        let (finish_tx, finish_rx) = std::sync::mpsc::channel();
        let finish_rx = Mutex::new(finish_rx);
        session
            .start_at(
                directory.clone(),
                Preferences::default(),
                Duration::from_secs(10),
                move |_, _, _| {
                    begun_tx.send(()).unwrap();
                    finish_rx.lock().unwrap().recv().unwrap();
                    Ok(Value::Null)
                },
            )
            .unwrap();
        let client_dir = directory.clone();
        let call =
            thread::spawn(move || exchange_at(&client_dir, "call", "test_tool", Value::Null));
        begun_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let before = Instant::now();
        session.stop();
        assert!(before.elapsed() < Duration::from_millis(100));
        assert!(exchange_at(&directory, "call", "test_tool", Value::Null).is_err());
        finish_tx.send(()).unwrap();
        assert!(call.join().unwrap().is_ok());
        thread::sleep(Duration::from_millis(40));
        let _ = fs::remove_dir_all(directory);
    }
}
