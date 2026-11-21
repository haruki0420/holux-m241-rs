//! MTK BootROM client.
//!
//! Every command is a nine-byte big-endian frame — opcode, address, count — and
//! the chip echoes every byte it receives before answering. The count field is
//! in *units*, not bytes: words for `0xA1`/`0xA2`/`0xA4`, dwords for `0xAF`.
//!
//! # Where the constants come from
//!
//! The values in this module are not all of the same kind, and reading them as
//! if they were is how a working value gets treated as a hardware limit. Two
//! headings separate them; every constant repeats its own label on its doc
//! comment.
//!
//! ## Established on hardware
//!
//! Read off a retail M-241 (V1.13, MT3318) and recorded in
//! `docs/dev/03-hardware-facts.md`. Do not re-derive these without new
//! evidence.
//!
//! * [`BASE_ADDR_FIRMWARE`] — where flash answers through the BootROM.
//! * [`BASE_ADDR_FIRMWARE_MT333X`] — the address that does *not* answer here,
//!   which is what pinned the base above down.
//! * [`BOOT_SYNC`] / [`BOOT_REPLY`] — the four probes and their replies.
//! * [`PMTK180_COMMAND`] — the entry route while NMEA is streaming.
//! * [`DEFAULT_NMEA_BAUD`] / [`DEFAULT_BOOTROM_BAUD`] — the two rates measured.
//!
//! ## Not established on hardware here
//!
//! These are working values with a different origin, and each says which:
//!
//! * `Vendor tooling:` — read out of the vendor utility rather than measured.
//!   See `docs/dev/04-prior-art.md` for how much weight that carries.
//!   [`BASE_ADDR_DA`].
//! * `Reference implementation:` — taken from the previous Python client. It
//!   worked, which is not the same as measured: no experiment established it.
//!   [`BASE_ADDR_FIRMWARE_M3318`], [`WRITE_CHUNK`], [`DUMP_CHUNK`].
//! * `Vendor images:` — decoded from the released firmware files in the local
//!   `holux/` drop and checked byte for byte; not a wire measurement.
//!   [`FIRMWARE_SIZE_OFFSET`].
//!
//! `Other chip:` appears on [`BASE_ADDR_FIRMWARE_MT333X`] as well: the address
//! belongs to a different MediaTek part and is kept only so a probe can rule it
//! out.
//!
//! # Frame layout
//!
//! ```text
//!   opcode (u8) | address (u32 BE) | count (u32 BE)
//! ```
//!
//! The chip echoes the nine bytes back, then sends its data. Reads carry **no**
//! checksum trailer; `0xA4` is a separate command and is the only one that
//! answers with a two-byte value.

use crate::error::{Error, Result};
use crate::protocol::frame::swap_pairs;
use crate::protocol::port::Transport;

/// Flash is visible at this address through the BootROM on the M-241.
///
/// `Measured:` reading `0x20000000` returns `42 20 02 02` repeatedly (unmapped
/// bus), while `0x00200000` returns the firmware's `18 f0 9f e5` vector table.
pub const BASE_ADDR_FIRMWARE: u32 = 0x0020_0000;

/// The MT3318's linear view of the same flash, used by the Download Agent.
///
/// `Reference implementation:` both prior clients use `0x0` as the view the DA
/// writes through, with flash at [`BASE_ADDR_FIRMWARE`] being the BootROM's
/// view of the same bytes. This crate has not addressed flash through the linear
/// view at all — every read this project measured went through the BootROM
/// address above — so the value is carried over, not re-established.
pub const BASE_ADDR_FIRMWARE_M3318: u32 = 0x0000_0000;

/// MT333x-style probe address. Unmapped on the M-241; kept to rule it out.
///
/// `Other chip:` this is the flash window some MT333x parts use, not the
/// MT3318's. `Measured:` on this part it reads back `42 20 02 02` (an unmapped
/// bus, not flash), which is how the M-241's own base was identified; see
/// `docs/dev/03-hardware-facts.md`.
pub const BASE_ADDR_FIRMWARE_MT333X: u32 = 0x2000_0000;

/// Where the Download Agent must be loaded.
///
/// `Vendor tooling:` `push 0x100800` appears exactly once in
/// `HoluxLoggerUtility.exe`; `0x0010_0000`–`0x0010_07FF` below it is the
/// BootROM's own stack/scratch region, and writing there kills the chip. The
/// load address itself is corroborated by both prior clients
/// (`docs/dev/04-prior-art.md`), not re-measured here.
pub const BASE_ADDR_DA: u32 = 0x0010_0800;

/// The four sync probe bytes, in order.
///
/// `Measured:` on a freshly reset retail unit the first `0xA0` draws `0x5F`
/// only after 36 attempts; the three replies that follow are as below.
pub const BOOT_SYNC: [u8; 4] = [0xA0, 0x0A, 0x50, 0x05];

/// The reply each sync probe must draw.
///
/// `Measured:` same session as [`BOOT_SYNC`] — `0xA0`→`0x5F`, `0x0A`→`0xF5`,
/// `0x50`→`0xAF`, `0x05`→`0xFA`.
pub const BOOT_REPLY: [u8; 4] = [0x5F, 0xF5, 0xAF, 0xFA];

/// `$PMTK180*3B` with its terminator: the only reliable way into BootROM on a
/// running firmware.
///
/// `Measured:` sending it at 38400 while NMEA is still streaming stops the
/// stream and makes the chip listen for `0xA0`; sending it with arguments, or
/// after the stream has stopped, is silently ignored.
pub const PMTK180_COMMAND: &[u8] = b"$PMTK180*3B\r\n";

/// The M-241's NMEA baud rate, used to send `$PMTK180`.
///
/// `Measured:` the retail unit streams `$GPGGA` and friends at 38400 8N1.
pub const DEFAULT_NMEA_BAUD: u32 = 38_400;

/// The BootROM's own baud rate.
///
/// `Measured:` after `$PMTK180*3B` the chip answers the `0xA0` probe at 115200;
/// that is the rate the retail unit's BootROM was entered at and dumped from.
pub const DEFAULT_BOOTROM_BAUD: u32 = 115_200;

/// Payload bytes per `0xA1` serial chunk.
///
/// `Reference implementation:` the previous Python client streams ten bytes per
/// chunk because the reference tooling did. No experiment here established 10
/// as a limit — it is a working value, not a measured maximum.
pub const WRITE_CHUNK: usize = 10;

/// Bytes per read command while dumping.
///
/// `Reference implementation:` a full dump needs around 100 round trips at this
/// size in both prior clients. It is a chosen size, not a device maximum.
pub const DUMP_CHUNK: usize = 0x1000;

/// Offset of the firmware-size field, relative to the firmware base.
///
/// `Vendor images:` both released images store the length there as a
/// little-endian `u32` equal to the file size minus four (406,616 for the
/// 406,620-byte V1.13 file). Verified against `holux/` locally; not a hardware
/// measurement.
pub const FIRMWARE_SIZE_OFFSET: u32 = 0xF4;

pub const OPCODE_WRITE16: u8 = 0xA1;
pub const OPCODE_READ16: u8 = 0xA2;
pub const OPCODE_CHECKSUM: u8 = 0xA4;
pub const OPCODE_JUMP: u8 = 0xA8;
pub const OPCODE_READ32: u8 = 0xAF;

/// A nine-byte BootROM command frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    pub opcode: u8,
    pub address: u32,
    pub count: u32,
}

impl Frame {
    /// Build a frame whose `count` is in bytes, converting to the unit the
    /// opcode expects.
    ///
    /// # Errors
    ///
    /// Returns [`Error::State`] when `count` is not a whole number of units for
    /// the opcode — a half-word read is a bug in the caller, not a device
    /// problem, and rounding it silently would read the wrong region.
    pub fn for_bytes(opcode: u8, address: u32, count: usize) -> Result<Self> {
        let unit = match opcode {
            OPCODE_READ32 => 4,
            OPCODE_WRITE16 | OPCODE_READ16 | OPCODE_CHECKSUM => 2,
            OPCODE_JUMP => 1,
            other => {
                return Err(Error::State(format!(
                    "opcode 0x{other:02X} has no defined length unit"
                )));
            }
        };
        if !count.is_multiple_of(unit) {
            return Err(Error::State(format!(
                "opcode 0x{opcode:02X} counts {unit}-byte units; {count} is not a multiple"
            )));
        }
        let units = u32::try_from(count / unit)
            .map_err(|_| Error::State(format!("count {count} does not fit in a frame")))?;
        Ok(Self {
            opcode,
            address,
            count: units,
        })
    }

    /// The nine bytes as they go on the wire.
    #[must_use]
    pub fn to_bytes(self) -> [u8; 9] {
        let mut out = [0u8; 9];
        out[0] = self.opcode;
        out[1..5].copy_from_slice(&self.address.to_be_bytes());
        out[5..9].copy_from_slice(&self.count.to_be_bytes());
        out
    }
}

/// BootROM client over any [`Transport`].
#[derive(Debug)]
pub struct MtkBootRom<T: Transport> {
    transport: T,
    timeout: std::time::Duration,
}

impl<T: Transport> MtkBootRom<T> {
    /// Wrap an already-open transport.
    #[must_use]
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            timeout: std::time::Duration::from_secs(5),
        }
    }

    /// Borrow the transport (tests use this to inspect what was written).
    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.transport
    }

    /// Take the transport back, for the client that follows this one.
    ///
    /// A BootROM client owns its transport for the duration of one conversation;
    /// a command that reads first and then hands the chip to the Download Agent
    /// needs the same port back. `m241-flash`'s driver takes one by this method
    /// (`DownloadAgent::new`), so the write path's own test harness depends on it.
    #[must_use]
    pub fn into_transport(self) -> T {
        self.transport
    }

    /// Send the four sync probes and verify each reply.
    ///
    /// The probes must go out **back to back**: a pause between them makes the
    /// chip give up on the handshake.
    ///
    /// # Errors
    ///
    /// [`Error::Timeout`] when `0xA0` draws no reply, [`Error::Framing`] when a
    /// reply byte is not the expected value.
    pub fn sync(&mut self) -> Result<()> {
        let deadline = std::time::Instant::now() + self.timeout;
        self.transport.clear_input()?;
        loop {
            self.transport.write_all(&[BOOT_SYNC[0]])?;
            self.transport.flush()?;
            let mut byte = [0u8; 1];
            if self.transport.read(&mut byte)? == 1 && byte[0] == BOOT_REPLY[0] {
                break;
            }
            if std::time::Instant::now() > deadline {
                return Err(Error::Timeout {
                    what: String::from("BootROM sync (no 0x5F reply to 0xA0)"),
                    timeout: self.timeout.into(),
                });
            }
        }
        for (probe, expect) in BOOT_SYNC.iter().zip(BOOT_REPLY.iter()).skip(1) {
            self.transport.write_all(&[*probe])?;
            self.transport.flush()?;
            let mut byte = [0u8; 1];
            if self.transport.read(&mut byte)? != 1 {
                return Err(Error::Timeout {
                    what: format!("BootROM sync (no reply to 0x{probe:02X})"),
                    timeout: self.timeout.into(),
                });
            }
            if byte[0] != *expect {
                return Err(Error::Framing {
                    what: format!(
                        "sync probe 0x{probe:02X} answered 0x{:02X}, expected 0x{expect:02X}",
                        byte[0]
                    ),
                    offset: None,
                });
            }
        }
        Ok(())
    }

    /// `0xA2` READ16: read `count` bytes, returning them in memory order.
    ///
    /// # Errors
    ///
    /// [`Error::Framing`] when the echo does not match what was sent — that
    /// means the line is desynchronised and continuing would read the wrong
    /// region.
    pub fn read16(&mut self, address: u32, count: usize) -> Result<Vec<u8>> {
        let raw = self.read_opcode(OPCODE_READ16, address, count)?;
        Ok(swap_pairs(&raw))
    }

    /// `0xAF` READ32: read `count` bytes, returning them in memory order.
    ///
    /// # Errors
    ///
    /// As [`Self::read16`].
    pub fn read32(&mut self, address: u32, count: usize) -> Result<Vec<u8>> {
        let raw = self.read_opcode(OPCODE_READ32, address, count)?;
        Ok(crate::protocol::frame::reverse_words32(&raw))
    }

    fn read_opcode(&mut self, opcode: u8, address: u32, count: usize) -> Result<Vec<u8>> {
        let frame = Frame::for_bytes(opcode, address, count)?;
        self.send_and_check_echo(&frame)?;
        let mut data = vec![0u8; count];
        self.read_exact(&mut data, &format!("0x{opcode:02X} data"))?;
        Ok(data)
    }

    fn send_and_check_echo(&mut self, frame: &Frame) -> Result<()> {
        let bytes = frame.to_bytes();
        self.transport.write_all(&bytes)?;
        self.transport.flush()?;
        let mut echo = [0u8; 9];
        self.read_exact(&mut echo, "command echo")?;
        if echo != bytes {
            return Err(Error::Framing {
                what: format!("echo was {echo:02X?}, expected {:02X?}", bytes),
                offset: None,
            });
        }
        Ok(())
    }

    fn read_exact(&mut self, buf: &mut [u8], what: &str) -> Result<()> {
        let deadline = std::time::Instant::now() + self.timeout;
        let mut filled = 0usize;
        while filled < buf.len() {
            let Some(rest) = buf.get_mut(filled..) else {
                break;
            };
            let got = self.transport.read(rest)?;
            filled += got;
            if std::time::Instant::now() > deadline {
                return Err(Error::Timeout {
                    what: format!("{what} ({filled} of {} bytes)", buf.len()),
                    timeout: self.timeout.into(),
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::port::Transport;

    /// A transport that answers every frame with a canned payload.
    #[derive(Debug)]
    struct Canned {
        reply: Vec<u8>,
        written: Vec<u8>,
        cursor: usize,
    }

    impl Canned {
        fn new(reply: Vec<u8>) -> Self {
            Self {
                reply,
                written: Vec::new(),
                cursor: 0,
            }
        }
    }

    impl Transport for Canned {
        fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
            let left = self.reply.len().saturating_sub(self.cursor);
            if left == 0 || buf.is_empty() {
                return Ok(0);
            }
            let take = left.min(buf.len());
            buf[..take].copy_from_slice(&self.reply[self.cursor..self.cursor + take]);
            self.cursor += take;
            Ok(take)
        }
        fn write_all(&mut self, buf: &[u8]) -> Result<()> {
            self.written.extend_from_slice(buf);
            Ok(())
        }
        fn flush(&mut self) -> Result<()> {
            Ok(())
        }
        fn clear_input(&mut self) -> Result<()> {
            self.cursor = 0;
            Ok(())
        }
        fn set_baud(&mut self, _baud: u32) -> Result<()> {
            Ok(())
        }
        fn set_control_lines(&mut self, _asserted: bool) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_read16_frame_counts_words_not_bytes() {
        let frame = Frame::for_bytes(OPCODE_READ16, 0x0020_0000, 16).expect("even byte count");
        assert_eq!(frame.count, 8);
        assert_eq!(
            frame.to_bytes(),
            [0xA2, 0x00, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x08]
        );
    }

    #[test]
    fn a_read32_frame_counts_dwords() {
        let frame = Frame::for_bytes(OPCODE_READ32, 0x0020_0000, 16).expect("dword multiple");
        assert_eq!(frame.count, 4);
    }

    #[test]
    fn a_half_unit_read_is_refused() {
        assert!(Frame::for_bytes(OPCODE_READ32, 0, 6).is_err());
        assert!(Frame::for_bytes(OPCODE_READ16, 0, 3).is_err());
    }

    #[test]
    fn read16_returns_memory_order_from_a_pair_swapped_wire() {
        // Vector table on the wire is pair-swapped; a read must undo that.
        let wire: Vec<u8> = vec![0xf0, 0x18, 0xe5, 0x9f];
        let mut reply = vec![0xA2, 0x00, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02];
        reply.extend_from_slice(&wire);
        let mut brom = MtkBootRom::new(Canned::new(reply));
        let data = brom.read16(0x0020_0000, 4).expect("read the vector table");
        assert_eq!(data, vec![0x18, 0xf0, 0x9f, 0xe5]);
    }

    #[test]
    fn a_wrong_echo_is_a_framing_error_not_a_retry() {
        let mut reply = vec![0xA2, 0x00, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02];
        reply[8] = 0x03; // wrong count echoed back
        let mut brom = MtkBootRom::new(Canned::new(reply));
        let err = brom.read16(0x0020_0000, 4).expect_err("echo mismatch");
        assert!(matches!(err, Error::Framing { .. }));
        assert!(err.is_fatal());
    }

    #[test]
    fn an_echo_mismatch_does_not_claim_to_be_at_offset_zero() {
        // The echo is a wire problem with no position in an image. Reporting
        // `(at 0x0)` would point the reader at the start of a file.
        let mut reply = vec![0xA2, 0x00, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02];
        reply[8] = 0x03;
        let mut brom = MtkBootRom::new(Canned::new(reply));
        let err = brom.read16(0x0020_0000, 4).expect_err("echo mismatch");
        let text = format!("{err}");
        assert!(!text.contains("0x0"), "got {text}");
        assert!(!text.contains("at "), "got {text}");
    }

    #[test]
    fn sync_accepts_the_four_documented_replies() {
        let mut reply = vec![0x5F];
        reply.extend_from_slice(&[0xF5, 0xAF, 0xFA]);
        let mut brom = MtkBootRom::new(Canned::new(reply));
        brom.sync().expect("the documented sync sequence");
    }

    #[test]
    fn sync_rejects_a_complemented_reply_it_does_not_know() {
        // 0x60 is not 0x5F; the loop must time out rather than accept it.
        let mut brom = MtkBootRom::new(Canned::new(vec![0x60]));
        let err = brom.sync().expect_err("wrong first reply");
        assert!(matches!(err, Error::Timeout { .. }));
    }

    /// Assert a sync framing failure carries no position in an image.
    ///
    /// A probe answered with the wrong byte means the line is desynchronised;
    /// the device is not "at" any offset. A fabricated `(at 0x0)` would send
    /// the reader looking at the first byte of a firmware image.
    fn assert_reports_no_position(err: Error, probe: u8) {
        let Error::Framing { what, offset } = err else {
            panic!("expected Error::Framing for probe 0x{probe:02X}, got {err:?}");
        };
        assert!(
            what.contains(&format!("0x{probe:02X}")),
            "the error must name the probe that disagreed, got {what}"
        );
        assert_eq!(
            offset, None,
            "probe 0x{probe:02X} disagreed on the wire, which is not a byte at offset 0: {what}"
        );
    }

    #[test]
    fn a_sync_probe_mismatch_does_not_claim_to_be_at_offset_zero() {
        // 0x0A must draw 0xF5. A chip answering 0x0A is echoing the probe
        // rather than replying to it, so this is a wire failure with no
        // position in an image even though the probe is the second byte.
        let mut brom = MtkBootRom::new(Canned::new(vec![BOOT_REPLY[0], BOOT_SYNC[1]]));
        let err = brom
            .sync()
            .expect_err("0x0A must not accept its own echo as the reply");
        assert_reports_no_position(err, BOOT_SYNC[1]);
    }

    #[test]
    fn every_probe_byte_that_disagrees_reports_no_position() {
        // Probes 2..4 draw 0xF5, 0xAF and 0xFA. Answer each with its own probe
        // byte (0x0A, 0x50, 0x05) - a plausible echo, still wrong - and check
        // that whichever probe fails reports no position.
        for (index, probe) in BOOT_SYNC.iter().enumerate().skip(1) {
            let mut reply = vec![BOOT_REPLY[0]];
            reply.extend_from_slice(&BOOT_REPLY[1..index]);
            reply.push(*probe);
            let mut brom = MtkBootRom::new(Canned::new(reply));
            let err = brom
                .sync()
                .expect_err("an echoed probe byte is not the documented reply");
            assert_reports_no_position(err, *probe);
        }
    }
}
