#[cfg(windows)]
mod qualification {
    use anada_windows::{AudioSystem, PersistedPlaybackPreference};
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use serde_json::{Value, json};
    use std::fs::{File, OpenOptions};
    use std::io::Write;

    fn silent_render_session(raw_id: &str) -> Result<cpal::Stream, String> {
        let host = cpal::default_host();
        let device = host
            .output_devices()
            .map_err(|error| format!("Enumerate render devices: {error}"))?
            .find(|device| device.id().is_ok_and(|id| id.id() == raw_id))
            .ok_or("Selected physical render endpoint is unavailable")?;
        let config = device
            .default_output_config()
            .map_err(|error| format!("Read render format: {error}"))?;
        if config.sample_format() != cpal::SampleFormat::F32 {
            return Err("This qualification requires an F32 render endpoint".into());
        }
        let stream = device
            .build_output_stream(
                config.into(),
                |samples: &mut [f32], _| samples.fill(0.0),
                |error| eprintln!("Silent qualification stream: {error}"),
                None,
            )
            .map_err(|error| format!("Open silent render session: {error}"))?;
        stream
            .play()
            .map_err(|error| format!("Start silent render session: {error}"))?;
        Ok(stream)
    }

    trait ProcessPolicy {
        fn set(&self, process_id: u32, saved: &PersistedPlaybackPreference) -> Result<(), String>;
        fn get(&self, process_id: u32) -> Result<PersistedPlaybackPreference, String>;
    }

    impl ProcessPolicy for AudioSystem {
        fn set(&self, process_id: u32, saved: &PersistedPlaybackPreference) -> Result<(), String> {
            self.set_process_playback_preferences(process_id, saved)
        }

        fn get(&self, process_id: u32) -> Result<PersistedPlaybackPreference, String> {
            self.get_process_playback_preference(process_id)
        }
    }

    fn preference_json(preference: &PersistedPlaybackPreference) -> Value {
        json!({
            "console": preference.console,
            "multimedia": preference.multimedia,
            "communications": preference.communications,
        })
    }

    fn emit(file: &mut File, record: &Value) -> Result<(), String> {
        serde_json::to_writer(&mut *file, record)
            .map_err(|error| format!("Serialize evidence: {error}"))?;
        file.write_all(b"\n")
            .and_then(|()| file.flush())
            .and_then(|()| file.sync_all())
            .map_err(|error| format!("Persist evidence: {error}"))
    }

    fn restore_exact(
        policy: &impl ProcessPolicy,
        process_id: u32,
        saved: &PersistedPlaybackPreference,
    ) -> (bool, Value, Vec<String>) {
        let mut errors = Vec::new();
        if let Err(error) = policy.set(process_id, saved) {
            errors.push(format!("Restore setter: {error}"));
        }
        let (verified, observed) = match policy.get(process_id) {
            Ok(observed) => {
                let verified = observed == *saved;
                if !verified {
                    errors.push("Restored preferences do not match all three saved roles".into());
                }
                (verified, preference_json(&observed))
            }
            Err(error) => {
                errors.push(format!("Verify restore getter: {error}"));
                (false, Value::Null)
            }
        };
        (verified, observed, errors)
    }

    struct RestoreGuard<'guard> {
        system: &'guard AudioSystem,
        file: &'guard mut File,
        process_id: u32,
        saved: PersistedPlaybackPreference,
        armed: bool,
    }

    impl RestoreGuard<'_> {
        fn restore(&mut self, source: &str) -> Result<(), String> {
            let (verified, observed, mut errors) =
                restore_exact(self.system, self.process_id, &self.saved);
            self.armed = !verified;
            if let Err(error) = emit(
                self.file,
                &json!({
                    "event": "restore", "source": source, "process_id": self.process_id,
                    "saved": preference_json(&self.saved), "observed": observed,
                    "verified": verified, "errors": errors,
                }),
            ) {
                errors.push(error);
            }
            if errors.is_empty() {
                Ok(())
            } else {
                Err(errors.join("; "))
            }
        }
    }

    impl Drop for RestoreGuard<'_> {
        fn drop(&mut self) {
            if self.armed
                && let Err(error) = self.restore("drop_fallback")
            {
                eprintln!("Process preference restore fallback: {error}");
            }
        }
    }

    pub fn run() -> Result<(), String> {
        let arguments: Vec<String> = std::env::args().skip(1).collect();
        if arguments.len() != 2 {
            return Err(
                "Usage: qualify_process <raw-virtual-render-id> <new-evidence.jsonl>".into(),
            );
        }
        let target = &arguments[0];
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&arguments[1])
            .map_err(|error| format!("Create new evidence file: {error}"))?;
        let system = AudioSystem::new()?;
        let snapshot = system.snapshot()?;
        let physical = snapshot
            .outputs
            .iter()
            .find(|endpoint| !endpoint.is_virtual)
            .ok_or("No physical render endpoint is available")?;
        let _stream = silent_render_session(&physical.id)?;
        std::thread::sleep(std::time::Duration::from_millis(300));
        let process_id = std::process::id();
        let saved = system.get_process_playback_preference(process_id)?;
        emit(
            &mut file,
            &json!({
                "event": "recovery_before_changes", "process_id": process_id,
                "saved": preference_json(&saved), "target": target,
                "mutation_started": false,
                "recovery_api": "AudioSystem::set_process_playback_preferences(pid, &saved)",
                "none_semantics": "Clear only the specified PID/render/role with a null HSTRING",
                "scope": "Own PID only; silent explicit render session; no global/default reset or microphone stream",
                "silent_render_endpoint": physical.id,
                "recovery_limit": "Drop cannot run after termination or abort. PID reuse makes later recovery unsafe without verifying process identity.",
            }),
        )?;
        let snapshot = system.snapshot()?;
        if !snapshot
            .outputs
            .iter()
            .any(|endpoint| endpoint.id == *target && endpoint.is_virtual)
        {
            return Err("Target is not an active recognized virtual render endpoint".into());
        }
        for endpoint in [
            saved.console.as_deref(),
            saved.multimedia.as_deref(),
            saved.communications.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            if !snapshot.outputs.iter().any(|output| output.id == endpoint) {
                return Err(format!(
                    "Saved endpoint is not active; refusing an unrestorable test: {endpoint}"
                ));
            }
        }
        let mut guard = RestoreGuard {
            system: &system,
            file: &mut file,
            process_id,
            saved,
            armed: true,
        };
        let exercise = (|| -> Result<(), String> {
            emit(
                guard.file,
                &json!({ "event": "mutation_start", "process_id": process_id }),
            )?;
            system.set_process_playback(process_id, target)?;
            let observed = system.get_process_playback_preference(process_id)?;
            let expected = PersistedPlaybackPreference {
                console: Some(target.clone()),
                multimedia: Some(target.clone()),
                communications: Some(target.clone()),
            };
            let verified = observed == expected;
            emit(
                guard.file,
                &json!({
                    "event": "target_preference_readback", "process_id": process_id,
                    "expected": preference_json(&expected), "observed": preference_json(&observed),
                    "all_three_roles_match": verified,
                    "claim": "Persisted preference readback only; no stream-movement or audible claim",
                }),
            )?;
            if verified {
                Ok(())
            } else {
                Err("Target readback does not match all three roles".into())
            }
        })();
        let restore = guard.restore("explicit_after_exercise");
        let mut errors = Vec::new();
        if let Err(error) = exercise {
            errors.push(format!("Qualification: {error}"));
        }
        if let Err(error) = restore {
            errors.push(error);
        }
        let result = if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        };
        emit(
            guard.file,
            &json!({
                "event": "qualification_result", "process_id": process_id,
                "passed": result.is_ok(), "errors": errors,
                "restoration_verified": !guard.armed,
            }),
        )?;
        result
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::cell::RefCell;

        struct FakePolicy {
            writes: RefCell<Vec<(u32, PersistedPlaybackPreference)>>,
            reads: RefCell<Vec<u32>>,
            setter_error: bool,
            observed: Result<PersistedPlaybackPreference, String>,
        }

        impl ProcessPolicy for FakePolicy {
            fn set(&self, pid: u32, saved: &PersistedPlaybackPreference) -> Result<(), String> {
                self.writes.borrow_mut().push((pid, saved.clone()));
                if self.setter_error {
                    Err("fake partial write".into())
                } else {
                    Ok(())
                }
            }

            fn get(&self, pid: u32) -> Result<PersistedPlaybackPreference, String> {
                self.reads.borrow_mut().push(pid);
                self.observed.clone()
            }
        }

        #[test]
        fn restore_preserves_absent_and_different_roles_and_reads_after_setter_error() {
            let saved = PersistedPlaybackPreference {
                console: None,
                multimedia: Some("original-media".into()),
                communications: Some("original-chat".into()),
            };
            let fake = FakePolicy {
                writes: RefCell::new(Vec::new()),
                reads: RefCell::new(Vec::new()),
                setter_error: true,
                observed: Ok(saved.clone()),
            };
            let (verified, _, errors) = restore_exact(&fake, 42, &saved);
            assert!(verified);
            assert_eq!(errors.len(), 1);
            assert_eq!(*fake.writes.borrow(), [(42, saved)]);
            assert_eq!(*fake.reads.borrow(), [42]);
        }

        #[test]
        fn getter_failure_or_role_mismatch_never_verifies_restore() {
            let saved = PersistedPlaybackPreference {
                console: None,
                multimedia: None,
                communications: None,
            };
            for observed in [
                Err("fake getter failure".into()),
                Ok(PersistedPlaybackPreference {
                    console: Some("wrong".into()),
                    multimedia: None,
                    communications: None,
                }),
            ] {
                let fake = FakePolicy {
                    writes: RefCell::new(Vec::new()),
                    reads: RefCell::new(Vec::new()),
                    setter_error: false,
                    observed,
                };
                let (verified, _, errors) = restore_exact(&fake, 42, &saved);
                assert!(!verified);
                assert!(!errors.is_empty());
                assert_eq!(*fake.reads.borrow(), [42]);
            }
        }
    }
}

fn main() {
    #[cfg(windows)]
    if let Err(error) = qualification::run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
    #[cfg(not(windows))]
    {
        eprintln!("Process preference qualification requires Windows");
        std::process::exit(1);
    }
}
