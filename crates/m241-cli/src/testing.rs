//! Test doubles: a simulated logger behind the [`crate::link::Links`] seam, and
//! a console that records instead of printing.
//!
//! Only compiled for tests. The point of the two seams is that a command can be
//! run end to end without a serial port and without a terminal, so a test can
//! assert on the exact text a user would see.
//!
//! [`DeviceHandle`] holds a *factory*, not a device: `m241 fw dump` opens the
//! port twice, and the second read is only meaningful if it starts from a fresh
//! boot. Handing a fake device out per `open` is the same thing a power cycle
//! does to a real unit, so [`DeviceHandle::power_cycle`] is what the test writes
//! where the user would remove the battery.

use std::cell::RefCell;
use std::rc::Rc;

use m241_core::protocol::port::Transport;
use m241_core::protocol::simulator::FakeDevice;
use m241_core::{Error, Result};
use serde_json::Value;

use crate::link::{Cable, Links, Opened, PortEntry};
use crate::output::Console;

/// A console that keeps what it was told, for assertions.
#[derive(Debug, Default)]
pub struct RecordingConsole {
    stdout: RefCell<String>,
    stderr: RefCell<String>,
}

impl RecordingConsole {
    /// Everything written as machine output.
    #[must_use]
    pub fn stdout(&self) -> String {
        self.stdout.borrow().clone()
    }

    /// Everything written as progress, warnings and notes.
    #[must_use]
    pub fn stderr(&self) -> String {
        self.stderr.borrow().clone()
    }
}

impl Console for RecordingConsole {
    fn result(&self, line: &str) {
        let mut out = self.stdout.borrow_mut();
        out.push_str(line);
        out.push('\n');
    }

    fn text(&self, text: &str) {
        self.stdout.borrow_mut().push_str(text);
    }

    fn json(&self, value: &Value) -> Result<()> {
        let text = serde_json::to_string_pretty(value)
            .map_err(|err| Error::State(format!("could not serialise JSON: {err}")))?;
        self.result(&text);
        Ok(())
    }

    fn progress(&self, done: usize, total: usize) {
        self.stderr
            .borrow_mut()
            .push_str(&format!("{done}/{total} "));
    }

    fn progress_done(&self) {
        self.stderr.borrow_mut().push('\n');
    }

    fn warn(&self, message: &str) {
        self.stderr
            .borrow_mut()
            .push_str(&format!("warning: {message}\n"));
    }

    fn note(&self, message: &str) {
        self.stderr.borrow_mut().push_str(message);
        self.stderr.borrow_mut().push('\n');
    }
}

/// A confirmation prompt that never waits and changes nothing.
#[derive(Debug, Default, Clone, Copy)]
pub struct AutoConfirm;

impl crate::Confirm for AutoConfirm {
    fn pause(&self, _message: &str) -> Result<()> {
        Ok(())
    }
}

/// A simulated chip that survives being closed and reopened.
///
/// The factory rebuilds it: opening the port again after [`Self::power_cycle`]
/// reaches a freshly booted unit, which is exactly the state the second dump
/// pass needs.
#[derive(Clone)]
pub struct DeviceHandle {
    factory: Rc<dyn Fn() -> FakeDevice>,
    current: Rc<RefCell<FakeDevice>>,
}

impl std::fmt::Debug for DeviceHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceHandle")
            .field("mode", &self.current.borrow().mode())
            .finish_non_exhaustive()
    }
}

impl DeviceHandle {
    /// Build a handle whose every power cycle runs `factory`.
    #[must_use]
    pub fn new(factory: impl Fn() -> FakeDevice + 'static) -> Self {
        Self {
            current: Rc::new(RefCell::new(factory())),
            factory: Rc::new(factory),
        }
    }

    /// Replace the chip with a freshly booted one.
    pub fn power_cycle(&self) {
        *self.current.borrow_mut() = (self.factory)();
    }

    /// Whether the fake firmware is currently recording.
    #[must_use]
    pub fn logging_enabled(&self) -> bool {
        self.current.borrow().logging_enabled()
    }

    /// The bytes the fake firmware's flash window holds.
    #[must_use]
    pub fn memory(&self) -> Vec<u8> {
        self.current.borrow().memory().to_vec()
    }

    /// The command bodies the host sent, in order.
    #[must_use]
    pub fn commands(&self) -> Vec<String> {
        self.current.borrow().commands().to_vec()
    }
}

/// The transport a command gets when it opens the simulated port.
#[derive(Debug)]
struct SharedFake(DeviceHandle);

impl Transport for SharedFake {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        self.0.current.borrow_mut().read(buf)
    }

    fn write_all(&mut self, buf: &[u8]) -> Result<()> {
        self.0.current.borrow_mut().write_all(buf)
    }

    fn flush(&mut self) -> Result<()> {
        self.0.current.borrow_mut().flush()
    }

    fn clear_input(&mut self) -> Result<()> {
        self.0.current.borrow_mut().clear_input()
    }

    fn set_baud(&mut self, baud: u32) -> Result<()> {
        self.0.current.borrow_mut().set_baud(baud)
    }

    fn set_control_lines(&mut self, asserted: bool) -> Result<()> {
        self.0.current.borrow_mut().set_control_lines(asserted)
    }
}

/// The port the simulated device is reachable as.
pub const FAKE_PORT: &str = "SIM0";

/// A [`Links`] implementation over a [`DeviceHandle`].
#[derive(Debug)]
pub struct FakePorts {
    ports: Vec<PortEntry>,
    device: Option<DeviceHandle>,
}

impl FakePorts {
    /// No ports at all; opening anything fails.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            ports: Vec::new(),
            device: None,
        }
    }

    /// A fixed port list. Opening any of them fails: no device is attached.
    #[must_use]
    pub fn with_ports(ports: Vec<PortEntry>) -> Self {
        Self {
            ports,
            device: None,
        }
    }

    /// One plausible port backed by a handle the test keeps.
    #[must_use]
    pub fn shared(device: DeviceHandle) -> Self {
        Self {
            ports: vec![PortEntry {
                name: String::from(FAKE_PORT),
                cable: Some(Cable::Cp210x),
                likely: true,
                description: Some(String::from("simulated M-241")),
            }],
            device: Some(device),
        }
    }

    /// One plausible port backed by `factory`, which runs on every power cycle.
    #[must_use]
    pub fn single(factory: impl Fn() -> FakeDevice + 'static) -> Self {
        Self::shared(DeviceHandle::new(factory))
    }
}

impl Links for FakePorts {
    fn ports(&self) -> Result<Vec<PortEntry>> {
        Ok(self.ports.clone())
    }

    fn open(&self, name: &str, baud: u32) -> Result<Opened> {
        let Some(device) = &self.device else {
            return Err(Error::Port {
                message: format!("no simulated device behind {name}"),
            });
        };
        // Record the baud rate the command asked for, so a test can assert that
        // it spoke NMEA at 38400 rather than at the BootROM's rate.
        device.current.borrow_mut().set_baud(baud)?;
        Ok(Opened::new(Box::new(SharedFake(device.clone()))))
    }
}

/// A synthetic V1.13 image: valid header, stock constants, correct length.
///
/// The length matters. `m241_core::firmware::identify` matches a profile by
/// size *and* device string *and* version marker, so a header alone is not
/// identifiable — a fixture has to be as long as the release. 406 KB of `0xFF`
/// costs nothing in a test and keeps the command on its production code path.
#[must_use]
pub fn synthetic_image() -> Vec<u8> {
    use m241_core::firmware::PROFILE_V1_13;
    use m241_core::firmware::image::{
        DEVICE_TYPE_OFFSET, DEVICE_TYPE_SIZE, EPOCH_CONSTANT_OFFSETS, FAMILY, FAMILY_OFFSET,
        FW_SIZE_OFFSET, GPS_EPOCH_UNIX,
    };

    let profile = PROFILE_V1_13;
    let mut data = vec![0xFFu8; profile.size];
    data[FAMILY_OFFSET..FAMILY_OFFSET + FAMILY.len()].copy_from_slice(FAMILY.as_bytes());
    data[DEVICE_TYPE_OFFSET..DEVICE_TYPE_OFFSET + DEVICE_TYPE_SIZE].fill(0);
    let device = profile.device_type.as_bytes();
    data[DEVICE_TYPE_OFFSET..DEVICE_TYPE_OFFSET + device.len()].copy_from_slice(device);
    let marker = profile.name.as_bytes();
    data[profile.version_marker..profile.version_marker + marker.len()].copy_from_slice(marker);
    for offset in EPOCH_CONSTANT_OFFSETS {
        data[offset..offset + 4].copy_from_slice(&GPS_EPOCH_UNIX.to_le_bytes());
    }
    // The measured image stores its length minus four at 0xF4 (406,616 for the
    // 406,620-byte V1.13 file), so the fixture does too: a command that only
    // accepted `declared == len` would pass a synthetic test and reject a real
    // dump.
    data[FW_SIZE_OFFSET..FW_SIZE_OFFSET + 4]
        .copy_from_slice(&(profile.size as u32 - 4).to_le_bytes());
    data
}

/// A one-block track-log dump with `points` records at the documented example
/// coordinate.
///
/// The records are the measured 16-byte layout (UTC + lat + lon + height + XOR
/// byte) under the V1.13 default bitmask.
#[must_use]
pub fn synthetic_dump(points: usize) -> Vec<u8> {
    use m241_core::log::block::{BLOCK_HEADER_SIZE, BLOCK_SIZE, POLICY_LOG_ENABLED, xor_checksum};

    // UTC + lat + lon + height: the measured V1.13 default mask.
    const MASK: u32 = 0x8000_001D;
    const LAT: f32 = 35.681_236;
    const LON: f32 = 139.767_13;
    const START: u32 = 1_700_000_000;

    let mut block = vec![0xFFu8; BLOCK_SIZE];
    block[0..2].copy_from_slice(&(points as u16).to_le_bytes());
    block[2..6].copy_from_slice(&MASK.to_le_bytes());
    block[6..8].copy_from_slice(&POLICY_LOG_ENABLED.to_le_bytes());
    block[8..12].copy_from_slice(&10u32.to_le_bytes());

    let mut cursor = BLOCK_HEADER_SIZE;
    for index in 0..points {
        let mut record = Vec::new();
        record.extend_from_slice(&(START + index as u32).to_le_bytes());
        record.extend_from_slice(&LAT.to_le_bytes());
        record.extend_from_slice(&LON.to_le_bytes());
        // 42 m as the three high bytes of an f32.
        record.extend_from_slice(&[0x00, 0xA0, 0x42]);
        let checksum = xor_checksum(&record);
        record.push(checksum);
        block[cursor..cursor + record.len()].copy_from_slice(&record);
        cursor += record.len();
    }
    block
}
