//! Build script required by Tauri.
//!
//! `tauri_build::build` validates `tauri.conf.json`, resolves the capability
//! files under `capabilities/` and emits the `cargo:rerun-if-changed` lines for
//! them. It reads the configuration from this crate's directory, so the crate
//! root is the Tauri project root (there is no `src-tauri/` level here: this
//! crate is a normal workspace member).

fn main() {
    tauri_build::build();
}
