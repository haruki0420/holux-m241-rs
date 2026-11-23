//! The Tauri application: a thin Rust shell over the `m241-core` toolkit.
//!
//! P0 wires the Connect screen ([`commands::list_ports`] and
//! [`commands::device_info`]); P2 adds the Log screen, which reads a saved
//! document instead of a port ([`commands::open_log`], [`commands::log_points`],
//! [`commands::load_photos`] and [`commands::export_log`]). The screens that
//! patch firmware or change settings arrive in later phases; the egui
//! application in `crates/m241-gui` stays in the tree until this one reaches
//! parity, so nothing here replaces it yet.
//!
//! Every Log-screen command takes an absolute path and never opens a serial
//! port. The parsed document is kept in [`logdoc::AppState`] so the summary can
//! be small and the point slices can be fetched on demand.
//!
//! # Why the port work is here and not in JavaScript
//!
//! A webview cannot open a serial port, and Tauri's capability set does not
//! grant one. Every byte that reaches the unit therefore passes through
//! `m241-core`, which is the crate that owns the protocol and the
//! `set_control_lines` safety rule. The frontend can only name a port.

mod commands;
mod device;
mod logdoc;

/// Start the window.
///
/// The `mobile_entry_point` attribute is what Tauri requires for the iOS and
/// Android entry points; on desktop it expands to nothing.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(logdoc::AppState::default())
        .invoke_handler(tauri::generate_handler![
            commands::list_ports,
            commands::device_info,
            commands::open_log,
            commands::open_sample,
            commands::log_points,
            commands::load_photos,
            commands::export_log
        ])
        .run(tauri::generate_context!())
        .unwrap_or_else(|error| {
            // The window could not be created (no display, a broken webview
            // dependency). There is no UI to report this in, so write one line
            // to stderr and exit non-zero instead of panicking with a backtrace.
            eprintln!("m241-app: the window could not be created: {error}");
            std::process::exit(1);
        });
}
