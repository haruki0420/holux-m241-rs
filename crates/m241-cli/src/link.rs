//! Finding and opening the serial port, as the CLI wants to present it.
//!
//! [`m241_core::protocol::Port`] knows how to open one port and list their
//! names. The command line needs more than a name: `m241 device ports` reports
//! which USB-serial bridge a port is, because "the CP210x is there" answers the
//! question "did I plug the logger in" and a bare `/dev/ttyUSB0` does not.
//!
//! That classification is presentation, so it lives here rather than in the
//! protocol layer. The handlers reach the wire through [`Links`] instead of
//! `Port` directly, which is also what lets the tests drive a whole command
//! against `FakeDevice` without a serial port existing.

use std::fmt;

use m241_core::protocol::port::Transport;
use m241_core::protocol::{PmtkClient, Port};

use crate::Timings;

/// The USB-serial bridges an M-241 cable can appear as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cable {
    /// Silicon Labs CP210x — what the genuine Holux cable uses.
    Cp210x,
    /// Prolific PL2303.
    Pl2303,
    /// FTDI FT232.
    Ftdi,
    /// WCH CH340 / CH341.
    Ch340,
}

impl Cable {
    /// The name shown in the port table.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Cp210x => "CP210x",
            Self::Pl2303 => "PL2303",
            Self::Ftdi => "FTDI",
            Self::Ch340 => "CH340",
        }
    }
}

impl fmt::Display for Cable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// One serial port, described the way the command line reports it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PortEntry {
    /// The OS name: `/dev/ttyUSB0`, `COM3`.
    pub name: String,
    /// The bridge chip, when the USB ids or the description identify it.
    pub cable: Option<Cable>,
    /// Whether this looks like a cable the logger can be on.
    pub likely: bool,
    /// The OS description, when it has one.
    pub description: Option<String>,
}

/// Classify a port from the USB ids and the OS's description strings.
///
/// The ids decide when they are meaningful; the text is a fallback for a port
/// the OS describes but cannot attribute to a USB vendor (and for `COM` ports,
/// where the description is often all there is).
#[must_use]
pub fn identify(
    vid: Option<u16>,
    pid: Option<u16>,
    manufacturer: Option<&str>,
    product: Option<&str>,
) -> (Option<Cable>, bool) {
    let cable = match (vid, pid) {
        // Silicon Labs. The CP210x family shares the vendor id; the PDA/EA60
        // pair is the common cable, but taking the vendor alone is what makes a
        // CP2105 dual bridge (EA70) work too.
        (Some(0x10C4), _) => Some(Cable::Cp210x),
        (Some(0x067B), Some(0x2303)) => Some(Cable::Pl2303),
        (Some(0x0403), _) => Some(Cable::Ftdi),
        // WCH CH340 (`0x7523`) and CH341 in serial mode (`0x5523`).
        (Some(0x1A86), Some(0x7523 | 0x5523)) => Some(Cable::Ch340),
        _ => None,
    };
    if let Some(cable) = cable {
        return (Some(cable), true);
    }
    let haystack = format!(
        "{} {}",
        manufacturer.unwrap_or_default(),
        product.unwrap_or_default()
    )
    .to_ascii_lowercase();
    let from_text = if haystack.contains("cp210") {
        Some(Cable::Cp210x)
    } else if haystack.contains("pl2303") || haystack.contains("prolific") {
        Some(Cable::Pl2303)
    } else if haystack.contains("ftdi") {
        Some(Cable::Ftdi)
    } else if haystack.contains("ch340") || haystack.contains("ch341") {
        Some(Cable::Ch340)
    } else {
        None
    };
    let likely = from_text.is_some()
        || [
            "silicon labs",
            "silabs",
            "usb-serial",
            "usb serial",
            "usb to serial",
            "uart bridge",
            "holux",
            "m-241",
            "m241",
            "gps",
        ]
        .iter()
        .any(|hint| haystack.contains(hint));
    (from_text, likely)
}

/// A connection opened through [`Links`], erased to the protocol's own trait.
///
/// Wrapping the box instead of requiring `Transport for Box<dyn Transport>` in
/// the core keeps the core from growing an impl it has no use for.
#[derive(Debug)]
pub struct Opened(Box<dyn Transport>);

impl Opened {
    /// Wrap an already-open transport, e.g. a simulator in a test.
    #[must_use]
    pub fn new(transport: Box<dyn Transport>) -> Self {
        Self(transport)
    }
}

impl Transport for Opened {
    fn read(&mut self, buf: &mut [u8]) -> m241_core::Result<usize> {
        self.0.read(buf)
    }

    fn write_all(&mut self, buf: &[u8]) -> m241_core::Result<()> {
        self.0.write_all(buf)
    }

    fn flush(&mut self) -> m241_core::Result<()> {
        self.0.flush()
    }

    fn clear_input(&mut self) -> m241_core::Result<()> {
        self.0.clear_input()
    }

    fn set_baud(&mut self, baud: u32) -> m241_core::Result<()> {
        self.0.set_baud(baud)
    }

    fn set_control_lines(&mut self, asserted: bool) -> m241_core::Result<()> {
        self.0.set_control_lines(asserted)
    }
}

/// Where the commands get their ports from.
///
/// The real implementation enumerates and opens the OS's ports; the tests
/// implement this over [`m241_core::protocol::simulator::FakeDevice`].
pub trait Links {
    /// Every serial port on this machine, likely M-241 cables first.
    fn ports(&self) -> m241_core::Result<Vec<PortEntry>>;

    /// Open `name` at `baud`.
    fn open(&self, name: &str, baud: u32) -> m241_core::Result<Opened>;
}

/// The real machine's serial ports.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemLinks;

impl Links for SystemLinks {
    fn ports(&self) -> m241_core::Result<Vec<PortEntry>> {
        let ports = serialport::available_ports().map_err(m241_core::Error::from)?;
        let mut entries: Vec<PortEntry> = ports
            .into_iter()
            .map(|port| match port.port_type {
                serialport::SerialPortType::UsbPort(usb) => {
                    let (cable, likely) = identify(
                        Some(usb.vid),
                        Some(usb.pid),
                        usb.manufacturer.as_deref(),
                        usb.product.as_deref(),
                    );
                    PortEntry {
                        name: port.port_name,
                        cable,
                        likely,
                        description: usb.product.clone().or_else(|| usb.manufacturer.clone()),
                    }
                }
                other => PortEntry {
                    name: port.port_name,
                    cable: None,
                    likely: false,
                    // `PciPort` / `BluetoothPort` / `Unknown` carry no text of
                    // their own; the OS name is all there is.
                    description: match other {
                        serialport::SerialPortType::BluetoothPort => {
                            Some(String::from("bluetooth"))
                        }
                        serialport::SerialPortType::PciPort => Some(String::from("pci")),
                        _ => None,
                    },
                },
            })
            .collect();
        // Likely cables first, OS order kept inside each group.
        entries.sort_by_key(|entry| !entry.likely);
        Ok(entries)
    }

    fn open(&self, name: &str, baud: u32) -> m241_core::Result<Opened> {
        Port::open(name, baud).map(|port| Opened::new(Box::new(port)))
    }
}

/// Resolve a `--port` argument, where `auto` means "the only plausible cable".
///
/// Ambiguity is a refusal rather than a guess: opening the wrong port costs the
/// user a confusing timeout, and listing the candidates costs them nothing.
///
/// # Errors
///
/// [`m241_core::Error::Port`] when no port looks like a cable, or when several
/// do and the user did not say which.
pub fn resolve(links: &dyn Links, spec: &str) -> m241_core::Result<String> {
    let spec = spec.trim();
    if !spec.is_empty() && !spec.eq_ignore_ascii_case("auto") {
        return Ok(spec.to_owned());
    }
    let ports = links.ports()?;
    let candidates: Vec<&PortEntry> = ports.iter().filter(|entry| entry.likely).collect();
    match candidates.as_slice() {
        [only] => Ok(only.name.clone()),
        [] => Err(m241_core::Error::Port {
            message: String::from(
                "no serial port looks like an M-241 cable; plug the logger in, switch it on, \
                 and run `m241 device ports`",
            ),
        }),
        many => Err(m241_core::Error::Port {
            message: format!(
                "several ports could be the logger ({}); pass --port with the right one",
                many.iter()
                    .map(|entry| entry.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }),
    }
}

/// Open the PMTK channel to a unit at NMEA baud with the session's timeouts.
///
/// # Errors
///
/// [`m241_core::Error::Port`] when the port cannot be opened.
pub(crate) fn open_pmtk(
    links: &dyn Links,
    name: &str,
    timings: Timings,
) -> m241_core::Result<PmtkClient<Opened>> {
    let transport = links.open(name, m241_core::protocol::pmtk::NMEA_BAUD)?;
    Ok(PmtkClient::new(transport)
        .with_reply_timeout(timings.reply)
        .with_stream_window(timings.stream)
        .with_entry_timing(timings.entry))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_genuine_cable_is_recognised_by_its_usb_ids() {
        assert_eq!(
            identify(Some(0x10C4), Some(0xEA60), None, None),
            (Some(Cable::Cp210x), true)
        );
        // The vendor alone is enough: the dual-bridge CP2105 uses another pid.
        assert_eq!(
            identify(Some(0x10C4), Some(0xEA70), None, None).0,
            Some(Cable::Cp210x)
        );
    }

    #[test]
    fn the_three_stand_in_bridges_are_recognised() {
        assert_eq!(
            identify(Some(0x067B), Some(0x2303), None, None).0,
            Some(Cable::Pl2303)
        );
        assert_eq!(
            identify(Some(0x0403), Some(0x6001), None, None).0,
            Some(Cable::Ftdi)
        );
        assert_eq!(
            identify(Some(0x1A86), Some(0x7523), None, None).0,
            Some(Cable::Ch340)
        );
        assert_eq!(
            identify(Some(0x1A86), Some(0x5523), None, None).0,
            Some(Cable::Ch340)
        );
    }

    #[test]
    fn a_bridge_with_another_pid_is_not_mistaken_for_a_cable() {
        // 0x067B is Prolific, but 0x2303 is the PL2303 converter; another pid
        // on the same vendor is a different product. Same for WCH outside
        // CH340/CH341 serial pids.
        assert_eq!(
            identify(Some(0x067B), Some(0x0001), None, None),
            (None, false)
        );
        assert_eq!(
            identify(Some(0x1A86), Some(0x0001), None, None),
            (None, false)
        );
    }

    #[test]
    fn the_description_is_the_fallback_when_the_ids_are_absent() {
        let (cable, likely) =
            identify(None, None, Some("Silicon Labs"), Some("CP2102 USB to UART"));
        assert_eq!(cable, Some(Cable::Cp210x));
        assert!(likely);

        let (cable, likely) = identify(None, None, Some("FTDI"), Some("FT232R USB UART"));
        assert_eq!(cable, Some(Cable::Ftdi));
        assert!(likely);
    }

    #[test]
    fn a_holux_description_flags_the_port_even_without_a_known_chip() {
        let (cable, likely) = identify(None, None, Some("Holux"), Some("M-241 GPS Logger"));
        assert_eq!(cable, None);
        assert!(likely, "the cable is the thing the user is looking for");
    }

    #[test]
    fn an_unrelated_port_is_neither_flagged_nor_classified() {
        assert_eq!(identify(None, None, None, None), (None, false));
        assert_eq!(
            identify(None, None, Some("Intel"), Some("AMT Serial Port")),
            (None, false)
        );
    }

    #[test]
    fn the_cable_labels_are_the_four_the_docs_promise() {
        let labels: Vec<&str> = [Cable::Cp210x, Cable::Pl2303, Cable::Ftdi, Cable::Ch340]
            .iter()
            .map(|cable| cable.label())
            .collect();
        assert_eq!(labels, vec!["CP210x", "PL2303", "FTDI", "CH340"]);
    }

    #[test]
    fn an_explicit_port_name_bypasses_auto_discovery() {
        let empty = crate::testing::FakePorts::empty();
        assert_eq!(
            resolve(&empty, "/dev/ttyUSB0").expect("explicit"),
            "/dev/ttyUSB0"
        );
        assert_eq!(resolve(&empty, " COM3 ").expect("trimmed"), "COM3");

        let ambiguous = crate::testing::FakePorts::with_ports(vec![
            PortEntry {
                name: String::from("/dev/ttyUSB0"),
                cable: Some(Cable::Cp210x),
                likely: true,
                description: None,
            },
            PortEntry {
                name: String::from("/dev/ttyUSB1"),
                cable: Some(Cable::Cp210x),
                likely: true,
                description: None,
            },
        ]);
        assert_eq!(
            resolve(&ambiguous, "/dev/ttyUSB1").expect("explicit beats ambiguity"),
            "/dev/ttyUSB1"
        );
        assert!(resolve(&ambiguous, "auto").is_err());
        assert!(resolve(&ambiguous, "AUTO").is_err());
        assert!(resolve(&ambiguous, "").is_err());
    }
}
