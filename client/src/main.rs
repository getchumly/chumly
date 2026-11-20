// Do not open a console window next to the GUI in Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod audio;
mod config;
mod crash;
mod hotkeys;
mod i18n;
mod icons;
mod identity;
mod net;
mod ptt;
mod screen;
mod theme;
mod timefmt;
mod updates;

/// Tests that emulate key presses through the system (F13) must not run at
/// the same time: each would see the other's presses. Those tests exist
/// only on Windows, as does system input emulation.
#[cfg(all(test, windows))]
static SYSTEM_INPUT: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(all(test, windows))]
fn lock_system_input() -> std::sync::MutexGuard<'static, ()> {
    SYSTEM_INPUT.lock().unwrap_or_else(|e| e.into_inner())
}

/// CI runners have no interactive desktop: emulated input does not arrive
/// and input hooks may not install. Tests of real system input skip there.
#[cfg(all(test, windows))]
fn skip_system_input_on_ci() -> bool {
    let ci = std::env::var_os("CI").is_some();
    if ci {
        eprintln!("skipped on CI: needs an interactive desktop");
    }
    ci
}

use std::path::PathBuf;

use eframe::egui;
use tracing_subscriber::EnvFilter;

const USAGE: &str = "\
Chumly client

Usage: chumly [--config <path to client.toml>]

By default settings are stored in the user's config directory.
Portable mode: put a client.toml (even an empty one) next to the
executable and settings will be kept there instead.
A custom path is handy for running two clients on one machine.";

/// Icon for the application windows.
pub fn window_icon() -> Option<egui::IconData> {
    eframe::icon_data::from_png_bytes(include_bytes!("../assets/png/chumly-icon-256.png"))
        .inspect_err(|e| tracing::warn!("failed to load window icon: {e}"))
        .ok()
}

fn main() -> eframe::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,wgpu=warn,eframe=warn")),
        )
        .with_target(false)
        .init();

    let mut config_path: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-c" | "--config" => config_path = args.next().map(PathBuf::from),
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            other => {
                eprintln!("unknown argument '{other}'\n\n{USAGE}");
                std::process::exit(2);
            }
        }
    }
    let config_path =
        config_path.or_else(config::ClientConfig::default_path).unwrap_or_else(|| PathBuf::from("client.toml"));
    tracing::info!("settings: {}", config_path.display());
    crash::install(crash::dir_for(&config_path));

    let mut viewport = egui::ViewportBuilder::default()
        .with_title(shared::APP_NAME)
        .with_inner_size([1000.0, 660.0])
        .with_min_inner_size([720.0, 420.0]);
    if let Some(icon) = window_icon() {
        viewport = viewport.with_icon(icon);
    }
    let saved = config::ClientConfig::load(&config_path);
    viewport = viewport.with_decorations(saved.system_frame);
    // Restore the window position and size from the previous session.
    if let Some(g) = saved.window.and_then(|g| g.sane()) {
        viewport = viewport.with_inner_size([g.width, g.height]).with_position([g.x, g.y]).with_maximized(g.maximized);
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native(shared::APP_NAME, options, Box::new(move |cc| Ok(Box::new(app::App::new(cc, config_path)))))
}
