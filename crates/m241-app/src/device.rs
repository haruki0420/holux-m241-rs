//! Port enumeration and the one read-only device probe the P0 screen performs.
//!
//! Everything that opens a serial port lives here, on the Rust side. The
//! frontend only passes a port name string to `device_info`; it cannot open,
//! configure or write a port itself, because WebView code has no serial access
//! and the capability set does not grant one.
//!
//! The probe is deliberately the same two read requests the CLI's
//! `m241 device info` and the egui Connect screen use:
//!
//! * `$PMTK605` → `$PMTK705` (`PmtkClient::request_identity`) for the release,
//!   build and model strings;
//! * `$PMTK182,2,8` (`PmtkClient::log_used_bytes`) for the track-log bytes in
//!   use;
//! * `$PMTK182,2,1` (`PmtkClient::log_enabled`) for the recording state, and
//!   only that one is allowed to fail without failing the probe: the identity
//!   and the usage are what the Connect card is for.
//!
//! No write, erase or logging-state change is sent. `m241-flash` is not even a
//! dependency of this crate, so a destructive opcode cannot be reached from
//! here at all (see `tools/check_safety.py`).

use m241_core::protocol::Port;
use m241_core::protocol::pmtk::NMEA_BAUD;
use m241_core::protocol::pmtk::PmtkClient;
use m241_core::{DeviceInfo as CoreDeviceInfo, Result};

/// The USB-serial bridges a Holux cable is known to appear as.
///
/// The vendor ids are the ones `m241-cli`'s `link::identify` uses; keeping the
/// list here rather than making the GUI depend on the CLI keeps the two
/// binaries independent, and the values are USB ids rather than logic that can
/// drift.
const KNOWN_VIDS: [u16; 4] = [
    0x10C4, // Silicon Labs CP210x — the genuine Holux cable
    0x067B, // Prolific PL2303
    0x0403, // FTDI FT232
    0x1A86, // WCH CH340 / CH341
];

/// One serial port, as the frontend's port picker needs it.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortInfo {
    /// The OS name: `/dev/ttyUSB0` on Linux, `COM3` on Windows.
    pub name: String,
    /// The OS description, when it has one.
    pub description: Option<String>,
    /// The port family the OS reported: `usb`, `pci`, `bluetooth` or `unknown`.
    pub kind: String,
    /// Whether the USB ids (or the description) look like an M-241 cable.
    pub likely: bool,
}

/// What the frontend receives after a successful `device_info` call.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    /// The port that was opened, echoed so a late reply cannot be attributed to
    /// a different selection.
    pub port: String,
    /// Release name, e.g. `B-core_1.20`.
    pub release: String,
    /// Build number as the firmware reports it.
    pub build: String,
    /// Model or product id, e.g. `01017-00F`.
    pub model: String,
    /// SDK version, when the firmware answers with one.
    pub sdk: Option<String>,
    /// Whether the release names a firmware family this build knows. A warning
    /// for the operator, never a refusal.
    pub known_release: bool,
    /// Bytes of track log the unit reports in use (`$PMTK182,3,8`).
    pub log_used_bytes: u64,
    /// Whether the unit is currently recording. `None` when the optional
    /// `$PMTK182,2,1` request failed; the rest of the card is still valid.
    pub log_enabled: Option<bool>,
}

/// List the serial ports, likely cables first.
///
/// An enumeration failure yields an empty list rather than a panic: the screen
/// then says no ports were found and offers a manual entry, which is the same
/// recovery a genuinely empty machine gets. The OS error itself is not useful
/// in the picker and there is no second channel for it in the P0 contract.
#[must_use]
pub fn list_ports() -> Vec<PortInfo> {
    let Ok(ports) = serialport::available_ports() else {
        return Vec::new();
    };
    let mut entries: Vec<PortInfo> = ports.into_iter().map(describe_port).collect();
    // Stable sort: likely cables first, the OS's own order kept otherwise.
    entries.sort_by_key(|entry| !entry.likely);
    entries
}

fn describe_port(port: serialport::SerialPortInfo) -> PortInfo {
    let name = port.port_name;
    match port.port_type {
        serialport::SerialPortType::UsbPort(usb) => {
            let description = usb.product.clone().or_else(|| usb.manufacturer.clone());
            let likely = looks_like_cable(Some(usb.vid), Some(usb.pid), description.as_deref());
            PortInfo {
                name,
                description,
                kind: String::from("usb"),
                likely,
            }
        }
        other => {
            let kind = match other {
                serialport::SerialPortType::PciPort => "pci",
                serialport::SerialPortType::BluetoothPort => "bluetooth",
                _ => "unknown",
            };
            PortInfo {
                name,
                description: None,
                kind: String::from(kind),
                likely: false,
            }
        }
    }
}

/// Whether a port's USB ids or description identify it as a logger cable.
///
/// The ids decide when they exist; the description is the fallback for a port
/// the OS names but cannot attribute to a USB vendor.
#[must_use]
pub fn looks_like_cable(vid: Option<u16>, _pid: Option<u16>, description: Option<&str>) -> bool {
    if let Some(vid) = vid
        && KNOWN_VIDS.contains(&vid)
    {
        return true;
    }
    let Some(description) = description else {
        return false;
    };
    let lowered = description.to_ascii_lowercase();
    [
        "holux",
        "cp210",
        "silicon labs",
        "prolific",
        "ftdi",
        "ch340",
    ]
    .iter()
    .any(|needle| lowered.contains(needle))
}

/// Open `port_name` at the NMEA baud and read the identity and log usage.
///
/// # Errors
///
/// [`m241_core::Error`] from opening the port or from either read-only request.
/// The port is closed when the function returns: the client owns it.
pub fn probe(port_name: &str) -> Result<DeviceInfo> {
    let port = Port::open(port_name, NMEA_BAUD)?;
    let mut client = PmtkClient::new(port);
    let info = client.request_identity()?;
    let log_used_bytes = client.log_used_bytes()?;
    // The recording state is a convenience figure, not part of the probe's
    // contract: a unit that answers the identity and the usage but not
    // `$PMTK182,2,1` still gets a useful card, with this one field unknown.
    let log_enabled = client.log_enabled().ok();
    Ok(DeviceInfo::from_core(
        port_name,
        info,
        log_used_bytes,
        log_enabled,
    ))
}

impl DeviceInfo {
    fn from_core(
        port: &str,
        info: CoreDeviceInfo,
        log_used_bytes: usize,
        log_enabled: Option<bool>,
    ) -> Self {
        Self {
            port: port.to_owned(),
            known_release: info.is_known_release(),
            release: info.release,
            build: info.build,
            model: info.model,
            sdk: info.sdk,
            log_used_bytes: u64::try_from(log_used_bytes).unwrap_or(u64::MAX),
            log_enabled,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_known_usb_vendor_is_flagged_without_a_description() {
        assert!(looks_like_cable(Some(0x10C4), Some(0xEA60), None));
        assert!(looks_like_cable(Some(0x1A86), Some(0x7523), None));
    }

    #[test]
    fn an_unrelated_usb_port_is_not_flagged() {
        assert!(!looks_like_cable(
            Some(0x046D),
            Some(0xC52B),
            Some("Logitech")
        ));
    }

    #[test]
    fn the_description_is_the_fallback_when_the_ids_are_absent() {
        assert!(looks_like_cable(None, None, Some("HOLUX M-241")));
        assert!(looks_like_cable(None, None, Some("Silicon Labs CP210x")));
        assert!(!looks_like_cable(None, None, Some("USB Serial")));
    }

    #[test]
    fn a_device_info_keeps_the_release_warning_and_the_usage() {
        let info = DeviceInfo::from_core(
            "/dev/ttyUSB0",
            CoreDeviceInfo {
                release: String::from("B-core_1.20"),
                build: String::from("0043"),
                model: String::from("01017-00F"),
                sdk: Some(String::from("1.0")),
            },
            0x0003_3F7C,
            Some(true),
        );
        assert_eq!(info.port, "/dev/ttyUSB0");
        assert!(info.known_release);
        assert_eq!(info.log_used_bytes, 212_860);
        assert_eq!(info.log_enabled, Some(true));
    }
}
