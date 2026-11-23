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

use crate::device;

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
                 対処: 少し待ってから「接続して確認」を再実行してください。"
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
            "端末の応答が想定と異なります。この画面では書き込みを行いません。\
             端末を再起動してから再試行してください。"
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
