#![windows_subsystem = "windows"]

fn main() {
    if let Err(error) = run() {
        eprintln!("Anada: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    match std::env::args().nth(1).as_deref() {
        Some("--mcp") => anada_app::mcp_stdio::run(),
        Some("--ui-demo") => anada_app::ui::run_demo().map_err(|error| error.to_string()),
        Some("--probe") => {
            let windows = anada_windows::AudioSystem::new()?.snapshot()?;
            println!(
                "{}",
                serde_json::to_string_pretty(&windows).map_err(|error| error.to_string())?
            );
            Ok(())
        }
        Some("--snapshot") => {
            anada_app::transport::ensure_host()?;
            let state = anada_app::transport::request(anada_app::model::Command::Snapshot)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&state).map_err(|error| error.to_string())?
            );
            Ok(())
        }
        Some("--stop-host") => {
            anada_app::transport::request(anada_app::model::Command::StopHost)?;
            Ok(())
        }
        Some("--command-json") => {
            use std::io::Read;
            let mut input = String::new();
            std::io::stdin()
                .take(1_048_577)
                .read_to_string(&mut input)
                .map_err(|error| error.to_string())?;
            if input.len() > 1_048_576 {
                return Err("Command exceeds the 1 MiB limit".into());
            }
            let command: anada_app::model::Command =
                serde_json::from_str(&input).map_err(|error| error.to_string())?;
            let state = anada_app::transport::request(command)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&state).map_err(|error| error.to_string())?
            );
            Ok(())
        }
        Some("--help") => {
            println!(
                "Anada\n  (no arguments)  Open the desktop interface\n  --mcp           Serve permissioned MCP over stdin/stdout (does not start audio)\n  --probe         Read Windows devices and sessions without starting audio\n  --snapshot      Read current Anada host state\n  --command-json  Read one host command as JSON from stdin\n  --stop-host     Stop the current user's audio host\nSettings: %LOCALAPPDATA%/Anada. QA isolation: ANADA_CONFIG_DIR."
            );
            Ok(())
        }
        Some(argument) => Err(format!("Unknown argument: {argument}")),
        None => anada_app::ui::run().map_err(|error| error.to_string()),
    }
}
