//! The Tauri application: a thin Rust shell over `m241-core` and `m241-flash`.
//!
//! Wires the Connect, Log/Geotag and Firmware screens through IPC commands in
//! [`commands`]. Every serial port open is centralised in [`device`], and
//! every firmware write or DA probe goes through [`m241_flash::flow`] after
//! checking the operator's typed confirmation word.

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
        .plugin(tauri_plugin_dialog::init())
        .manage(logdoc::AppState::default())
        .invoke_handler(tauri::generate_handler![
            commands::list_ports,
            commands::device_info,
            commands::download_log,
            commands::open_log,
            commands::open_sample,
            commands::log_points,
            commands::load_photos,
            commands::export_log,
            commands::geotag_preview,
            commands::geotag_apply,
            commands::fw_dump,
            commands::fw_patch,
            commands::fw_inspect,
            commands::fw_probe_da,
            commands::fw_flash
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
