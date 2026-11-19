use std::{error::Error, path::PathBuf, time::Duration};

use slint::{Timer, TimerMode};

#[cfg(target_os = "linux")]
mod content;
#[cfg(target_os = "linux")]
mod feed;
#[cfg(target_os = "linux")]
mod protocol;
#[cfg(target_os = "linux")]
mod power;
#[cfg(target_os = "linux")]
mod schedule;
#[cfg(target_os = "linux")]
mod settings;

slint::include_modules!();

fn main() -> Result<(), Box<dyn Error>> {
    #[cfg(target_os = "linux")]
    let backend = std::rc::Rc::new(slint_backend_kindle::install(include_bytes!(
        "../assets/AtkinsonHyperlegible-Regular.ttf"
    ))?);

    let window = Dashboard::new()?;
    #[cfg(target_os = "linux")]
    {
        backend.set_black_and_white(true);
        backend.register_font_from_memory(include_bytes!("../assets/NotoSansSC-Dashboard.ttf"))?;
    }

    let exit_window = window.as_weak();
    window.on_exit_requested(move || {
        let exit_window = exit_window.clone();
        // Give the event loop a chance to draw the latched button on e-ink.
        Timer::single_shot(Duration::from_millis(500), move || {
            if let Err(error) = slint::quit_event_loop() {
                eprintln!("Could not quit: {error}");
                if let Some(window) = exit_window.upgrade() {
                    window.set_exiting(false);
                }
            }
        });
    });

    // A second timer tick lets the first event-loop render run before the
    // launcher sees the marker. Failure to create it triggers launcher rollback.
    let heartbeat = Timer::default();
    if let Some(path) = std::env::var_os("DASHBOARD_READY").map(PathBuf::from) {
        Timer::single_shot(Duration::from_millis(100), move || {
            Timer::single_shot(Duration::from_millis(100), move || {
                if let Err(error) = std::fs::write(path, b"ready\n") {
                    eprintln!("Could not mark the UI ready: {error}");
                }
            });
        });
    }

    if let Some(path) = std::env::var_os("DASHBOARD_HEARTBEAT").map(PathBuf::from) {
        heartbeat.start(TimerMode::Repeated, Duration::from_secs(5), move || {
            if let Err(error) = std::fs::write(&path, b"alive\n") {
                eprintln!("Could not write heartbeat: {error}");
            }
        });
    }

    #[cfg(target_os = "linux")]
    let _power_timer = power::start(&window, backend);

    #[cfg(target_os = "linux")]
    let _settings_timer = settings::start(&window);

    #[cfg(target_os = "linux")]
    let _content_timer = content::start(&window);

    window.run()?;
    Ok(())
}
