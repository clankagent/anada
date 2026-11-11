//! Bounded, latest-value UI scheduling. The UI never waits for host IPC.
use crate::model::{Command, HostState};
use eframe::egui;
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
const METER_INTERVAL: Duration = Duration::from_millis(50);

pub type RequestId = u64;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tag {
    Poll,
    Meters,
    Start,
    Save,
    Stop,
    Defaults,
    Assignment,
    Other,
}

pub struct Outcome {
    pub id: RequestId,
    pub tag: Tag,
    pub command: Option<Command>,
    pub result: Result<HostState, String>,
}

#[derive(Clone)]
struct Work {
    id: RequestId,
    tag: Tag,
    command: Option<Command>,
}
#[derive(Default)]
struct State {
    next_id: RequestId,
    actions: VecDeque<Work>,
    controls: BTreeMap<(String, bool), Work>,
    save: Option<Work>,
    inflight: Option<Work>,
    outcome: Option<Outcome>,
    closed: bool,
    save_streak: u8,
    meter_enabled: bool,
}
impl State {
    fn take_control(&mut self) -> Option<Work> {
        let key = self
            .controls
            .iter()
            .min_by_key(|(_, work)| work.id)
            .map(|(key, _)| key.clone())?;
        self.controls.remove(&key)
    }
    fn queued(&self) -> bool {
        self.save.is_some() || !self.actions.is_empty() || !self.controls.is_empty()
    }
    fn take(&mut self) -> Option<Work> {
        // Explicit actions, including Save, cannot be starved by slider motion.
        if self.save_streak >= 2 && !self.controls.is_empty() {
            self.save_streak = 0;
            return self.take_control();
        }
        if let Some(save) = self.save.take() {
            self.save_streak = self.save_streak.saturating_add(1);
            return Some(save);
        }
        self.save_streak = 0;
        self.actions.pop_front().or_else(|| self.take_control())
    }
    fn submit(&mut self, mut command: Option<Command>, tag: Tag) -> Result<RequestId, String> {
        if self.closed {
            return Err("The command worker has stopped. Reopen this window.".into());
        }
        // Completion can race the UI frame: poll() saw no result, then the
        // previous Save completed before this frame submitted its newer draft.
        // Apply only the exact own-success revision transition still awaiting
        // delivery, rather than using arbitrary observed host revisions.
        if let Some(Outcome {
            command:
                Some(Command::SaveConfig {
                    expected_revision: previous,
                    ..
                }),
            result: Ok(host),
            ..
        }) = &self.outcome
            && let Some(Command::SaveConfig {
                expected_revision, ..
            }) = &mut command
            && *expected_revision == *previous
            && host.revision == previous.saturating_add(1)
        {
            *expected_revision = host.revision;
        }
        let key = match &command {
            Some(
                Command::SetVolume { endpoint_id, .. } | Command::SetVolumeDb { endpoint_id, .. },
            ) => Some((endpoint_id.clone(), false)),
            Some(Command::SetMute { endpoint_id, .. }) => Some((endpoint_id.clone(), true)),
            _ => None,
        };
        let same = |work: &Work| work.tag == tag && same_command(&work.command, &command);
        // Deduplicate a command only while it is queued or in flight. A later
        // explicit action may need to restore a setting changed outside Anada.
        let coalescible = key.is_some() || matches!(command, Some(Command::SaveConfig { .. }));
        if let Some(work) = self
            .save
            .iter()
            .chain(self.controls.values())
            .find(|work| coalescible && same(work))
        {
            return Ok(work.id);
        }
        let replacing_pending = key
            .as_ref()
            .is_some_and(|key| self.controls.contains_key(key))
            || (matches!(command, Some(Command::SaveConfig { .. })) && self.save.is_some());
        if coalescible
            && !replacing_pending
            && let Some(work) = self.inflight.as_ref().filter(|work| same(work))
        {
            return Ok(work.id);
        }
        if let Some(key) = &key {
            if !self.controls.contains_key(key) && self.controls.len() >= 256 {
                return Err(
                    "Too many endpoint controls are pending. Wait for the host, then retry.".into(),
                );
            }
        } else if !matches!(command, Some(Command::SaveConfig { .. })) && self.actions.len() >= 32 {
            return Err(
                "The host is busy with explicit actions. Wait for completion, then retry.".into(),
            );
        }
        self.next_id += 1;
        let work = Work {
            id: self.next_id,
            tag,
            command,
        };
        if let Some(key) = key {
            self.controls.insert(key, work);
        } else if matches!(work.command, Some(Command::SaveConfig { .. })) {
            self.save = Some(work);
        } else {
            self.actions.push_back(work);
        }
        Ok(self.next_id)
    }
}
fn same_command(left: &Option<Command>, right: &Option<Command>) -> bool {
    serde_json::to_value(left).ok() == serde_json::to_value(right).ok()
}

pub struct Client {
    shared: Arc<(Mutex<State>, Condvar)>,
}
impl Client {
    pub fn spawn(context: egui::Context) -> Self {
        Self::with_backend(context, Duration::from_secs(1), |command| match command {
            Some(command) => crate::transport::request(command),
            None => crate::transport::ensure_host()
                .and_then(|()| crate::transport::request(Command::Snapshot)),
        })
    }
    /// Explicit backend injection for isolated UI tests and the demo fixture.
    /// Normal application windows must use `spawn` to connect to the real host.
    pub(crate) fn with_backend(
        context: egui::Context,
        interval: Duration,
        mut backend: impl FnMut(Option<Command>) -> Result<HostState, String> + Send + 'static,
    ) -> Self {
        let shared = Arc::new((Mutex::new(State::default()), Condvar::new()));
        let worker = shared.clone();
        std::thread::spawn(move || {
            let (lock, wake) = &*worker;
            let mut next_inventory = Instant::now() + interval;
            let mut next_meter = Instant::now();
            loop {
                let mut state = lock.lock().unwrap();
                // A single result slot applies backpressure even when the UI
                // is minimized or not painting. Polls never accumulate.
                while state.outcome.is_some() && !state.closed {
                    state = wake.wait(state).unwrap();
                }
                if state.closed {
                    break;
                }
                let now = Instant::now();
                let work = if let Some(work) = state.take() {
                    work
                } else if now >= next_inventory {
                    next_inventory = now + interval;
                    Work {
                        id: 0,
                        tag: Tag::Poll,
                        command: Some(Command::Snapshot),
                    }
                } else if state.meter_enabled && now >= next_meter {
                    next_meter = now + METER_INTERVAL;
                    Work {
                        id: 0,
                        tag: Tag::Meters,
                        command: Some(Command::SnapshotMeters),
                    }
                } else {
                    let next_poll = if state.meter_enabled {
                        next_meter.min(next_inventory)
                    } else {
                        next_inventory
                    };
                    drop(
                        wake.wait_timeout(state, next_poll.saturating_duration_since(now))
                            .unwrap(),
                    );
                    continue;
                };
                state.inflight = Some(work.clone());
                drop(state);
                let result = backend(work.command.clone());
                let mut state = lock.lock().unwrap();
                // Only an exact successful own Save transition can rebase a
                // newer queued Save. Never adopt a revision seen in a poll or
                // returned after another window changed the configuration.
                if let (
                    Some(Command::SaveConfig {
                        expected_revision, ..
                    }),
                    Ok(host),
                ) = (&work.command, &result)
                    && host.revision == expected_revision.saturating_add(1)
                    && let Some(Work {
                        command:
                            Some(Command::SaveConfig {
                                expected_revision: pending,
                                ..
                            }),
                        ..
                    }) = state.save.as_mut()
                    && *pending == *expected_revision
                {
                    *pending = host.revision;
                }
                state.inflight = None;
                state.outcome = Some(Outcome {
                    id: work.id,
                    tag: work.tag,
                    command: work.command,
                    result,
                });
                drop(state);
                context.request_repaint();
            }
        });
        Self { shared }
    }
    pub fn submit(&self, command: Command, tag: Tag) -> Result<RequestId, String> {
        self.enqueue(Some(command), tag)
    }
    pub fn start(&self) -> Result<RequestId, String> {
        self.enqueue(None, Tag::Start)
    }
    /// Enable lightweight 20 Hz meter reads while the mixer is visible and
    /// connected. Disabled initially, including for isolated test backends.
    pub fn set_metering(&self, enabled: bool) {
        let (lock, wake) = &*self.shared;
        let mut state = lock.lock().unwrap();
        if state.meter_enabled != enabled {
            state.meter_enabled = enabled;
            wake.notify_one();
        }
    }
    fn enqueue(&self, command: Option<Command>, tag: Tag) -> Result<RequestId, String> {
        let (lock, wake) = &*self.shared;
        let result = lock.lock().unwrap().submit(command, tag);
        wake.notify_one();
        result
    }
    pub fn drain(&self) -> Vec<Outcome> {
        let (lock, wake) = &*self.shared;
        let result = lock.lock().unwrap().outcome.take().into_iter().collect();
        wake.notify_one();
        result
    }
    pub fn has_pending(&self) -> bool {
        let state = self.shared.0.lock().unwrap();
        state.queued()
            || state
                .inflight
                .as_ref()
                .is_some_and(|work| !matches!(work.tag, Tag::Poll | Tag::Meters))
            || state
                .outcome
                .as_ref()
                .is_some_and(|outcome| !matches!(outcome.tag, Tag::Poll | Tag::Meters))
    }
    pub fn has_pending_actions(&self) -> bool {
        let state = self.shared.0.lock().unwrap();
        state.save.is_some()
            || !state.actions.is_empty()
            || state.inflight.as_ref().is_some_and(|work| {
                !matches!(work.tag, Tag::Poll | Tag::Meters)
                    && !matches!(
                        work.command,
                        Some(
                            Command::SetVolume { .. }
                                | Command::SetVolumeDb { .. }
                                | Command::SetMute { .. }
                        )
                    )
            })
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        self.shared.0.lock().unwrap().closed = true;
        self.shared.1.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn meters_are_bounded_low_priority_and_do_not_poll_inventory_at_meter_rate() {
        use std::sync::mpsc;
        let (entered, calls) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        let mut first = true;
        let client = Client::with_backend(
            egui::Context::default(),
            Duration::from_secs(1),
            move |command| {
                entered.send(command).unwrap();
                if first {
                    first = false;
                    blocked.recv().unwrap();
                }
                Err("meter test response".into())
            },
        );
        client.set_metering(true);
        assert!(matches!(
            calls.recv_timeout(Duration::from_secs(2)).unwrap(),
            Some(Command::SnapshotMeters)
        ));
        assert!(!client.has_pending());
        for step in 0..1000 {
            client
                .submit(
                    Command::SetVolume {
                        endpoint_id: "one".into(),
                        volume: step as f32 / 1000.0,
                    },
                    Tag::Other,
                )
                .unwrap();
        }
        client
            .submit(
                Command::SaveConfig {
                    config: Default::default(),
                    expected_revision: 4,
                },
                Tag::Save,
            )
            .unwrap();
        release.send(()).unwrap();
        let drain_one = || {
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                if let Some(outcome) = client.drain().pop() {
                    return outcome;
                }
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            }
        };
        assert_eq!(drain_one().tag, Tag::Meters);
        assert!(matches!(
            calls.recv_timeout(Duration::from_secs(2)).unwrap(),
            Some(Command::SaveConfig { .. })
        ));
        assert_eq!(drain_one().tag, Tag::Save);
        assert!(
            matches!(calls.recv_timeout(Duration::from_secs(2)).unwrap(), Some(Command::SetVolume { volume, .. }) if volume == 0.999)
        );
        assert_eq!(drain_one().tag, Tag::Other);
        let deadline = Instant::now() + Duration::from_millis(1100);
        let mut inventory = 0;
        let mut meters = 0;
        while Instant::now() < deadline {
            for outcome in client.drain() {
                match outcome.tag {
                    Tag::Poll => inventory += 1,
                    Tag::Meters => meters += 1,
                    _ => panic!("unexpected action"),
                }
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(
            inventory, 1,
            "only the 1 Hz inventory deadline may read full state"
        );
        assert!(
            (10..=24).contains(&meters),
            "expected bounded ~20Hz meter reads, got {meters}"
        );
        assert!(!client.has_pending());
        client.set_metering(false);
        // Calls already observed remain bounded even if the UI stops draining.
        assert!(calls.try_iter().count() <= 25);
    }
    fn host(revision: u64) -> HostState {
        HostState {
            audio_diagnostics: Default::default(),
            protocol_version: crate::model::PROTOCOL_VERSION,
            revision,
            config: Default::default(),
            windows: anada_windows::WindowsSnapshot {
                outputs: vec![],
                inputs: vec![],
                sessions: vec![],
                default_output_id: None,
                communications_output_id: None,
            },
            selected_output_id: None,
            manual_output: false,
            audio_running: false,
            audio_status: String::new(),
            warnings: vec![],
            host_pid: 0,
            quiet: Default::default(),
        }
    }
    #[test]
    fn save_submitted_after_own_completion_rebases_but_external_revision_does_not() {
        for returned_revision in [11, 12] {
            let mut state = State {
                outcome: Some(Outcome {
                    id: 1,
                    tag: Tag::Save,
                    command: Some(Command::SaveConfig {
                        config: Default::default(),
                        expected_revision: 10,
                    }),
                    result: Ok(host(returned_revision)),
                }),
                ..Default::default()
            };
            state
                .submit(
                    Some(Command::SaveConfig {
                        config: Default::default(),
                        expected_revision: 10,
                    }),
                    Tag::Save,
                )
                .unwrap();
            let Some(Command::SaveConfig {
                expected_revision, ..
            }) = state.take().unwrap().command
            else {
                panic!("save expected");
            };
            assert_eq!(
                expected_revision,
                if returned_revision == 11 { 11 } else { 10 }
            );
        }
        let mut state = State {
            outcome: Some(Outcome {
                id: 0,
                tag: Tag::Poll,
                command: Some(Command::Snapshot),
                result: Ok(host(11)),
            }),
            ..Default::default()
        };
        state
            .submit(
                Some(Command::SaveConfig {
                    config: Default::default(),
                    expected_revision: 10,
                }),
                Tag::Save,
            )
            .unwrap();
        assert!(matches!(
            state.take().unwrap().command,
            Some(Command::SaveConfig {
                expected_revision: 10,
                ..
            })
        ));
    }
    #[test]
    fn repeated_explicit_action_keeps_latest_order_after_different_action() {
        let mut state = State::default();
        let action = |id: &str| Some(Command::SelectOutput { id: id.into() });
        let first = state.submit(action("a"), Tag::Other).unwrap();
        state.inflight = state.take();
        state.submit(action("b"), Tag::Other).unwrap();
        let last = state.submit(action("a"), Tag::Other).unwrap();
        assert_ne!(first, last);
        assert!(
            matches!(state.take().unwrap().command, Some(Command::SelectOutput { id }) if id == "b")
        );
        assert!(
            matches!(state.take().unwrap().command, Some(Command::SelectOutput { id }) if id == "a")
        );
    }
    #[test]
    fn scalar_and_db_writes_share_one_latest_endpoint_level() {
        let mut state = State::default();
        state
            .submit(
                Some(Command::SetVolume {
                    endpoint_id: "one".into(),
                    volume: 0.4,
                }),
                Tag::Other,
            )
            .unwrap();
        state
            .submit(
                Some(Command::SetVolumeDb {
                    endpoint_id: "one".into(),
                    level_db: -9.0,
                }),
                Tag::Other,
            )
            .unwrap();
        assert_eq!(state.controls.len(), 1);
        assert!(matches!(
            state.take().unwrap().command,
            Some(Command::SetVolumeDb { level_db: -9.0, .. })
        ));
    }

    #[test]
    fn old_percent_slider_rounds_and_marks_changed_without_input() {
        let context = egui::Context::default();
        let mut changed = false;
        let mut value = 63.000004_f32;
        let mut output = context.run_ui(egui::RawInput::default(), |ui| {
            changed = ui
                .add(egui::Slider::new(&mut value, 0.0..=100.0).fixed_decimals(0))
                .changed();
        });
        output.textures_delta.clear();
        assert!(
            changed,
            "the previous UI sent this widget change as a volume write"
        );
        assert_eq!(value, 63.0);
    }
    #[test]
    fn continuous_saves_yield_to_controls_and_latest_intent_can_match_inflight() {
        let mut state = State::default();
        let volume = |volume| {
            Some(Command::SetVolume {
                endpoint_id: "one".into(),
                volume,
            })
        };
        state.submit(volume(0.2), Tag::Other).unwrap();
        state.inflight = state.take();
        state.submit(volume(0.8), Tag::Other).unwrap();
        state.submit(volume(0.2), Tag::Other).unwrap();
        assert!(matches!(
            state.controls.values().next().unwrap().command,
            Some(Command::SetVolume { volume: 0.2, .. })
        ));
        for _ in 0..2 {
            state
                .submit(
                    Some(Command::SaveConfig {
                        config: Default::default(),
                        expected_revision: 4,
                    }),
                    Tag::Save,
                )
                .unwrap();
            assert_eq!(state.take().unwrap().tag, Tag::Save);
        }
        state
            .submit(
                Some(Command::SaveConfig {
                    config: Default::default(),
                    expected_revision: 4,
                }),
                Tag::Save,
            )
            .unwrap();
        assert!(matches!(
            state.take().unwrap().command,
            Some(Command::SetVolume { .. })
        ));
        assert!(state.save.is_some());
    }
    #[test]
    fn thousand_controls_keep_latest_and_save_has_priority() {
        let mut state = State::default();
        for step in 0..1000 {
            state
                .submit(
                    Some(Command::SetVolume {
                        endpoint_id: "one".into(),
                        volume: step as f32 / 1000.0,
                    }),
                    Tag::Other,
                )
                .unwrap();
        }
        assert_eq!(state.controls.len(), 1);
        let save = Command::SaveConfig {
            config: Default::default(),
            expected_revision: 4,
        };
        let id = state.submit(Some(save.clone()), Tag::Save).unwrap();
        assert_eq!(state.submit(Some(save), Tag::Save).unwrap(), id);
        assert_eq!(state.take().unwrap().id, id);
        assert!(
            matches!(state.take().unwrap().command, Some(Command::SetVolume { volume, .. }) if volume == 0.999)
        );
        assert!(state.take().is_none());
    }
    #[test]
    fn blocking_backend_accepts_burst_and_bounds_poll_results() {
        use std::sync::mpsc;
        let (entered, calls) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        let client = Client::with_backend(
            egui::Context::default(),
            Duration::from_millis(5),
            move |command| {
                entered.send(command).unwrap();
                blocked.recv().unwrap();
                Err("visible test failure".into())
            },
        );
        client.start().unwrap();
        assert!(
            calls
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .is_none()
        );
        for step in 0..1000 {
            client
                .submit(
                    Command::SetVolume {
                        endpoint_id: "one".into(),
                        volume: step as f32 / 1000.0,
                    },
                    Tag::Other,
                )
                .unwrap();
        }
        client
            .submit(
                Command::SaveConfig {
                    config: Default::default(),
                    expected_revision: 4,
                },
                Tag::Save,
            )
            .unwrap();
        release.send(()).unwrap();
        let drain_one = || {
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            loop {
                let outcomes = client.drain();
                if !outcomes.is_empty() {
                    return outcomes;
                }
                assert!(std::time::Instant::now() < deadline);
                std::thread::yield_now();
            }
        };
        assert!(drain_one()[0].result.is_err());
        assert!(matches!(
            calls.recv_timeout(Duration::from_secs(2)).unwrap(),
            Some(Command::SaveConfig {
                expected_revision: 4,
                ..
            })
        ));
        release.send(()).unwrap();
        drain_one();
        assert!(
            matches!(calls.recv_timeout(Duration::from_secs(2)).unwrap(), Some(Command::SetVolume { volume, .. }) if volume == 0.999)
        );
        release.send(()).unwrap();
        drain_one();
        assert!(matches!(
            calls.recv_timeout(Duration::from_secs(2)).unwrap(),
            Some(Command::Snapshot)
        ));
        release.send(()).unwrap();
        assert!(calls.recv_timeout(Duration::from_millis(30)).is_err());
        assert_eq!(
            client
                .shared
                .0
                .lock()
                .unwrap()
                .outcome
                .as_ref()
                .unwrap()
                .tag,
            Tag::Poll
        );
        assert!(!client.has_pending());
    }
}
