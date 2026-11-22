//! One device session over an already-open transport: the path the GUI drives.
//!
//! The CLI spells its write sequence out in `m241-cli/src/fw.rs` because each
//! step there has its own console output, retry policy and `--json` document.
//! The GUI needs the same sequence with progress reporting and no console, so
//! the sequence lives here once and the GUI calls it. Both paths still reach
//! flash through [`DownloadAgent`]: this module adds no write of its own, and
//! every acceptance rule (`check_layout`, `check_image`, `verify_device`,
//! `verify_unit_markers`) is the one in [`crate::da`].
//!
//! The functions take the transport by value and consume it. A session is one
//! conversation with the unit; handing the same port to two sessions is the
//! mistake this shape makes impossible.
//!
//! # What this module does not do
//!
//! It does not open a port, resolve a path, read a file or build a
//! [`Confirmation`]. A caller that has none of those has not reached the write
//! step yet. In particular, a typed `WRITE` remains the GUI's own
//! acknowledgement: [`Confirmation::granted`] is constructed by the screen after
//! the user typed the word, and [`FlashRequest::confirmation`] is the only
//! confirmation this module accepts.

use std::time::Duration;

use m241_core::protocol::brom::{BASE_ADDR_FIRMWARE, DEFAULT_BOOTROM_BAUD, DUMP_CHUNK, MtkBootRom};
use m241_core::protocol::pmtk::{
    DEFAULT_REPLY_TIMEOUT, DEFAULT_STREAM_WINDOW, EntryTiming, NMEA_BAUD, PmtkClient, StreamState,
};
use m241_core::protocol::port::Transport;
use m241_core::{Error, Result};

use crate::da::{
    Confirmation, DaDigest, DaLoadId, DaLoader, DownloadAgent, FLASH_WRITE_LENGTH,
    FLASH_WRITE_START, PROBE_ADDRESSES, PendingWrite, VECTOR, VendorImage, WriteReport,
    check_da_image_against, read_candidates,
};

/// Bytes a GUI dump reads: the shipped firmware range.
///
/// The same constant the write uses, not a second number: a dump taken for the
/// backup §2.3 requires must be the span `verify_device` will compare.
pub const FIRMWARE_DUMP_LENGTH: usize = FLASH_WRITE_LENGTH;

/// How long the unit is given to boot its firmware after `0xD9` before the
/// BootROM re-entry probe.
///
/// **Intended** to be the CLI's value (`BOOTROM_REENTRY_SETTLE`), but the two are
/// independent literals: this one and `m241-cli/src/fw.rs`'s must be kept equal
/// by hand, and a change to one does not change the other. A shared constant
/// would be better; until then, "the same value" is a convention, not a fact the
/// compiler enforces.
///
/// ⚠️ 未検証 for this combination: the CLI's write route has run on hardware
/// (docs/dev/05-verification.md), but a GUI-driven write and read-back have not.
pub const REENTRY_SETTLE: Duration = Duration::from_millis(250);

/// Where a session has got to, for a progress line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Opening the BootROM: the NMEA probe and `$PMTK180`, or `--assume-brom`.
    Entering,
    /// Reading the firmware range through the BootROM.
    Reading,
    /// Comparing the unit with the backup before any erase.
    Comparing,
    /// Writing the DA into the chip's RAM.
    LoadingDa,
    /// Programming the range (`0xD3` + `0xD5`).
    Writing,
    /// Restarting with `0xD9` and reading the range back.
    ReadingBack,
}

/// What a running session reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A stage began.
    Stage(Stage),
    /// Bytes read by a dump.
    Dump { read: usize, total: usize },
    /// The DA answered and named itself.
    DaLoaded { version: u16, region: u8 },
    /// Packets acknowledged by the DA, and the bytes they cover.
    Write {
        packets: usize,
        bytes: usize,
        total: usize,
    },
}

/// Put an open transport where a BootROM read works.
///
/// `assume_brom` is the caller's choice, the GUI's `--assume-brom` equivalent:
/// the port is opened at 115200 and the four-probe sync is tried directly.
/// Otherwise the safe route runs: probe NMEA once, and only a streaming
/// firmware is handed over with `$PMTK180`; definitive silence is treated as
/// "already in the BootROM", and an unclear line is a refusal. Nothing here
/// asserts DTR/RTS.
fn enter_bootrom<T: Transport>(mut transport: T, assume_brom: bool) -> Result<T> {
    if assume_brom {
        transport.set_baud(DEFAULT_BOOTROM_BAUD)?;
        return Ok(transport);
    }
    let mut client = PmtkClient::new(transport)
        .with_reply_timeout(DEFAULT_REPLY_TIMEOUT)
        .with_stream_window(DEFAULT_STREAM_WINDOW)
        .with_entry_timing(EntryTiming::real());
    match client.probe_stream()? {
        StreamState::Streaming => {
            client.enter_bootrom_after(StreamState::Streaming)?;
            Ok(client.into_transport())
        }
        StreamState::Silent => {
            let mut transport = client.into_transport();
            transport.set_baud(DEFAULT_BOOTROM_BAUD)?;
            Ok(transport)
        }
        StreamState::Unknown => Err(Error::Safety {
            reason: String::from(
                "bytes arrived but no complete NMEA sentence did, so the line state is not \
                 usable; reconnect the cable and retry, or select the already-in-BootROM route",
            ),
        }),
    }
}

/// Read `length` bytes from [`BASE_ADDR_FIRMWARE`] in [`DUMP_CHUNK`] frames.
fn read_flash<T: Transport>(
    rom: &mut MtkBootRom<T>,
    length: usize,
    on_event: &mut dyn FnMut(Event),
) -> Result<Vec<u8>> {
    let mut image: Vec<u8> = Vec::with_capacity(length);
    while image.len() < length {
        let want = DUMP_CHUNK.min(length - image.len());
        let offset = u32::try_from(image.len()).map_err(|_| {
            Error::State(format!(
                "dump offset {} does not fit the 32-bit BootROM address space",
                image.len()
            ))
        })?;
        let address = BASE_ADDR_FIRMWARE.checked_add(offset).ok_or_else(|| {
            Error::State(format!(
                "BootROM read of {length} bytes runs past the 32-bit address space"
            ))
        })?;
        let chunk = rom.read16(address, want)?;
        image.extend_from_slice(&chunk);
        on_event(Event::Dump {
            read: image.len(),
            total: length,
        });
    }
    Ok(image)
}

/// Enter the BootROM and sync, without reading anything.
///
/// The screen's "is the unit in the BootROM now?" action: it answers whether the
/// four-probe sync works over the chosen entry route, which is what a person
/// needs after a failed write. It sends no DA command and touches no flash.
///
/// # Errors
///
/// The entry refusal, or a sync timeout.
pub fn probe_bootrom<T: Transport>(transport: T, assume_brom: bool) -> Result<()> {
    let transport = enter_bootrom(transport, assume_brom)?;
    let mut rom = MtkBootRom::new(transport);
    rom.sync()
}

/// Read the firmware range off the unit. Read-only: no erase, no program.
///
/// # Errors
///
/// The BootROM entry refusal, a sync timeout, or a framing error from a read.
pub fn dump<T: Transport>(
    transport: T,
    assume_brom: bool,
    on_event: &mut dyn FnMut(Event),
) -> Result<Vec<u8>> {
    on_event(Event::Stage(Stage::Entering));
    let transport = enter_bootrom(transport, assume_brom)?;
    on_event(Event::Stage(Stage::Reading));
    let mut rom = MtkBootRom::new(transport);
    rom.sync()?;
    read_flash(&mut rom, FIRMWARE_DUMP_LENGTH, on_event)
}

/// Everything one write needs, after the caller has read every file.
#[derive(Debug)]
pub struct FlashRequest<'a> {
    /// The user's own dump: the unit is compared with it, and the strict
    /// acceptance rule diffs the image against it.
    pub backup: &'a [u8],
    /// The image to write on the strict path.
    pub image: &'a [u8],
    /// The pinned vendor DA, checked here before a byte goes out.
    pub da: &'a [u8],
    /// The digest the DA file must have. Production callers pass
    /// [`DaDigest::vendor`]; tests use the simulator feature's token so the
    /// whole write path can run against a fake DA without the vendor binary.
    pub da_digest: DaDigest,
    /// The typed acknowledgement the user gave.
    pub confirmation: Confirmation,
    /// `--recover`: write the backup itself and skip the whole-unit compare,
    /// keeping the vector and family markers.
    pub recover: bool,
    /// Take the `--assume-brom` entry route.
    pub assume_brom: bool,
    /// The vendor token when `--allow-vendor` applies.
    pub vendor: Option<VendorImage<'a>>,
}

/// Check the DA without opening a port, so a bad file is refused first.
///
/// # Errors
///
/// [`Error::Safety`] when the file is not the pinned vendor DA.
pub fn check_da(da: &[u8]) -> Result<()> {
    check_da_image_against(da, DaDigest::vendor())
}

/// The image this request writes, for the read-back comparison.
fn written_image<'a>(request: &FlashRequest<'a>) -> &'a [u8] {
    match request.vendor {
        Some(vendor) => vendor.image(),
        None if request.recover => request.backup,
        None => request.image,
    }
}

/// One `0xD3` + `0xD5` attempt on the strict, recovery or vendor path.
///
/// Every arm reports progress: a recovery write is the moment the user most needs
/// to see that packets are moving, and a bar that stays at 0 % because the sink
/// was only wired to the strict arm is a display that lies about the state.
fn write_once<T: Transport>(
    da: &mut DownloadAgent<T>,
    request: &FlashRequest<'_>,
    on_event: &mut dyn FnMut(Event),
) -> Result<PendingWrite> {
    let total = written_image(request).len();
    let mut progress = |packets: usize, bytes: usize| {
        on_event(Event::Write {
            packets,
            bytes,
            total,
        });
    };
    match request.vendor {
        Some(vendor) => da.write_vendor_with_progress(
            request.confirmation,
            request.backup,
            vendor,
            &mut progress,
        ),
        None if request.recover => da.write_with_progress(
            request.confirmation,
            request.backup,
            request.backup,
            &mut progress,
        ),
        None => da.write_with_progress(
            request.confirmation,
            request.backup,
            request.image,
            &mut progress,
        ),
    }
}

/// Restart with `0xD9`, then re-enter the BootROM over NMEA and `$PMTK180`.
///
/// This is the CLI's read-back route, including the settle and the one-probe
/// hand-over. It never asserts DTR/RTS.
fn reenter_for_read_back<T: Transport>(transport: T) -> Result<T> {
    std::thread::sleep(REENTRY_SETTLE);
    let mut transport = transport;
    transport.set_baud(NMEA_BAUD)?;
    // The DA conversation leaves bytes queued; a stale byte must not be read as
    // the restarted firmware's stream.
    transport.clear_input()?;
    let mut client = PmtkClient::new(transport)
        .with_reply_timeout(DEFAULT_REPLY_TIMEOUT)
        .with_stream_window(DEFAULT_STREAM_WINDOW)
        .with_entry_timing(EntryTiming::real());
    match client.probe_stream()? {
        StreamState::Streaming => {
            client.enter_bootrom_after(StreamState::Streaming)?;
            Ok(client.into_transport())
        }
        StreamState::Silent | StreamState::Unknown => Err(Error::State(String::from(
            "the unit did not stream NMEA after the restart, so the BootROM could not be \
             re-entered and the read-back was not performed. The write itself finished; do not \
             assume it failed. Use the GUI's restart and BootROM re-entry actions, then read the \
             unit with the dump step to see what it holds",
        ))),
    }
}

/// Append a restart note to an error, keeping its variant where one exists.
fn with_note(err: Error, note: &str) -> Error {
    match err {
        Error::Framing { what, offset } => Error::Framing {
            what: format!("{what}. {note}"),
            offset,
        },
        Error::Timeout { what, timeout } => Error::Timeout {
            what: format!("{what}. {note}"),
            timeout,
        },
        Error::Rejected { what } => Error::Rejected {
            what: format!("{what}. {note}"),
        },
        Error::Safety { reason } => Error::Safety {
            reason: format!("{reason}. {note}"),
        },
        other => Error::State(format!("{other}. {note}")),
    }
}

/// Write the range and read it back. The only GUI write route.
///
/// The sequence is the CLI's: DA digest check, BootROM entry, sync, whole-unit
/// comparison against the backup (or the markers in recovery), DA load, write,
/// `0xD9`, re-entry, read-back comparison. On a write error the unit is
/// restarted with `0xD9` (best effort) and the error says so; this module does
/// **not** retry on its own, and recovery is an explicit second call with
/// [`FlashRequest::recover`].
///
/// # Errors
///
/// [`Error::Safety`] before a port byte is written for a bad DA, a confirmation
/// token that was never granted, a unit that does not hold the backup, or an
/// image outside the backup diff. Once `0xD3` has gone out, the DA's errors
/// carry [`crate::da::PartialWrite::recovery`].
pub fn flash<T: Transport>(
    transport: T,
    request: &FlashRequest<'_>,
    on_event: &mut dyn FnMut(Event),
) -> Result<WriteReport> {
    check_da_image_against(request.da, request.da_digest.clone())?;
    on_event(Event::Stage(Stage::Entering));
    let transport = enter_bootrom(transport, request.assume_brom)?;
    let mut rom = MtkBootRom::new(transport);
    rom.sync()?;
    on_event(Event::Stage(Stage::Comparing));
    if request.recover {
        DownloadAgent::verify_unit_markers(&mut rom)?;
    } else {
        DownloadAgent::verify_device(&mut rom, request.backup)?;
    }
    on_event(Event::Stage(Stage::LoadingDa));
    let mut loader = DaLoader::new(rom.transport_mut());
    let id = loader.load_against(request.confirmation, request.da, request.da_digest.clone())?;
    on_event(Event::DaLoaded {
        version: id.version,
        region: id.region,
    });
    on_event(Event::Stage(Stage::Writing));
    let mut da = DownloadAgent::new(rom.into_transport());
    let pending = match write_once(&mut da, request, on_event) {
        Ok(pending) => pending,
        Err(err) => {
            let restart = da.restart();
            let note = match restart {
                Ok(()) => String::from(
                    "0xD9 was sent to restart the unit; it has no reply, so the restart cannot \
                     be confirmed. Nothing here is retried automatically",
                ),
                Err(restart_err) => format!(
                    "the automatic 0xD9 restart failed ({restart_err}); the DA may still be \
                     running in RAM. Use the GUI's restart or the already-in-BootROM route"
                ),
            };
            return Err(with_note(err, &note));
        }
    };
    on_event(Event::Stage(Stage::ReadingBack));
    pending.verify_written(
        da,
        written_image(request),
        FLASH_WRITE_START,
        reenter_for_read_back,
    )
}

/// One `0xD6` page at each candidate address, as the GUI shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbePage {
    /// The address asked for.
    pub address: u32,
    /// The first four bytes, when the page was read.
    pub first_bytes: Option<[u8; 4]>,
    /// Whether those bytes are the M-241 vector.
    pub matches_vector: bool,
    /// Why the page could not be read, when it could not.
    pub error: Option<String>,
}

/// What `fw probe-da` found, without the console document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeDaReport {
    /// The DA the chip answered with.
    pub id: DaLoadId,
    /// One entry per candidate address, in [`PROBE_ADDRESSES`] order.
    pub pages: [ProbePage; PROBE_ADDRESSES.len()],
    /// The address that holds the image, when exactly one matched.
    pub settled: Option<u32>,
}

/// The single address a probe settles on, or `None` when 0 or 2+ pages matched.
///
/// Pulled out of [`probe_da`] so the rule — exactly one match settles the range,
/// none or several do not — is testable without a transport. A probe that matched
/// two pages must not pick one: the whole point of the probe is that the evidence
/// is ambiguous.
fn settled_address(matches: &[u32]) -> Option<u32> {
    match matches {
        [only] => Some(*only),
        _ => None,
    }
}

/// Load the DA into RAM and read both candidate addresses. Writes no flash.
///
/// `confirmation` is the GUI's typed `PROBE` acknowledgement, constructed by the
/// screen; the DA loader refuses an ungranted token before writing RAM.
/// `da_digest` is the file's digest token, as in [`flash`]: production callers
/// pass [`DaDigest::vendor`], and the simulator feature's token makes the probe
/// path testable against a fake DA.
///
/// # Errors
///
/// [`Error::Safety`] for a bad DA or an ungranted confirmation, then the BootROM
/// entry, sync and load errors. `0xD3` / `0xD4` / `0xD5` / `0xD9` are never sent.
pub fn probe_da<T: Transport>(
    transport: T,
    da: &[u8],
    da_digest: DaDigest,
    confirmation: Confirmation,
    assume_brom: bool,
    on_event: &mut dyn FnMut(Event),
) -> Result<ProbeDaReport> {
    check_da_image_against(da, da_digest.clone())?;
    on_event(Event::Stage(Stage::Entering));
    let transport = enter_bootrom(transport, assume_brom)?;
    let mut rom = MtkBootRom::new(transport);
    rom.sync()?;
    on_event(Event::Stage(Stage::LoadingDa));
    let mut loader = DaLoader::new(rom.transport_mut());
    let id = loader.load_against(confirmation, da, da_digest)?;
    on_event(Event::DaLoaded {
        version: id.version,
        region: id.region,
    });
    let mut da_session = DownloadAgent::new(rom.into_transport());
    let results = read_candidates(&mut da_session);
    let mut pages = Vec::with_capacity(PROBE_ADDRESSES.len());
    for (address, result) in PROBE_ADDRESSES.into_iter().zip(results) {
        pages.push(match result {
            Ok(page) => {
                let first: [u8; 4] = page.get(..VECTOR.len()).map_or([0; 4], |head| {
                    let mut bytes = [0u8; 4];
                    bytes.copy_from_slice(head);
                    bytes
                });
                ProbePage {
                    address,
                    first_bytes: Some(first),
                    matches_vector: first == VECTOR,
                    error: None,
                }
            }
            Err(err) => ProbePage {
                address,
                first_bytes: None,
                matches_vector: false,
                error: Some(err.to_string()),
            },
        });
    }
    let matches: Vec<u32> = pages
        .iter()
        .filter(|page| page.matches_vector)
        .map(|page| page.address)
        .collect();
    let settled = settled_address(&matches);
    let pages: [ProbePage; PROBE_ADDRESSES.len()] = pages.try_into().map_err(|_| {
        Error::State(String::from(
            "the DA probe returned a different number of pages than addresses",
        ))
    })?;
    Ok(ProbeDaReport { id, pages, settled })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    use crate::da::{DA_IMAGE_HEAD, DA_IMAGE_SIZE, PACKET_SIZE};
    use crate::simulator::FakeDa;

    /// A synthetic stock V1.13 image: the header fields the identifier reads,
    /// the stock epoch constants and the release marker. Everything else is
    /// erased flash, which is enough for the real planner, the real acceptance
    /// rule and the real device comparison.
    fn stock_image() -> Vec<u8> {
        use m241_core::firmware::PROFILE_V1_13;
        use m241_core::firmware::image::{
            BUILD_NUMBER_OFFSET, DEVICE_TYPE_OFFSET, EPOCH_CONSTANT_OFFSETS, FAMILY, FAMILY_OFFSET,
            FW_SIZE_OFFSET, GPS_EPOCH_UNIX,
        };
        let profile = PROFILE_V1_13;
        let mut data = vec![0xFF_u8; profile.size];
        data[..VECTOR.len()].copy_from_slice(&VECTOR);
        // Header fields are C strings: the bytes after them are NUL, not 0xFF.
        use m241_core::firmware::image::FAMILY_SIZE;
        data[FAMILY_OFFSET..FAMILY_OFFSET + FAMILY_SIZE].fill(0);
        data[FAMILY_OFFSET..FAMILY_OFFSET + FAMILY.len()].copy_from_slice(FAMILY.as_bytes());
        data[DEVICE_TYPE_OFFSET..].fill(0);
        let device = profile.device_type.as_bytes();
        data[DEVICE_TYPE_OFFSET..DEVICE_TYPE_OFFSET + device.len()].copy_from_slice(device);
        let marker = profile.name.as_bytes();
        data[profile.version_marker..profile.version_marker + marker.len()].copy_from_slice(marker);
        let stock = GPS_EPOCH_UNIX.to_le_bytes();
        for offset in EPOCH_CONSTANT_OFFSETS {
            data[offset..offset + stock.len()].copy_from_slice(&stock);
        }
        let declared = u32::try_from(profile.size).unwrap_or(0).to_le_bytes();
        data[FW_SIZE_OFFSET..FW_SIZE_OFFSET + declared.len()].copy_from_slice(&declared);
        data[BUILD_NUMBER_OFFSET..BUILD_NUMBER_OFFSET + 2].copy_from_slice(&67_u16.to_le_bytes());
        data
    }

    fn patched(stock: &[u8]) -> Vec<u8> {
        let image = m241_core::firmware::FirmwareImage::new(stock).expect("stock image parses");
        m241_core::firmware::plan(&image)
            .expect("the stock image is planned")
            .apply(stock)
            .expect("the plan applies")
    }

    /// A DA fixture the simulator feature can digest-check.
    ///
    /// The vendor DA binary is not in the repository (AGENTS.md §6), so the
    /// write path's tests use the test-only digest token. The file gate itself
    /// is covered by the CLI tests and `da.rs`.
    fn fake_da() -> (Vec<u8>, DaDigest) {
        let mut image = vec![0u8; DA_IMAGE_SIZE];
        image[..DA_IMAGE_HEAD.len()].copy_from_slice(&DA_IMAGE_HEAD);
        let digest = DaDigest::for_test_da(&m241_core::firmware::sha256_hex(&image));
        (image, digest)
    }

    /// Share one fake chip across several sessions in one test.
    #[derive(Debug, Clone)]
    struct SharedDa(Rc<RefCell<FakeDa>>);

    impl Transport for SharedDa {
        fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
            self.0.borrow_mut().read(buf)
        }
        fn write_all(&mut self, buf: &[u8]) -> Result<()> {
            self.0.borrow_mut().write_all(buf)
        }
        fn flush(&mut self) -> Result<()> {
            self.0.borrow_mut().flush()
        }
        fn clear_input(&mut self) -> Result<()> {
            self.0.borrow_mut().clear_input()
        }
        fn set_baud(&mut self, baud: u32) -> Result<()> {
            self.0.borrow_mut().set_baud(baud)
        }
        fn set_control_lines(&mut self, asserted: bool) -> Result<()> {
            self.0.borrow_mut().set_control_lines(asserted)
        }
    }

    fn request<'a>(
        backup: &'a [u8],
        image: &'a [u8],
        da: &'a [u8],
        digest: DaDigest,
        recover: bool,
    ) -> FlashRequest<'a> {
        FlashRequest {
            backup,
            image,
            da,
            da_digest: digest,
            confirmation: Confirmation::granted(),
            recover,
            assume_brom: true,
            vendor: None,
        }
    }

    #[test]
    fn a_dump_reads_the_firmware_range_and_never_pulses_the_control_lines() {
        let stock = stock_image();
        let fake = Rc::new(RefCell::new(FakeDa::with_firmware(&stock)));
        let mut last = (0, 0);
        let read = dump(SharedDa(fake.clone()), true, &mut |event| {
            if let Event::Dump { read, total } = event {
                last = (read, total);
            }
        })
        .expect("a dump from a BootROM-resident chip succeeds");
        assert_eq!(read, stock, "the dump is not the flash image");
        assert_eq!(last, (FIRMWARE_DUMP_LENGTH, FIRMWARE_DUMP_LENGTH));
        assert!(
            fake.borrow().control_lines().is_empty(),
            "the dump pulsing DTR/RTS would leave a streaming unit unresponsive"
        );
    }

    #[test]
    fn dump_patch_write_and_read_back_agree() {
        let stock = stock_image();
        let target = patched(&stock);
        let (da, digest) = fake_da();
        let fake = Rc::new(RefCell::new(FakeDa::with_firmware(&stock)));

        let backup = dump(SharedDa(fake.clone()), true, &mut |_| {}).expect("dump");
        assert_eq!(backup, stock);

        let mut peak_packets = 0usize;
        let mut peak_bytes = 0usize;
        let mut total = 0usize;
        let report = flash(
            SharedDa(fake.clone()),
            &request(&backup, &target, &da, digest.clone(), false),
            &mut |event| {
                if let Event::Write {
                    packets,
                    bytes,
                    total: announced,
                } = event
                {
                    peak_packets = peak_packets.max(packets);
                    peak_bytes = peak_bytes.max(bytes);
                    total = announced;
                }
            },
        )
        .expect("the write and its read-back must agree");

        assert_eq!(report.length, FLASH_WRITE_LENGTH);
        assert_eq!(report.verified, FLASH_WRITE_LENGTH);
        assert_eq!(report.packets, FLASH_WRITE_LENGTH.div_ceil(PACKET_SIZE));
        assert_eq!(
            peak_packets, report.packets,
            "the progress line saw every packet"
        );
        assert_eq!(peak_bytes, target.len(), "the progress line saw every byte");
        assert_eq!(total, target.len());
        assert_eq!(
            fake.borrow().memory(),
            &target[..],
            "the flash holds the image"
        );
    }

    #[test]
    fn a_read_back_that_disagrees_is_reported_and_the_backup_restores_the_unit() {
        let stock = stock_image();
        let target = patched(&stock);
        let (da, digest) = fake_da();
        let fake = Rc::new(RefCell::new(FakeDa::with_firmware(&stock)));
        let backup = dump(SharedDa(fake.clone()), true, &mut |_| {}).expect("dump");

        // A half-word that failed to program: the packets are all accepted, the
        // flash holds one wrong byte, and only the read-back notices.
        fake.borrow_mut().damaging_readback(0x40000, 0xFF);
        let error = flash(
            SharedDa(fake.clone()),
            &request(&backup, &target, &da, digest.clone(), false),
            &mut |_| {},
        )
        .expect_err("a read-back that disagrees must not be a success");
        assert!(
            error.to_string().contains("read-back differs"),
            "the failure does not name the read-back: {error}"
        );

        // `--recover`: the backup itself, markers only, whole-unit compare
        // skipped because the unit is now different by design.
        let report = flash(
            SharedDa(fake.clone()),
            &request(&backup, &stock, &da, digest.clone(), true),
            &mut |_| {},
        )
        .expect("recovery with the user's own dump succeeds");
        assert_eq!(report.verified, FLASH_WRITE_LENGTH);
        assert_eq!(
            fake.borrow().memory(),
            &backup[..],
            "the unit does not hold the backup after recovery"
        );
    }

    #[test]
    fn a_unit_that_does_not_hold_the_backup_is_refused_before_any_write_packet() {
        let stock = stock_image();
        let other = {
            let mut image = stock.clone();
            image[0x30000] ^= 0x01;
            image
        };
        let target = patched(&stock);
        let (da, digest) = fake_da();
        let fake = Rc::new(RefCell::new(FakeDa::with_firmware(&other)));
        let error = flash(
            SharedDa(fake.clone()),
            &request(&stock, &target, &da, digest, false),
            &mut |_| {},
        )
        .expect_err("a unit that differs from the backup must be refused");
        assert!(
            error.to_string().contains("differs from the backup"),
            "the refusal does not name the device comparison: {error}"
        );
        assert_eq!(
            fake.borrow().packets().len(),
            0,
            "a packet went out before the unit was compared with the backup"
        );
    }

    #[test]
    fn a_probe_settles_only_on_exactly_one_matching_page() {
        // The rule the `fw probe-da` report turns into "the image is at 0x…".
        // Zero matches and two matches are both "cannot tell", and picking one
        // of two would be a guess dressed as a measurement.
        assert_eq!(settled_address(&[]), None);
        assert_eq!(
            settled_address(&[PROBE_ADDRESSES[1]]),
            Some(PROBE_ADDRESSES[1])
        );
        assert_eq!(
            settled_address(&[PROBE_ADDRESSES[0], PROBE_ADDRESSES[1]]),
            None
        );
    }

    #[test]
    fn probe_da_reads_both_candidate_pages_through_the_fake_and_writes_no_flash() {
        let stock = stock_image();
        let (da, digest) = fake_da();
        let fake = Rc::new(RefCell::new(FakeDa::with_firmware(&stock)));
        let report = probe_da(
            SharedDa(fake.clone()),
            &da,
            digest,
            Confirmation::granted(),
            true,
            &mut |_| {},
        )
        .expect("the DA probe succeeds against the fake");
        assert_eq!(report.id.version, 0x0308);
        assert_eq!(
            report.settled,
            Some(PROBE_ADDRESSES[0]),
            "the fake holds the image at the DA's own view of offset 0"
        );
        assert_eq!(report.pages[0].first_bytes, Some(VECTOR));
        assert!(report.pages[0].matches_vector);
        assert!(
            !report.pages[1].matches_vector,
            "the second candidate is past the fixture and must not match"
        );
        assert!(
            fake.borrow().packets().is_empty(),
            "a probe sent a 0xD3/0xD5 packet"
        );
    }

    #[test]
    fn probe_bootrom_confirms_the_sync_without_touching_the_device() {
        let stock = stock_image();
        let fake = Rc::new(RefCell::new(FakeDa::with_firmware(&stock)));
        probe_bootrom(SharedDa(fake.clone()), true).expect("the fake answers the four sync probes");
        assert!(fake.borrow().packets().is_empty(), "a packet was sent");
        assert!(
            fake.borrow().control_lines().is_empty(),
            "the BootROM probe pulsed DTR/RTS"
        );
    }
}
