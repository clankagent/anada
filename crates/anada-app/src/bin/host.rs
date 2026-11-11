#![windows_subsystem = "windows"]

fn main() {
    let result = anada_app::controller::Controller::new().and_then(|mut controller| {
        // The first callback runs only after the user-scoped pipe has been acquired.
        // A competing host therefore never owns a second tray icon.
        let mut tray = None;
        let mut tray_attempted = false;
        anada_app::transport::serve(|command| {
            if !tray_attempted {
                tray_attempted = true;
                match anada_app::tray::Tray::start() {
                    Ok(value) => tray = Some(value),
                    Err(error) => eprintln!("Anada tray: {error}"),
                }
            }
            controller.handle(command)
        })
    });
    if let Err(error) = result {
        eprintln!("Anada host: {error}");
        std::process::exit(1);
    }
}
