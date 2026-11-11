#[cfg(windows)]
mod qualification {
    use anada_windows::AudioSystem;
    use serde_json::{Value, json};
    use std::fs::{File, OpenOptions};
    use std::io::{self, Write};
    use std::time::{SystemTime, UNIX_EPOCH};
    use windows::Win32::Media::Audio::{
        ERole, IMMDeviceEnumerator, MMDeviceEnumerator, eCommunications, eConsole, eMultimedia,
        eRender,
    };
    use windows::Win32::System::Com::{
        CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
        CoUninitialize,
    };

    struct Apartment;

    impl Drop for Apartment {
        fn drop(&mut self) {
            unsafe { CoUninitialize() };
        }
    }

    fn read_default(enumerator: &IMMDeviceEnumerator, role: ERole) -> Result<String, String> {
        let device = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, role) }
            .map_err(|error| format!("Get default endpoint: {error}"))?;
        let allocated =
            unsafe { device.GetId() }.map_err(|error| format!("Get default ID: {error}"))?;
        let result =
            unsafe { allocated.to_string() }.map_err(|error| format!("Decode default ID: {error}"));
        unsafe { CoTaskMemFree(Some(allocated.0.cast())) };
        let endpoint_id = result?;
        if endpoint_id.is_empty() || endpoint_id.contains('\0') {
            return Err("Default endpoint ID is empty or contains NUL".into());
        }
        Ok(endpoint_id)
    }

    fn read_defaults(enumerator: &IMMDeviceEnumerator) -> (Value, Vec<String>) {
        let mut defaults = json!({});
        let mut errors = Vec::new();
        for (name, role) in [
            ("console", eConsole),
            ("multimedia", eMultimedia),
            ("communications", eCommunications),
        ] {
            match read_default(enumerator, role) {
                Ok(endpoint_id) => defaults[name] = json!(endpoint_id),
                Err(error) => {
                    defaults[name] = Value::Null;
                    errors.push(format!("{name}: {error}"));
                }
            }
        }
        (defaults, errors)
    }

    struct RestoreGuard<'guard> {
        system: &'guard AudioSystem,
        enumerator: &'guard IMMDeviceEnumerator,
        initial: &'guard Value,
        attempts: &'guard mut Vec<Value>,
        restored: bool,
    }

    impl RestoreGuard<'_> {
        fn restore(&mut self, source: &str) {
            let mut errors = Vec::new();
            if let Err(error) = self.system.set_default_playback(
                self.initial["console"]
                    .as_str()
                    .expect("Validated console ID"),
            ) {
                errors.push(format!("Restore ordinary defaults: {error}"));
            }
            if let Err(error) = self.system.set_default_communications_playback(
                self.initial["communications"]
                    .as_str()
                    .expect("Validated communications ID"),
            ) {
                errors.push(format!("Restore communications default: {error}"));
            }
            let (observed, read_errors) = read_defaults(self.enumerator);
            errors.extend(read_errors);
            self.restored = observed == *self.initial;
            if !self.restored {
                errors.push("Restoration did not reproduce all three exact initial IDs".into());
            }
            self.attempts.push(json!({
                "source": source, "observed": observed,
                "restored": self.restored, "errors": errors,
            }));
        }
    }

    impl Drop for RestoreGuard<'_> {
        fn drop(&mut self) {
            if !self.restored {
                self.restore("drop_fallback");
                let _ = writeln!(
                    io::stderr(),
                    "{}",
                    json!({
                        "event": "drop_restoration", "report": self.attempts.last(),
                    })
                );
            }
        }
    }

    fn emit(file: &mut File, event: &Value) -> Result<(), String> {
        serde_json::to_writer(&mut *file, event)
            .map_err(|error| format!("Serialize report: {error}"))?;
        file.write_all(b"\n")
            .and_then(|()| file.flush())
            .and_then(|()| file.sync_all())
            .map_err(|error| format!("Persist report: {error}"))?;
        let stdout = io::stdout();
        let mut output = stdout.lock();
        serde_json::to_writer(&mut output, event)
            .map_err(|error| format!("Serialize stdout report: {error}"))?;
        output
            .write_all(b"\n")
            .and_then(|()| output.flush())
            .map_err(|error| format!("Flush stdout report: {error}"))
    }

    fn exercise(
        system: &AudioSystem,
        enumerator: &IMMDeviceEnumerator,
        gaming: &str,
        chat: &str,
        observations: &mut Value,
    ) -> Result<(), String> {
        system
            .set_default_playback(gaming)
            .map_err(|error| format!("Set Gaming ordinary defaults: {error}"))?;
        system
            .set_default_communications_playback(chat)
            .map_err(|error| format!("Set Chat communications default: {error}"))?;
        let snapshot = system.snapshot();
        let multimedia = read_default(enumerator, eMultimedia);
        let mut errors = Vec::new();
        match snapshot {
            Ok(snapshot) => {
                observations["snapshot"] = json!({
                    "console": snapshot.default_output_id,
                    "communications": snapshot.communications_output_id,
                });
                if snapshot.default_output_id.as_deref() != Some(gaming) {
                    errors.push("Snapshot console default does not match Gaming".to_string());
                }
                if snapshot.communications_output_id.as_deref() != Some(chat) {
                    errors.push("Snapshot communications default does not match Chat".to_string());
                }
            }
            Err(error) => errors.push(format!("Read qualification snapshot: {error}")),
        }
        match multimedia {
            Ok(endpoint_id) => {
                observations["multimedia_direct_com"] = json!(endpoint_id);
                if endpoint_id != gaming {
                    errors.push("Direct COM multimedia default does not match Gaming".into());
                }
            }
            Err(error) => errors.push(format!("Read multimedia default: {error}")),
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }

    pub fn run() -> Result<bool, String> {
        let arguments: Vec<String> = std::env::args().skip(1).collect();
        if !(2..=3).contains(&arguments.len()) {
            return Err("Usage: qualify_defaults <raw-gaming-render-id> <raw-chat-render-id> [report.jsonl]".into());
        }
        let gaming = &arguments[0];
        let chat = &arguments[1];
        let report_path = match arguments.get(2) {
            Some(path) => std::path::PathBuf::from(path),
            None => {
                let timestamp = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|error| format!("Read clock: {error}"))?
                    .as_nanos();
                std::fs::create_dir_all("work")
                    .map_err(|error| format!("Create report directory: {error}"))?;
                format!(
                    "work/qualify-defaults-{}-{timestamp}.jsonl",
                    std::process::id()
                )
                .into()
            }
        };
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&report_path)
            .map_err(|error| format!("Create new report {}: {error}", report_path.display()))?;
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .ok()
            .map_err(|error| format!("Initialize qualification COM thread: {error}"))?;
        let _apartment = Apartment;
        let enumerator: IMMDeviceEnumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
                .map_err(|error| format!("Create endpoint enumerator: {error}"))?;
        let (initial, mut errors) = read_defaults(&enumerator);
        emit(
            &mut file,
            &json!({
                "event": "initial_defaults", "report_path": report_path,
                "process_id": std::process::id(), "initial": initial,
                "targets": { "gaming": gaming, "chat": chat },
                "errors": errors, "mutation_started": false,
                "format": "JSON Lines: initial recovery record followed by final report",
                "recovery_limit": "Drop cannot run after process termination or abort; retain initial IDs for manual recovery",
            }),
        )?;
        let system = AudioSystem::new()?;
        let preference = match system.get_process_playback_preference(std::process::id()) {
            Ok(preference) => json!({
                "success": true, "console": preference.console,
                "multimedia": preference.multimedia, "communications": preference.communications,
            }),
            Err(error) => json!({ "success": false, "error": error }),
        };
        if initial["console"].is_string()
            && initial["multimedia"].is_string()
            && initial["console"] != initial["multimedia"]
        {
            errors.push(
                "Console and multimedia originals differ; facade cannot restore them independently"
                    .into(),
            );
        }
        if gaming == chat {
            errors.push("Gaming and Chat targets must be distinct".into());
        }
        match system.snapshot() {
            Ok(snapshot) => {
                for (name, target) in [("Gaming", gaming), ("Chat", chat)] {
                    if target.trim().is_empty()
                        || target.contains('\0')
                        || !snapshot
                            .outputs
                            .iter()
                            .any(|endpoint| endpoint.id == *target && endpoint.is_virtual)
                    {
                        errors.push(format!("{name} target is not an active, recognized virtual render endpoint with this exact raw ID"));
                    }
                }
            }
            Err(error) => errors.push(format!("Validate targets: {error}")),
        }
        let mut observations = json!({});
        let mut restoration = Vec::new();
        let mut mutation_started = false;
        if errors.is_empty() {
            let (current, read_errors) = read_defaults(&enumerator);
            errors.extend(read_errors);
            if current != initial {
                errors.push(
                    "Defaults changed during preflight; refusing to overwrite external changes"
                        .into(),
                );
            }
        }
        if errors.is_empty() {
            let mut guard = RestoreGuard {
                system: &system,
                enumerator: &enumerator,
                initial: &initial,
                attempts: &mut restoration,
                restored: false,
            };
            mutation_started = true;
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                exercise(&system, &enumerator, gaming, chat, &mut observations)
            }));
            match result {
                Ok(Ok(())) => (),
                Ok(Err(error)) => errors.push(error),
                Err(_) => errors.push("Qualification panicked; restoration attempted".into()),
            }
            guard.restore("explicit");
        }
        let (final_defaults, final_errors) = read_defaults(&enumerator);
        errors.extend(final_errors);
        let restored = initial["console"].is_string()
            && initial["multimedia"].is_string()
            && initial["communications"].is_string()
            && final_defaults == initial;
        for attempt in &restoration {
            if let Some(attempt_errors) = attempt["errors"].as_array() {
                errors.extend(
                    attempt_errors
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned),
                );
            }
        }
        if !restored {
            errors.push(
                "Final defaults are not verified equal to all three saved initial IDs".into(),
            );
        }
        let success = mutation_started && errors.is_empty() && restored;
        emit(
            &mut file,
            &json!({
                "event": "qualification_result", "success": success, "errors": errors,
                "mutation_started": mutation_started, "initial": initial,
                "observations": observations, "restoration": restoration,
                "final_defaults": final_defaults, "restored": restored,
                "process_preference_read_only": {
                    "process_id": std::process::id(), "result": preference,
                    "audible_proof": false, "writes_tested": false,
                    "note": "Preference readback is not audible proof; absent-preference clearing is not wired",
                },
                "audible_proof": false,
            }),
        )?;
        Ok(success)
    }
}

fn main() {
    #[cfg(windows)]
    let result = qualification::run();
    #[cfg(not(windows))]
    let result: Result<bool, String> = Err("This qualification requires Windows".into());
    match result {
        Ok(true) => (),
        Ok(false) => std::process::exit(1),
        Err(error) => {
            let _ = std::io::Write::write_all(&mut std::io::stderr(), format!("{}\n", serde_json::json!({
                "event": "harness_error", "success": false, "errors": [error],
                "restored": null, "note": "Inspect persisted initial and restoration records",
            })).as_bytes());
            std::process::exit(1);
        }
    }
}
