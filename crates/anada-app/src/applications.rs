use crate::model::HostState;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupedApplication {
    pub key: String,
    pub name: String,
    pub process_ids: Vec<u32>,
    /// Active observed endpoints, or inactive endpoints if none are active.
    pub endpoint_ids: Vec<String>,
    pub active: bool,
    pub channel_id: Option<String>,
    pub mixed: bool,
}

pub fn group_applications(state: &HostState) -> Vec<GroupedApplication> {
    let mut groups: BTreeMap<String, Vec<&anada_windows::AudioSession>> = BTreeMap::new();
    for session in &state.windows.sessions {
        if session.process_id == 0 || session.process_id == state.host_pid {
            continue;
        }
        let key = session
            .executable_path
            .as_deref()
            .filter(|path| !path.trim().is_empty())
            .map(|path| format!("exe:{}", path.replace('/', "\\").to_lowercase()))
            .unwrap_or_else(|| format!("pid:{}", session.process_id));
        groups.entry(key).or_default().push(session);
    }
    let mut result: Vec<_> = groups
        .into_iter()
        .map(|(key, sessions)| {
            let active = sessions.iter().any(|session| session.active);
            let process_ids = sessions
                .iter()
                .map(|session| session.process_id)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            let endpoint_ids: Vec<_> = sessions
                .iter()
                .filter(|session| !active || session.active)
                .map(|session| session.endpoint_id.clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            let channels: BTreeSet<_> = endpoint_ids
                .iter()
                .map(|endpoint| {
                    state
                        .config
                        .channels
                        .iter()
                        .find(|channel| channel.endpoint_id.as_ref() == Some(endpoint))
                        .map(|channel| channel.id.clone())
                })
                .collect();
            let mixed = channels.len() > 1;
            let channel_id = if mixed {
                None
            } else {
                channels.into_iter().next().flatten()
            };
            let name = sessions
                .iter()
                .filter_map(|session| session.executable_path.as_deref())
                .find_map(|path| {
                    path.rsplit(['\\', '/'])
                        .next()
                        .filter(|name| !name.is_empty())
                })
                .map(str::to_owned)
                .unwrap_or_else(|| sessions[0].name.clone());
            GroupedApplication {
                key,
                name,
                process_ids,
                endpoint_ids,
                active,
                channel_id,
                mixed,
            }
        })
        .collect();
    result.sort_by(|left, right| {
        (left.name.to_lowercase(), &left.key).cmp(&(right.name.to_lowercase(), &right.key))
    });
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Config, PROTOCOL_VERSION};
    fn state() -> HostState {
        let mut config = Config::default();
        config.channels[0].endpoint_id = Some("gaming".into());
        HostState {
            audio_diagnostics: Default::default(),
            protocol_version: PROTOCOL_VERSION,
            revision: 1,
            config,
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
            host_pid: 99,
            quiet: Default::default(),
        }
    }
    fn session(
        pid: u32,
        path: Option<&str>,
        endpoint: &str,
        active: bool,
    ) -> anada_windows::AudioSession {
        anada_windows::AudioSession {
            executable_path: path.map(str::to_owned),
            process_id: pid,
            name: "Same name".into(),
            endpoint_id: endpoint.into(),
            active,
        }
    }
    #[test]
    fn groups_executable_processes_and_sessions_without_losing_routing_pids() {
        let mut state = state();
        state.windows.sessions = vec![
            session(1, Some("C:/Browser/App.exe"), "gaming", true),
            session(1, Some("c:\\browser\\app.exe"), "gaming", true),
            session(2, Some("C:/Browser/App.exe"), "old", false),
            session(3, Some("C:/Different/App.exe"), "gaming", true),
            session(0, None, "gaming", true),
            session(99, None, "gaming", true),
        ];
        let apps = group_applications(&state);
        assert_eq!(apps.len(), 2);
        let app = apps
            .iter()
            .find(|app| app.process_ids.contains(&1))
            .unwrap();
        assert_eq!(app.process_ids, [1, 2]);
        assert_eq!(app.endpoint_ids, ["gaming"]);
        assert_eq!(app.channel_id.as_deref(), Some("gaming"));
    }
    #[test]
    fn unknown_identity_never_merges_names_and_active_mixed_routes_are_explicit() {
        let mut state = state();
        state.windows.sessions = vec![
            session(1, None, "gaming", true),
            session(2, None, "gaming", true),
            session(3, Some("app.exe"), "gaming", true),
            session(4, Some("app.exe"), "physical", true),
        ];
        let apps = group_applications(&state);
        assert_eq!(apps.len(), 3);
        let mixed = apps
            .iter()
            .find(|app| app.process_ids.contains(&3))
            .unwrap();
        assert!(mixed.mixed);
        assert_eq!(mixed.channel_id, None);
    }
}
