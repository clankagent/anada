//! Development cleanup only. Stop the test host before restoring a saved --probe file.
use anada_windows::{AudioSystem, WindowsSnapshot};
fn main() {
    if let Err(error) = run() {
        eprintln!("restore_levels: {error}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), String> {
    let path = std::env::args()
        .nth(1)
        .ok_or("Pass the original --probe JSON file")?;
    let snapshot: WindowsSnapshot =
        serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let system = AudioSystem::new()?;
    let mut restored = 0;
    for original in &snapshot.outputs {
        let observed = system.endpoint_state(&original.id)?;
        if let Some(db) = &original.level_db {
            if observed
                .level_db
                .as_ref()
                .is_none_or(|current| (current.current - db.current).abs() > 0.00001)
            {
                system.set_endpoint_volume_db(&original.id, db.current)?;
            }
            let after = system.endpoint_state(&original.id)?;
            if after
                .level_db
                .as_ref()
                .is_none_or(|current| (current.current - db.current).abs() > 0.01)
            {
                return Err(format!("Exact dB restore failed for {}", original.name));
            }
        } else {
            if (observed.volume - original.volume).abs() > 0.00001 {
                system.set_endpoint_volume(&original.id, original.volume)?;
            }
            let after = system.endpoint_state(&original.id)?;
            if (after.volume - original.volume).abs() > 0.001 {
                return Err(format!("Scalar restore failed for {}", original.name));
            }
        }
        restored += 1;
    }
    println!("Restored and verified {restored} endpoint levels; no audio stream opened.");
    Ok(())
}
