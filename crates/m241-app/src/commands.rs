//! The two IPC commands the P0 Connect screen calls.
//!
//! The contract is intentionally small: the frontend can list ports and can ask
//! the Rust side to open one and read the identity. There is no command that
//! writes, erases or changes a device setting, and `m241-flash` is not a
//! dependency of this crate.
//!
//! Errors cross the IPC boundary as one string whose **second line is the next
//! action**, in Japanese. Tauri commands can return a richer error type, but a
//! string keeps the contract in the task's shape (`Result<DeviceInfo, String>`)
//! and lets the screen render the message and the recovery step in two places:
//! the first line is what went wrong, the second is what to do about it.

use m241_core::Error as CoreError;
use tauri::State;

use crate::device;
use crate::logdoc::{self, AppState, ExportOutcome, LoadedLog, LogSummary, PhotoLoad, PointDto};

/// One serial port as the picker displays it.
pub use crate::device::PortInfo;

/// The identity and log usage read from one unit.
pub use crate::device::DeviceInfo;

/// List the serial ports the OS knows about.
#[tauri::command]
#[must_use]
pub fn list_ports() -> Vec<PortInfo> {
    device::list_ports()
}

/// Open `port` at the NMEA baud and read the unit's identity and log usage.
///
/// The blocking serial work runs on Tauri's blocking pool so a five-second reply
/// timeout cannot freeze the webview's thread.
///
/// # Errors
///
/// A Japanese two-line string: the device's or OS's own message first, the next
/// action second.
#[tauri::command]
pub async fn device_info(port: String) -> Result<DeviceInfo, String> {
    let trimmed = port.trim().to_owned();
    if trimmed.is_empty() {
        return Err(describe(&CoreError::Port {
            message: String::from("no port name was given"),
        }));
    }
    tauri::async_runtime::spawn_blocking(move || device::probe(&trimmed))
        .await
        .map_err(|error| {
            format!(
                "端末情報の読み取り中に内部エラーが発生しました（{error}）\n\
                 対処: 少し待ってから「端末情報を読む」を再実行してください。"
            )
        })?
        .map_err(|error| describe(&error))
}

/// Render a core error as the two-line message the Connect screen shows.
///
/// The variants are matched one by one, with no catch-all: a new
/// [`m241_core::Error`] variant must be given a hint here rather than inherit a
/// vague default.
#[must_use]
pub fn describe(error: &CoreError) -> String {
    let hint = match error {
        CoreError::Port { .. } => {
            "USB ケーブルが挿さっているか、端末の電源が入っているかを確認してください。\
             Linux ではポートを開く権限（dialout / uucp グループなど）も必要です。"
        }
        CoreError::Timeout { .. } => {
            "端末の電源が入っているか、他のアプリが同じポートを開いていないかを確認し、\
             数秒待ってから再試行してください。"
        }
        CoreError::Framing { .. } => {
            "USB ケーブルを短いものに替えるか、別の USB ポートで試してください。\
             それでも続く場合は端末を再起動してください。"
        }
        CoreError::Rejected { .. } => "端末を再起動し、ポートを選び直してから再試行してください。",
        CoreError::Safety { .. } => {
            "この画面は読み取りのみです。表示された理由を確認してください。"
        }
        CoreError::Firmware { .. } | CoreError::Profile { .. } | CoreError::Log { .. } => {
            "端末の応答が想定と異なります。端末を再起動してから再試行してください。"
        }
        CoreError::Nmea { .. } => {
            "端末の応答を解釈できませんでした。数秒待ってから再試行してください。"
        }
        CoreError::State(_) => {
            "この操作はいまの状態では実行できません。端末を接続し直してから再試行してください。"
        }
        CoreError::Io { .. } => "書き込み先の権限と空き容量を確認してください。",
    };
    format!("{error}\n対処: {hint}")
}

/// Open a log document and return its summary.
///
/// `path` is an absolute path ending in `.bin`, `.gpx`, `.kml` or `.kmz`. The
/// parsed track stays in [`AppState`]; the point list is fetched separately with
/// [`log_points`] so this reply stays small for a large dump.
///
/// # Errors
///
/// The Japanese two-line message from [`logdoc::read`].
#[tauri::command]
pub fn open_log(state: State<'_, AppState>, path: String) -> Result<LogSummary, String> {
    open(&state, logdoc::read(&path)?)
}

/// Load the built-in sample track.
///
/// The sample is the same shape as an opened document, with `source` set to
/// `sample`, so every other command on this screen works against it unchanged.
///
/// # Errors
///
/// Only an internal-state failure; the sample itself cannot fail to load.
#[tauri::command]
pub fn open_sample(state: State<'_, AppState>) -> Result<LogSummary, String> {
    open(&state, logdoc::sample_log())
}

fn open(state: &State<'_, AppState>, loaded: LoadedLog) -> Result<LogSummary, String> {
    let summary = logdoc::summary(&loaded);
    let mut guard = state.log.lock().map_err(|_| internal("log"))?;
    *guard = Some(loaded);
    Ok(summary)
}

/// Return the `[from, to)` slice of the loaded track.
///
/// `path` must be the `source` the summary reported; it is compared rather than
/// trusted so a point list cannot be read out of a document the screen did not
/// open.
///
/// # Errors
///
/// A Japanese two-line message when no document is loaded, the source differs,
/// or the range is invalid.
#[tauri::command]
pub fn log_points(
    state: State<'_, AppState>,
    path: String,
    from: usize,
    to: usize,
) -> Result<Vec<PointDto>, String> {
    with_log(&state, &path, |loaded| {
        logdoc::point_slice(loaded, from, to)
    })
}

/// Read a folder of photos and match them to the loaded track.
///
/// The three numeric arguments are the §6.1 clock and grouping settings: the
/// camera's offset from UTC in minutes, the track's whole-day offset (the same
/// meaning as `--utc-offset-days`) and the grouping distance in metres
/// (`0` disables grouping).
///
/// # Errors
///
/// A Japanese two-line message from [`logdoc::load_photos`], or when no document
/// is loaded or the source differs.
#[tauri::command]
pub fn load_photos(
    state: State<'_, AppState>,
    path: String,
    directory: String,
    camera_offset_minutes: i32,
    log_offset_days: i32,
    group_distance_meters: f64,
) -> Result<PhotoLoad, String> {
    let mut guard = state.log.lock().map_err(|_| internal("log"))?;
    let loaded = guard.as_mut().ok_or_else(no_document)?;
    if loaded.source != path {
        return Err(source_changed());
    }
    let options = logdoc::PhotoOptions {
        camera_offset_minutes,
        log_offset_days,
        group_distance_meters,
        ..logdoc::PhotoOptions::default()
    };
    logdoc::load_photos(loaded, &directory, &options)
}

/// Write the loaded track to `destination` in `format`.
///
/// `format` is one of GPX / KML / CSV / NMEA / JSON / KMZ / HTML / RTF.
/// `destination` is an absolute path; the frontend proposes the source path with
/// the new extension and lets the operator change it.
///
/// # Errors
///
/// A Japanese two-line message from [`logdoc::export`], or when no document is
/// loaded or the source differs.
#[tauri::command]
pub fn export_log(
    state: State<'_, AppState>,
    path: String,
    format: String,
    destination: String,
) -> Result<ExportOutcome, String> {
    with_log(&state, &path, |loaded| {
        logdoc::export(loaded, &format, &destination)
    })
}

/// Run `action` against the loaded document named by `source`.
fn with_log<T>(
    state: &State<'_, AppState>,
    source: &str,
    action: impl FnOnce(&LoadedLog) -> Result<T, String>,
) -> Result<T, String> {
    let guard = state.log.lock().map_err(|_| internal("log"))?;
    let loaded = guard.as_ref().ok_or_else(no_document)?;
    if loaded.source != source {
        return Err(source_changed());
    }
    action(loaded)
}

fn no_document() -> String {
    String::from("ログが読み込まれていません\n対処: ログを開くか、見本で試してください。")
}

fn source_changed() -> String {
    String::from("表示中のログと読み込み先が一致しません\n対処: ログを開き直してください。")
}

fn internal(what: &str) -> String {
    format!("ログ画面の内部状態を読めませんでした（{what}）\n対処: 画面を開き直してください。")
}

#[cfg(test)]
mod tests {
    use super::*;
    use m241_core::Error;

    #[test]
    fn a_port_error_keeps_the_os_wording_and_adds_the_next_action() {
        let text = describe(&Error::Port {
            message: String::from("No such file or directory"),
        });
        let mut lines = text.lines();
        assert_eq!(lines.next(), Some("serial port: No such file or directory"));
        let hint = lines.next().unwrap_or_default();
        assert!(hint.starts_with("対処: "), "got {hint}");
        assert!(hint.contains("USB ケーブル"), "got {hint}");
    }

    #[test]
    fn a_timeout_says_to_check_the_power_and_the_other_application() {
        let text = describe(&Error::Timeout {
            what: String::from("$PMTK705 identity reply"),
            timeout: std::time::Duration::from_secs(5).into(),
        });
        assert!(text.contains("$PMTK705"), "got {text}");
        assert!(text.contains("他のアプリ"), "got {text}");
    }

    #[test]
    fn every_error_variant_has_exactly_one_next_action_line() {
        let errors = [
            Error::Port {
                message: String::from("busy"),
            },
            Error::Rejected {
                what: String::from("$PMTK001"),
            },
            Error::State(String::from("not connected")),
        ];
        for error in errors {
            let text = describe(&error);
            assert_eq!(text.lines().count(), 2, "got {text}");
            assert!(text.contains("\n対処: "), "got {text}");
        }
    }
}
