use crate::model::{Command, HostState, PROTOCOL_VERSION, Request, Response};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::io::{self, Write};
use std::time::{Duration, Instant};

const MAX_FRAME_BYTES: usize = 1024 * 1024;
const MAX_PAYLOAD_BYTES: usize = MAX_FRAME_BYTES - 4;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);
const CLIENT_TIMEOUT: Duration = Duration::from_secs(2);
const START_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(5);
const IDLE_INTERVAL: Duration = Duration::from_secs(1);
const ACK: u8 = 0x06;

#[derive(Serialize, Deserialize)]
struct Reply {
    protocol_version: u32,
    response: Response,
}

#[derive(Default)]
struct LimitedBuffer(Vec<u8>);

impl Write for LimitedBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_PAYLOAD_BYTES.saturating_sub(self.0.len()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "IPC frame exceeds 1 MiB",
            ));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn encode<T: Serialize>(message: &T) -> Result<Vec<u8>, String> {
    let mut payload = LimitedBuffer::default();
    serde_json::to_writer(&mut payload, message).map_err(|error| error.to_string())?;
    let mut frame = Vec::with_capacity(payload.0.len() + 4);
    frame.extend_from_slice(&(payload.0.len() as u32).to_le_bytes());
    frame.extend_from_slice(&payload.0);
    Ok(frame)
}

fn payload_length(header: [u8; 4]) -> Result<usize, String> {
    let length = u32::from_le_bytes(header) as usize;
    if length == 0 || length > MAX_PAYLOAD_BYTES {
        return Err("Invalid IPC frame length".into());
    }
    Ok(length)
}

fn decode<T: DeserializeOwned>(payload: &[u8]) -> Result<T, String> {
    if payload.is_empty() || payload.len() > MAX_PAYLOAD_BYTES {
        return Err("Invalid IPC payload length".into());
    }
    serde_json::from_slice(payload).map_err(|error| format!("Invalid IPC JSON: {error}"))
}

pub fn request(command: Command) -> Result<HostState, String> {
    platform::request_until(command, Instant::now() + REQUEST_TIMEOUT)
}

fn validate_reply(reply: Reply) -> Result<HostState, String> {
    if reply.protocol_version != PROTOCOL_VERSION {
        return Err("Anada host uses an incompatible IPC protocol".into());
    }
    match (reply.response.state, reply.response.error) {
        (Some(state), None) if state.protocol_version == PROTOCOL_VERSION => Ok(state),
        (_, Some(error)) => Err(error),
        _ => Err("Anada host returned an invalid response".into()),
    }
}

pub fn ensure_host() -> Result<(), String> {
    platform::ensure_host()
}

pub fn serve(handler: impl FnMut(Command) -> Response) -> Result<(), String> {
    platform::serve(handler)
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::ffi::c_void;
    use std::mem::size_of;
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Foundation::{
        CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_NO_DATA, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED,
        ERROR_PIPE_LISTENING, GENERIC_READ, GENERIC_WRITE, HANDLE, HLOCAL, INVALID_HANDLE_VALUE,
        LocalFree,
    };
    use windows::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        SDDL_REVISION_1,
    };
    use windows::Win32::Security::{
        GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
        TokenUser,
    };
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_SHARE_MODE, OPEN_EXISTING,
        PIPE_ACCESS_DUPLEX, ReadFile, SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT, WriteFile,
    };
    use windows::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeServerProcessId,
        PIPE_NOWAIT, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
        SetNamedPipeHandleState,
    };
    use windows::Win32::System::Threading::{
        CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, CreateProcessW, DETACHED_PROCESS,
        GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_INFORMATION,
        PROCESS_QUERY_LIMITED_INFORMATION, STARTUPINFOW,
    };
    use windows::core::{PCWSTR, PWSTR};

    struct OwnedHandle(HANDLE);
    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    struct LocalAllocation(*mut c_void);
    impl Drop for LocalAllocation {
        fn drop(&mut self) {
            unsafe {
                let _ = LocalFree(Some(HLOCAL(self.0)));
            }
        }
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }

    fn user_sid(process: HANDLE) -> Result<String, String> {
        unsafe {
            let mut token = HANDLE::default();
            OpenProcessToken(process, TOKEN_QUERY, &mut token)
                .map_err(|error| error.to_string())?;
            let token = OwnedHandle(token);
            let mut required = 0;
            let _ = GetTokenInformation(token.0, TokenUser, None, 0, &mut required);
            if required < size_of::<TOKEN_USER>() as u32 || required > 64 * 1024 {
                return Err("Cannot determine the Windows token user".into());
            }
            let mut buffer = vec![0usize; (required as usize).div_ceil(size_of::<usize>())];
            GetTokenInformation(
                token.0,
                TokenUser,
                Some(buffer.as_mut_ptr().cast()),
                required,
                &mut required,
            )
            .map_err(|error| error.to_string())?;
            let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
            let mut sid = PWSTR::null();
            ConvertSidToStringSidW(user.User.Sid, &mut sid).map_err(|error| error.to_string())?;
            let _allocation = LocalAllocation(sid.0.cast());
            sid.to_string().map_err(|error| error.to_string())
        }
    }

    fn isolation_suffix(
        config: Option<&std::ffi::OsStr>,
        instance: Option<&std::ffi::OsStr>,
    ) -> String {
        if config.is_none() && instance.is_none() {
            return String::new();
        }
        let mut hash = 0xcbf29ce484222325u64;
        for value in [config, instance] {
            for byte in [u8::from(value.is_some())]
                .into_iter()
                .chain(
                    value
                        .into_iter()
                        .flat_map(|text| text.encode_wide())
                        .flat_map(u16::to_le_bytes),
                )
                .chain([0xff])
            {
                hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
            }
        }
        format!("-{hash:016x}")
    }

    fn identity() -> Result<(String, String), String> {
        let sid = user_sid(unsafe { GetCurrentProcess() })?;
        let config = std::env::var_os("ANADA_CONFIG_DIR");
        let instance = std::env::var_os("ANADA_INSTANCE");
        let suffix = isolation_suffix(config.as_deref(), instance.as_deref());
        Ok((format!(r"\\.\pipe\Anada-{sid}-control-v1{suffix}"), sid))
    }

    fn pause(deadline: Instant) -> Result<(), String> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("Anada IPC deadline expired".into());
        }
        std::thread::sleep(POLL_INTERVAL.min(remaining));
        Ok(())
    }

    fn check_deadline(deadline: Instant) -> Result<(), String> {
        if Instant::now() >= deadline {
            Err("Anada IPC deadline expired".into())
        } else {
            Ok(())
        }
    }

    fn create_pipe(name: &str, sid: &str) -> Result<OwnedHandle, String> {
        let descriptor = wide(&format!("O:{sid}D:P(A;;GA;;;SY)(A;;GA;;;{sid})"));
        let mut security = PSECURITY_DESCRIPTOR::default();
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(descriptor.as_ptr()),
                SDDL_REVISION_1,
                &mut security,
                None,
            )
            .map_err(|error| error.to_string())?;
        }
        let _allocation = LocalAllocation(security.0);
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: security.0,
            bInheritHandle: false.into(),
        };
        let name = wide(name);
        let pipe = unsafe {
            CreateNamedPipeW(
                PCWSTR(name.as_ptr()),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_NOWAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                64 * 1024,
                64 * 1024,
                0,
                Some(&attributes),
            )
        };
        if pipe == INVALID_HANDLE_VALUE {
            return Err(format!(
                "Cannot own the Anada pipe (another host may be running): {}",
                windows::core::Error::from_thread()
            ));
        }
        Ok(OwnedHandle(pipe))
    }

    fn connect(name: &str, sid: &str, deadline: Instant) -> Result<OwnedHandle, String> {
        let name = wide(name);
        let pipe = loop {
            check_deadline(deadline)?;
            let opened = unsafe {
                CreateFileW(
                    PCWSTR(name.as_ptr()),
                    (GENERIC_READ | GENERIC_WRITE).0,
                    FILE_SHARE_MODE(0),
                    None,
                    OPEN_EXISTING,
                    SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                    None,
                )
            };
            match opened {
                Ok(handle) => break OwnedHandle(handle),
                Err(error) if error.code() == ERROR_PIPE_BUSY.to_hresult() => pause(deadline)?,
                Err(error) if error.code() == ERROR_FILE_NOT_FOUND.to_hresult() => {
                    return Err("Anada host is not running".into());
                }
                Err(error) => return Err(format!("Cannot connect to Anada host: {error}")),
            }
        };
        unsafe {
            let mode = PIPE_READMODE_BYTE | PIPE_NOWAIT;
            SetNamedPipeHandleState(pipe.0, Some(&mode), None, None)
                .map_err(|error| error.to_string())?;
            let mut process_id = 0;
            GetNamedPipeServerProcessId(pipe.0, &mut process_id)
                .map_err(|error| error.to_string())?;
            let process = OwnedHandle(
                OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id)
                    .map_err(|error| error.to_string())?,
            );
            if user_sid(process.0)? != sid {
                return Err("Anada pipe server belongs to a different Windows user".into());
            }
        }
        check_deadline(deadline)?;
        Ok(pipe)
    }

    fn read_exact(pipe: HANDLE, mut bytes: &mut [u8], deadline: Instant) -> Result<(), String> {
        while !bytes.is_empty() {
            check_deadline(deadline)?;
            let mut read = 0;
            let result = unsafe { ReadFile(pipe, Some(bytes), Some(&mut read), None) };
            match result {
                Ok(()) if read > 0 => bytes = &mut bytes[read as usize..],
                Ok(()) => return Err("Anada pipe closed during a read".into()),
                Err(error) if error.code() == ERROR_NO_DATA.to_hresult() => pause(deadline)?,
                Err(error) => return Err(format!("Anada pipe read failed: {error}")),
            }
        }
        Ok(())
    }

    fn write_all(pipe: HANDLE, mut bytes: &[u8], deadline: Instant) -> Result<(), String> {
        while !bytes.is_empty() {
            check_deadline(deadline)?;
            let mut written = 0;
            let chunk = &bytes[..bytes.len().min(16 * 1024)];
            unsafe { WriteFile(pipe, Some(chunk), Some(&mut written), None) }
                .map_err(|error| format!("Anada pipe write failed: {error}"))?;
            if written == 0 {
                pause(deadline)?;
            } else {
                bytes = &bytes[written as usize..];
            }
        }
        Ok(())
    }

    fn read_message<T: DeserializeOwned>(pipe: HANDLE, deadline: Instant) -> Result<T, String> {
        let mut header = [0; 4];
        read_exact(pipe, &mut header, deadline)?;
        let mut payload = vec![0; payload_length(header)?];
        read_exact(pipe, &mut payload, deadline)?;
        decode(&payload)
    }

    fn exchange(
        name: &str,
        sid: &str,
        command: Command,
        deadline: Instant,
    ) -> Result<Reply, String> {
        let frame = encode(&Request {
            protocol_version: PROTOCOL_VERSION,
            command,
        })?;
        let pipe = connect(name, sid, deadline)?;
        write_all(pipe.0, &frame, deadline)?;
        let reply = read_message(pipe.0, deadline)?;
        write_all(pipe.0, &[ACK], deadline)?;
        Ok(reply)
    }

    pub(super) fn request_until(command: Command, deadline: Instant) -> Result<HostState, String> {
        let (name, sid) = identity()?;
        let reply = exchange(&name, &sid, command, deadline)?;
        validate_reply(reply)
    }

    pub(super) fn ensure_host() -> Result<(), String> {
        let deadline = Instant::now() + START_TIMEOUT;
        if request_until(Command::Snapshot, Instant::now() + REQUEST_TIMEOUT).is_ok() {
            return Ok(());
        }
        let executable = std::env::current_exe()
            .map_err(|error| error.to_string())?
            .with_file_name("anada-host.exe");
        let application: Vec<u16> = executable
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        let startup = STARTUPINFOW {
            cb: size_of::<STARTUPINFOW>() as u32,
            ..Default::default()
        };
        let mut process = PROCESS_INFORMATION::default();
        unsafe {
            CreateProcessW(
                PCWSTR(application.as_ptr()),
                None,
                None,
                None,
                false,
                DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW,
                None,
                None,
                &startup,
                &mut process,
            )
        }
        .map_err(|error| format!("Cannot start {}: {error}", executable.display()))?;
        let _process = OwnedHandle(process.hProcess);
        let _thread = OwnedHandle(process.hThread);
        loop {
            let attempt_deadline = deadline.min(Instant::now() + REQUEST_TIMEOUT);
            match request_until(Command::Snapshot, attempt_deadline) {
                Ok(_) => return Ok(()),
                Err(error) => {
                    if pause(deadline).is_err() {
                        return Err(format!("Anada host did not become ready: {error}"));
                    }
                }
            }
        }
    }

    fn accept(pipe: HANDLE, on_idle: &mut impl FnMut()) -> Result<(), String> {
        let mut next_tick = Instant::now() + IDLE_INTERVAL;
        loop {
            if Instant::now() >= next_tick {
                on_idle();
                next_tick = Instant::now() + IDLE_INTERVAL;
            }
            match unsafe { ConnectNamedPipe(pipe, None) } {
                Ok(()) => {}
                Err(error) if error.code() == ERROR_PIPE_CONNECTED.to_hresult() => return Ok(()),
                Err(error) if error.code() == ERROR_PIPE_LISTENING.to_hresult() => {
                    std::thread::sleep(POLL_INTERVAL);
                }
                Err(error) if error.code() == ERROR_NO_DATA.to_hresult() => unsafe {
                    let _ = DisconnectNamedPipe(pipe);
                },
                Err(error) => return Err(format!("Cannot accept an Anada client: {error}")),
            }
        }
    }

    fn handle_client(
        pipe: HANDLE,
        handler: &mut impl FnMut(Command) -> Response,
    ) -> Result<bool, String> {
        let deadline = Instant::now() + CLIENT_TIMEOUT;
        let request: Request = read_message(pipe, deadline)?;
        let stop = request.protocol_version == PROTOCOL_VERSION
            && matches!(&request.command, Command::StopHost);
        let response = if request.protocol_version == PROTOCOL_VERSION {
            handler(request.command)
        } else {
            Response {
                state: None,
                error: Some("Incompatible Anada IPC protocol version".into()),
            }
        };
        let reply = encode(&Reply {
            protocol_version: PROTOCOL_VERSION,
            response,
        })
        .or_else(|_| {
            encode(&Reply {
                protocol_version: PROTOCOL_VERSION,
                response: Response {
                    state: None,
                    error: Some("Anada response exceeds the IPC frame limit".into()),
                },
            })
        })?;
        let sent = write_all(pipe, &reply, deadline);
        if sent.is_ok() {
            let mut acknowledgement = [0];
            let _ = read_exact(pipe, &mut acknowledgement, deadline);
        }
        if stop { Ok(true) } else { sent.map(|()| false) }
    }

    pub(super) fn serve(mut handler: impl FnMut(Command) -> Response) -> Result<(), String> {
        let (name, sid) = identity()?;
        let pipe = create_pipe(&name, &sid)?;
        loop {
            accept(pipe.0, &mut || {
                let _ = handler(Command::Snapshot);
            })?;
            let stop = handle_client(pipe.0, &mut handler).unwrap_or(false);
            unsafe {
                let _ = DisconnectNamedPipe(pipe.0);
            }
            if stop {
                return Ok(());
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::ffi::OsStr;
        use std::sync::atomic::{AtomicU32, Ordering};

        fn test_identity() -> (String, String) {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let (name, sid) = identity().unwrap();
            (
                format!(
                    "{name}-test-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ),
                sid,
            )
        }

        #[test]
        fn isolation_is_stable_and_opt_in() {
            let config = Some(OsStr::new(r"C:\Anada QA"));
            assert_eq!(isolation_suffix(None, None), "");
            assert_eq!(
                isolation_suffix(config, None),
                isolation_suffix(config, None)
            );
            assert_ne!(
                isolation_suffix(config, None),
                isolation_suffix(None, config)
            );
            assert_ne!(
                isolation_suffix(config, None),
                isolation_suffix(Some(OsStr::new(r"C:\Other QA")), None)
            );
            assert_ne!(
                isolation_suffix(None, None),
                isolation_suffix(Some(OsStr::new("")), None)
            );
            assert_ne!(
                isolation_suffix(config, None),
                isolation_suffix(config, Some(OsStr::new("qa2")))
            );
        }

        #[test]
        fn real_pipe_round_trip_and_single_host_guard() {
            let (name, sid) = test_identity();
            let server_name = name.clone();
            let server_sid = sid.clone();
            let (ready, wait) = std::sync::mpsc::channel();
            let server = std::thread::spawn(move || {
                let pipe = create_pipe(&server_name, &server_sid).unwrap();
                assert!(create_pipe(&server_name, &server_sid).is_err());
                ready.send(()).unwrap();
                accept(pipe.0, &mut || {}).unwrap();
                let stopped = handle_client(pipe.0, &mut |command| {
                    assert!(matches!(command, Command::StopHost));
                    Response {
                        state: None,
                        error: Some("test response".into()),
                    }
                })
                .unwrap();
                assert!(stopped);
                unsafe { DisconnectNamedPipe(pipe.0).unwrap() };
            });
            wait.recv_timeout(REQUEST_TIMEOUT).unwrap();
            let reply = exchange(
                &name,
                &sid,
                Command::StopHost,
                Instant::now() + REQUEST_TIMEOUT,
            )
            .unwrap();
            assert_eq!(reply.protocol_version, PROTOCOL_VERSION);
            assert_eq!(reply.response.error.as_deref(), Some("test response"));
            server.join().unwrap();
        }

        #[test]
        fn stalled_client_is_dropped_within_deadline() {
            let (name, sid) = test_identity();
            let pipe = create_pipe(&name, &sid).unwrap();
            let _client = connect(&name, &sid, Instant::now() + REQUEST_TIMEOUT).unwrap();
            accept(pipe.0, &mut || {}).unwrap();
            let started = Instant::now();
            let result = handle_client(pipe.0, &mut |_| panic!("No request was sent"));
            assert!(result.unwrap_err().contains("deadline"));
            assert!(started.elapsed() < CLIENT_TIMEOUT + Duration::from_secs(1));
        }

        #[test]
        fn client_read_and_full_pipe_write_have_deadlines() {
            let (name, sid) = test_identity();
            let pipe = create_pipe(&name, &sid).unwrap();
            let client = connect(&name, &sid, Instant::now() + REQUEST_TIMEOUT).unwrap();
            accept(pipe.0, &mut || {}).unwrap();
            let mut byte = [0];
            let started = Instant::now();
            assert!(
                read_exact(
                    client.0,
                    &mut byte,
                    Instant::now() + Duration::from_millis(30)
                )
                .is_err()
            );
            assert!(
                write_all(
                    pipe.0,
                    &vec![0; MAX_FRAME_BYTES],
                    Instant::now() + Duration::from_millis(30)
                )
                .is_err()
            );
            assert!(started.elapsed() < Duration::from_secs(1));
        }

        #[test]
        fn idle_refresh_runs_without_a_client() {
            let (name, sid) = test_identity();
            let server_name = name.clone();
            let server_sid = sid.clone();
            let (ready, wait) = std::sync::mpsc::channel();
            let (ticked, tick) = std::sync::mpsc::channel();
            let server = std::thread::spawn(move || {
                let pipe = create_pipe(&server_name, &server_sid).unwrap();
                ready.send(()).unwrap();
                accept(pipe.0, &mut || {
                    let _ = ticked.send(());
                })
                .unwrap();
            });
            wait.recv_timeout(REQUEST_TIMEOUT).unwrap();
            tick.recv_timeout(REQUEST_TIMEOUT).unwrap();
            let _client = connect(&name, &sid, Instant::now() + REQUEST_TIMEOUT).unwrap();
            server.join().unwrap();
        }

        #[test]
        fn invalid_version_does_not_dispatch_and_large_response_is_transferred() {
            let (name, sid) = test_identity();
            let server_name = name.clone();
            let server_sid = sid.clone();
            let (ready, wait) = std::sync::mpsc::channel();
            let server = std::thread::spawn(move || {
                let pipe = create_pipe(&server_name, &server_sid).unwrap();
                ready.send(()).unwrap();
                accept(pipe.0, &mut || {}).unwrap();
                assert!(
                    !handle_client(pipe.0, &mut |_| panic!("Wrong protocol dispatched")).unwrap()
                );
                unsafe { DisconnectNamedPipe(pipe.0).unwrap() };
                accept(pipe.0, &mut || {}).unwrap();
                assert!(
                    !handle_client(pipe.0, &mut |_| Response {
                        state: None,
                        error: Some("x".repeat(256 * 1024)),
                    })
                    .unwrap()
                );
                unsafe { DisconnectNamedPipe(pipe.0).unwrap() };
            });
            wait.recv_timeout(REQUEST_TIMEOUT).unwrap();
            let deadline = Instant::now() + REQUEST_TIMEOUT;
            let client = connect(&name, &sid, deadline).unwrap();
            let frame = encode(&Request {
                protocol_version: PROTOCOL_VERSION + 1,
                command: Command::StopHost,
            })
            .unwrap();
            for chunk in frame.chunks(3) {
                write_all(client.0, chunk, deadline).unwrap();
            }
            let reply: Reply = read_message(client.0, deadline).unwrap();
            assert!(reply.response.error.unwrap().contains("version"));
            write_all(client.0, &[ACK], deadline).unwrap();
            drop(client);
            let reply = exchange(
                &name,
                &sid,
                Command::Snapshot,
                Instant::now() + REQUEST_TIMEOUT,
            )
            .unwrap();
            assert_eq!(reply.response.error.unwrap().len(), 256 * 1024);
            server.join().unwrap();
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use super::*;
    pub(super) fn request_until(_: Command, _: Instant) -> Result<HostState, String> {
        Err("Anada IPC requires Windows".into())
    }
    pub(super) fn ensure_host() -> Result<(), String> {
        Err("Anada host requires Windows".into())
    }
    pub(super) fn serve(_: impl FnMut(Command) -> Response) -> Result<(), String> {
        Err("Anada host requires Windows".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_error_is_preserved_when_current_state_is_included() {
        let state = HostState {
            audio_diagnostics: Default::default(),
            protocol_version: PROTOCOL_VERSION,
            revision: 1,
            config: crate::model::Config::default(),
            windows: anada_windows::WindowsSnapshot {
                outputs: Vec::new(),
                inputs: Vec::new(),
                sessions: Vec::new(),
                default_output_id: None,
                communications_output_id: None,
            },
            selected_output_id: None,
            manual_output: false,
            audio_running: false,
            audio_status: String::new(),
            warnings: Vec::new(),
            host_pid: 0,
            quiet: Default::default(),
        };
        let result = validate_reply(Reply {
            protocol_version: PROTOCOL_VERSION,
            response: Response {
                state: Some(state),
                error: Some("Settings changed in another window".into()),
            },
        });
        assert_eq!(result.unwrap_err(), "Settings changed in another window");
    }

    #[test]
    fn frame_round_trip() {
        let frame = encode(&Request {
            protocol_version: PROTOCOL_VERSION,
            command: Command::Snapshot,
        })
        .unwrap();
        assert_eq!(
            payload_length(frame[..4].try_into().unwrap()).unwrap(),
            frame.len() - 4
        );
        let request: Request = decode(&frame[4..]).unwrap();
        assert_eq!(request.protocol_version, PROTOCOL_VERSION);
        assert!(matches!(request.command, Command::Snapshot));
    }

    #[test]
    fn invalid_lengths_are_rejected_before_allocation() {
        for length in [0, MAX_PAYLOAD_BYTES as u32 + 1, u32::MAX] {
            assert!(payload_length(length.to_le_bytes()).is_err());
        }
        assert_eq!(
            payload_length((MAX_PAYLOAD_BYTES as u32).to_le_bytes()).unwrap(),
            MAX_PAYLOAD_BYTES
        );
    }

    #[test]
    fn outbound_limit_includes_header() {
        let exact = "x".repeat(MAX_PAYLOAD_BYTES - 2);
        assert_eq!(encode(&exact).unwrap().len(), MAX_FRAME_BYTES);
        assert!(encode(&(exact + "x")).is_err());
    }

    #[test]
    fn invalid_json_and_trailing_content_are_rejected() {
        for payload in [b"{".as_slice(), b"null {}", b"\xff", b""] {
            assert!(decode::<Request>(payload).is_err());
        }
    }
}
