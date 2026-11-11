use serde::{Deserialize, Serialize};

#[cfg(windows)]
mod routing;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PersistedPlaybackPreference {
    pub console: Option<String>,
    pub multimedia: Option<String>,
    pub communications: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EndpointLevelDb {
    pub current: f32,
    pub min: f32,
    pub max: f32,
    pub step: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Endpoint {
    #[serde(default)]
    pub level_db: Option<EndpointLevelDb>,
    pub id: String,
    pub name: String,
    pub volume: f32,
    pub muted: bool,
    pub is_virtual: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AudioSession {
    /// Full executable identity; unavailable identities fall back to PID.
    #[serde(default)]
    pub executable_path: Option<String>,
    pub process_id: u32,
    pub name: String,
    pub endpoint_id: String,
    pub active: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WindowsSnapshot {
    pub outputs: Vec<Endpoint>,
    pub inputs: Vec<Endpoint>,
    pub sessions: Vec<AudioSession>,
    pub default_output_id: Option<String>,
    pub communications_output_id: Option<String>,
}

fn validate_volume(volume: f32) -> Result<(), String> {
    if volume.is_finite() && (0.0..=1.0).contains(&volume) {
        Ok(())
    } else {
        Err("Endpoint volume must be finite and between 0.0 and 1.0 inclusive".into())
    }
}

fn validate_id(id: &str) -> Result<(), String> {
    if id.is_empty() || id.contains('\0') {
        Err("Endpoint ID must be nonempty and must not contain a NUL character".into())
    } else {
        Ok(())
    }
}

fn validate_process_id(process_id: u32) -> Result<(), String> {
    if process_id == 0 {
        Err("A nonzero application process ID is required".into())
    } else {
        Ok(())
    }
}

fn validate_output_name_ids(ids: &[String]) -> Result<(), String> {
    if ids.len() > 64 {
        return Err("Resolve at most 64 output names at a time".into());
    }
    for id in ids {
        validate_id(id)?;
    }
    Ok(())
}

fn validate_playback_preferences(
    process_id: u32,
    preferences: &PersistedPlaybackPreference,
    mut validate_target: impl FnMut(&str) -> Result<(), String>,
) -> Result<(), String> {
    validate_process_id(process_id)?;
    for endpoint in [
        preferences.console.as_deref(),
        preferences.multimedia.as_deref(),
        preferences.communications.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        validate_id(endpoint)?;
        validate_target(endpoint)?;
    }
    Ok(())
}

#[cfg(any(windows, test))]
fn known_virtual_transport(driver_identity: &[String]) -> bool {
    driver_identity.iter().any(|identity| {
        let identity = identity.to_ascii_lowercase();
        [
            "virtual audio cable",
            "vb-audio virtual cable",
            "vb-audio cable",
            "vb-audio hi-fi cable",
            "vb-cable",
            "steelseries sonar",
        ]
        .iter()
        .any(|signature| identity.contains(signature))
    })
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::collections::BTreeMap;
    use std::marker::PhantomData;
    use std::rc::Rc;
    use std::sync::mpsc::{self, SyncSender};
    use std::thread::{self, JoinHandle};
    use windows::Win32::Devices::FunctionDiscovery::{
        PKEY_Device_DeviceDesc, PKEY_Device_FriendlyName, PKEY_Device_HardwareIds,
        PKEY_DeviceInterface_FriendlyName,
    };
    use windows::Win32::Foundation::{
        CloseHandle, ERROR_DEVICE_NOT_CONNECTED, ERROR_NOT_FOUND, HANDLE,
    };
    use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
    use windows::Win32::Media::Audio::{
        AUDCLNT_E_DEVICE_INVALIDATED, AUDCLNT_E_RESOURCES_INVALIDATED, AudioSessionStateActive,
        AudioSessionStateExpired, DEVICE_STATE, DEVICE_STATE_ACTIVE, DEVICE_STATEMASK_ALL,
        EDataFlow, ERole, IAudioSessionControl, IAudioSessionControl2, IAudioSessionManager2,
        IMMDevice, IMMDeviceEnumerator, IMMEndpoint, MMDeviceEnumerator, eCapture, eCommunications,
        eConsole, eRender,
    };
    use windows::Win32::System::Com::StructuredStorage::{
        PROPVARIANT, PropVariantClear, PropVariantGetElementCount, PropVariantGetStringElem,
        PropVariantToStringAlloc,
    };
    use windows::Win32::System::Com::{
        CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
        CoUninitialize, STGM_READ,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    };
    use windows::core::{Error, HRESULT, Interface, PCWSTR, PWSTR};

    pub struct AudioSystem {
        commands: SyncSender<Command>,
        worker: Option<JoinHandle<()>>,
        thread_bound: PhantomData<Rc<()>>,
    }

    enum Command {
        Snapshot(SyncSender<Result<WindowsSnapshot, String>>),
        ReadEndpointState(String, SyncSender<Result<Endpoint, String>>),
        ResolveOutputNames(
            Vec<String>,
            SyncSender<Result<BTreeMap<String, String>, String>>,
        ),
        Volume(String, f32, SyncSender<Result<(), String>>),
        VolumeDb(String, f32, SyncSender<Result<(), String>>),
        Mute(String, bool, SyncSender<Result<(), String>>),
        DefaultPlayback(String, SyncSender<Result<(), String>>),
        DefaultCommunicationsPlayback(String, SyncSender<Result<(), String>>),
        ProcessPlayback(u32, String, SyncSender<Result<(), String>>),
        ProcessPlaybackPreferences(
            u32,
            PersistedPlaybackPreference,
            SyncSender<Result<(), String>>,
        ),
        ProcessPlaybackPreference(u32, SyncSender<Result<PersistedPlaybackPreference, String>>),
        Shutdown,
    }

    impl AudioSystem {
        pub fn new() -> Result<Self, String> {
            let (commands, receiver) = mpsc::sync_channel(1);
            let (ready, started) = mpsc::sync_channel(1);
            let worker = thread::Builder::new()
                .name("anada-windows-com".into())
                .spawn(move || {
                    let apartment = match Apartment::new() {
                        Ok(apartment) => apartment,
                        Err(error) => {
                            let _ = ready.send(Err(error));
                            return;
                        }
                    };
                    let enumerator = match create_enumerator() {
                        Ok(enumerator) => enumerator,
                        Err(error) => {
                            let _ = ready.send(Err(error));
                            return;
                        }
                    };
                    if ready.send(Ok(())).is_err() {
                        return;
                    }
                    while let Ok(command) = receiver.recv() {
                        match command {
                            Command::Snapshot(reply) => {
                                let _ = reply.send(snapshot(&enumerator));
                            }
                            Command::ReadEndpointState(id, reply) => {
                                let wide: Vec<u16> = id.encode_utf16().chain(Some(0)).collect();
                                let result = unsafe { enumerator.GetDevice(PCWSTR(wide.as_ptr())) }
                                    .and_then(|device| read_endpoint(&device, false))
                                    .map(|(endpoint, _)| endpoint)
                                    .map_err(|error| {
                                        contextual("Read Windows endpoint state", error)
                                    });
                                let _ = reply.send(result);
                            }
                            Command::ResolveOutputNames(ids, reply) => {
                                let _ = reply.send(resolve_output_names(&enumerator, &ids));
                            }
                            Command::Volume(id, volume, reply) => {
                                let result =
                                    endpoint_volume(&enumerator, &id).and_then(|control| {
                                        unsafe {
                                            control.SetMasterVolumeLevelScalar(
                                                volume,
                                                std::ptr::null(),
                                            )
                                        }
                                        .map_err(|error| {
                                            contextual(&format!("Set volume for {id}"), error)
                                        })
                                    });
                                let _ = reply.send(result);
                            }
                            Command::VolumeDb(id, level, reply) => {
                                let result =
                                    endpoint_volume(&enumerator, &id).and_then(|control| {
                                        let range =
                                            endpoint_level_db(&control).ok_or_else(|| {
                                                "Endpoint dB range is unavailable".to_owned()
                                            })?;
                                        if !level.is_finite()
                                            || level < range.min
                                            || level > range.max
                                        {
                                            return Err(
                                                "Endpoint level is outside its Windows dB range"
                                                    .into(),
                                            );
                                        }
                                        unsafe {
                                            control.SetMasterVolumeLevel(level, std::ptr::null())
                                        }
                                        .map_err(|error| {
                                            contextual("Set Windows endpoint dB level", error)
                                        })
                                    });
                                let _ = reply.send(result);
                            }
                            Command::Mute(id, muted, reply) => {
                                let result =
                                    endpoint_volume(&enumerator, &id).and_then(|control| {
                                        unsafe { control.SetMute(muted, std::ptr::null()) }.map_err(
                                            |error| {
                                                contextual(&format!("Set mute for {id}"), error)
                                            },
                                        )
                                    });
                                let _ = reply.send(result);
                            }
                            Command::DefaultPlayback(id, reply) => {
                                let result = validate_render_endpoint(&enumerator, &id)
                                    .and_then(|()| routing::set_default_playback(&id));
                                let _ = reply.send(result);
                            }
                            Command::DefaultCommunicationsPlayback(id, reply) => {
                                let result =
                                    validate_render_endpoint(&enumerator, &id).and_then(|()| {
                                        routing::set_default_communications_playback(&id)
                                    });
                                let _ = reply.send(result);
                            }
                            Command::ProcessPlayback(process_id, id, reply) => {
                                let result =
                                    validate_render_endpoint(&enumerator, &id).and_then(|()| {
                                        routing::set_process_playback_preference(process_id, &id)
                                    });
                                let _ = reply.send(result);
                            }
                            Command::ProcessPlaybackPreferences(process_id, preferences, reply) => {
                                let result =
                                    validate_playback_preferences(process_id, &preferences, |id| {
                                        validate_render_endpoint_states(
                                            &enumerator,
                                            id,
                                            DEVICE_STATE(DEVICE_STATEMASK_ALL),
                                        )
                                    })
                                    .and_then(|()| {
                                        routing::set_process_playback_preferences(
                                            process_id,
                                            &preferences,
                                        )
                                    });
                                let _ = reply.send(result);
                            }
                            Command::ProcessPlaybackPreference(process_id, reply) => {
                                let result = routing::get_process_playback_preference(process_id)
                                    .map(|preference| PersistedPlaybackPreference {
                                        console: preference.console,
                                        multimedia: preference.multimedia,
                                        communications: preference.communications,
                                    });
                                let _ = reply.send(result);
                            }
                            Command::Shutdown => break,
                        }
                    }
                    drop(enumerator);
                    drop(apartment);
                })
                .map_err(|error| format!("Start Windows audio COM worker: {error}"))?;
            match started.recv() {
                Ok(Ok(())) => Ok(Self {
                    commands,
                    worker: Some(worker),
                    thread_bound: PhantomData,
                }),
                status => {
                    let _ = worker.join();
                    Err(match status {
                        Ok(Err(error)) => error,
                        _ => "Windows audio COM worker stopped during initialization".into(),
                    })
                }
            }
        }

        pub fn snapshot(&self) -> Result<WindowsSnapshot, String> {
            self.request(Command::Snapshot)
        }

        /// Read one endpoint's volume and mute without enumerating sessions.
        pub fn endpoint_state(&self, id: &str) -> Result<Endpoint, String> {
            validate_id(id)?;
            self.request(|reply| Command::ReadEndpointState(id.into(), reply))
        }

        /// Resolve registered playback names without activating audio or volume.
        /// Disconnected registered devices are included; unregistered IDs are omitted.
        pub fn output_names(&self, ids: &[String]) -> Result<BTreeMap<String, String>, String> {
            validate_output_name_ids(ids)?;
            self.request(|reply| Command::ResolveOutputNames(ids.to_vec(), reply))
        }

        pub fn set_endpoint_volume(&self, id: &str, volume: f32) -> Result<(), String> {
            validate_id(id)?;
            validate_volume(volume)?;
            self.request(|reply| Command::Volume(id.into(), volume, reply))
        }

        pub fn set_endpoint_volume_db(&self, id: &str, level_db: f32) -> Result<(), String> {
            validate_id(id)?;
            if !level_db.is_finite() {
                return Err("Endpoint dB level must be finite".into());
            }
            self.request(|reply| Command::VolumeDb(id.into(), level_db, reply))
        }

        pub fn set_endpoint_mute(&self, id: &str, muted: bool) -> Result<(), String> {
            validate_id(id)?;
            self.request(|reply| Command::Mute(id.into(), muted, reply))
        }

        pub fn set_default_playback(&self, id: &str) -> Result<(), String> {
            validate_id(id)?;
            self.request(|reply| Command::DefaultPlayback(id.into(), reply))
        }

        pub fn set_default_communications_playback(&self, id: &str) -> Result<(), String> {
            validate_id(id)?;
            self.request(|reply| Command::DefaultCommunicationsPlayback(id.into(), reply))
        }

        pub fn set_process_playback(&self, process_id: u32, id: &str) -> Result<(), String> {
            validate_process_id(process_id)?;
            validate_id(id)?;
            self.request(|reply| Command::ProcessPlayback(process_id, id.into(), reply))
        }

        pub fn set_process_playback_preferences(
            &self,
            process_id: u32,
            preferences: &PersistedPlaybackPreference,
        ) -> Result<(), String> {
            validate_playback_preferences(process_id, preferences, |_| Ok(()))?;
            self.request(|reply| {
                Command::ProcessPlaybackPreferences(process_id, preferences.clone(), reply)
            })
        }

        pub fn get_process_playback_preference(
            &self,
            process_id: u32,
        ) -> Result<PersistedPlaybackPreference, String> {
            validate_process_id(process_id)?;
            self.request(|reply| Command::ProcessPlaybackPreference(process_id, reply))
        }

        fn request<Response>(
            &self,
            command: impl FnOnce(SyncSender<Result<Response, String>>) -> Command,
        ) -> Result<Response, String> {
            let (reply, result) = mpsc::sync_channel(1);
            self.commands
                .send(command(reply))
                .map_err(|_| "Windows audio COM worker is unavailable".to_string())?;
            result
                .recv()
                .map_err(|_| "Windows audio COM worker stopped before replying".to_string())?
        }
    }

    impl Drop for AudioSystem {
        fn drop(&mut self) {
            let _ = self.commands.send(Command::Shutdown);
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }

    struct Apartment;

    impl Apartment {
        fn new() -> Result<Self, String> {
            unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
                .ok()
                .map_err(|error| contextual("Initialize Windows audio COM apartment", error))?;
            Ok(Self)
        }
    }

    impl Drop for Apartment {
        fn drop(&mut self) {
            unsafe { CoUninitialize() };
        }
    }

    struct TaskString(PWSTR);

    impl TaskString {
        fn text(&self) -> String {
            if self.0.is_null() {
                String::new()
            } else {
                unsafe { String::from_utf16_lossy(self.0.as_wide()) }
            }
        }
    }

    impl Drop for TaskString {
        fn drop(&mut self) {
            unsafe { CoTaskMemFree(Some(self.0.as_ptr().cast())) };
        }
    }

    struct Property(PROPVARIANT);

    impl Drop for Property {
        fn drop(&mut self) {
            let _ = unsafe { PropVariantClear(&mut self.0) };
        }
    }

    struct ProcessHandle(HANDLE);

    impl Drop for ProcessHandle {
        fn drop(&mut self) {
            let _ = unsafe { CloseHandle(self.0) };
        }
    }

    fn contextual(operation: &str, error: Error) -> String {
        format!(
            "{operation}: {error} (HRESULT 0x{:08X})",
            error.code().0 as u32
        )
    }

    fn disappeared(error: &Error) -> bool {
        [
            HRESULT::from_win32(ERROR_NOT_FOUND.0),
            HRESULT::from_win32(ERROR_DEVICE_NOT_CONNECTED.0),
            AUDCLNT_E_DEVICE_INVALIDATED,
            AUDCLNT_E_RESOURCES_INVALIDATED,
        ]
        .contains(&error.code())
    }

    fn create_enumerator() -> Result<IMMDeviceEnumerator, String> {
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
            .map_err(|error| contextual("Create Windows audio device enumerator", error))
    }

    fn device_id(device: &IMMDevice) -> windows::core::Result<String> {
        unsafe { device.GetId() }.map(|pointer| TaskString(pointer).text())
    }

    fn friendly_name(device: &IMMDevice) -> windows::core::Result<String> {
        let properties = unsafe { device.OpenPropertyStore(STGM_READ) }?;
        let property = Property(unsafe { properties.GetValue(&PKEY_Device_FriendlyName) }?);
        let text = TaskString(unsafe { PropVariantToStringAlloc(&property.0) }?);
        Ok(text.text())
    }

    fn resolve_output_names(
        enumerator: &IMMDeviceEnumerator,
        ids: &[String],
    ) -> Result<BTreeMap<String, String>, String> {
        let mut names = BTreeMap::new();
        for id in ids {
            if names.contains_key(id) {
                continue;
            }
            let wide: Vec<u16> = id.encode_utf16().chain(Some(0)).collect();
            let device = match unsafe { enumerator.GetDevice(PCWSTR(wide.as_ptr())) } {
                Ok(device) => device,
                Err(error) if disappeared(&error) => continue,
                Err(error) => return Err(contextual("Resolve registered output", error)),
            };
            let flow = device
                .cast::<IMMEndpoint>()
                .and_then(|endpoint| unsafe { endpoint.GetDataFlow() })
                .map_err(|error| contextual("Read registered endpoint direction", error))?;
            if flow != eRender {
                continue;
            }
            match friendly_name(&device) {
                Ok(name) if !name.trim().is_empty() => {
                    names.insert(id.clone(), name);
                }
                Ok(_) => {}
                Err(error) if disappeared(&error) => {}
                Err(error) => return Err(contextual("Read registered output name", error)),
            }
        }
        Ok(names)
    }

    fn virtual_transport(device: &IMMDevice) -> bool {
        let Ok(properties) = (unsafe { device.OpenPropertyStore(STGM_READ) }) else {
            return false;
        };
        let mut driver_identity = Vec::new();
        for key in [
            PKEY_DeviceInterface_FriendlyName,
            PKEY_Device_DeviceDesc,
            PKEY_Device_HardwareIds,
        ] {
            let Ok(value) = (unsafe { properties.GetValue(&key) }) else {
                continue;
            };
            let property = Property(value);
            let count = unsafe { PropVariantGetElementCount(&property.0) };
            for index in 0..count {
                if let Ok(pointer) = unsafe { PropVariantGetStringElem(&property.0, index) } {
                    driver_identity.push(TaskString(pointer).text());
                }
            }
        }
        known_virtual_transport(&driver_identity)
    }

    fn endpoint_volume(
        enumerator: &IMMDeviceEnumerator,
        id: &str,
    ) -> Result<IAudioEndpointVolume, String> {
        let wide: Vec<u16> = id.encode_utf16().chain(Some(0)).collect();
        let device = unsafe { enumerator.GetDevice(PCWSTR(wide.as_ptr())) }
            .map_err(|error| contextual(&format!("Find endpoint {id}"), error))?;
        let state = unsafe { device.GetState() }
            .map_err(|error| contextual(&format!("Read state for endpoint {id}"), error))?;
        if state != DEVICE_STATE_ACTIVE {
            return Err(format!(
                "Endpoint {id} is not active; refresh the Windows snapshot"
            ));
        }
        unsafe { device.Activate(CLSCTX_ALL, None) }
            .map_err(|error| contextual(&format!("Open endpoint volume for {id}"), error))
    }

    fn validate_render_endpoint(enumerator: &IMMDeviceEnumerator, id: &str) -> Result<(), String> {
        validate_render_endpoint_states(enumerator, id, DEVICE_STATE_ACTIVE)
    }

    fn validate_render_endpoint_states(
        enumerator: &IMMDeviceEnumerator,
        id: &str,
        states: DEVICE_STATE,
    ) -> Result<(), String> {
        validate_id(id)?;
        let collection =
            unsafe { enumerator.EnumAudioEndpoints(eRender, states) }.map_err(|error| {
                contextual("Enumerate playback endpoints for policy validation", error)
            })?;
        let count = unsafe { collection.GetCount() }
            .map_err(|error| contextual("Read playback endpoint count", error))?;
        for index in 0..count {
            let device = match unsafe { collection.Item(index) } {
                Ok(device) => device,
                Err(error) if disappeared(&error) => continue,
                Err(error) => return Err(contextual("Read playback endpoint", error)),
            };
            let endpoint_id = match device_id(&device) {
                Ok(endpoint_id) => endpoint_id,
                Err(error) if disappeared(&error) => continue,
                Err(error) => return Err(contextual("Read playback endpoint ID", error)),
            };
            if endpoint_id == id {
                let state = unsafe { device.GetState() }
                    .map_err(|error| contextual("Read playback endpoint state", error))?;
                if (state.0 & states.0) != 0 {
                    return Ok(());
                }
            }
        }
        Err(format!(
            "Endpoint {id} is not a registered playback endpoint in the permitted states; refresh the Windows snapshot"
        ))
    }

    fn snapshot(enumerator: &IMMDeviceEnumerator) -> Result<WindowsSnapshot, String> {
        let (outputs, sessions) = enumerate(enumerator, eRender, true)?;
        let (inputs, _) = enumerate(enumerator, eCapture, false)?;
        Ok(WindowsSnapshot {
            outputs,
            inputs,
            sessions,
            default_output_id: default_id(enumerator, eConsole)?,
            communications_output_id: default_id(enumerator, eCommunications)?,
        })
    }

    fn default_id(enumerator: &IMMDeviceEnumerator, role: ERole) -> Result<Option<String>, String> {
        match unsafe { enumerator.GetDefaultAudioEndpoint(eRender, role) }
            .and_then(|device| device_id(&device))
        {
            Ok(id) => Ok(Some(id)),
            Err(error) if disappeared(&error) => Ok(None),
            Err(error) => Err(contextual(
                &format!("Read default render endpoint for role {}", role.0),
                error,
            )),
        }
    }

    fn enumerate(
        enumerator: &IMMDeviceEnumerator,
        flow: EDataFlow,
        include_sessions: bool,
    ) -> Result<(Vec<Endpoint>, Vec<AudioSession>), String> {
        let collection = unsafe { enumerator.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE) }
            .map_err(|error| {
                contextual(
                    &format!("Enumerate active endpoints for flow {}", flow.0),
                    error,
                )
            })?;
        let count = unsafe { collection.GetCount() }
            .map_err(|error| contextual("Read endpoint count", error))?;
        let mut endpoints = Vec::new();
        let mut sessions = Vec::new();
        for index in 0..count {
            let device = match unsafe { collection.Item(index) } {
                Ok(device) => device,
                Err(error) if disappeared(&error) => continue,
                Err(error) => {
                    return Err(contextual(
                        &format!("Read endpoint at index {index}"),
                        error,
                    ));
                }
            };
            match read_endpoint(&device, include_sessions) {
                Ok((endpoint, mut device_sessions)) => {
                    endpoints.push(endpoint);
                    sessions.append(&mut device_sessions);
                }
                Err(error) if disappeared(&error) => continue,
                Err(error) => {
                    match unsafe { device.GetState() } {
                        Ok(state) if state != DEVICE_STATE_ACTIVE => continue,
                        Err(state_error) if disappeared(&state_error) => continue,
                        _ => {}
                    }
                    let id = device_id(&device).unwrap_or_else(|_| format!("index {index}"));
                    return Err(contextual(
                        &format!("Read endpoint {id}, flow {}", flow.0),
                        error,
                    ));
                }
            }
        }
        endpoints.sort_by(|left, right| left.id.cmp(&right.id));
        sessions.sort_by(|left, right| {
            (&left.endpoint_id, left.process_id).cmp(&(&right.endpoint_id, right.process_id))
        });
        Ok((endpoints, sessions))
    }

    fn endpoint_level_db(control: &IAudioEndpointVolume) -> Option<EndpointLevelDb> {
        let current = unsafe { control.GetMasterVolumeLevel() }.ok()?;
        let (mut min, mut max, mut step) = (0.0_f32, 0.0_f32, 0.0_f32);
        unsafe { control.GetVolumeRange(&mut min, &mut max, &mut step) }.ok()?;
        if ![current, min, max, step].into_iter().all(f32::is_finite) || min >= max || step <= 0.0 {
            return None;
        }
        Some(EndpointLevelDb {
            current,
            min,
            max,
            step,
        })
    }

    fn read_endpoint(
        device: &IMMDevice,
        include_sessions: bool,
    ) -> windows::core::Result<(Endpoint, Vec<AudioSession>)> {
        let id = device_id(device)?;
        let name = friendly_name(device)?;
        let control: IAudioEndpointVolume = unsafe { device.Activate(CLSCTX_ALL, None) }?;
        let volume = unsafe { control.GetMasterVolumeLevelScalar() }?;
        let muted = unsafe { control.GetMute() }?.as_bool();
        let sessions = if include_sessions {
            read_sessions(device, &id)?
        } else {
            Vec::new()
        };
        Ok((
            Endpoint {
                level_db: endpoint_level_db(&control),
                id,
                is_virtual: virtual_transport(device),
                name,
                volume,
                muted,
            },
            sessions,
        ))
    }

    fn read_sessions(
        device: &IMMDevice,
        endpoint_id: &str,
    ) -> windows::core::Result<Vec<AudioSession>> {
        let manager: IAudioSessionManager2 = unsafe { device.Activate(CLSCTX_ALL, None) }?;
        let enumerator = unsafe { manager.GetSessionEnumerator() }?;
        let count = unsafe { enumerator.GetCount() }?;
        let mut sessions: BTreeMap<u32, AudioSession> = BTreeMap::new();
        for index in 0..count {
            let row = unsafe { enumerator.GetSession(index) }
                .and_then(|control| read_session(&control, endpoint_id));
            match row {
                Ok(Some(session)) => {
                    sessions
                        .entry(session.process_id)
                        .and_modify(|existing| existing.active |= session.active)
                        .or_insert(session);
                }
                Ok(None) => {}
                Err(error) if disappeared(&error) => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(sessions.into_values().collect())
    }

    fn read_session(
        control: &IAudioSessionControl,
        endpoint_id: &str,
    ) -> windows::core::Result<Option<AudioSession>> {
        let control2: IAudioSessionControl2 = control.cast()?;
        let process_id = unsafe { control2.GetProcessId() }?;
        if process_id == std::process::id() {
            return Ok(None);
        }
        let state = unsafe { control.GetState() }?;
        if state == AudioSessionStateExpired {
            return Ok(None);
        }
        let display_name = unsafe { control.GetDisplayName() }
            .map(|pointer| TaskString(pointer).text())
            .unwrap_or_default();
        let executable_path = process_path(process_id);
        let name = if process_id == 0 {
            "System sounds".into()
        } else if !display_name.trim().is_empty() && !display_name.starts_with('@') {
            display_name
        } else {
            executable_path
                .as_deref()
                .and_then(|path| path.rsplit(['\\', '/']).next())
                .map(str::to_owned)
                .unwrap_or_else(|| format!("Process {process_id}"))
        };
        Ok(Some(AudioSession {
            executable_path,
            process_id,
            name,
            endpoint_id: endpoint_id.into(),
            active: state == AudioSessionStateActive,
        }))
    }

    fn process_path(process_id: u32) -> Option<String> {
        let handle = ProcessHandle(
            unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id) }.ok()?,
        );
        let mut buffer = vec![0u16; 32768];
        let mut length = buffer.len() as u32;
        unsafe {
            QueryFullProcessImageNameW(
                handle.0,
                PROCESS_NAME_WIN32,
                PWSTR(buffer.as_mut_ptr()),
                &mut length,
            )
        }
        .ok()?;
        Some(String::from_utf16_lossy(&buffer[..length as usize]))
    }

    #[cfg(test)]
    fn process_name(process_id: u32) -> Option<String> {
        process_path(process_id)?
            .rsplit(['\\', '/'])
            .next()
            .map(str::to_owned)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn routing_facade_dispatches_commands_and_preserves_role_preferences() {
            let (commands, receiver) = mpsc::sync_channel(1);
            let worker = thread::spawn(move || {
                match receiver.recv().unwrap() {
                    Command::DefaultPlayback(id, reply) => {
                        assert_eq!(id, "render-id");
                        reply.send(Ok(())).unwrap();
                    }
                    _ => panic!("Expected default playback command"),
                }
                match receiver.recv().unwrap() {
                    Command::DefaultCommunicationsPlayback(id, reply) => {
                        assert_eq!(id, "render-id");
                        reply.send(Ok(())).unwrap();
                    }
                    _ => panic!("Expected communications playback command"),
                }
                match receiver.recv().unwrap() {
                    Command::ProcessPlayback(process_id, id, reply) => {
                        assert_eq!(process_id, 42);
                        assert_eq!(id, "render-id");
                        reply.send(Err("policy unavailable".into())).unwrap();
                    }
                    _ => panic!("Expected process playback command"),
                }
                match receiver.recv().unwrap() {
                    Command::ProcessPlaybackPreference(process_id, reply) => {
                        assert_eq!(process_id, 42);
                        reply
                            .send(Ok(PersistedPlaybackPreference {
                                console: Some("render-id".into()),
                                multimedia: None,
                                communications: Some("other-render-id".into()),
                            }))
                            .unwrap();
                    }
                    _ => panic!("Expected process playback preference command"),
                }
                assert!(matches!(receiver.recv().unwrap(), Command::Shutdown));
            });
            let system = AudioSystem {
                commands,
                worker: Some(worker),
                thread_bound: PhantomData,
            };
            system.set_default_playback("render-id").unwrap();
            system
                .set_default_communications_playback("render-id")
                .unwrap();
            assert_eq!(
                system.set_process_playback(42, "render-id"),
                Err("policy unavailable".into())
            );
            assert_eq!(
                system.get_process_playback_preference(42).unwrap(),
                PersistedPlaybackPreference {
                    console: Some("render-id".into()),
                    multimedia: None,
                    communications: Some("other-render-id".into()),
                }
            );
            system.commands.send(Command::Shutdown).unwrap();
            let mut system = system;
            system.worker.take().unwrap().join().unwrap();
        }

        #[test]
        fn exact_preference_facade_dispatch_preserves_absent_and_different_roles() {
            let expected = PersistedPlaybackPreference {
                console: None,
                multimedia: Some("media-render".into()),
                communications: Some("chat-render".into()),
            };
            let worker_expected = expected.clone();
            let (commands, receiver) = mpsc::sync_channel(1);
            let worker = thread::spawn(move || {
                match receiver.recv().unwrap() {
                    Command::ProcessPlaybackPreferences(process_id, preferences, reply) => {
                        assert_eq!(process_id, 42);
                        assert_eq!(preferences, worker_expected);
                        reply
                            .send(Err("fake partial policy failure".into()))
                            .unwrap();
                    }
                    _ => panic!("Expected exact per-role preference command"),
                }
                assert!(matches!(receiver.recv().unwrap(), Command::Shutdown));
            });
            let system = AudioSystem {
                commands,
                worker: Some(worker),
                thread_bound: PhantomData,
            };
            assert_eq!(
                system.set_process_playback_preferences(42, &expected),
                Err("fake partial policy failure".into()),
            );
            drop(system);
        }

        #[test]
        fn db_facade_dispatches_exact_windows_units_and_rejects_nonfinite() {
            let (commands, receiver) = mpsc::sync_channel(1);
            let worker = thread::spawn(move || {
                match receiver.recv().unwrap() {
                    Command::VolumeDb(id, level, reply) => {
                        assert_eq!(id, "cable");
                        assert_eq!(level, -6.0);
                        reply.send(Ok(())).unwrap();
                    }
                    _ => panic!("Expected dB endpoint command"),
                }
                assert!(matches!(receiver.recv().unwrap(), Command::Shutdown));
            });
            let system = AudioSystem {
                commands,
                worker: Some(worker),
                thread_bound: PhantomData,
            };
            for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
                assert!(system.set_endpoint_volume_db("cable", invalid).is_err());
            }
            assert!(system.set_endpoint_volume_db("", -6.0).is_err());
            system.set_endpoint_volume_db("cable", -6.0).unwrap();
            drop(system);
        }

        #[test]
        fn output_names_validates_before_dispatch_and_returns_registered_names() {
            let (commands, receiver) = mpsc::sync_channel(1);
            let worker = thread::spawn(move || {
                match receiver.recv().unwrap() {
                    Command::ResolveOutputNames(ids, reply) => {
                        assert_eq!(ids, ["offline-output", "unregistered-output"]);
                        reply
                            .send(Ok(BTreeMap::from([(
                                "offline-output".into(),
                                "USB Headphones".into(),
                            )])))
                            .unwrap();
                    }
                    _ => panic!("Expected name resolution command"),
                }
                assert!(matches!(receiver.recv().unwrap(), Command::Shutdown));
            });
            let system = AudioSystem {
                commands,
                worker: Some(worker),
                thread_bound: PhantomData,
            };
            assert!(system.output_names(&vec!["valid".into(); 65]).is_err());
            assert!(system.output_names(&["valid".into(), "".into()]).is_err());
            assert!(
                system
                    .output_names(&["valid".into(), "bad\0id".into()])
                    .is_err()
            );
            let names = system
                .output_names(&["offline-output".into(), "unregistered-output".into()])
                .unwrap();
            assert_eq!(
                names.get("offline-output").map(String::as_str),
                Some("USB Headphones")
            );
            assert!(!names.contains_key("unregistered-output"));
            drop(system);
        }

        #[test]
        fn endpoint_state_validates_before_dispatch_and_returns_target_only() {
            let (commands, receiver) = mpsc::sync_channel(1);
            let worker = thread::spawn(move || {
                match receiver.recv().unwrap() {
                    Command::ReadEndpointState(id, reply) => {
                        assert_eq!(id, "target-output");
                        reply
                            .send(Ok(Endpoint {
                                id,
                                name: "Headphones".into(),
                                volume: 0.2,
                                muted: true,
                                is_virtual: false,
                                level_db: None,
                            }))
                            .unwrap();
                    }
                    _ => panic!("Expected one endpoint state command"),
                }
                assert!(matches!(receiver.recv().unwrap(), Command::Shutdown));
            });
            let system = AudioSystem {
                commands,
                worker: Some(worker),
                thread_bound: PhantomData,
            };
            assert!(system.endpoint_state("").is_err());
            assert!(system.endpoint_state("bad\0id").is_err());
            let endpoint = system.endpoint_state("target-output").unwrap();
            assert_eq!(endpoint.id, "target-output");
            assert_eq!(endpoint.volume, 0.2);
            assert!(endpoint.muted);
            drop(system);
        }

        #[test]
        fn routing_facade_rejects_invalid_arguments_before_dispatch() {
            let (commands, receiver) = mpsc::sync_channel(1);
            let system = AudioSystem {
                commands,
                worker: None,
                thread_bound: PhantomData,
            };
            for id in ["", "endpoint\0suffix"] {
                assert!(system.set_default_playback(id).is_err());
                assert!(system.set_default_communications_playback(id).is_err());
                assert!(system.set_process_playback(42, id).is_err());
            }
            assert!(system.set_process_playback(0, "render-id").is_err());
            assert!(system.get_process_playback_preference(0).is_err());
            let invalid = PersistedPlaybackPreference {
                console: Some("render-id".into()),
                multimedia: None,
                communications: Some("".into()),
            };
            assert!(
                system
                    .set_process_playback_preferences(42, &invalid)
                    .is_err()
            );
            assert!(
                system
                    .set_process_playback_preferences(0, &invalid)
                    .is_err()
            );
            assert!(matches!(
                receiver.try_recv(),
                Err(mpsc::TryRecvError::Empty)
            ));
        }

        #[test]
        #[ignore = "Read-only Windows inventory; run explicitly on a Windows audio host"]
        fn playback_validation_rejects_capture_and_missing_endpoints() {
            let _apartment = Apartment::new().unwrap();
            let enumerator = create_enumerator().unwrap();
            let snapshot = snapshot(&enumerator).unwrap();
            assert!(validate_render_endpoint(&enumerator, "missing-endpoint-id").is_err());
            for endpoint in snapshot.inputs {
                assert!(validate_render_endpoint(&enumerator, &endpoint.id).is_err());
                assert!(
                    validate_render_endpoint_states(
                        &enumerator,
                        &endpoint.id,
                        DEVICE_STATE(DEVICE_STATEMASK_ALL)
                    )
                    .is_err()
                );
            }
            for endpoint in snapshot.outputs {
                assert!(validate_render_endpoint(&enumerator, &endpoint.id).is_ok());
                assert!(
                    validate_render_endpoint_states(
                        &enumerator,
                        &endpoint.id,
                        DEVICE_STATE(DEVICE_STATEMASK_ALL)
                    )
                    .is_ok()
                );
            }
            let registered = unsafe {
                enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE(DEVICE_STATEMASK_ALL))
            }
            .unwrap();
            for index in 0..unsafe { registered.GetCount() }.unwrap() {
                let device = unsafe { registered.Item(index) }.unwrap();
                let id = device_id(&device).unwrap();
                assert!(
                    validate_render_endpoint_states(
                        &enumerator,
                        &id,
                        DEVICE_STATE(DEVICE_STATEMASK_ALL)
                    )
                    .is_ok()
                );
                assert_eq!(
                    validate_render_endpoint(&enumerator, &id).is_ok(),
                    unsafe { device.GetState() }.unwrap() == DEVICE_STATE_ACTIVE
                );
            }
        }

        #[test]
        fn resolves_current_process_name_without_exposing_its_path() {
            let name = super::process_name(std::process::id()).expect("Read current process image");
            assert!(name.ends_with(".exe"));
            assert!(!name.contains(['\\', '/']));
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use super::*;

    pub struct AudioSystem;

    impl AudioSystem {
        pub fn new() -> Result<Self, String> {
            Err("Windows audio integration is supported only on Windows".into())
        }

        pub fn snapshot(&self) -> Result<WindowsSnapshot, String> {
            Err("Windows audio integration is supported only on Windows".into())
        }

        pub fn endpoint_state(&self, id: &str) -> Result<Endpoint, String> {
            validate_id(id)?;
            Err("Windows audio integration is supported only on Windows".into())
        }

        pub fn output_names(
            &self,
            ids: &[String],
        ) -> Result<std::collections::BTreeMap<String, String>, String> {
            validate_output_name_ids(ids)?;
            Err("Windows audio integration is supported only on Windows".into())
        }

        pub fn set_endpoint_volume(&self, id: &str, volume: f32) -> Result<(), String> {
            validate_id(id)?;
            validate_volume(volume)?;
            Err("Windows audio integration is supported only on Windows".into())
        }

        pub fn set_endpoint_volume_db(&self, id: &str, level_db: f32) -> Result<(), String> {
            validate_id(id)?;
            if !level_db.is_finite() {
                return Err("Endpoint dB level must be finite".into());
            }
            Err("Windows audio integration is supported only on Windows".into())
        }

        pub fn set_endpoint_mute(&self, id: &str, _muted: bool) -> Result<(), String> {
            validate_id(id)?;
            Err("Windows audio integration is supported only on Windows".into())
        }

        pub fn set_default_playback(&self, id: &str) -> Result<(), String> {
            validate_id(id)?;
            Err("Windows audio integration is supported only on Windows".into())
        }

        pub fn set_default_communications_playback(&self, id: &str) -> Result<(), String> {
            validate_id(id)?;
            Err("Windows audio integration is supported only on Windows".into())
        }

        pub fn set_process_playback(&self, process_id: u32, id: &str) -> Result<(), String> {
            validate_process_id(process_id)?;
            validate_id(id)?;
            Err("Windows audio integration is supported only on Windows".into())
        }

        pub fn get_process_playback_preference(
            &self,
            process_id: u32,
        ) -> Result<PersistedPlaybackPreference, String> {
            validate_process_id(process_id)?;
            Err("Windows audio integration is supported only on Windows".into())
        }

        pub fn set_process_playback_preferences(
            &self,
            process_id: u32,
            preferences: &PersistedPlaybackPreference,
        ) -> Result<(), String> {
            validate_playback_preferences(process_id, preferences, |_| Ok(()))?;
            Err("Windows audio integration is supported only on Windows".into())
        }
    }
}

pub use platform::AudioSystem;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preference_validation_finishes_before_dispatch_and_skips_absent_roles() {
        let preferences = PersistedPlaybackPreference {
            console: Some("active-render".into()),
            multimedia: None,
            communications: Some("inactive-or-capture".into()),
        };
        let mut validated = Vec::new();
        let mut writes = 0;
        let result = validate_playback_preferences(42, &preferences, |id| {
            validated.push(id.to_owned());
            if id == "active-render" {
                Ok(())
            } else {
                Err("not active render".into())
            }
        })
        .map(|()| {
            writes += 1;
        });
        assert!(result.is_err());
        assert_eq!(validated, ["active-render", "inactive-or-capture"]);
        assert_eq!(writes, 0);
        assert!(validate_playback_preferences(0, &preferences, |_| panic!("PID first")).is_err());
        let absent = PersistedPlaybackPreference {
            console: None,
            multimedia: None,
            communications: None,
        };
        assert!(validate_playback_preferences(42, &absent, |_| panic!("No target")).is_ok());
    }

    #[test]
    fn accepts_finite_volume_including_boundaries() {
        for volume in [0.0, -0.0, 0.0001, 0.5, 0.9999, 1.0] {
            assert!(validate_volume(volume).is_ok(), "{volume}");
        }
    }

    #[test]
    fn rejects_nonfinite_and_out_of_range_volume() {
        for volume in [
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            -0.0001,
            1.0001,
            f32::MIN,
            f32::MAX,
        ] {
            assert!(validate_volume(volume).is_err(), "{volume}");
        }
    }

    #[test]
    fn rejects_invalid_endpoint_ids_without_truncation() {
        assert!(validate_id("").is_err());
        assert!(validate_id("endpoint\0suffix").is_err());
        assert!(validate_id("{0.0.0.00000000}.{example-id}").is_ok());
    }

    #[test]
    fn rejects_zero_process_id() {
        assert!(validate_process_id(0).is_err());
        assert!(validate_process_id(1).is_ok());
        assert!(validate_process_id(u32::MAX).is_ok());
    }

    #[test]
    fn virtual_detection_requires_approved_transport_identity() {
        for identity in [
            "VB-Audio Virtual Cable",
            "VB-Audio Cable A",
            "VB-Audio Hi-Fi Cable",
            "VB-CABLE",
            "Virtual Audio Cable",
            "SteelSeries Sonar Virtual Audio Device",
        ] {
            assert!(known_virtual_transport(&[identity.into()]), "{identity}");
        }
        for identity in [
            "",
            "Microphone",
            "Microphone Array (Realtek Audio)",
            "USB Headset",
            "Speakers",
            "Virtual Microphone",
            "Cable Input",
            "Line In",
            "App Sandbox Virtual Audio",
            "VB-Audio",
            "VoiceMeeter Input",
            "Gaming",
            "Chat",
            "ROOT\\MEDIA\\0000",
        ] {
            assert!(!known_virtual_transport(&[identity.into()]), "{identity}");
        }
        assert!(!known_virtual_transport(&[]));
    }

    #[test]
    fn renamed_endpoint_retains_driver_transport_classification() {
        let driver_identity = vec!["Virtual Audio Cable".into()];
        for label in ["Line 1", "Gaming", "Chat", "Media", "Aux"] {
            let endpoint = Endpoint {
                level_db: None,
                id: "stable-endpoint-id".into(),
                name: label.into(),
                volume: 1.0,
                muted: false,
                is_virtual: known_virtual_transport(&driver_identity),
            };
            assert!(endpoint.is_virtual, "{}", endpoint.name);
        }
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "Read-only Windows inventory; run explicitly on a Windows audio host"]
    fn inventory() {
        let system = AudioSystem::new().expect("Initialize Windows audio");
        for _ in 0..2 {
            let snapshot = system.snapshot().expect("Read Windows audio snapshot");
            for endpoint in snapshot.outputs.iter().chain(&snapshot.inputs) {
                assert!(!endpoint.id.is_empty());
                assert!(validate_volume(endpoint.volume).is_ok());
            }
            let mut keys = std::collections::HashSet::new();
            for session in &snapshot.sessions {
                assert_ne!(session.process_id, std::process::id());
                assert!(keys.insert((&session.endpoint_id, session.process_id)));
                assert!(
                    snapshot
                        .outputs
                        .iter()
                        .any(|endpoint| endpoint.id == session.endpoint_id)
                );
            }
            println!("{snapshot:#?}");
        }
    }
}
