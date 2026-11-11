//! Development-only broker fixture. No Windows or audio APIs are called.
//! Run with ANADA_CONFIG_DIR set to a private temporary directory. Wait for
//! {"mcp_fixture_ready":true}, run anada.exe --mcp with the same environment,
//! then send this process a newline (or close stdin) to revoke its session.
use anada_app::{
    mcp_session::Session,
    mcp_tools::{Backend, execute_with_backend},
    mcp_types::{Permission, Preferences},
    model::{Command, Config, HostState},
    presets::EqPreset,
};
use anada_dsp::EqSettings;
use serde_json::json;
use std::{
    io::{BufRead, Write},
    sync::Mutex,
};
struct FakeBackend {
    state: HostState,
}
impl Backend for FakeBackend {
    fn request(&mut self, command: Command) -> Result<HostState, String> {
        match command {
            Command::Snapshot | Command::SnapshotMeters => {}
            Command::SaveConfig {
                config,
                expected_revision,
            } => {
                if expected_revision != self.state.revision {
                    return Err("Revision conflict".into());
                }
                config.validate()?;
                self.state.config = config;
                self.state.revision += 1;
            }
            _ => return Err("Fixture permits read and EQ configuration operations only".into()),
        }
        Ok(self.state.clone())
    }
    fn presets(&mut self) -> Result<Vec<EqPreset>, String> {
        Err("Fixture does not expose presets".into())
    }
    fn save_preset(&mut self, _: &str, _: &EqSettings, _: bool) -> Result<(), String> {
        Err("Fixture does not save presets".into())
    }
    fn delete_preset(&mut self, _: &str) -> Result<(), String> {
        Err("Fixture does not delete presets".into())
    }
    fn startup(&mut self, _: Option<bool>) -> Result<bool, String> {
        Err("Fixture does not access Windows startup".into())
    }
}
fn run() -> Result<(), String> {
    if std::env::var_os("ANADA_CONFIG_DIR").is_none() {
        return Err("Set ANADA_CONFIG_DIR to an isolated fixture directory".into());
    }
    let fake = Mutex::new(FakeBackend {
        state: HostState {
            protocol_version: 1,
            revision: 1,
            config: Config::default(),
            windows: anada_windows::WindowsSnapshot {
                outputs: vec![anada_windows::Endpoint {
                    id: "fixture-output".into(),
                    name: "Fixture speakers".into(),
                    volume: 0.1,
                    muted: false,
                    is_virtual: false,
                    level_db: Some(anada_windows::EndpointLevelDb {
                        current: -24.0,
                        min: -60.0,
                        max: 0.0,
                        step: 1.0,
                    }),
                }],
                inputs: vec![],
                sessions: vec![],
                default_output_id: Some("fixture-output".into()),
                communications_output_id: None,
            },
            selected_output_id: Some("fixture-output".into()),
            manual_output: false,
            audio_running: false,
            audio_status: "Fixture stopped".into(),
            warnings: vec![],
            host_pid: 0,
            quiet: Default::default(),
            audio_diagnostics: Default::default(),
        },
    });
    let mut session = Session::new();
    session.start(
        Preferences {
            duration_minutes: 15,
            granted: [
                Permission::ReadStatus,
                Permission::ReadDevices,
                Permission::ReadEq,
                Permission::ReadLevels,
                Permission::ReadSpectrum,
                Permission::WriteChannelEq,
            ]
            .into_iter()
            .collect(),
        },
        move |name, args, grants| {
            execute_with_backend(
                name,
                args,
                grants,
                &mut *fake.lock().map_err(|_| "Fixture state unavailable")?,
            )
        },
    )?;
    println!("{}", json!({"mcp_fixture_ready":true}));
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    drop(session);
    Ok(())
}
fn main() {
    if let Err(error) = run() {
        eprintln!("MCP fixture: {error}");
        std::process::exit(1);
    }
}
