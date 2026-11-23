//! Binary entry point. All of the application lives in the library so tests can
//! link it without starting a window.

// Release builds must not open a console window on Windows; debug builds keep it
// so a startup failure is visible.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    m241_app::run();
}
