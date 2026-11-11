#![cfg(windows)]

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

fn command(directory: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_anada"));
    command.env("ANADA_CONFIG_DIR", directory);
    command
}

#[test]
#[ignore = "Starts an isolated host and reads actual Windows audio inventory"]
fn cold_start_closes_caller_pipes_and_reuses_host() {
    let directory = std::env::temp_dir().join(format!("anada-lifecycle-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let child = command(&directory)
        .arg("--snapshot")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let (sender, receiver) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let _ = sender.send(child.wait_with_output());
    });
    let first = receiver.recv_timeout(Duration::from_secs(12));
    if first.is_err() {
        let _ = command(&directory).arg("--stop-host").output();
        if reader.is_finished() {
            reader.join().unwrap();
        }
        panic!("Cold startup did not close caller pipes within 12 seconds");
    }
    reader.join().unwrap();
    let first = first.unwrap().unwrap();
    let second = command(&directory).arg("--snapshot").output().unwrap();
    let stopped = command(&directory).arg("--stop-host").output().unwrap();
    assert!(
        stopped.status.success(),
        "{}",
        String::from_utf8_lossy(&stopped.stderr)
    );
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    let first: anada_app::model::HostState = serde_json::from_slice(&first.stdout).unwrap();
    let second: anada_app::model::HostState = serde_json::from_slice(&second.stdout).unwrap();
    assert_eq!(first.host_pid, second.host_pid);
    assert!(!first.audio_running);
    std::fs::remove_dir_all(directory).unwrap();
}
