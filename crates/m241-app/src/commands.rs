//! The IPC commands exposed to the Tauri frontend.
//!
//! Every command that touches the serial port, filesystem or images runs on
//! Tauri's blocking pool (`spawn_blocking`), not on the webview thread.
//!
//! Errors cross the IPC boundary as one string whose **second line is the next
//! action**, in Japanese.

use std::sync::{Arc, Mutex};

use m241_core::Error as CoreError;
use tauri::State;

use crate::device::{
    self, FwDumpOutcome, FwFlashOutcome, FwInspectDto, FwPatchOutcome, FwProbeDaOutcome,
};
use crate::logdoc::{
    self, AppState, ExportOutcome, GeotagApplyOutcome, GeotagOptionsDto, GeotagPreviewDto,
    LoadedLog, LogSummary, PhotoLoad, PhotoOptionsDto, PointDto,
};

/// One serial port as the picker displays it.
pub use crate::device::PortInfo;

/// The identity and log usage read from one unit.
pub use crate::device::DeviceInfo;

/// Outcome returned by `download_log`.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadLogOutcome {
    /// Absolute path of the saved `.bin` dump.
    pub destination: String,
    /// Bytes downloaded and written.
    pub bytes: u64,
    /// Parsed log summary loaded into [`AppState`].
    pub summary: LogSummary,
}

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
            "安全チェックにより処理が停止されました。表示された理由と入力内容・選択ファイルを確認してください。"
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

/// Download the track log from `port`, save it to `destination`, and load it
/// into [`AppState`].
///
/// # Errors
///
/// A Japanese two-line string on port, transfer, write, or parse failure.
#[tauri::command]
pub async fn download_log(
    state: State<'_, AppState>,
    port: String,
    destination: String,
) -> Result<DownloadLogOutcome, String> {
    let trimmed_port = port.trim().to_owned();
    if trimmed_port.is_empty() {
        return Err(describe(&CoreError::Port {
            message: String::from("no port name was given"),
        }));
    }
    let dest_path =
        device::prepare_output_path(&destination, "destination").map_err(|err| describe(&err))?;

    let (byte_len, loaded) = tauri::async_runtime::spawn_blocking(move || {
        let bytes = device::download_log_bytes(&trimmed_port).map_err(|err| describe(&err))?;
        if bytes.is_empty() {
            return Err(String::from(
                "本体に記録されたログがありません（0 バイト）\n\
                 対処: ログ記録が有効な状態で移動してから再度お試しください。",
            ));
        }
        std::fs::write(&dest_path, &bytes).map_err(|error| {
            format!(
                "{} に書き込めません（{error}）\n対処: 保存先の権限と空き容量を確認してください。",
                dest_path.display()
            )
        })?;
        let loaded = logdoc::read(&dest_path.display().to_string())?;
        Ok((bytes.len(), loaded))
    })
    .await
    .map_err(|error| blocking_failed("ログの吸い出し", error))??;

    let saved_path = loaded.source.clone();
    let summary = open(&state, loaded)?;
    Ok(DownloadLogOutcome {
        destination: saved_path,
        bytes: u64::try_from(byte_len).unwrap_or(u64::MAX),
        summary,
    })
}

/// Open a log document and return its summary.
///
/// `path` is an absolute path ending in `.bin`, `.gpx`, `.kml` or `.kmz`. The
/// parsed track stays in [`AppState`]; the point list is fetched separately with
/// [`log_points`] so this reply stays small for a large dump.
///
/// Reading and parsing a 16 MiB dump runs on the blocking pool; only the store
/// of the result touches the state lock.
///
/// # Errors
///
/// The Japanese two-line message from [`logdoc::read`].
#[tauri::command]
pub async fn open_log(state: State<'_, AppState>, path: String) -> Result<LogSummary, String> {
    let loaded = tauri::async_runtime::spawn_blocking(move || logdoc::read(&path))
        .await
        .map_err(|error| blocking_failed("ログの読み込み", error))??;
    open(&state, loaded)
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
/// When `path` is supplied it must match the loaded document's `source`; when
/// omitted, the currently loaded document is read.
///
/// # Errors
///
/// A Japanese two-line message when no document is loaded, the source differs,
/// or the range is invalid.
#[tauri::command]
pub fn log_points(
    state: State<'_, AppState>,
    path: Option<String>,
    from: usize,
    to: usize,
) -> Result<Vec<PointDto>, String> {
    with_log(&state, path.as_deref(), |loaded| {
        logdoc::point_slice(loaded, from, to)
    })
}

/// Read a folder of photos and match them to the loaded track.
///
/// Accepts either `{ directory, options }` or the flat option arguments.
///
/// # Errors
///
/// A Japanese two-line message from [`logdoc::load_photos`], or when no document
/// is loaded or the source differs.
#[tauri::command]
pub async fn load_photos(
    state: State<'_, AppState>,
    path: Option<String>,
    directory: String,
    options: Option<PhotoOptionsDto>,
    camera_offset_minutes: Option<i32>,
    log_offset_days: Option<i32>,
    group_distance_meters: Option<f64>,
) -> Result<PhotoLoad, String> {
    let log = Arc::clone(&state.log);
    let resolved_options = match options {
        Some(dto) => logdoc::PhotoOptions::from(dto),
        None => logdoc::PhotoOptions {
            camera_offset_minutes: camera_offset_minutes.unwrap_or(0),
            log_offset_days: log_offset_days.unwrap_or(0),
            group_distance_meters: group_distance_meters.unwrap_or(0.0),
            ..logdoc::PhotoOptions::default()
        },
    };
    tauri::async_runtime::spawn_blocking(move || {
        with_log_mut(&log, path.as_deref(), |loaded| {
            logdoc::load_photos(loaded, &directory, &resolved_options)
        })
    })
    .await
    .map_err(|error| blocking_failed("写真の読み込み", error))?
}

/// Write the loaded track to `destination` in `format`.
///
/// # Errors
///
/// A Japanese two-line message from [`logdoc::export`], or when no document is
/// loaded or the source differs.
#[tauri::command]
pub async fn export_log(
    state: State<'_, AppState>,
    path: Option<String>,
    format: String,
    destination: String,
) -> Result<ExportOutcome, String> {
    let log = Arc::clone(&state.log);
    tauri::async_runtime::spawn_blocking(move || {
        with_log_shared(&log, path.as_deref(), |loaded| {
            logdoc::export(loaded, &format, &destination)
        })
    })
    .await
    .map_err(|error| blocking_failed("書き出し", error))?
}

/// Preview EXIF geotag matches for photos in `directory` against the loaded log.
///
/// # Errors
///
/// A Japanese two-line message when no log is loaded or `directory` is unreadable.
#[tauri::command]
pub async fn geotag_preview(
    state: State<'_, AppState>,
    directory: String,
    options: GeotagOptionsDto,
) -> Result<GeotagPreviewDto, String> {
    let log = Arc::clone(&state.log);
    tauri::async_runtime::spawn_blocking(move || {
        with_log_shared(&log, None, |loaded| {
            logdoc::geotag_preview(loaded, &directory, &options)
        })
    })
    .await
    .map_err(|error| blocking_failed("ジオタグのプレビュー", error))?
}

/// Apply EXIF geotags to matched photos in `directory` against the loaded log.
///
/// # Errors
///
/// A Japanese two-line message when no log is loaded or planning fails.
#[tauri::command]
pub async fn geotag_apply(
    state: State<'_, AppState>,
    directory: String,
    options: GeotagOptionsDto,
) -> Result<GeotagApplyOutcome, String> {
    let log = Arc::clone(&state.log);
    tauri::async_runtime::spawn_blocking(move || {
        with_log_shared(&log, None, |loaded| {
            logdoc::geotag_apply(loaded, &directory, &options)
        })
    })
    .await
    .map_err(|error| blocking_failed("ジオタグの書き込み", error))?
}

/// Dump firmware from `port` (read-only) to `destination`.
///
/// # Errors
///
/// A Japanese two-line message on port, BootROM, or file failure.
#[tauri::command]
pub async fn fw_dump(
    port: String,
    destination: String,
    assume_brom: bool,
) -> Result<FwDumpOutcome, String> {
    tauri::async_runtime::spawn_blocking(move || device::fw_dump(&port, &destination, assume_brom))
        .await
        .map_err(|error| blocking_failed("ファームウェア吸い出し", error))?
        .map_err(|error| describe(&error))
}

/// Apply the WNRO patch to `input_path` and write `output_path` (offline).
///
/// # Errors
///
/// A Japanese two-line message when the image cannot be patched or written.
#[tauri::command]
pub async fn fw_patch(input_path: String, output_path: String) -> Result<FwPatchOutcome, String> {
    tauri::async_runtime::spawn_blocking(move || device::fw_patch(&input_path, &output_path))
        .await
        .map_err(|error| blocking_failed("WNRO パッチ適用", error))?
        .map_err(|error| describe(&error))
}

/// Inspect a firmware image and optional backup (offline).
///
/// # Errors
///
/// A Japanese two-line message when reading the image or backup fails.
#[tauri::command]
pub async fn fw_inspect(
    image_path: String,
    backup_path: Option<String>,
) -> Result<FwInspectDto, String> {
    tauri::async_runtime::spawn_blocking(move || {
        device::fw_inspect(&image_path, backup_path.as_deref())
    })
    .await
    .map_err(|error| blocking_failed("ファームウェア検査", error))?
    .map_err(|error| describe(&error))
}

/// Load the vendor DA into RAM and probe both candidate flash addresses.
///
/// # Errors
///
/// A Japanese two-line message on confirmation refusal, bad DA, or transport failure.
#[tauri::command]
pub async fn fw_probe_da(
    port: String,
    da_path: String,
    assume_brom: bool,
    confirm_word: String,
) -> Result<FwProbeDaOutcome, String> {
    tauri::async_runtime::spawn_blocking(move || {
        device::fw_probe_da(&port, &da_path, assume_brom, &confirm_word)
    })
    .await
    .map_err(|error| blocking_failed("DA 確認", error))?
    .map_err(|error| describe(&error))
}

/// Program firmware through `m241_flash::flow::flash` and verify by read-back.
///
/// # Errors
///
/// A Japanese two-line message on confirmation/safety refusal or flash/read-back failure.
#[tauri::command]
pub async fn fw_flash(
    port: String,
    image_path: String,
    backup_path: String,
    da_path: String,
    mode: String,
    assume_brom: bool,
    confirm_word: String,
) -> Result<FwFlashOutcome, String> {
    tauri::async_runtime::spawn_blocking(move || {
        device::fw_flash(
            &port,
            &image_path,
            &backup_path,
            &da_path,
            &mode,
            assume_brom,
            &confirm_word,
        )
    })
    .await
    .map_err(|error| blocking_failed("ファームウェア書き込み", error))?
    .map_err(|error| describe(&error))
}

/// Run `action` against the loaded document (optionally verifying `source`).
fn with_log<T>(
    state: &State<'_, AppState>,
    source: Option<&str>,
    action: impl FnOnce(&LoadedLog) -> Result<T, String>,
) -> Result<T, String> {
    let guard = state.log.lock().map_err(|_| internal("log"))?;
    let loaded = guard.as_ref().ok_or_else(no_document)?;
    if let Some(expected) = source
        && loaded.source != expected
    {
        return Err(source_changed());
    }
    action(loaded)
}

/// The same check as [`with_log`] for a blocking thread holding its own state.
///
/// The `Arc` clone is what lets `spawn_blocking` take the document without
/// borrowing the managed state across the await: the lock is taken and dropped
/// inside the blocking closure.
fn with_log_shared<T>(
    log: &Arc<Mutex<Option<LoadedLog>>>,
    source: Option<&str>,
    action: impl FnOnce(&LoadedLog) -> Result<T, String>,
) -> Result<T, String> {
    let guard = log.lock().map_err(|_| internal("log"))?;
    let loaded = guard.as_ref().ok_or_else(no_document)?;
    if let Some(expected) = source
        && loaded.source != expected
    {
        return Err(source_changed());
    }
    action(loaded)
}

/// [`with_log_shared`] for a command that updates the document (photo matching
/// replaces the photo rows).
fn with_log_mut<T>(
    log: &Arc<Mutex<Option<LoadedLog>>>,
    source: Option<&str>,
    action: impl FnOnce(&mut LoadedLog) -> Result<T, String>,
) -> Result<T, String> {
    let mut guard = log.lock().map_err(|_| internal("log"))?;
    let loaded = guard.as_mut().ok_or_else(no_document)?;
    if let Some(expected) = source
        && loaded.source != expected
    {
        return Err(source_changed());
    }
    action(loaded)
}

/// The message for a blocking task that panicked or was cancelled.
fn blocking_failed(what: &str, error: impl std::fmt::Display) -> String {
    format!(
        "{what}の処理中に内部エラーが発生しました（{error}）\n\
         対処: 少し待ってから、もう一度実行してください。"
    )
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
        // Every variant, so the "two lines, the second starts with 対処:" contract
        // is checked for all of them rather than for three representatives. The
        // `describe` match has no catch-all, so adding a variant fails to
        // compile here; this test is what checks the text of the ones that exist.
        let errors = [
            Error::Port {
                message: String::from("busy"),
            },
            Error::Timeout {
                what: String::from("$PMTK705"),
                timeout: std::time::Duration::from_secs(5).into(),
            },
            Error::Framing {
                what: String::from("sync probe"),
                offset: None,
            },
            Error::Rejected {
                what: String::from("$PMTK001"),
            },
            Error::Safety {
                reason: String::from("the range was not settled"),
            },
            Error::Firmware {
                message: String::from("bad image"),
                offset: Some(0x402F4),
            },
            Error::Profile {
                message: String::from("unknown build"),
                offset: None,
            },
            Error::Log {
                message: String::from("truncated record"),
                offset: Some(0x200),
            },
            Error::Nmea {
                message: String::from("bad checksum"),
            },
            Error::State(String::from("not connected")),
            Error::Io {
                path: Some(String::from("/tmp/track.gpx")),
                source: std::io::Error::new(std::io::ErrorKind::NotFound, "missing"),
            },
        ];
        assert_eq!(errors.len(), 11, "the list must cover every Error variant");
        for error in errors {
            let text = describe(&error);
            assert_eq!(text.lines().count(), 2, "got {text}");
            assert!(text.contains("\n対処: "), "got {text}");
        }
    }

    #[test]
    fn ipc_dtos_serialize_with_camel_case_keys() {
        let dl = serde_json::to_value(DownloadLogOutcome {
            destination: String::from("/tmp/log.bin"),
            bytes: 128,
            summary: logdoc::summary(&logdoc::sample_log()),
        })
        .expect("serialize DownloadLogOutcome");
        assert!(dl.get("destination").is_some());
        assert!(dl.get("bytes").is_some());
        assert!(dl.get("summary").is_some());

        let inspect = serde_json::to_value(FwInspectDto {
            image_bytes: 406_620,
            image_sha256: String::from("abc"),
            backup_bytes: Some(406_620),
            backup_sha256: Some(String::from("def")),
            profile: Some(String::from("V1.13")),
            wnro_patched: Some(true),
            diff_count: Some(3),
            outside_patch_offsets: Some(false),
            arm_vector_ok: true,
            is_vendor_v113: false,
            is_vendor_v112: false,
            recommended_mode: device::FlashMode::Strict,
            warnings: vec![],
        })
        .expect("serialize FwInspectDto");
        for key in [
            "imageBytes",
            "imageSha256",
            "backupBytes",
            "backupSha256",
            "wnroPatched",
            "diffCount",
            "outsidePatchOffsets",
            "armVectorOk",
            "isVendorV113",
            "isVendorV112",
            "recommendedMode",
        ] {
            assert!(inspect.get(key).is_some(), "missing {key}");
        }
        assert_eq!(inspect["recommendedMode"], "strict");

        let flash = serde_json::to_value(FwFlashOutcome {
            port: String::from("/dev/ttyUSB0"),
            mode: device::FlashMode::Vendor,
            bytes_written: 406_620,
            image_sha256: String::from("abc"),
            verified_read_back: true,
            restored_per_unit_bytes: 4,
            elapsed_ms: 1500,
        })
        .expect("serialize FwFlashOutcome");
        for key in [
            "port",
            "mode",
            "bytesWritten",
            "imageSha256",
            "verifiedReadBack",
            "restoredPerUnitBytes",
            "elapsedMs",
        ] {
            assert!(flash.get(key).is_some(), "missing {key}");
        }
        assert_eq!(flash["mode"], "vendor");
    }

    #[test]
    fn with_log_shared_works_with_or_without_source_check() {
        let log = Arc::new(Mutex::new(Some(logdoc::sample_log())));
        let count = with_log_shared(&log, None, |loaded| Ok(loaded.track.points.len()))
            .expect("no source check succeeds");
        assert!(count > 0);
        let err = with_log_shared(&log, Some("/other.bin"), |_| Ok(()))
            .expect_err("mismatched source rejected");
        assert!(err.contains("一致しません"));
    }
}
