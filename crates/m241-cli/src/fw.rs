//! `m241 fw dump`, `m241 fw patch`, `m241 fw flash`, `m241 fw probe-da` and
//! `m241 fw da-restart`.
//!
//! The halves of the repair, deliberately asymmetric. [`dump`] touches the
//! device and is read-only: it reads the image twice, comparing hashes, so a
//! read that silently truncated cannot be mistaken for success. [`patch`] never
//! opens a port at all — it rewrites a file on this machine, and writing to the
//! device is a different command. [`flash`] is that command and the only path in
//! this module that writes the device: everything decidable without a port — the
//! confirmation, the layout, the image acceptance and the DA file — is refused
//! before one is opened. [`probe_da`] loads the DA into the chip's RAM
//! and reads two candidate addresses through it; it writes no flash either, and
//! exists because the address `fw flash` refuses on cannot be settled any other
//! way. [`da_restart`] sends the DA's one-byte `0xD9` to free a unit that a
//! failed DA phase left in RAM; it also writes no flash.
//!
//! # Why the second pass exists
//!
//! One read that returns 406,620 plausible-looking bytes is not evidence that
//! the flash was read correctly: a marginal cable can drop the last few hundred
//! bytes and the result still parses as an image. Two reads, from two boots,
//! agreeing byte for byte is the cheapest evidence available without a reference
//! hash — a user's own unit hashes differently from every other unit.

use m241_core::firmware::image::{
    EFFECTIVE_EPOCH_OFFSET, FW_SIZE_OFFSET, FirmwareImage, GPS_EPOCH_UNIX, GPS_EPOCH_UNIX_WNRO,
};
use m241_core::firmware::profiles::{
    Evidence, PROFILE_V1_12, PROFILE_V1_13, VERSION_MARKER_LEN, identify,
};
use m241_core::firmware::{self, sha256_hex};
use m241_core::protocol::brom::{BASE_ADDR_FIRMWARE, DEFAULT_BOOTROM_BAUD, DUMP_CHUNK, MtkBootRom};
use m241_core::protocol::pmtk::{NMEA_BAUD, PmtkClient, StreamState};
use m241_core::protocol::port::Transport;
use m241_core::{Error, FirmwareProfile, PatchPlan, Result};
use m241_flash::da::{
    Confirmation, DaDigest, DaLoadId, DaLoader, DownloadAgent, FLASH_WRITE_START, PROBE_ADDRESSES,
    PROBE_PAGE_SIZE, PendingWrite, ReadBackPolicy, UnverifiedImage, UnverifiedWritePlan, VECTOR,
    VENDOR_PER_UNIT_DATA_BYTES, VendorImage, WritePlan, WriteReport, accept_unverified_image,
    accept_vendor_image, check_da_image_against, check_layout, check_physical_image,
    read_candidates, write_plan, write_plan_unverified, write_plan_vendor,
};

use crate::Session;
use crate::cli::{DaRestartOptions, DumpOptions, FlashOptions, PatchOptions, ProbeDaOptions};
use crate::fileio;
use crate::link::{self, Opened};
use crate::output::Console;

/// High two bytes of the little-endian effective epoch constant at
/// [`EFFECTIVE_EPOCH_OFFSET`] (`0x402F4` + 2 / + 3): the low two bytes (`0x80`,
/// `0x3D`) are identical before and after the +619,315,200 s WNRO shift.
const EFFECTIVE_EPOCH_HIGH_LOW: usize = EFFECTIVE_EPOCH_OFFSET + 2;
const EFFECTIVE_EPOCH_HIGH_TOP: usize = EFFECTIVE_EPOCH_OFFSET + 3;

/// Final ASCII digit of the five-byte version marker (`V1.13` → `V1.14`,
/// `V1.12` → `V1.14`), derived from each profile's `version_marker` base.
const V1_13_VERSION_DIGIT: usize = PROFILE_V1_13.version_marker + VERSION_MARKER_LEN - 1;
const V1_12_VERSION_DIGIT: usize = PROFILE_V1_12.version_marker + VERSION_MARKER_LEN - 1;

/// The digest `fw flash` and `fw probe-da` require of `--da` (`AGENTS.md` §2.5).
///
/// A normal build has one value to hand back: [`DaDigest::vendor`], the pinned
/// vendor `DA_v3.8.4.bin` digest, and `m241_flash::da` exposes no other
/// constructor in this build, so the commands cannot be told to accept a
/// different DA. [`DaDigest::for_test_da`] does not exist here.
///
/// **This branch is not compiled by `cargo test`.** The test harness always
/// takes the `#[cfg(test)]` branch below, so a shipped-binary test of this
/// function is not possible; the shipped value is fixed at compile time and the
/// call sites below cannot pass anything else.
#[cfg(not(test))]
fn da_digest() -> DaDigest {
    DaDigest::vendor()
}

/// The same digest in a test build: the CLI fixture's own.
///
/// The vendor binary is never committed (`AGENTS.md` §6), so every CLI write test
/// drives the command with the synthetic DA in [`tests::da_fixture_bytes`].
/// Computing that fixture's digest here keeps the full comparison running — it
/// just requires a value a test can produce. The shipped binary takes the branch
/// above and never this one.
#[cfg(test)]
fn da_digest() -> DaDigest {
    use std::sync::OnceLock;
    static DIGEST: OnceLock<String> = OnceLock::new();
    let hex = DIGEST.get_or_init(|| sha256_hex(&tests::da_fixture_bytes()));
    DaDigest::for_test_da(hex)
}

/// Read the firmware off the device, twice, and save it once both reads agree.
///
/// # Errors
///
/// * [`Error::Safety`] when the output exists and `--force` was not given, or
///   when the serial line cannot be classified as streaming or silent;
/// * [`Error::Port`] when no plausible port can be opened;
/// * [`Error::Timeout`] / [`Error::Framing`] when the BootROM does not answer;
/// * [`Error::Framing`] when the two reads differ — exit code 4.
pub fn dump(options: &DumpOptions, session: Session<'_>) -> Result<()> {
    validate_length(options.length)?;
    fileio::ensure_free(&options.output, options.force)?;

    let port = link::resolve(session.links, &options.port)?;
    session
        .console
        .note(&format!("reading {} bytes from {port}", options.length));

    let first = read_once(options, session, &port)?;
    let first_digest = sha256_hex(&first);

    // The comparison only means something if the second read starts from a fresh
    // boot, which is why this is a prompt and not a silent retry.
    session.confirm.pause(
        "Power-cycle the logger now: remove the battery for about ten seconds, put it back, \
         switch the unit on, and then press Enter. The second read starts from that fresh boot.",
    )?;

    let second = read_once(options, session, &port)?;
    let second_digest = sha256_hex(&second);
    if first_digest != second_digest {
        return Err(Error::Framing {
            what: format!(
                "the two dumps differ (first sha256 {first_digest}, second {second_digest}); \
                 the flash contents are not stable, so this image must not be flashed — retry \
                 with a shorter cable and a stable USB port"
            ),
            offset: None,
        });
    }

    // Only now is there a file: a failed comparison must not leave something
    // that looks like a backup next to the real one. If header parsing then
    // fails, keep the verified capture on disk and report where it was saved.
    fileio::write(&options.output, &first)?;

    let image = match FirmwareImage::new(&first) {
        Ok(image) => image,
        Err(err) => {
            session.console.warn(&format!(
                "saved {} verified bytes to {}, but parsing the firmware header failed",
                first.len(),
                options.output.display()
            ));
            return Err(err);
        }
    };
    let profile = match m241_core::firmware::identify(&image) {
        Ok(found) => Some(found.profile),
        Err(err) => {
            session.console.warn(&format!(
                "this image matches no known profile ({err}); it was saved, but check the size \
                 and the device string before patching it"
            ));
            None
        }
    };

    print_dump_result(
        options,
        session.console,
        &port,
        &image,
        &first_digest,
        profile,
    )
}

/// Format the result of a verified `fw dump` as JSON or labelled lines.
fn print_dump_result(
    options: &DumpOptions,
    console: &dyn Console,
    port: &str,
    image: &FirmwareImage<'_>,
    digest: &str,
    profile: Option<&'static FirmwareProfile>,
) -> Result<()> {
    report_declared_size(console, image, image.len());

    if options.json {
        return console.json(&serde_json::json!({
            "port": port,
            "output": options.output.display().to_string(),
            "size": image.len(),
            "sha256": digest,
            "profile": profile.map(|profile| profile.name),
            "verified": true,
            "declared_size": image.u32_at(FW_SIZE_OFFSET),
        }));
    }
    console.result(&format!("{:<12}: {port}", "port"));
    console.result(&format!("{:<12}: {}", "size", image.len()));
    console.result(&format!("{:<12}: {digest}", "sha256"));
    console.result(&format!(
        "{:<12}: {}",
        "profile",
        profile.map_or("-", |profile| profile.name)
    ));
    console.result(&format!("{:<12}: yes (two identical reads)", "verified"));
    console.result(&format!("{:<12}: {}", "written", options.output.display()));
    Ok(())
}

/// Compare the image header's declared length against the actual file length.
///
/// Released images store `length - 4` at [`FW_SIZE_OFFSET`], so that specific
/// difference is noted as expected; any other mismatch is warned about because
/// it usually indicates a truncated or wrong-model dump.
fn report_declared_size(console: &dyn Console, image: &FirmwareImage<'_>, actual_len: usize) {
    let Some(declared) = image.u32_at(FW_SIZE_OFFSET) else {
        return;
    };
    let Ok(declared_len) = usize::try_from(declared) else {
        console.warn(&format!(
            "the header at 0x{FW_SIZE_OFFSET:X} declares {declared} bytes while the file is \
             {actual_len}; check --length and the model before patching"
        ));
        return;
    };
    if declared_len == actual_len {
        return;
    }
    if declared_len == actual_len.saturating_sub(4) {
        console.note(&format!(
            "note: the header at 0x{FW_SIZE_OFFSET:X} declares {declared} bytes while the file \
             is {actual_len}; the released images store `length - 4` there, so this is expected"
        ));
    } else {
        console.warn(&format!(
            "the header at 0x{FW_SIZE_OFFSET:X} declares {declared} bytes while the file is \
             {actual_len}; check --length and the model before patching"
        ));
    }
}

/// Reject a read length that the BootROM cannot answer.
fn validate_length(length: usize) -> Result<()> {
    if length == 0 {
        return Err(Error::State(String::from(
            "--length must be positive; a firmware image is about 400 KiB",
        )));
    }
    if !length.is_multiple_of(2) {
        // `0xA2` counts 16-bit units, so an odd length cannot be asked for.
        return Err(Error::State(format!(
            "--length {length} is odd; the BootROM reads 16-bit units"
        )));
    }
    if u32::try_from(length)
        .ok()
        .and_then(|bytes| BASE_ADDR_FIRMWARE.checked_add(bytes))
        .is_none()
    {
        return Err(Error::State(format!(
            "--length {length} overflows the 32-bit BootROM address space from \
             0x{BASE_ADDR_FIRMWARE:08X}"
        )));
    }
    Ok(())
}

/// Open, enter the BootROM, and read `options.length` bytes once.
fn read_once(options: &DumpOptions, session: Session<'_>, port: &str) -> Result<Vec<u8>> {
    let transport = enter_bootrom(options.assume_brom, session, port)?;
    let mut rom = MtkBootRom::new(transport);
    rom.sync()?;
    session
        .console
        .note("BootROM sync answered; reading the firmware image");
    read_flash(&mut rom, options.length, session.console)
}

/// Open `port` and put the transport at BootROM baud using the safe entry route.
///
/// `assume_brom` is the caller's own `--assume-brom` flag. Taking the flag
/// rather than one command's options struct is what lets `fw probe-da` reach the
/// same route without copying it.
fn enter_bootrom(assume_brom: bool, session: Session<'_>, port: &str) -> Result<Opened> {
    if assume_brom {
        // Asked to assume the chip is already listening. Nothing is asserted:
        // this path exists for a unit that a failed write left in the BootROM.
        let mut transport = session.links.open(port, NMEA_BAUD)?;
        transport.set_baud(DEFAULT_BOOTROM_BAUD)?;
        return Ok(transport);
    }

    let mut client = link::open_pmtk(session.links, port, session.timings)?;
    // One probe only. `enter_bootrom_after` takes the answer rather than probing
    // again: a second probe reads a line the first one has already started to
    // drain, and the hand-over is timing-sensitive enough that it matters.
    let state = client.probe_stream()?;
    match state {
        StreamState::Streaming => {
            session
                .console
                .note("NMEA is streaming; sending $PMTK180 to hand over to the BootROM");
            client.enter_bootrom_after(state)?;
            Ok(client.into_transport())
        }
        StreamState::Silent => {
            // Definitive silence: the firmware is not running, so the chip
            // may already be in the BootROM. The reset lines are not pulsed
            // at anything — a running unit whose stream was simply missed
            // must not be reset.
            session.console.note(
                "no NMEA stream; assuming the chip is already in the BootROM and trying the \
                 four-probe sync (pass --assume-brom to skip the probe next time)",
            );
            let mut transport = client.into_transport();
            transport.set_baud(DEFAULT_BOOTROM_BAUD)?;
            Ok(transport)
        }
        StreamState::Unknown => {
            // The tri-state exists so that "cannot tell" never collapses
            // into either answer: the sentence would be ignored and the
            // reset route would be unsafe, so the only safe move is a
            // safety refusal that says how to get a definite answer.
            Err(Error::Safety {
                reason: String::from(
                    "bytes arrived but no complete NMEA sentence did, so the line state is not \
                     usable; power-cycle the logger, reconnect the cable, and retry — or pass \
                     --assume-brom if the unit is already in the BootROM",
                ),
            })
        }
    }
}

/// Read `length` bytes sequentially from [`BASE_ADDR_FIRMWARE`].
fn read_flash(
    rom: &mut MtkBootRom<Opened>,
    length: usize,
    console: &dyn Console,
) -> Result<Vec<u8>> {
    let mut image: Vec<u8> = Vec::with_capacity(length);
    while image.len() < length {
        let want = DUMP_CHUNK.min(length - image.len());
        let address = u32::try_from(image.len())
            .ok()
            .and_then(|offset| BASE_ADDR_FIRMWARE.checked_add(offset))
            .ok_or_else(|| {
                Error::State(format!(
                    "dump offset {} overflows the 32-bit BootROM address space from \
                     0x{BASE_ADDR_FIRMWARE:08X}",
                    image.len()
                ))
            })?;
        let chunk = rom.read16(address, want)?;
        image.extend_from_slice(&chunk);
        console.progress(image.len(), length);
    }
    console.progress_done();
    Ok(image)
}

/// What `fw patch` concluded about one file.
struct PatchReport {
    /// The profile the image was identified as.
    profile: &'static m241_core::FirmwareProfile,
    /// What made the match.
    evidence: Evidence,
    /// Every byte the patch would change.
    plan: PatchPlan,
}

/// Rewrite the epoch constant and version marker in a **file**.
///
/// The device is never opened. `--dry-run` prints the plan and writes nothing,
/// applying the same refusals as a real run so it predicts whether patching
/// will succeed.
///
/// # Errors
///
/// * [`Error::Profile`] when the image matches no known release;
/// * [`Error::Firmware`] when its epoch constant is not the stock value, was
///   edited by something else, or is already patched;
/// * [`Error::Safety`] when the output would overwrite the source image;
/// * [`Error::Io`] when a file cannot be read or written.
pub fn patch(options: &PatchOptions, session: Session<'_>) -> Result<()> {
    let bytes = fileio::read(&options.source)?;
    let report = plan_patch(&bytes)?;

    if !report.plan.changes_anything() {
        // `docs/reference/02-cli.md` lists an already-patched image among the
        // images `fw patch` refuses. Refusing before `--dry-run` ensures a dry
        // run predicts the exact same outcome as a real run.
        return Err(Error::Firmware {
            message: format!(
                "the image is already patched, so there is nothing to write: {}",
                report
                    .plan
                    .no_op_reason
                    .as_deref()
                    .unwrap_or("the plan is empty")
            ),
            offset: Some(EFFECTIVE_EPOCH_OFFSET),
        });
    }

    if options.dry_run {
        if options.json {
            return session.console.json(&plan_document(&report, None));
        }
        print_plan(session.console, &report);
        session
            .console
            .note("dry run: nothing was written. Drop --dry-run to apply the plan.");
        return Ok(());
    }

    let Some(output) = &options.output else {
        // clap makes this unreachable (`required_unless_present = "dry_run"`),
        // but the handler does not lean on that: a missing destination must not
        // fall through to "write nowhere".
        return Err(Error::State(String::from(
            "no output path: pass -o FILE (or --dry-run to print the plan)",
        )));
    };
    fileio::ensure_distinct(&options.source, output)?;

    let patched = report.plan.apply(&bytes)?;
    fileio::write(output, &patched)?;

    // Read the result back through the same parser the rest of the tool uses,
    // so "the patch applied" means the file on disk, not the buffer in memory.
    let written = fileio::read(output)?;
    let image = FirmwareImage::new(&written)?;
    if !image.is_patched() {
        return Err(Error::Firmware {
            message: format!(
                "the file was written but its effective epoch constant is 0x{:08X}, not the \
                 patched 0x{GPS_EPOCH_UNIX_WNRO:08X}",
                image.effective_epoch().unwrap_or(0)
            ),
            offset: Some(EFFECTIVE_EPOCH_OFFSET),
        });
    }

    if options.json {
        return session.console.json(&plan_document(&report, Some(output)));
    }
    print_plan(session.console, &report);
    session
        .console
        .result(&format!("{:<12}: {}", "written", output.display()));
    session
        .console
        .result(&format!("{:<12}: {}", "sha256", sha256_hex(&written)));
    session.console.note(
        "the device was not touched. Write this file back with `m241 fw flash` when you are \
         ready, keeping the stock image as the backup.",
    );
    Ok(())
}

/// Read an image, identify it, and plan the patch.
fn plan_patch(bytes: &[u8]) -> Result<PatchReport> {
    let image = FirmwareImage::new(bytes)?;
    let identification = m241_core::firmware::identify(&image)?;
    let plan = firmware::plan(&image)?;
    Ok(PatchReport {
        profile: identification.profile,
        evidence: identification.evidence,
        plan,
    })
}

/// The labelled plan a `--dry-run` shows.
fn print_plan(console: &dyn Console, report: &PatchReport) {
    let profile = report.profile;
    console.result(&format!(
        "{:<12}: {} ({}, {} bytes)",
        "profile", profile.name, profile.device_type, profile.size
    ));
    console.result(&format!(
        "{:<12}: {}",
        "identified",
        match report.evidence {
            Evidence::Sha256 => "exact SHA-256 match (official image)",
            Evidence::Structure => "size, device string and version marker",
        }
    ));
    console.result(&format!(
        "{:<12}: 0x{GPS_EPOCH_UNIX:08X} -> 0x{GPS_EPOCH_UNIX_WNRO:08X}",
        "epoch"
    ));
    if let Some(reason) = &report.plan.no_op_reason {
        console.result(&format!("{:<12}: {reason}", "no change"));
        return;
    }
    console.result(&format!("{:<12}: {}", "changes", report.plan.edits.len()));
    for edit in &report.plan.edits {
        console.result(&format!(
            "  0x{:05X}  0x{:02X} -> 0x{:02X}  ({})",
            edit.offset,
            edit.before,
            edit.after,
            describe_offset(edit.offset)
        ));
    }
}

/// Describe a patched byte offset.
///
/// Any single image changes three bytes: the two differing high bytes of the
/// effective epoch constant plus the final version-marker digit for its
/// profile (four known offsets in total across V1.13 and V1.12).
fn describe_offset(offset: usize) -> &'static str {
    match offset {
        EFFECTIVE_EPOCH_HIGH_LOW | EFFECTIVE_EPOCH_HIGH_TOP => "effective epoch constant",
        V1_13_VERSION_DIGIT => "version marker (V1.13)",
        V1_12_VERSION_DIGIT => "version marker (V1.12)",
        _ => "unexpected offset — stop and check this image",
    }
}

/// The JSON document both `--dry-run` and a real run print.
fn plan_document(report: &PatchReport, written: Option<&std::path::Path>) -> serde_json::Value {
    let edits: Vec<serde_json::Value> = report
        .plan
        .edits
        .iter()
        .map(|edit| {
            serde_json::json!({
                "offset": format!("0x{:05X}", edit.offset),
                "before": edit.before,
                "after": edit.after,
                "meaning": describe_offset(edit.offset),
            })
        })
        .collect();
    serde_json::json!({
        "profile": report.profile.name,
        "device_type": report.profile.device_type,
        "size": report.profile.size,
        "evidence": match report.evidence {
            Evidence::Sha256 => "sha256",
            Evidence::Structure => "structure",
        },
        "epoch_before": GPS_EPOCH_UNIX,
        "epoch_after": GPS_EPOCH_UNIX_WNRO,
        "changes": report.plan.edits.len(),
        "edits": edits,
        "no_op_reason": report.plan.no_op_reason,
        "written": written.map(|path| path.display().to_string()),
    })
}

/// `m241 fw flash`: check an image against a backup, then write and verify it.
///
/// `--dry-run` reads the two files, identifies the image against a known profile,
/// runs [`write_plan`] — or [`write_plan_vendor`] when `--allow-vendor` accepted
/// the pinned vendor image — and prints what a write would do. It **opens no
/// port and sends nothing**.
///
/// A real run does everything decidable without a device first, in this order:
/// the `--yes` confirmation, [`check_layout`], the driver's own acceptance rule
/// against the pinned V1.13 range ([`check_image`] or
/// [`accept_vendor_image`]), and the DA file ([`check_da_image_against`], which
/// requires the pinned vendor SHA-256). Only then is a port opened. On the
/// device it:
///
/// 1. enters the BootROM ([`enter_bootrom`]) and syncs;
/// 2. calls [`DownloadAgent::verify_device`], so a unit that does not hold
///    `backup` byte for byte is refused **before** anything is erased. This is
///    required for the vendor path too;
/// 3. loads the DA into the chip's RAM ([`DaLoader::load`], RAM only);
/// 4. programs the image ([`DownloadAgent::write`] or, for the pinned vendor
///    image, [`DownloadAgent::write_vendor`], `0xD3` + `0xD5`);
/// 5. if `0xD3` fails before any `0xD5` byte, restarts the chip (`0xD9`),
///    re-enters the BootROM through the same NMEA probe and `$PMTK180*3B`
///    hand-over, reloads the DA and retries the write **once**; both attempts
///    use the same pinned range. A failure after `0xD5` has started is not
///    retried, because the range may then be partly written;
/// 6. restarts the chip (`0xD9`), re-enters the BootROM through the same NMEA
///    probe and `$PMTK180*3B` hand-over the initial entry used, and reads the
///    range back ([`PendingWrite::verify_written`]), reporting success only
///    when every byte matches. For `--allow-vendor` the comparison is against
///    the vendor image.
///
/// The vendor path is the explicit user decision in `AGENTS.md` §2.3: it
/// overwrites the unit's 14 bytes of per-unit data. The dry run and the success
/// output say so; this function never treats it as a backup-preserving write.
///
/// The read-back after `0xD9` can fail: the unit boots its firmware, and if
/// that firmware does not stream NMEA the BootROM cannot be re-entered. That is
/// reported as a read-back that could not be reached, with the power-cycle and
/// `fw dump` checks; this function never upgrades a failed read-back to success.
/// ⚠️ 未検証 on hardware: the write path, including the post-`0xD9` re-entry and
/// the vendor-image write, has run against the fake only.
///
/// # Errors
///
/// * [`Error::Io`] when a file cannot be read.
/// * [`Error::Profile`] when the image matches no known profile. The backup is
///   not identified; it is checked by length and the byte-for-byte diff.
/// * [`Error::Firmware`] when the image is too short to hold a header.
/// * [`Error::Safety`] when the image is not acceptable ([`write_plan`] /
///   [`check_image`] / [`accept_vendor_image`]), when `--yes` was not given,
///   from [`check_layout`], or from [`check_da_image_against`] / the device
///   check.
/// * [`Error::Port`] when no plausible port can be opened.
/// * [`Error::State`] when the BootROM cannot be re-entered after `0xD9`.
/// * [`Error::Timeout`] / [`Error::Framing`] / [`Error::Rejected`] from the
///   BootROM, the DA, the write or the read-back; each carries the crate's
///   recovery paragraph once anything has been sent.
enum PlannedFlash<'a> {
    Profile {
        plan: WritePlan,
        vendor: Option<VendorImage<'a>>,
    },
    Unverified {
        plan: UnverifiedWritePlan,
        token: UnverifiedImage<'a>,
    },
}

impl PlannedFlash<'_> {
    fn write_length(&self) -> usize {
        match self {
            Self::Profile { plan, .. } => plan.write_length,
            Self::Unverified { plan, .. } => plan.write_length,
        }
    }

    fn packets(&self) -> usize {
        match self {
            Self::Profile { plan, .. } => plan.packets,
            Self::Unverified { plan, .. } => plan.packets,
        }
    }

    fn read_back_policy(&self) -> ReadBackPolicy {
        match self {
            Self::Profile { vendor: None, .. } => ReadBackPolicy::Strict,
            Self::Profile {
                vendor: Some(_), ..
            }
            | Self::Unverified { .. } => ReadBackPolicy::AllowPerUnitRestore,
        }
    }
}

pub fn flash(options: &FlashOptions, session: Session<'_>) -> Result<()> {
    let backup = fileio::read(&options.backup)?;
    let image = fileio::read(&options.image)?;

    // Physical flash bounds always apply first (non-empty, even, <= 512 KiB).
    check_physical_image(&image)?;

    let planned = if options.allow_unverified {
        let token = accept_unverified_image(&image)?;
        let plan = write_plan_unverified(Some(&backup), &token);
        PlannedFlash::Unverified { plan, token }
    } else {
        // Identify the image; it decides the profile a plan is made against.
        // The backup is verified structurally against its own profile when
        // using `--allow-vendor`, or byte-for-byte on the strict path.
        let image_view = FirmwareImage::new(&image)?;
        let profile = identify(&image_view)?.profile;

        // Strict first: the backup-diff rule is the normal path. A vendor image
        // fails that diff by design, so it reaches the separate fixed-SHA gate
        // only when `--allow-vendor` is set. The strict `check_image` never
        // grows a digest branch.
        let strict_plan = write_plan(profile, &backup, &image);
        let (plan, vendor) = match strict_plan {
            Ok(plan) => (plan, None),
            Err(strict_err) => {
                if !options.allow_vendor {
                    return Err(strict_err);
                }
                let token = accept_vendor_image(profile, &image)?;
                let plan = write_plan_vendor(profile, &backup, token)?;
                (plan, Some(token))
            }
        };
        PlannedFlash::Profile { plan, vendor }
    };

    if options.dry_run {
        match &planned {
            PlannedFlash::Profile { plan, .. } => {
                print_write_plan(options, session.console, plan, None)?;
            }
            PlannedFlash::Unverified { plan, .. } => {
                print_unverified_write_plan(options, session.console, plan)?;
            }
        }
        session.console.note(&dry_run_note());
        return Ok(());
    }

    // A real write. Everything that can be decided without a port is decided
    // first, in the order a person would want to be told: the confirmation,
    // warnings, then the layout and the files the write needs.
    let confirmation = Confirmation::of_command_line(options.yes)?;
    match &planned {
        PlannedFlash::Profile { plan, .. } => warn_vendor_write(session.console, plan),
        PlannedFlash::Unverified { plan, .. } => warn_unverified_write(session.console, plan),
    }
    check_layout()?;
    check_physical_image(&backup)?;
    if options.recover && image != backup {
        return Err(Error::Safety {
            reason: String::from(
                "--recover writes the backup itself back over a unit a failed write left \
                 different, so the image must be byte-identical to --backup. This image differs; \
                 use the normal path if it is a WNRO-patched backup",
            ),
        });
    }

    // clap requires `--da` unless `--dry-run`, but the handler does not lean on
    // the grammar: a missing file must not fall through to "load nothing".
    let Some(da_path) = options.da.as_deref() else {
        return Err(Error::State(String::from(
            "no DA image: pass --da FILE (the vendor DA_v3.8.4.bin; not needed with --dry-run)",
        )));
    };
    let da = fileio::read(da_path)?;
    check_da_image_against(&da, da_digest())?;
    session.console.note(&format!(
        "DA file {} is {} bytes with SHA-256 {}; it is the pinned DA_v3.8.4.bin digest \
         (AGENTS.md §2.5), so the DA may be loaded",
        da_path.display(),
        da.len(),
        sha256_hex(&da)
    ));

    // The port is opened only after every refusal above, so a dry run, a
    // missing confirmation and a bad file never touch a cable.
    let port = link::resolve(session.links, &options.port)?;
    session.console.note(&format!(
        "writing {} bytes to {port}; the backup at {} is not modified",
        planned.write_length(),
        options.backup.display()
    ));

    let transport = enter_bootrom(options.assume_brom, session, &port)?;
    let mut rom = MtkBootRom::new(transport);
    rom.sync()?;
    if options.recover {
        session.console.note(
            "recovery mode: the unit may differ from the backup by design, so the whole-unit \
             comparison is skipped; checking only the M-241 vector and family markers",
        );
        DownloadAgent::verify_unit_markers(&mut rom)?;
        session.console.note(
            "the unit's markers identify an M-241; loading the DA into the chip's RAM (no flash \
             write yet)",
        );
    } else {
        session.console.note(
            "BootROM sync answered; comparing the whole unit with the backup before any erase",
        );
        DownloadAgent::verify_device(&mut rom, &backup)?;
        session.console.note(
            "the unit matches the backup; loading the DA into the chip's RAM (no flash write yet)",
        );
    }
    let mut loader = DaLoader::new(rom.transport_mut());
    let id = loader.load_against(confirmation, &da, da_digest())?;
    session.console.note(&format!(
        "DA v{}.{} answered; programming {} bytes in {} packets",
        id.version >> 8,
        id.version & 0xFF,
        planned.write_length(),
        planned.packets()
    ));

    let read_back_policy = planned.read_back_policy();
    let (vendor, unverified) = match &planned {
        PlannedFlash::Profile { vendor, .. } => (*vendor, None),
        PlannedFlash::Unverified { token, .. } => (None, Some(token.clone())),
    };

    let mut da_session = DownloadAgent::new(rom.into_transport());
    let attempt = WriteAttempt {
        confirmation,
        backup: &backup,
        image: &image,
        da: &da,
        vendor,
        unverified,
    };
    let (pending, da_session) = match attempt.write(&mut da_session) {
        Ok(pending) => (pending, da_session),
        Err(first_err) => recover_failed_write(da_session, first_err, &attempt, options, session)?,
    };
    session.console.note(
        "every packet was acknowledged; restarting the unit and reading the written range back",
    );

    let report = match pending.verify_written_with_policy(
        da_session,
        &image,
        FLASH_WRITE_START,
        |transport| reenter_bootrom(transport, session, Reentry::ReadBack),
        read_back_policy,
    ) {
        Ok(report) => report,
        Err(err) => {
            note_write_failure(session.console, &options.backup);
            return Err(err);
        }
    };

    match &planned {
        PlannedFlash::Profile { plan, .. } => {
            print_write_report(options, session.console, &port, plan, &report)
        }
        PlannedFlash::Unverified { plan, .. } => {
            print_unverified_write_report(options, session.console, &port, plan, &report)
        }
    }
}

/// The note appended when the automatic `0xD9` restart succeeded but no write
/// had started yet.
///
/// `0xD9` is a one-way byte: the client sends it and reads no reply, so the
/// wording must not claim the restart was observed. No battery removal is
/// needed whether or not the firmware actually booted.
const RESTARTED_NO_RETRY_NOTE: &str = concat!(
    "0xD9 was sent to restart the unit; the restart cannot be confirmed because `0xD9` has no ",
    "reply, but no battery removal is needed. The write did not start, so there is nothing to ",
    "retry automatically. Run `m241 fw dump` to confirm what it holds"
);

/// The note appended after `0xD5` started: the range may be partly written.
const RESTARTED_PARTIAL_NOTE: &str = concat!(
    "0xD9 was sent to restart the unit; the restart cannot be confirmed because `0xD9` has no ",
    "reply, but no battery removal is needed. The write may have stopped part-way, so it is not ",
    "retried automatically. Run `m241 fw dump`; if the image is incomplete, restore your backup ",
    "with `m241 fw flash --recover ... --yes`"
);

/// The note appended when the one automatic retry failed before any `0xD5`.
///
/// Both the first attempt and the retry stopped before `0xD5`, so no byte of
/// the flash was erased or programmed; `--recover` is not the next step here.
const RESTARTED_RETRY_FAILED_NOTE: &str = concat!(
    "0xD9 was sent to restart the unit; the restart cannot be confirmed because `0xD9` has no ",
    "reply, but no battery removal is needed. The automatic retry also failed before any 0xD5, ",
    "so nothing was written. Run `m241 fw dump` before deciding whether to retry"
);

/// The note appended when the automatic retry could not even start.
///
/// Re-entering the BootROM or reloading the DA failed. The failed `0xD3` wrote
/// nothing, so this must not suggest `--recover` as for a partial write.
const RESTARTED_NOT_RETRIED_NOTE: &str = concat!(
    "0xD9 was sent to restart the unit; the restart cannot be confirmed because `0xD9` has no ",
    "reply, but no battery removal is needed. The automatic retry was not attempted, and nothing ",
    "was written: the failed 0xD3 records a range and programs nothing. Run `m241 fw dump` to ",
    "confirm what it holds"
);

/// One write attempt and the inputs its automatic recovery needs.
///
/// `image` is unused on the vendor path: [`Self::write`] writes the pinned
/// vendor image carried by `vendor` instead.
struct WriteAttempt<'a> {
    confirmation: Confirmation,
    backup: &'a [u8],
    image: &'a [u8],
    da: &'a [u8],
    vendor: Option<VendorImage<'a>>,
    unverified: Option<UnverifiedImage<'a>>,
}

impl WriteAttempt<'_> {
    /// Run one DA write: the strict backup-diff, vendor, or unverified path.
    fn write<T: Transport>(&self, da: &mut DownloadAgent<T>) -> Result<PendingWrite> {
        if let Some(token) = self.vendor {
            da.write_vendor(self.confirmation, self.backup, token)
        } else if let Some(unverified) = &self.unverified {
            da.write_unverified(self.confirmation, unverified)
        } else {
            da.write(self.confirmation, self.backup, self.image)
        }
    }
}

/// Recover from a failed write attempt, then retry it at most once.
///
/// The chip is restarted with `0xD9` in every case. A retry is only attempted
/// when the failure happened before any `0xD5` byte: `0xD3` records a range and
/// calls no flash driver (`docs/reference/01-da-protocol.md` §4.1, §4.6), so the
/// unit cannot have changed. From `0xD5` on a partial write is possible, and
/// restoring the user's backup with `--recover` is the route.
///
/// Returns the pending write and the DA session the retry left in place, so the
/// caller's read-back runs on the same transport.
fn recover_failed_write(
    mut da_session: DownloadAgent<Opened>,
    first_err: Error,
    attempt: &WriteAttempt<'_>,
    options: &FlashOptions,
    session: Session<'_>,
) -> Result<(PendingWrite, DownloadAgent<Opened>)> {
    let range_attempted = da_session.range_attempted();
    let program_started = da_session.program_started();
    if let Err(restart_err) = da_session.restart() {
        return Err(write_failure(
            session.console,
            &options.backup,
            first_err,
            &format!(
                "The automatic 0xD9 restart failed ({restart_err}); the DA may still be running in \
                 RAM. Retry with --assume-brom or power-cycle, then run `m241 fw dump`"
            ),
        ));
    }
    if !range_attempted {
        return Err(write_failure(
            session.console,
            &options.backup,
            first_err,
            RESTARTED_NO_RETRY_NOTE,
        ));
    }
    if program_started {
        return Err(write_failure(
            session.console,
            &options.backup,
            first_err,
            RESTARTED_PARTIAL_NOTE,
        ));
    }

    session.console.warn(
        "0xD3 failed before 0xD5; the DA was restarted with 0xD9. Re-entering the BootROM, \
         reloading the DA, and retrying the write once",
    );
    let transport = reenter_bootrom(da_session.into_transport(), session, Reentry::BeforeRetry)
        .map_err(|err| {
            write_failure(
                session.console,
                &options.backup,
                err,
                RESTARTED_NOT_RETRIED_NOTE,
            )
        })?;
    let mut retry_session = reload_for_retry(transport, attempt.confirmation, attempt.da, session)
        .map_err(|err| {
            write_failure(
                session.console,
                &options.backup,
                err,
                RESTARTED_NOT_RETRIED_NOTE,
            )
        })?;
    match attempt.write(&mut retry_session) {
        Ok(pending) => Ok((pending, retry_session)),
        Err(retry_err) => {
            let note = if retry_session.program_started() {
                RESTARTED_PARTIAL_NOTE
            } else {
                RESTARTED_RETRY_FAILED_NOTE
            };
            let note = match retry_session.restart() {
                Ok(()) => note.to_owned(),
                Err(restart_err) => format!(
                    "The automatic 0xD9 restart after the retry failed ({restart_err}); {note}"
                ),
            };
            Err(write_failure(
                session.console,
                &options.backup,
                retry_err,
                &note,
            ))
        }
    }
}

/// Report a failed write on the console and return the error with `note`.
///
/// The console gets the backup-is-untouched paragraph; the error keeps its
/// machine-readable variant where [`with_note`] can keep it.
fn write_failure(console: &dyn Console, backup: &std::path::Path, err: Error, note: &str) -> Error {
    note_write_failure(console, backup);
    with_note(err, note)
}

/// Re-enter the BootROM and reload the DA for the one automatic retry.
///
/// Called only after a failed `0xD3` before `0xD5`: the flash was not changed,
/// the chip has already been restarted with `0xD9`, and the caller has checked
/// that no partial write is possible. The range is not passed in; both
/// [`DownloadAgent::write`] and [`write_vendor`] use the same pinned constants.
///
/// [`DownloadAgent::verify_device`] is **not** repeated here even though
/// `AGENTS.md` §2.3 requires the comparison before a DA is loaded: this path
/// exists only because `0xD3` failed, and `0xD3` records a range without calling
/// a flash driver (`docs/reference/01-da-protocol.md` §4.1, §4.6). Every erase
/// and program happens inside `0xD5` (§5.6), which the failed attempt never sent,
/// so the unit still matches the backup byte for byte.
fn reload_for_retry(
    transport: Opened,
    confirmation: Confirmation,
    da: &[u8],
    session: Session<'_>,
) -> Result<DownloadAgent<Opened>> {
    let mut rom = MtkBootRom::new(transport);
    rom.sync()?;
    session
        .console
        .note("BootROM re-entered after the restart; reloading the DA into RAM");
    let mut loader = DaLoader::new(rom.transport_mut());
    let _id = loader.load_against(confirmation, da, da_digest())?;
    Ok(DownloadAgent::new(rom.into_transport()))
}

/// Append `note` to an error, keeping its variant where one exists.
///
/// `Framing`, `Timeout`, `Rejected` and `Safety` keep their variant and the note
/// is appended to their message. Every other variant — `Io`, `Port`, `Profile`,
/// `State`, `Firmware` — becomes [`Error::State`], losing the machine-readable
/// kind: those are transport or file failures whose extra fields do not fit the
/// four above, and the caller's next action is the one the note names.
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

/// Which step is waiting for the BootROM after a `0xD9`.
///
/// Both steps use the same NMEA probe and `$PMTK180*3B` hand-over, but a failed
/// re-entry means opposite things: after a finished write the flash holds the
/// image, while after a failed `0xD3` nothing was written. The error must not
/// tell the user to restore a backup for a write that never happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reentry {
    /// The read-back after a completed write.
    ReadBack,
    /// The one retry after a failed `0xD3`; nothing has been written.
    BeforeRetry,
}

/// Put the transport back where a BootROM read works, after `0xD9`.
///
/// The restart boots the firmware, so this is the same route the initial entry
/// takes: wait for the firmware to start streaming, probe one NMEA sentence,
/// then send `$PMTK180*3B`, which leaves the transport at
/// [`DEFAULT_BOOTROM_BAUD`]. Nothing here touches DTR/RTS: the tri-state probe
/// in [`enter_bootrom`] exists because pulsing them at a running firmware can
/// leave the unit unresponsive, and the same rule applies after a restart.
///
/// The timeouts are the session's: [`PmtkClient::probe_stream`] uses the stream
/// window and [`PmtkClient::enter_bootrom_after`] uses the `$PMTK180` timings,
/// so a unit that never answers fails instead of hanging. The settle before the
/// probe gives the firmware time to boot; ⚠️ 未検証 on hardware.
///
/// # Errors
///
/// [`Error::State`] when no complete NMEA sentence arrives (the firmware did not
/// start, or the line is not usable), or when `$PMTK180` is not honoured.
/// [`Error::Io`] from the transport; the text of the [`Error::State`] depends on
/// `why`, because a re-entry failure means opposite things after a completed
/// write and after a failed `0xD3`.
fn reenter_bootrom<T: Transport>(transport: T, session: Session<'_>, why: Reentry) -> Result<T> {
    std::thread::sleep(BOOTROM_REENTRY_SETTLE);
    let mut transport = transport;
    transport.set_baud(NMEA_BAUD)?;
    // Drop anything the DA conversation left queued before looking for NMEA;
    // a stale byte must not be mistaken for the restarted firmware's stream.
    transport.clear_input()?;
    let mut client = PmtkClient::new(transport)
        .with_reply_timeout(session.timings.reply)
        .with_stream_window(session.timings.stream)
        .with_entry_timing(session.timings.entry);
    match client.probe_stream()? {
        StreamState::Streaming => {
            let purpose = match why {
                Reentry::ReadBack => "the read-back",
                Reentry::BeforeRetry => "the automatic retry",
            };
            session.console.note(&format!(
                "NMEA is streaming again after the restart; sending $PMTK180 to re-enter the \
                 BootROM for {purpose}"
            ));
            client.enter_bootrom_after(StreamState::Streaming)?;
            Ok(client.into_transport())
        }
        StreamState::Silent => Err(Error::State(String::from(match why {
            Reentry::ReadBack => {
                "the unit did not stream NMEA after the restart, so the BootROM could not be \
                 re-entered and the read-back was not performed. The write itself finished; do \
                 not assume it failed. Power-cycle the unit and run `m241 fw dump` to see what it \
                 holds, then write your own backup back with `--recover` if needed"
            }
            Reentry::BeforeRetry => {
                "the unit did not stream NMEA after the restart, so the BootROM could not be \
                 re-entered and the automatic retry was not attempted. Nothing was written: a \
                 failed 0xD3 records a range and programs nothing. Run `m241 fw dump` to confirm \
                 what the unit holds"
            }
        }))),
        StreamState::Unknown => Err(Error::State(String::from(match why {
            Reentry::ReadBack => {
                "the line carried bytes but no complete NMEA sentence after the restart, so the \
                 BootROM could not be re-entered and the read-back was not performed. Power-cycle \
                 the unit and run `m241 fw dump` to see what it holds"
            }
            Reentry::BeforeRetry => {
                "the line carried bytes but no complete NMEA sentence after the restart, so the \
                 BootROM could not be re-entered and the automatic retry was not attempted. \
                 Nothing was written: a failed 0xD3 records a range and programs nothing. Run \
                 `m241 fw dump` to confirm what it holds"
            }
        }))),
    }
}

/// How long the unit is given after `0xD9` before the first BootROM probe.
///
/// A conservative settle, not a measured value: ⚠️ 未検証 on hardware because the
/// write path has not run on a unit. The probe's stream window and the
/// read-back's own timeout are what fail the command if this is too short.
const BOOTROM_REENTRY_SETTLE: std::time::Duration = std::time::Duration::from_millis(250);

/// Say what a failed write left behind, without claiming it wrote nothing.
///
/// The error above carries the crate's own recovery paragraph, which differs
/// before and after `0xD9`. This note adds the one fact the error cannot know:
/// the backup is a local file and this command never touched it.
fn note_write_failure(console: &dyn Console, backup: &std::path::Path) {
    console.note(&format!(
        "the run did not finish, so do not assume the write succeeded. The backup at {} was not \
         modified by this command; follow the recovery steps in the error above. A power-cycle \
         followed by `m241 fw dump` shows what the unit actually holds — do not treat the run as \
         successful without that check.",
        backup.display()
    ));
}

/// Warn about what writing the vendor image changes, before any port is opened.
///
/// Shared by the dry run and the real path so the two cannot drift. The vendor
/// V1.13 image is the stock, unpatched release: it overwrites the unit's 14
/// per-unit bytes and leaves the WNRO fix off (epoch `315964800`, marker
/// `V1.13`), so dates are 7168 days behind until the image is patched.
fn warn_vendor_write(console: &dyn Console, plan: &WritePlan) {
    if !plan.vendor {
        return;
    }
    let hardware_note = if plan.profile.name == PROFILE_V1_12.name {
        "; V1.12 has not been verified on hardware (未検証)"
    } else {
        ""
    };
    console.warn(&format!(
        "warning: this is the official vendor image, not this unit's backup. Writing it \
         replaces the unit's {} bytes of per-unit data with the vendor values and leaves the \
         WNRO fix off: the vendor {} epoch is 315964800 and its marker is {}, so dates \
         are 7168 days behind until the image is patched{hardware_note}. The backup is still \
         required and is compared with the unit before the DA is loaded",
        VENDOR_PER_UNIT_DATA_BYTES, plan.profile.name, plan.profile.name
    ));
}

/// Warn before writing an unverified / custom image.
fn warn_unverified_write(console: &dyn Console, plan: &UnverifiedWritePlan) {
    let vector_note = if plan.has_arm_vector {
        "ARM reset vector (18 F0 9F E5): present"
    } else {
        "ARM reset vector (18 F0 9F E5): ABSENT — the unit may not boot if this is not a valid \
         firmware image"
    };
    console.warn(&format!(
        "warning: unverified image (--allow-unverified, 未検証): {} bytes, SHA-256 {}. \
         {vector_note}. This image has not been verified by this project; keep your backup safe",
        plan.size, plan.sha256
    ));
}

/// The label for `plan`'s image against its backup.
///
/// A zero-diff image is the backup itself, which is a distinct case from a WNRO
/// patch: `--recover` requires exactly this, and a user should not be told a
/// patch was applied when none was.
fn acceptance_label(plan: &WritePlan) -> &'static str {
    if plan.vendor {
        return "official vendor image";
    }
    match plan.differing_bytes {
        Some(0) => "identical to the backup",
        Some(_) => "patched backup",
        None => "backup diff",
    }
}

/// Show the completed write: the range, the packets and the read-back count.
///
/// # Errors
///
/// Whatever [`Console::json`] returns when the document cannot be written.
fn print_write_report(
    options: &FlashOptions,
    console: &dyn Console,
    port: &str,
    plan: &WritePlan,
    report: &WriteReport,
) -> Result<()> {
    if options.json {
        return console.json(&serde_json::json!({
            "dry_run": false,
            "port": port,
            "image": options.image.display().to_string(),
            "backup": options.backup.display().to_string(),
            "profile": plan.profile.name,
            "acceptance": acceptance_label(plan),
            "vendor_image": plan.vendor,
            "per_unit_data_overwritten": plan.vendor,
            "vendor_image_unpatched": plan.vendor,
            "start": format!("0x{:08X}", report.start),
            "length": report.length,
            "packets": report.packets,
            "verified": report.verified,
            "restored_per_unit_bytes": report.restored_per_unit_bytes,
            "verified_matches_image": true,
            "flash_write": true,
            "power_cycle_required": true,
        }));
    }
    console.result(&format!("{:<12}: {port}", "port"));
    console.result(&format!("{:<12}: {}", "image", options.image.display()));
    console.result(&format!("{:<12}: {}", "backup", options.backup.display()));
    console.result(&format!(
        "{:<12}: {} ({}, {} bytes)",
        "profile", plan.profile.name, plan.profile.device_type, plan.size
    ));
    console.result(&format!(
        "{:<12}: {}",
        "accepted as",
        acceptance_label(plan)
    ));
    if plan.vendor {
        console.result(&format!(
            "{:<12}: yes; the unit's {} bytes of per-unit data were overwritten",
            "vendor image", VENDOR_PER_UNIT_DATA_BYTES
        ));
        console.result(&format!(
            "{:<12}: no (the vendor {} image is unpatched; dates are 7168 days behind)",
            "wnro patch", plan.profile.name
        ));
    }
    console.result(&format!("{:<12}: 0x{:08X}", "start", report.start));
    console.result(&format!("{:<12}: {}", "length", report.length));
    console.result(&format!("{:<12}: {}", "packets", report.packets));
    console.result(&format!(
        "{:<12}: {} bytes read back and identical",
        "verified", report.verified
    ));
    if report.restored_per_unit_bytes > 0 {
        console.note(&format!(
            "note: {} byte(s) in 0x5C..0x60 were restored by the booted firmware on reboot",
            report.restored_per_unit_bytes
        ));
    }
    console.result(&format!("{:<12}: yes", "flash write"));
    console.result(&format!(
        "{:<12}: remove the battery for about ten seconds, put it back, and switch the unit on; \
         the read-back left it in the BootROM",
        "next"
    ));
    Ok(())
}

/// Show the completed write for an `--allow-unverified` image.
fn print_unverified_write_report(
    options: &FlashOptions,
    console: &dyn Console,
    port: &str,
    plan: &UnverifiedWritePlan,
    report: &WriteReport,
) -> Result<()> {
    if options.json {
        return console.json(&serde_json::json!({
            "dry_run": false,
            "port": port,
            "image": options.image.display().to_string(),
            "backup": options.backup.display().to_string(),
            "profile": serde_json::Value::Null,
            "acceptance": "unverified",
            "unverified_image": true,
            "sha256": plan.sha256,
            "has_arm_vector": plan.has_arm_vector,
            "start": format!("0x{:08X}", report.start),
            "length": report.length,
            "packets": report.packets,
            "verified": report.verified,
            "restored_per_unit_bytes": report.restored_per_unit_bytes,
            "verified_matches_image": true,
            "flash_write": true,
            "power_cycle_required": true,
        }));
    }
    console.result(&format!("{:<12}: {port}", "port"));
    console.result(&format!("{:<12}: {}", "image", options.image.display()));
    console.result(&format!("{:<12}: {}", "backup", options.backup.display()));
    console.result(&format!(
        "{:<12}: unverified ({} bytes)",
        "profile", plan.size
    ));
    console.result(&format!("{:<12}: unverified image", "accepted as"));
    console.result(&format!("{:<12}: {}", "sha256", plan.sha256));
    console.result(&format!(
        "{:<12}: {}",
        "arm vector",
        if plan.has_arm_vector {
            "yes (18 F0 9F E5)"
        } else {
            "no"
        }
    ));
    console.result(&format!("{:<12}: 0x{:08X}", "start", report.start));
    console.result(&format!("{:<12}: {}", "length", report.length));
    console.result(&format!("{:<12}: {}", "packets", report.packets));
    console.result(&format!(
        "{:<12}: {} bytes read back and identical",
        "verified", report.verified
    ));
    if report.restored_per_unit_bytes > 0 {
        console.note(&format!(
            "note: {} byte(s) in 0x5C..0x60 were restored by the booted firmware on reboot",
            report.restored_per_unit_bytes
        ));
    }
    console.result(&format!("{:<12}: yes", "flash write"));
    console.result(&format!(
        "{:<12}: remove the battery for about ten seconds, put it back, and switch the unit on; \
         the read-back left it in the BootROM",
        "next"
    ));
    Ok(())
}

/// The sentence a dry run ends with.
///
/// One string rather than a literal at the call site, so the test that asserts
/// the dry run opened nothing reads the same text a user does.
fn dry_run_note() -> String {
    String::from(
        "dry run: no port was opened and no command was sent. A real run asks for --yes, loads \
         the DA, compares the unit with the backup, writes, and reads the range back before it \
         reports success",
    )
}

/// Format the plan as labelled lines or one JSON object.
///
/// `write_refusal` is the driver's own acceptance error for this image against
/// the pinned V1.13 range, when there is one. A dry run still prints the plan —
/// that is what it is for — but it must not say a real write would proceed when
/// [`DownloadAgent::write`] would refuse it.
///
/// # Errors
///
/// Whatever [`Console::json`] returns when the document cannot be written.
fn print_write_plan(
    options: &FlashOptions,
    console: &dyn Console,
    plan: &WritePlan,
    write_refusal: Option<&str>,
) -> Result<()> {
    let differing_line = match plan.differing_bytes {
        Some(count) => format!(
            "{count} byte(s) differ from the backup{}",
            if plan.differing_offsets.is_empty() {
                String::new()
            } else {
                let shown: Vec<String> = plan
                    .differing_offsets
                    .iter()
                    .map(|offset| format!("0x{offset:X}"))
                    .collect();
                format!("; first at {}", shown.join(", "))
            }
        ),
        None => String::from("the backup is a different length"),
    };
    let write_line = match (check_layout(), write_refusal) {
        (Err(err), _) => format!("refused: {err}"),
        (Ok(()), Some(reason)) => format!("refused: {reason}"),
        (Ok(()), None) => String::from(
            "permitted; a real run still needs --yes, compares the unit with the backup, and \
             reads the range back before reporting success",
        ),
    };
    let write_refused = check_layout().is_err() || write_refusal.is_some();

    if options.json {
        return console.json(&serde_json::json!({
            "dry_run": true,
            "image": options.image.display().to_string(),
            "backup": options.backup.display().to_string(),
            "profile": plan.profile.name,
            "acceptance": acceptance_label(plan),
            "vendor_image": plan.vendor,
            "per_unit_data_overwritten": plan.vendor,
            "vendor_image_unpatched": plan.vendor,
            "size": plan.size,
            "device_type": plan.device_type,
            "write_length": plan.write_length,
            "start": format!("0x{:08X}", plan.start),
            "packets": plan.packets,
            "differing_bytes": plan.differing_bytes,
            "differing_offsets": plan
                .differing_offsets
                .iter()
                .map(|offset| format!("0x{offset:X}"))
                .collect::<Vec<_>>(),
            "opens_port": false,
            "wrote_flash": false,
            "actual_write_refused": write_refused,
            "actual_write": write_line,
        }));
    }

    console.result(&format!("{:<12}: {}", "image", options.image.display()));
    console.result(&format!("{:<12}: {}", "backup", options.backup.display()));
    console.result(&format!(
        "{:<12}: {} ({}, {} bytes)",
        "profile", plan.profile.name, plan.profile.device_type, plan.size
    ));
    console.result(&format!(
        "{:<12}: {}",
        "accepted as",
        acceptance_label(plan)
    ));
    console.result(&format!("{:<12}: {}", "device", plan.device_type));
    if plan.vendor {
        console.result(&format!(
            "{:<12}: yes; the unit's {} bytes of per-unit data will be overwritten",
            "vendor image", VENDOR_PER_UNIT_DATA_BYTES
        ));
        console.result(&format!(
            "{:<12}: no (the vendor {} image is unpatched; dates are 7168 days behind)",
            "wnro patch", plan.profile.name
        ));
        warn_vendor_write(console, plan);
    }
    console.result(&format!(
        "{:<12}: {} bytes ({} packets of {} bytes)",
        "write range",
        plan.write_length,
        plan.packets,
        m241_flash::da::PACKET_SIZE
    ));
    console.result(&format!(
        "{:<12}: 0x{:08X} (settled DA-side base)",
        "start", plan.start
    ));
    console.result(&format!("{:<12}: {differing_line}", "diff"));
    console.result(&format!("{:<12}: {write_line}", "actual write"));
    console.result(&format!("{:<12}: no (this was --dry-run)", "port opened"));
    console.result(&format!(
        "{:<12}: no (0xD3/0xD4/0xD5/0xD9 were not sent)",
        "flash write"
    ));
    Ok(())
}

/// Format the `--dry-run` plan for an `--allow-unverified` image.
fn print_unverified_write_plan(
    options: &FlashOptions,
    console: &dyn Console,
    plan: &UnverifiedWritePlan,
) -> Result<()> {
    let differing_line = match plan.differing_bytes {
        Some(count) => format!(
            "{count} byte(s) differ from the backup{}",
            if plan.differing_offsets.is_empty() {
                String::new()
            } else {
                let shown: Vec<String> = plan
                    .differing_offsets
                    .iter()
                    .map(|offset| format!("0x{offset:X}"))
                    .collect();
                format!("; first at {}", shown.join(", "))
            }
        ),
        None => String::from("the backup is a different length"),
    };
    let write_line = match check_layout() {
        Err(err) => format!("refused: {err}"),
        Ok(()) => String::from(
            "permitted; a real run still needs --yes, compares the unit with the backup, and \
             reads the range back before reporting success",
        ),
    };
    let write_refused = check_layout().is_err();

    if options.json {
        return console.json(&serde_json::json!({
            "dry_run": true,
            "image": options.image.display().to_string(),
            "backup": options.backup.display().to_string(),
            "profile": serde_json::Value::Null,
            "acceptance": "unverified",
            "unverified_image": true,
            "sha256": plan.sha256,
            "has_arm_vector": plan.has_arm_vector,
            "size": plan.size,
            "write_length": plan.write_length,
            "start": format!("0x{:08X}", plan.start),
            "packets": plan.packets,
            "differing_bytes": plan.differing_bytes,
            "differing_offsets": plan
                .differing_offsets
                .iter()
                .map(|offset| format!("0x{offset:X}"))
                .collect::<Vec<_>>(),
            "opens_port": false,
            "wrote_flash": false,
            "actual_write_refused": write_refused,
            "actual_write": write_line,
        }));
    }

    console.result(&format!("{:<12}: {}", "image", options.image.display()));
    console.result(&format!("{:<12}: {}", "backup", options.backup.display()));
    console.result(&format!(
        "{:<12}: unverified ({} bytes)",
        "profile", plan.size
    ));
    console.result(&format!("{:<12}: unverified image", "accepted as"));
    console.result(&format!("{:<12}: {}", "sha256", plan.sha256));
    console.result(&format!(
        "{:<12}: {}",
        "arm vector",
        if plan.has_arm_vector {
            "yes (18 F0 9F E5)"
        } else {
            "no"
        }
    ));
    warn_unverified_write(console, plan);
    console.result(&format!(
        "{:<12}: {} bytes ({} packets of {} bytes)",
        "write range",
        plan.write_length,
        plan.packets,
        m241_flash::da::PACKET_SIZE
    ));
    console.result(&format!(
        "{:<12}: 0x{:08X} (settled DA-side base)",
        "start", plan.start
    ));
    console.result(&format!("{:<12}: {differing_line}", "diff"));
    console.result(&format!("{:<12}: {write_line}", "actual write"));
    console.result(&format!("{:<12}: no (this was --dry-run)", "port opened"));
    console.result(&format!(
        "{:<12}: no (0xD3/0xD4/0xD5/0xD9 were not sent)",
        "flash write"
    ));
    Ok(())
}

///
/// `page` is the reader's own result: `Ok` for a page that arrived and passed
/// its checksum, `Err` with the reason it did not. The distinction matters —
/// "the DA does not read there" and "the DA did not answer" are different
/// findings, and only the first says anything about where the image is.
#[derive(Debug)]
pub struct ProbeCandidate {
    /// The address, one of [`PROBE_ADDRESSES`].
    pub address: u32,
    /// The 64-byte page, or why there is none.
    pub page: Result<Vec<u8>>,
}

impl ProbeCandidate {
    /// Whether the page arrived at all.
    #[must_use]
    pub fn was_read(&self) -> bool {
        self.page.is_ok()
    }

    /// The page's first four bytes, when it arrived.
    #[must_use]
    pub fn first_bytes(&self) -> Option<&[u8]> {
        self.page
            .as_ref()
            .ok()
            .map(|page| page.get(..VECTOR.len()).unwrap_or(page))
    }

    /// The first four bytes as the CLI prints them.
    #[must_use]
    pub fn first_bytes_hex(&self) -> String {
        hex_bytes(self.first_bytes().unwrap_or_default())
    }

    /// Whether the first four bytes are the M-241 vector table.
    ///
    /// Exactly four bytes are compared, and that is the whole decision: 64 were
    /// read so a person can see the surroundings, the rest take no part in the
    /// verdict.
    #[must_use]
    pub fn vector_matches(&self) -> bool {
        self.first_bytes() == Some(VECTOR.as_slice())
    }
}

/// Which of the two probed addresses holds the firmware image.
///
/// The assumption behind the probe is that the provisioning image starts with
/// the vector table `18 F0 9F E5`. That is **measured in the BootROM's view**
/// (`docs/dev/03-hardware-facts.md` "Firmware base, BootROM view") and, on
/// 2026-11-21, through the DA (`docs/dev/05-verification.md` §1): DA address
/// `0` returned the vector and DA address `0x00200000` returned
/// `02 02 20 42`, consistent with an unmapped bus. A 64-byte page emits 64 data
/// bytes plus the two-byte trailer; multi-page reads and the advance after the
/// final page of a multi-page read remain unmeasured
/// (`docs/reference/01-da-protocol.md` §5.5, §7 item 4). A run whose cause is
/// unknown therefore produces *unread*, not *no image here*, and the two are
/// never folded together.
///
/// Two variants carry five outcomes:
///
/// | Read results | Verdict |
/// |---|---|
/// | exactly one page read, its first four bytes are the vector | `Settled(address)` |
/// | both read, neither starts with the vector | `Inconclusive(NoMatch)` |
/// | both read, both start with the vector | `Inconclusive(Ambiguous)` |
/// | one page not read | `Inconclusive(Unread { .. })` |
/// | neither read | `Inconclusive(Unread { .. })` |
///
/// A candidate that could not be read **blocks** `Settled`: the address that
/// came back as a timeout could be the one holding the image, and a timeout says
/// nothing about its contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaseVerdict {
    /// Both candidates were read and exactly one starts with [`VECTOR`].
    Settled(u32),
    /// Nothing can be concluded, and the variant names why.
    Inconclusive(Inconclusive),
}

/// Why [`BaseVerdict`] could not settle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Inconclusive {
    /// At least one candidate was not read (it timed out, or the DA refused).
    /// `read` lists the addresses that did answer, in probe order.
    Unread {
        /// The candidates that answered.
        read: [bool; PROBE_ADDRESSES.len()],
    },
    /// Both were read, and neither starts with the vector.
    NoMatch,
    /// Both were read, and both start with the vector.
    Ambiguous,
}

impl BaseVerdict {
    /// The one-line verdict, as the console and the JSON document print it.
    #[must_use]
    pub fn describe(self) -> String {
        match self {
            Self::Settled(address) => format!("the image is at 0x{address:08X}"),
            Self::Inconclusive(Inconclusive::Unread { read }) => {
                let missing: Vec<String> = PROBE_ADDRESSES
                    .iter()
                    .zip(read.iter())
                    .filter(|(_address, ok)| !**ok)
                    .map(|(address, _ok)| format!("0x{address:08X}"))
                    .collect();
                format!(
                    "inconclusive: {} not read (0xD6 timed out or was refused), so the address of \
                     the image is not decided — this is a failure of the read, not evidence that \
                     the image is absent. Check the cable and the unit, and record that the \
                     DA-side vector check itself is not yet verified on this unit",
                    missing.join(" and ")
                )
            }
            Self::Inconclusive(Inconclusive::NoMatch) => String::from(
                "inconclusive: both addresses were read and neither starts with the vector \
                 18 F0 9F E5 — the image is at neither candidate, or the DA's view is not the \
                 firmware's",
            ),
            Self::Inconclusive(Inconclusive::Ambiguous) => String::from(
                "inconclusive: both addresses start with the vector 18 F0 9F E5 — two matches are \
                 not a tie this tool may break",
            ),
        }
    }
}

/// Which candidate base a page of `length` bytes starting with `vector` implies.
///
/// Pure, so the rule is testable without a device. `pages` carries one entry per
/// [`PROBE_ADDRESSES`] slot, in that order — [`base_verdict`] refuses anything
/// else, which is what keeps a single-candidate run from passing — and `Err` for
/// an address whose read failed.
///
/// # Errors
///
/// [`Error::State`] when `pages` is not one entry per [`PROBE_ADDRESSES`] slot,
/// or when its addresses are not those slots in order.
pub fn base_verdict(pages: &[ProbeCandidate]) -> Result<BaseVerdict> {
    if pages.len() != PROBE_ADDRESSES.len()
        || pages
            .iter()
            .zip(PROBE_ADDRESSES)
            .any(|(candidate, address)| candidate.address != address)
    {
        return Err(Error::State(format!(
            "the probe reads exactly {PROBE_ADDRESSES:08X?}; the verdict was given {} other \
             address(es)",
            pages.len()
        )));
    }
    let read = PROBE_ADDRESSES.map(|address| {
        pages
            .iter()
            .find(|candidate| candidate.address == address)
            .is_some_and(ProbeCandidate::was_read)
    });
    if !read.iter().all(|ok| *ok) {
        return Ok(BaseVerdict::Inconclusive(Inconclusive::Unread { read }));
    }
    let matched: Vec<u32> = pages
        .iter()
        .filter(|candidate| candidate.vector_matches())
        .map(|candidate| candidate.address)
        .collect();
    match matched.as_slice() {
        [only] => Ok(BaseVerdict::Settled(*only)),
        [] => Ok(BaseVerdict::Inconclusive(Inconclusive::NoMatch)),
        _ => Ok(BaseVerdict::Inconclusive(Inconclusive::Ambiguous)),
    }
}

/// `m241 fw da-restart`: send the DA's one-byte `0xD9` restart.
///
/// A failed DA phase can leave the Download Agent running in RAM, where the
/// unit no longer answers the firmware's commands. `0xD9` makes the chip boot
/// its firmware; the handler resets the chip and never returns, so no reply is
/// awaited. It neither erases nor programs flash, which is why the command
/// needs no `--yes`. DTR/RTS are not touched: pulsing them at a running
/// firmware can leave the unit unresponsive.
///
/// The command opens the port at [`DEFAULT_BOOTROM_BAUD`], sends the single
/// byte through [`DownloadAgent::restart`], flushes, and returns. It reads
/// nothing and sends no other opcode, so it cannot confirm that the restart
/// happened; the result says so and names `fw dump` as the check.
///
/// ⚠️ 未検証 on hardware: no unit has been sent a standalone `0xD9` after a
/// failed DA phase (`docs/dev/05-verification.md` §2); the fake is the only
/// device the command has run against.
///
/// # Errors
///
/// * [`Error::Port`] when no plausible port can be opened.
/// * [`Error::Io`] when the byte cannot be written.
pub fn da_restart(options: &DaRestartOptions, session: Session<'_>) -> Result<()> {
    let port = link::resolve(session.links, &options.port)?;
    let transport = session.links.open(&port, DEFAULT_BOOTROM_BAUD)?;
    let mut da = DownloadAgent::new(transport);
    session
        .console
        .note(&format!("sending the one-byte 0xD9 restart to {port}"));
    da.restart()?;
    session.console.result(
        "DA restart sent; no flash was written. 0xD9 has no reply, so the restart cannot be \
         confirmed from this command: run `m241 fw dump` to check the unit answers.",
    );
    Ok(())
}

/// `m241 fw probe-da`: load the DA, read the two candidate addresses, say which one
/// holds the image.
///
/// Writes **no** flash. The only writes in the whole path are the BootROM's
/// `0xA1` frames that put the DA into the chip's RAM, and the DA is gone at the
/// next power-on. `0xD3` / `0xD4` / `0xD5` / `0xD9` are never sent; a test drives
/// the same path against the fake and asserts the fake saw none of them
/// (`probe_da_reads_both_candidates_through_the_fake_and_sends_no_write_opcode`
/// in this module's tests, over the fake in [`crate::testing::fake_da`]).
///
/// The candidate addresses come from [`m241_flash::da::read_candidates`]; this
/// function has no address of its own.
///
/// # Errors
///
/// * [`Error::Safety`] without `--yes`, or for a DA file that is not the pinned
///   DA (wrong size, wrong first bytes, wrong full SHA-256). Both run before a
///   port is opened.
/// * [`Error::Io`] when the DA file cannot be read, [`Error::State`] when the
///   verdict is asked for addresses other than the two candidates.
/// * [`Error::Port`] when no plausible port can be opened, [`Error::Timeout`] /
///   [`Error::Framing`] when the unit or the DA does not answer as the protocol
///   says.
/// * The error from writing the result, when stdout is gone.
pub fn probe_da(options: &ProbeDaOptions, session: Session<'_>) -> Result<()> {
    // Everything that can be checked without a device is checked first.
    let confirmation = Confirmation::of_probe_command_line(options.yes)?;
    let da = fileio::read(&options.da)?;
    check_da_image_against(&da, da_digest())?;

    let port = link::resolve(session.links, &options.port)?;
    session.console.note(&format!(
        "loading the DA into RAM on {port} (this writes no flash), then reading {} bytes at \
         0x{:08X} and 0x{:08X}",
        PROBE_PAGE_SIZE, PROBE_ADDRESSES[0], PROBE_ADDRESSES[1]
    ));

    let transport = enter_bootrom(options.assume_brom, session, &port)?;
    let mut rom = MtkBootRom::new(transport);
    rom.sync()?;
    session
        .console
        .note("BootROM sync answered; writing the DA into the chip's RAM");

    // The loader borrows the transport from the synced client; the DA
    // conversation then takes it back, so the port has one owner throughout.
    let mut loader = DaLoader::new(rom.transport_mut());
    let id = loader.load_against(confirmation, &da, da_digest())?;
    session.console.note(&format!(
        "DA v{}.{} answered (region 0x{:02X}, flash {:04X}:{:04X}, {} bytes); reading",
        id.version >> 8,
        id.version & 0xFF,
        id.region,
        id.manufacturer,
        id.device,
        id.flash_bytes
    ));

    let mut da_session = DownloadAgent::new(rom.into_transport());
    let results = read_candidates(&mut da_session);
    let candidates: Vec<ProbeCandidate> = PROBE_ADDRESSES
        .into_iter()
        .zip(results)
        .map(|(address, page)| {
            if let Err(err) = &page {
                session.console.warn(&format!(
                    "0xD6 at 0x{address:08X} failed: {err}. Recorded as not read; a failed read \
                     is not a mismatch"
                ));
            }
            ProbeCandidate { address, page }
        })
        .collect();

    let verdict = base_verdict(&candidates)?;
    print_probe_result(options, session.console, &port, &id, &candidates, verdict)
}

/// Format the `fw probe-da` finding. Only the first four bytes of each page take
/// part in the verdict; the rest is printed for a person to read.
///
/// # Errors
///
/// Whatever [`Console::json`] returns when the document cannot be written.
fn print_probe_result(
    options: &ProbeDaOptions,
    console: &dyn crate::output::Console,
    port: &str,
    id: &DaLoadId,
    candidates: &[ProbeCandidate],
    verdict: BaseVerdict,
) -> Result<()> {
    if options.json {
        let probe: Vec<serde_json::Value> = candidates
            .iter()
            .map(|candidate| {
                serde_json::json!({
                    "address": format!("0x{:08X}", candidate.address),
                    "read": candidate.was_read(),
                    "read_error": match &candidate.page {
                        Ok(_) => serde_json::Value::Null,
                        Err(err) => serde_json::Value::String(format!("{err}")),
                    },
                    "first_bytes": candidate.first_bytes_hex(),
                    "vector_match": candidate.vector_matches(),
                })
            })
            .collect();
        let settled = match verdict {
            BaseVerdict::Settled(address) => Some(format!("0x{address:08X}")),
            BaseVerdict::Inconclusive(_) => None,
        };
        console.json(&serde_json::json!({
            "port": port,
            "da": {
                "version": format!("0x{:04X}", id.version),
                "region": format!("0x{:02X}", id.region),
                "manufacturer": format!("0x{:04X}", id.manufacturer),
                "device": format!("0x{:04X}", id.device),
                "flash_bytes": id.flash_bytes,
            },
            "vector": hex_bytes(&VECTOR),
            "vector_verified_through_da": false,
            "probe": probe,
            "verdict": verdict.describe(),
            "conclusion": match verdict {
                BaseVerdict::Settled(_) => "settled",
                BaseVerdict::Inconclusive(Inconclusive::Unread { .. }) => "unread",
                BaseVerdict::Inconclusive(Inconclusive::NoMatch) => "no_match",
                BaseVerdict::Inconclusive(Inconclusive::Ambiguous) => "ambiguous",
            },
            "firmware_base": settled,
            "wrote_flash": false,
        }))?;
        return Ok(());
    }
    console.result(&format!("{:<12}: {port}", "port"));
    console.result(&format!(
        "{:<12}: 0x{:04X} (region 0x{:02X}, {:04X}:{:04X}, {} bytes)",
        "DA", id.version, id.region, id.manufacturer, id.device, id.flash_bytes
    ));
    for candidate in candidates {
        console.result(&describe_candidate(candidate));
    }
    console.result(&format!("{:<12}: {}", "verdict", verdict.describe()));
    console.result(&format!("{:<12}: no", "flash write"));
    console.note(
        "the DA was loaded into the chip's RAM and reads only; no flash write, erase or \
         restart command was sent. Power-cycling the unit removes the DA.",
    );
    Ok(())
}

/// One candidate as two lines: what was read and whether it matched, then the
/// first sixteen bytes for a person to look at (they take no part in the
/// verdict).
fn describe_candidate(candidate: &ProbeCandidate) -> String {
    let head = format!("0x{:08X}", candidate.address);
    match &candidate.page {
        Err(err) => format!("{head:<12}: not read — {err}"),
        Ok(page) => format!(
            "{head:<12}: read, first bytes {} ({})  first 16: {}",
            candidate.first_bytes_hex(),
            if candidate.vector_matches() {
                format!("matches the vector {}", hex_bytes(&VECTOR))
            } else {
                String::from("does not match the vector")
            },
            hex_bytes(page.get(..16).unwrap_or_default())
        ),
    }
}

/// Upper-case hex, space-separated, as a person reads a flash dump.
fn hex_bytes(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Timings;
    use crate::testing::da_write;
    use crate::testing::fake_da;
    use crate::testing::{AutoConfirm, DeviceHandle, FakePorts, RecordingConsole, synthetic_image};
    use m241_core::protocol::simulator::FakeDevice;
    use m241_flash::da::{FLASH_WRITE_END, FLASH_WRITE_LENGTH};
    use std::cell::{Cell, RefCell};
    use std::path::{Path, PathBuf};
    use std::rc::Rc;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("m241-cli-fw-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("scratch");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn join(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn dump_options(output: &Path) -> DumpOptions {
        DumpOptions {
            port: String::from("auto"),
            output: output.to_path_buf(),
            length: crate::cli::DEFAULT_DUMP_LENGTH,
            assume_brom: false,
            force: false,
            json: false,
        }
    }

    /// A factory for a fake unit that is streaming NMEA with `image` loaded.
    fn streaming_unit(image: Vec<u8>) -> impl Fn() -> FakeDevice + 'static {
        move || FakeDevice::new().with_firmware(&image).with_nmea(4)
    }

    #[test]
    fn patch_dry_run_plans_three_bytes_and_writes_nothing() {
        let scratch = Scratch::new("patch-dry");
        let source = scratch.join("stock.bin");
        let output = scratch.join("fixed.bin");
        fileio::write(&source, &synthetic_image()).expect("write source");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let session = Session::new(&links, &console, &AutoConfirm);

        patch(
            &PatchOptions {
                source: source.clone(),
                output: Some(output.clone()),
                dry_run: true,
                json: false,
            },
            session,
        )
        .expect("dry run");
        assert!(!output.exists(), "a dry run writes nothing");
        let text = console.stdout();
        assert!(text.contains("0x402F6"), "got {text}");
        assert!(text.contains("0x402F7"), "got {text}");
        assert!(text.contains("0x59C2C"), "got {text}");
        assert!(text.contains("changes     : 3"), "got {text}");
        assert!(
            console.stderr().contains("nothing was written"),
            "got {}",
            console.stderr()
        );
    }

    #[test]
    fn patch_writes_a_verified_image_and_leaves_the_source_alone() {
        let scratch = Scratch::new("patch-apply");
        let source = scratch.join("stock.bin");
        let output = scratch.join("fixed.bin");
        let stock = synthetic_image();
        fileio::write(&source, &stock).expect("write source");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let session = Session::new(&links, &console, &AutoConfirm);

        patch(
            &PatchOptions {
                source: source.clone(),
                output: Some(output.clone()),
                dry_run: false,
                json: false,
            },
            session,
        )
        .expect("patch");

        assert_eq!(
            fileio::read(&source).expect("source"),
            stock,
            "the source image must be untouched"
        );
        let patched = fileio::read(&output).expect("patched");
        assert_eq!(patched.len(), stock.len());
        let image = FirmwareImage::new(&patched).expect("header");
        assert!(image.is_patched());
        assert_eq!(image.effective_epoch(), Some(GPS_EPOCH_UNIX_WNRO));
        // Exactly three bytes differ: the two high epoch bytes and one digit.
        let changed: Vec<usize> = stock
            .iter()
            .zip(patched.iter())
            .enumerate()
            .filter(|(_, (before, after))| before != after)
            .map(|(index, _)| index)
            .collect();
        assert_eq!(changed, vec![0x402F6, 0x402F7, 0x59C2C]);
    }

    #[test]
    fn patch_json_is_one_object_naming_every_edit() {
        let scratch = Scratch::new("patch-json");
        let source = scratch.join("stock.bin");
        fileio::write(&source, &synthetic_image()).expect("write source");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let session = Session::new(&links, &console, &AutoConfirm);
        patch(
            &PatchOptions {
                source,
                output: None,
                dry_run: true,
                json: true,
            },
            session,
        )
        .expect("dry run");
        let document: serde_json::Value =
            serde_json::from_str(&console.stdout()).expect("one JSON object");
        assert_eq!(document["profile"], "V1.13");
        assert_eq!(document["changes"], 3);
        assert_eq!(document["edits"][0]["offset"], "0x402F6");
        assert_eq!(document["written"], serde_json::Value::Null);
    }

    #[test]
    fn patch_refuses_to_rewrite_its_own_input() {
        let scratch = Scratch::new("patch-inplace");
        let source = scratch.join("stock.bin");
        fileio::write(&source, &synthetic_image()).expect("write source");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let session = Session::new(&links, &console, &AutoConfirm);
        let err = patch(
            &PatchOptions {
                source: source.clone(),
                output: Some(source),
                dry_run: false,
                json: false,
            },
            session,
        )
        .expect_err("same file");
        assert!(matches!(err, Error::Safety { .. }), "got {err}");
    }

    #[test]
    fn patch_of_an_already_patched_image_is_refused_and_says_why() {
        let scratch = Scratch::new("patch-repeat");
        let source = scratch.join("stock.bin");
        let patched_path = scratch.join("fixed.bin");
        let again = scratch.join("fixed-again.bin");
        fileio::write(&source, &synthetic_image()).expect("write source");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let session = Session::new(&links, &console, &AutoConfirm);
        patch(
            &PatchOptions {
                source: source.clone(),
                output: Some(patched_path.clone()),
                dry_run: false,
                json: false,
            },
            session,
        )
        .expect("first patch");

        // `docs/reference/02-cli.md` says a real run refuses an already-patched
        // image: the user is about to flash something they believe this command
        // changed, so stopping is the only answer that cannot mislead.
        let console = RecordingConsole::default();
        let session = Session::new(&links, &console, &AutoConfirm);
        let err = patch(
            &PatchOptions {
                source: patched_path.clone(),
                output: Some(again.clone()),
                dry_run: false,
                json: false,
            },
            session,
        )
        .expect_err("an already-patched image is a refusal");
        assert!(matches!(err, Error::Firmware { .. }), "got {err}");
        assert!(format!("{err}").contains("already patched"), "got {err}");
        assert_eq!(crate::exit::exit_code_for(&err), 4);
        assert!(!again.exists(), "a refusal writes nothing");

        // `--dry-run` follows the same refusal rule so it accurately predicts
        // what a real `fw patch` run would do.
        let console = RecordingConsole::default();
        let session = Session::new(&links, &console, &AutoConfirm);
        let dry_err = patch(
            &PatchOptions {
                source: patched_path,
                output: None,
                dry_run: true,
                json: false,
            },
            session,
        )
        .expect_err("a dry run of an already-patched image is also refused");
        assert!(matches!(dry_err, Error::Firmware { .. }), "got {dry_err}");
        assert!(
            format!("{dry_err}").contains("already patched"),
            "got {dry_err}"
        );
        assert_eq!(crate::exit::exit_code_for(&dry_err), 4);
    }

    #[test]
    fn patch_apply_json_is_one_object_naming_the_written_file() {
        let scratch = Scratch::new("patch-apply-json");
        let source = scratch.join("stock.bin");
        let output = scratch.join("fixed.bin");
        fileio::write(&source, &synthetic_image()).expect("write source");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let session = Session::new(&links, &console, &AutoConfirm);
        patch(
            &PatchOptions {
                source,
                output: Some(output.clone()),
                dry_run: false,
                json: true,
            },
            session,
        )
        .expect("patch --json");
        let document: serde_json::Value =
            serde_json::from_str(&console.stdout()).expect("stdout must be one valid JSON object");
        assert_eq!(document["profile"], "V1.13");
        assert_eq!(document["changes"], 3);
        assert_eq!(document["written"], output.display().to_string());
    }

    #[test]
    fn patch_refuses_an_image_with_an_unexpected_epoch_constant() {
        use m241_core::firmware::patch::INACTIVE_EPOCH_OFFSET;

        for (name, offset) in [
            ("bad-effective", EFFECTIVE_EPOCH_OFFSET),
            ("bad-inactive", INACTIVE_EPOCH_OFFSET),
        ] {
            let scratch = Scratch::new(name);
            let source = scratch.join("altered.bin");
            let mut image = synthetic_image();
            image[offset..offset + 4].copy_from_slice(&0x1234_5678u32.to_le_bytes());
            fileio::write(&source, &image).expect("write altered");
            let links = FakePorts::empty();
            let console = RecordingConsole::default();
            let session = Session::new(&links, &console, &AutoConfirm);
            let err = patch(
                &PatchOptions {
                    source,
                    output: Some(scratch.join("fixed.bin")),
                    dry_run: false,
                    json: false,
                },
                session,
            )
            .expect_err("unexpected epoch constant must be refused");
            assert!(matches!(err, Error::Firmware { .. }), "{name}: got {err}");
            assert_eq!(crate::exit::exit_code_for(&err), 4, "{name}: got {err}");
        }
    }

    #[test]
    fn patch_of_a_non_firmware_file_is_a_profile_error() {
        let scratch = Scratch::new("patch-garbage");
        let source = scratch.join("garbage.bin");
        fileio::write(&source, &vec![0u8; 1_000]).expect("write");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let session = Session::new(&links, &console, &AutoConfirm);
        let err = patch(
            &PatchOptions {
                source,
                output: None,
                dry_run: true,
                json: false,
            },
            session,
        )
        .expect_err("not an image");
        assert_eq!(crate::exit::exit_code_for(&err), 4, "got {err}");
    }

    #[test]
    fn an_image_one_byte_short_of_its_profile_is_refused_not_patched() {
        let scratch = Scratch::new("patch-short");
        let source = scratch.join("short.bin");
        let mut image = synthetic_image();
        image.truncate(image.len() - 1);
        fileio::write(&source, &image).expect("write");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let session = Session::new(&links, &console, &AutoConfirm);
        let err = patch(
            &PatchOptions {
                source,
                output: None,
                dry_run: true,
                json: false,
            },
            session,
        )
        .expect_err("a short dump is not a profile");
        assert_eq!(crate::exit::exit_code_for(&err), 4, "got {err}");
    }

    /// The confirmation that performs the real power cycle, on the fake chip.
    ///
    /// It records the messages it was given, because the prompt's wording is
    /// part of the contract: the user has to know to remove the battery, not
    /// merely to "continue".
    #[derive(Debug)]
    struct PowerCycle {
        device: DeviceHandle,
        messages: Rc<RefCell<Vec<String>>>,
    }

    impl PowerCycle {
        fn new(device: DeviceHandle) -> Self {
            Self {
                device,
                messages: Rc::new(RefCell::new(Vec::new())),
            }
        }

        fn messages(&self) -> Vec<String> {
            self.messages.borrow().clone()
        }
    }

    impl crate::Confirm for PowerCycle {
        fn pause(&self, message: &str) -> m241_core::Result<()> {
            self.messages.borrow_mut().push(message.to_owned());
            self.device.power_cycle();
            Ok(())
        }
    }

    #[test]
    fn dump_reads_two_identical_passes_and_saves_the_image() {
        let scratch = Scratch::new("dump-ok");
        let output = scratch.join("stock.bin");
        let handle = DeviceHandle::new(streaming_unit(synthetic_image()));
        let links = FakePorts::shared(handle.clone());
        let console = RecordingConsole::default();
        // The confirmation step is a power cycle, which is exactly what removing
        // the battery does to the real unit.
        let confirm = PowerCycle::new(handle.clone());
        let session = Session::new(&links, &console, &confirm);

        dump(&dump_options(&output), session).expect("two reads agree");
        let saved = fileio::read(&output).expect("saved image");
        assert_eq!(saved.len(), crate::cli::DEFAULT_DUMP_LENGTH);
        assert_eq!(saved, synthetic_image());
        assert_eq!(
            saved,
            handle.memory(),
            "the file must be the bytes the chip answered with"
        );
        let text = console.stdout();
        assert!(text.contains("verified"), "got {text}");
        assert!(text.contains("V1.13"), "got {text}");
        assert!(text.contains(&sha256_hex(&synthetic_image())), "got {text}");
        assert!(
            console.stderr().contains("$PMTK180"),
            "the entry route is reported: {}",
            console.stderr()
        );
        let prompts = confirm.messages();
        let prompt = prompts.first().expect("the power-cycle prompt was shown");
        assert!(prompt.contains("Power-cycle"), "got {prompt}");
        assert!(prompt.contains("second read"), "got {prompt}");
        assert!(
            handle.commands().iter().any(|command| command == "PMTK180"),
            "the BootROM was entered with the documented sentence: {:?}",
            handle.commands()
        );
    }

    #[test]
    fn a_second_read_that_differs_is_verification_failure_and_writes_nothing() {
        let scratch = Scratch::new("dump-mismatch");
        let output = scratch.join("stock.bin");
        // Each power cycle hands out a different image, which is what an unstable
        // read looks like.
        let calls = Rc::new(Cell::new(0u32));
        let factory = {
            let calls = calls.clone();
            move || {
                let mut image = synthetic_image();
                let round = calls.get();
                calls.set(round + 1);
                if round > 0
                    && let Some(byte) = image.get_mut(0x500)
                {
                    *byte ^= 0xFF;
                }
                FakeDevice::new().with_firmware(&image).with_nmea(4)
            }
        };
        let handle = DeviceHandle::new(factory);
        let links = FakePorts::shared(handle.clone());
        let console = RecordingConsole::default();
        let confirm = PowerCycle::new(handle.clone());
        let session = Session::new(&links, &console, &confirm);

        let err = dump(&dump_options(&output), session).expect_err("the two reads differ");
        assert!(matches!(err, Error::Framing { .. }), "got {err}");
        assert!(format!("{err}").contains("two dumps differ"), "got {err}");
        assert_eq!(crate::exit::exit_code_for(&err), 4);
        assert!(!output.exists(), "a mismatch must not leave a file behind");
    }

    #[test]
    fn dump_refuses_to_overwrite_an_existing_backup() {
        let scratch = Scratch::new("dump-clobber");
        let output = scratch.join("stock.bin");
        fileio::write(&output, b"previous").expect("write");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let session = Session::new(&links, &console, &AutoConfirm);
        let err = dump(&dump_options(&output), session).expect_err("must refuse");
        assert!(matches!(err, Error::Safety { .. }), "got {err}");
        assert_eq!(crate::exit::exit_code_for(&err), 5);
        assert_eq!(fileio::read(&output).expect("still there"), b"previous");
    }

    #[test]
    fn dump_refuses_an_odd_length_before_opening_a_port() {
        let scratch = Scratch::new("dump-odd");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let session = Session::new(&links, &console, &AutoConfirm);
        let mut options = dump_options(&scratch.join("stock.bin"));
        options.length = 4_001;
        let err = dump(&options, session).expect_err("odd");
        assert!(format!("{err}").contains("odd"), "got {err}");
    }

    #[test]
    fn dump_reports_a_port_error_as_exit_code_three() {
        let scratch = Scratch::new("dump-noport");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let session = Session::new(&links, &console, &AutoConfirm);
        let err = dump(&dump_options(&scratch.join("stock.bin")), session).expect_err("no port");
        assert_eq!(crate::exit::exit_code_for(&err), 3, "got {err}");
    }

    #[test]
    fn dump_on_silent_line_attempts_sync_and_times_out_when_not_in_bootrom() {
        // Silence (`StreamState::Silent`) is safe to probe for BootROM sync
        // rather than refusing on line state. Because the fake here is not in
        // BootROM mode, the four-probe sync is what fails.
        let scratch = Scratch::new("dump-silent-timeout");
        let links = FakePorts::single(|| FakeDevice::new().without_nmea());
        let console = RecordingConsole::default();
        let session = Session::new(&links, &console, &AutoConfirm).with_timings(Timings::fast());
        let err = dump(&dump_options(&scratch.join("stock.bin")), session)
            .expect_err("a silent unit not in BootROM cannot answer sync");
        assert!(matches!(err, Error::Timeout { .. }), "got {err}");
        assert!(
            !format!("{err}").contains("no complete NMEA"),
            "silence is not the Unknown state: {err}"
        );
    }

    #[test]
    fn dump_refuses_unknown_line_state_with_safety_error() {
        use std::collections::VecDeque;

        #[derive(Debug)]
        struct GarbageLineTransport {
            bytes: VecDeque<u8>,
        }

        impl Transport for GarbageLineTransport {
            fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
                let mut count = 0;
                while count < buf.len() {
                    let Some(byte) = self.bytes.pop_front() else {
                        break;
                    };
                    buf[count] = byte;
                    count += 1;
                }
                Ok(count)
            }

            fn write_all(&mut self, _buf: &[u8]) -> Result<()> {
                Ok(())
            }

            fn flush(&mut self) -> Result<()> {
                Ok(())
            }

            fn clear_input(&mut self) -> Result<()> {
                self.bytes.clear();
                Ok(())
            }

            fn set_baud(&mut self, _baud: u32) -> Result<()> {
                Ok(())
            }

            fn set_control_lines(&mut self, _asserted: bool) -> Result<()> {
                Ok(())
            }
        }

        struct GarbageLineLinks;

        impl crate::Links for GarbageLineLinks {
            fn ports(&self) -> Result<Vec<crate::link::PortEntry>> {
                Ok(vec![crate::link::PortEntry {
                    name: String::from("SIM0"),
                    cable: Some(crate::link::Cable::Cp210x),
                    likely: true,
                    description: None,
                }])
            }

            fn open(&self, _name: &str, _baud: u32) -> Result<Opened> {
                Ok(Opened::new(Box::new(GarbageLineTransport {
                    bytes: VecDeque::from(b"noise-without-dollar\r\n".to_vec()),
                })))
            }
        }

        let scratch = Scratch::new("dump-unknown");
        let output = scratch.join("stock.bin");
        let links = GarbageLineLinks;
        let console = RecordingConsole::default();
        let session = Session::new(&links, &console, &AutoConfirm).with_timings(Timings::fast());
        let err = dump(&dump_options(&output), session)
            .expect_err("unclassifiable line state must be refused");
        assert!(matches!(err, Error::Safety { .. }), "got {err}");
        assert_eq!(crate::exit::exit_code_for(&err), 5, "got {err}");
        assert!(format!("{err}").contains("no complete NMEA"), "got {err}");
        assert!(!output.exists());
    }

    #[test]
    fn dump_json_is_one_object_with_clean_stdout() {
        let scratch = Scratch::new("dump-json");
        let output = scratch.join("stock.bin");
        let handle = DeviceHandle::new(streaming_unit(synthetic_image()));
        let links = FakePorts::shared(handle.clone());
        let console = RecordingConsole::default();
        let confirm = PowerCycle::new(handle);
        let session = Session::new(&links, &console, &confirm);
        let mut options = dump_options(&output);
        options.json = true;

        dump(&options, session).expect("dump --json");
        let stdout = console.stdout();
        let document: serde_json::Value =
            serde_json::from_str(&stdout).expect("stdout must be a single valid JSON object");
        assert_eq!(document["verified"], true);
        assert_eq!(document["profile"], "V1.13");
        assert_eq!(document["size"], crate::cli::DEFAULT_DUMP_LENGTH);
        assert_eq!(document["sha256"], sha256_hex(&synthetic_image()));
        assert_eq!(document["output"], output.display().to_string());
        assert!(!console.stderr().is_empty(), "progress belongs on stderr");
    }

    #[test]
    fn dump_succeeds_with_assume_brom_and_on_silent_bootrom_line() {
        for (name, assume_brom) in [("dump-assume-brom", true), ("dump-silent-brom", false)] {
            let scratch = Scratch::new(name);
            let output = scratch.join("stock.bin");
            let expected = synthetic_image();
            let handle = DeviceHandle::new({
                let expected = expected.clone();
                move || FakeDevice::new().with_firmware(&expected).in_bootrom()
            });
            let links = FakePorts::shared(handle.clone());
            let console = RecordingConsole::default();
            let confirm = PowerCycle::new(handle);
            // 500 ms, not `fast()`'s 20 ms: the fake answers synchronously but
            // loses replies when the workspace tests run in parallel.
            let session =
                Session::new(&links, &console, &confirm).with_timings(Timings::generous());
            let mut options = dump_options(&output);
            options.assume_brom = assume_brom;

            dump(&options, session).expect("bootrom dump succeeds");
            assert_eq!(fileio::read(&output).expect("saved"), expected);
            if !assume_brom {
                assert!(
                    console.stderr().contains("no NMEA stream"),
                    "silent route should be noted on stderr: {}",
                    console.stderr()
                );
            }
        }
    }

    #[test]
    fn declared_size_note_fires_only_for_length_minus_four_and_warns_otherwise() {
        let stock = synthetic_image();
        let image = FirmwareImage::new(&stock).expect("header");
        let expected_console = RecordingConsole::default();
        report_declared_size(&expected_console, &image, stock.len());
        let expected_stderr = expected_console.stderr();
        assert!(
            expected_stderr.contains("so this is expected"),
            "got {expected_stderr}"
        );
        assert!(
            !expected_stderr.contains("warning:"),
            "length - 4 should be a note, not a warning: {expected_stderr}"
        );

        let mut mismatch = synthetic_image();
        mismatch[FW_SIZE_OFFSET..FW_SIZE_OFFSET + 4].copy_from_slice(&100_000u32.to_le_bytes());
        let mismatch_image = FirmwareImage::new(&mismatch).expect("header");
        let warn_console = RecordingConsole::default();
        report_declared_size(&warn_console, &mismatch_image, mismatch.len());
        let warn_stderr = warn_console.stderr();
        assert!(
            warn_stderr.contains("warning:"),
            "unexpected declared size must warn: {warn_stderr}"
        );
        assert!(
            !warn_stderr.contains("so this is expected"),
            "unexpected declared size must not claim to be expected: {warn_stderr}"
        );
    }

    #[test]
    fn dump_refuses_an_overflowing_length_before_opening_a_port() {
        let scratch = Scratch::new("dump-overflow");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let session = Session::new(&links, &console, &AutoConfirm);
        let mut options = dump_options(&scratch.join("stock.bin"));
        // `0xFFE0_0000` is even and equals `2^32 - BASE_ADDR_FIRMWARE`, so
        // `BASE_ADDR_FIRMWARE + length` overflows `u32`.
        options.length = (u32::MAX - BASE_ADDR_FIRMWARE) as usize + 1;
        let err = dump(&options, session).expect_err("overflowing length must fail");
        assert!(format!("{err}").contains("overflow"), "got {err}");
    }

    // -- fw probe-da -------------------------------------------------------

    /// A page that starts with the M-241 vector table and nothing else set.
    ///
    /// Only the first byte is followed by zeros: a four-byte vector compare
    /// must not pass on `18 00 00 00`, so the remaining three bytes of the
    /// vector have to differ from the filler around them.
    fn vector_page() -> Vec<u8> {
        let mut page = vec![0x00u8; PROBE_PAGE_SIZE];
        page.get_mut(..VECTOR.len())
            .unwrap()
            .copy_from_slice(&VECTOR);
        page
    }

    /// A page whose first byte is the vector's but whose rest is not.
    fn one_byte_of_vector() -> Vec<u8> {
        let mut page = vec![0x00u8; PROBE_PAGE_SIZE];
        if let Some(first) = page.first_mut() {
            *first = VECTOR[0];
        }
        page
    }

    /// A candidate slot: `Ok(page)` or `Err(reason)`.
    fn candidate(slot: usize, page: Result<Vec<u8>>) -> ProbeCandidate {
        ProbeCandidate {
            address: PROBE_ADDRESSES[slot],
            page,
        }
    }

    fn settled(candidates: &[ProbeCandidate]) -> Result<BaseVerdict> {
        base_verdict(candidates)
    }

    #[test]
    fn the_probe_verdict_settles_only_on_exactly_one_vector_match() {
        let page = vector_page();
        let other = vec![0x00u8; PROBE_PAGE_SIZE];
        // One match: settled, and it names the address that matched.
        assert_eq!(
            settled(&[
                candidate(0, Ok(other.clone())),
                candidate(1, Ok(page.clone())),
            ])
            .expect("two candidates"),
            BaseVerdict::Settled(PROBE_ADDRESSES[1])
        );
        assert_eq!(
            settled(&[
                candidate(0, Ok(page.clone())),
                candidate(1, Ok(other.clone())),
            ])
            .expect("two candidates"),
            BaseVerdict::Settled(PROBE_ADDRESSES[0])
        );
        // Both match: two answers is not a tie this tool breaks.
        assert_eq!(
            settled(&[
                candidate(0, Ok(page.clone())),
                candidate(1, Ok(page.clone())),
            ])
            .expect("two candidates"),
            BaseVerdict::Inconclusive(Inconclusive::Ambiguous)
        );
        // Neither matches: a real answer, not a failed read.
        assert_eq!(
            settled(&[
                candidate(0, Ok(other.clone())),
                candidate(1, Ok(other.clone())),
            ])
            .expect("two candidates"),
            BaseVerdict::Inconclusive(Inconclusive::NoMatch)
        );
    }

    #[test]
    fn a_candidate_that_was_not_read_blocks_settling_even_when_the_other_matches() {
        // The findings of this branch's first review: "unread" was read as
        // "mismatch", so a timed-out first candidate plus a matching second one
        // produced `Settled`. Unread says nothing about contents, so it must
        // block the verdict in both slots.
        let page = vector_page();
        let unread = || -> Result<Vec<u8>> {
            Err(Error::Timeout {
                what: String::from("0xD6 page 0"),
                timeout: m241_core::error::TimeoutDuration::from_std(
                    std::time::Duration::from_secs(5),
                ),
            })
        };
        assert_eq!(
            settled(&[candidate(0, unread()), candidate(1, Ok(page.clone()))])
                .expect("two candidates"),
            BaseVerdict::Inconclusive(Inconclusive::Unread {
                read: [false, true]
            }),
            "an unread first candidate must not let the second one settle the address"
        );
        assert_eq!(
            settled(&[candidate(0, Ok(page.clone())), candidate(1, unread())])
                .expect("two candidates"),
            BaseVerdict::Inconclusive(Inconclusive::Unread {
                read: [true, false]
            })
        );
        assert_eq!(
            settled(&[candidate(0, unread()), candidate(1, unread())]).expect("two candidates"),
            BaseVerdict::Inconclusive(Inconclusive::Unread {
                read: [false, false]
            })
        );
        let text = BaseVerdict::Inconclusive(Inconclusive::Unread {
            read: [false, true],
        })
        .describe();
        assert!(text.contains("inconclusive"), "got {text}");
        assert!(text.contains("not read"), "got {text}");
        assert!(
            text.contains(&format!("0x{:08X}", PROBE_ADDRESSES[0])),
            "the unread address is named: {text}"
        );
        assert!(
            !text.contains(&format!("0x{:08X}", PROBE_ADDRESSES[1])),
            "the address that was read is not reported as missing: {text}"
        );
        // The other two reasons read differently, so a reader can tell them
        // apart in a log.
        let no_match = BaseVerdict::Inconclusive(Inconclusive::NoMatch).describe();
        let ambiguous = BaseVerdict::Inconclusive(Inconclusive::Ambiguous).describe();
        assert!(no_match.contains("neither"), "got {no_match}");
        assert!(ambiguous.contains("both"), "got {ambiguous}");
        assert_ne!(no_match, text);
        assert_ne!(ambiguous, text);
    }

    #[test]
    fn the_probe_verdict_compares_all_four_vector_bytes() {
        // A page that shares only the vector's first byte is not the vector.
        // Comparing one byte would pass here; comparing all four cannot.
        let page = vector_page();
        assert_eq!(
            settled(&[
                candidate(0, Ok(one_byte_of_vector())),
                candidate(1, Ok(vec![0x00u8; PROBE_PAGE_SIZE])),
            ])
            .expect("two candidates"),
            BaseVerdict::Inconclusive(Inconclusive::NoMatch),
            "18 00 00 00 is not 18 F0 9F E5"
        );
        // And a page that differs in the last vector byte only.
        let mut almost = page.clone();
        if let Some(slot) = almost.get_mut(VECTOR.len() - 1) {
            *slot ^= 0xFF;
        }
        assert_eq!(
            settled(&[
                candidate(0, Ok(almost)),
                candidate(1, Ok(vec![0u8; PROBE_PAGE_SIZE]))
            ])
            .expect("two candidates"),
            BaseVerdict::Inconclusive(Inconclusive::NoMatch),
            "a difference in the fourth byte must be seen"
        );
    }

    #[test]
    fn the_probe_verdict_refuses_anything_but_the_two_candidate_slots() {
        // The verdict is the only place in the CLI that decides an address, so
        // it refuses a list that is not exactly `PROBE_ADDRESSES`: a command
        // that read one candidate, or a third address, fails here instead of
        // producing a verdict from half the evidence.
        let page = vector_page();
        let only_one = [candidate(0, Ok(page.clone()))];
        assert!(matches!(settled(&only_one), Err(Error::State(_))));
        let wrong_order = [
            ProbeCandidate {
                address: PROBE_ADDRESSES[1],
                page: Ok(page.clone()),
            },
            ProbeCandidate {
                address: PROBE_ADDRESSES[0],
                page: Ok(vec![0u8; PROBE_PAGE_SIZE]),
            },
        ];
        assert!(matches!(settled(&wrong_order), Err(Error::State(_))));
        let third = [
            candidate(0, Ok(vec![0u8; PROBE_PAGE_SIZE])),
            ProbeCandidate {
                address: PROBE_ADDRESSES[1] + 4,
                page: Ok(page),
            },
        ];
        assert!(matches!(settled(&third), Err(Error::State(_))));
    }

    fn probe_options(da: &Path) -> ProbeDaOptions {
        ProbeDaOptions {
            da: da.to_path_buf(),
            port: String::from("auto"),
            yes: false,
            assume_brom: false,
            json: false,
        }
    }

    fn da_load_id() -> DaLoadId {
        DaLoadId {
            version: 0x0308,
            region: 0x49,
            flash_bytes: 0x0008_0000,
            manufacturer: 0x00C2,
            device: 0x22BA,
        }
    }

    #[test]
    fn probe_da_refuses_without_yes_before_opening_a_port_or_reading_a_file() {
        let scratch = Scratch::new("probe-no-yes");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let session = Session::new(&links, &console, &AutoConfirm);
        // The file does not exist either, so a command that reached the read
        // would fail with an I/O error instead of a safety refusal.
        let err =
            probe_da(&probe_options(&scratch.join("absent.bin")), session).expect_err("no --yes");
        assert!(matches!(err, Error::Safety { .. }), "got {err}");
        let text = format!("{err}");
        assert!(text.contains("--yes"), "got {text}");
        // There is no typed prompt for this command, and the refusal must not
        // claim there is one.
        assert!(
            !text.contains("prompt") || text.contains("no typed prompt"),
            "the refusal must not offer a prompt that does not exist: {text}"
        );
        assert!(
            console.stderr().is_empty(),
            "nothing is announced before the refusal: {}",
            console.stderr()
        );
    }

    #[test]
    fn probe_da_refuses_a_da_of_the_wrong_size_before_opening_a_port() {
        let scratch = Scratch::new("probe-size");
        let path = scratch.join("DA_v3.8.4.bin");
        fileio::write(&path, &vec![0u8; 14_895]).expect("write");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let session = Session::new(&links, &console, &AutoConfirm);
        let mut options = probe_options(&path);
        options.yes = true;
        let err = probe_da(&options, session).expect_err("wrong size");
        assert!(matches!(err, Error::Safety { .. }), "got {err}");
        assert!(format!("{err}").contains("14896"), "got {err}");
    }

    #[test]
    fn probe_da_refuses_a_14896_byte_file_that_does_not_start_like_the_da() {
        // The size alone used to be the whole check. The same length filled
        // with zeros must be refused before the port step, not after a RAM
        // write and a jump.
        let scratch = Scratch::new("probe-head");
        let path = scratch.join("DA_v3.8.4.bin");
        fileio::write(&path, &vec![0u8; 14_896]).expect("write");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let session = Session::new(&links, &console, &AutoConfirm);
        let mut options = probe_options(&path);
        options.yes = true;
        let err = probe_da(&options, session).expect_err("not the DA");
        assert!(matches!(err, Error::Safety { .. }), "got {err}");
        let text = format!("{err}");
        assert!(
            text.contains("A0 E1"),
            "the refusal names the first bytes it wanted: {text}"
        );
        assert!(
            console.stderr().is_empty(),
            "the port step is never reached: {}",
            console.stderr()
        );
    }

    #[test]
    fn probe_da_with_the_test_fixture_da_reaches_the_port_step_and_refuses_without_one() {
        // The DA path itself is exercised against the fake in `m241-flash`; here
        // the point is that the command gets past its own refusals and then
        // reports the missing port as a port error, not a safety one. The fixture
        // carries a digest the test build pins (`da_digest`); the shipped binary
        // pins the vendor value instead and rejects this file.
        let scratch = Scratch::new("probe-port");
        let path = da_file(scratch.path());
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let session = Session::new(&links, &console, &AutoConfirm);
        let mut options = probe_options(&path);
        options.yes = true;
        let err = probe_da(&options, session).expect_err("no port is available");
        assert!(matches!(err, Error::Port { .. }), "got {err}");
        assert_eq!(crate::exit::exit_code_for(&err), 3, "got {err}");
    }

    #[test]
    fn probe_da_refuses_a_same_size_da_with_the_right_head_but_a_different_digest() {
        // Follow-up W2a: length and head used to be the whole gate, so any
        // 14,896-byte file starting `00 00 A0 E1` would be loaded and jumped to.
        // This file is exactly that — the fixture with one byte changed — and it
        // must be refused before the port step.
        let scratch = Scratch::new("probe-digest");
        let path = scratch.join("DA_v3.8.4.bin");
        let mut image = da_fixture_bytes();
        let last = image.last_mut().expect("a 14,896-byte image");
        *last ^= 0xFF;
        fileio::write(&path, &image).expect("write");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let session = Session::new(&links, &console, &AutoConfirm);
        let mut options = probe_options(&path);
        options.yes = true;
        let err = probe_da(&options, session).expect_err("a lookalike DA is not loaded");
        assert!(matches!(err, Error::Safety { .. }), "got {err}");
        let text = format!("{err}");
        assert!(
            text.contains("SHA-256"),
            "the refusal names the digest check: {text}"
        );
        assert!(
            console.stderr().is_empty(),
            "the port step is never reached: {}",
            console.stderr()
        );
    }

    #[test]
    fn probe_da_json_document_names_both_candidates_and_says_no_flash_write() {
        let scratch = Scratch::new("probe-json");
        let console = RecordingConsole::default();
        let candidates = vec![
            candidate(0, Ok(vector_page())),
            candidate(
                1,
                Err(Error::Timeout {
                    what: String::from("0xD6 page 0"),
                    timeout: m241_core::error::TimeoutDuration::from_std(
                        std::time::Duration::from_secs(5),
                    ),
                }),
            ),
        ];
        let verdict = base_verdict(&candidates).expect("two candidates");
        print_probe_result(
            &ProbeDaOptions {
                da: scratch.join("DA_v3.8.4.bin"),
                port: String::from("auto"),
                yes: true,
                assume_brom: false,
                json: true,
            },
            &console,
            "SIM0",
            &da_load_id(),
            &candidates,
            verdict,
        )
        .expect("json");
        let document: serde_json::Value =
            serde_json::from_str(&console.stdout()).expect("one JSON object");
        assert_eq!(document["wrote_flash"], false);
        assert_eq!(
            document["firmware_base"],
            serde_json::Value::Null,
            "one candidate was unread, so nothing is settled"
        );
        assert_eq!(document["conclusion"], "unread");
        assert_eq!(document["vector"], "18 F0 9F E5");
        assert_eq!(document["vector_verified_through_da"], false);
        assert_eq!(document["probe"][0]["read"], true);
        assert_eq!(document["probe"][0]["first_bytes"], "18 F0 9F E5");
        assert_eq!(document["probe"][0]["vector_match"], true);
        assert_eq!(document["probe"][1]["read"], false);
        assert_eq!(document["probe"][1]["vector_match"], false);
        assert!(
            document["probe"][1]["read_error"]
                .as_str()
                .is_some_and(|text| text.contains("0xD6 page 0")),
            "the read failure is carried, not just the absence: {}",
            document["probe"][1]["read_error"]
        );
        assert_eq!(document["da"]["version"], "0x0308");
    }

    #[test]
    fn probe_da_text_result_shows_read_state_first_bytes_and_the_match_for_both() {
        // The log a person pastes into `05-verification.md` has to state, per
        // candidate, whether it was read, its first four bytes and whether they
        // matched — not just a verdict line.
        let scratch = Scratch::new("probe-text");
        let console = RecordingConsole::default();
        let candidates = vec![
            candidate(0, Ok(vec![0x00u8; PROBE_PAGE_SIZE])),
            candidate(1, Ok(vector_page())),
        ];
        let verdict = base_verdict(&candidates).expect("two candidates");
        print_probe_result(
            &ProbeDaOptions {
                da: scratch.join("DA_v3.8.4.bin"),
                port: String::from("auto"),
                yes: true,
                assume_brom: false,
                json: false,
            },
            &console,
            "SIM0",
            &da_load_id(),
            &candidates,
            verdict,
        )
        .expect("text");
        let stdout = console.stdout();
        for address in PROBE_ADDRESSES {
            assert!(
                stdout.contains(&format!("0x{address:08X}")),
                "both candidates are printed: {stdout}"
            );
        }
        assert!(
            stdout.contains("read, first bytes 18 F0 9F E5 (matches the vector 18 F0 9F E5)"),
            "the matching candidate states its first bytes and the match: {stdout}"
        );
        assert!(
            stdout.contains("does not match the vector"),
            "the other candidate states the mismatch: {stdout}"
        );
        assert!(
            stdout.contains(&format!(
                "verdict     : the image is at 0x{:08X}",
                PROBE_ADDRESSES[1]
            )) || stdout.contains(&format!(
                "verdict: the image is at 0x{:08X}",
                PROBE_ADDRESSES[1]
            )),
            "the settled verdict names the address: {stdout}"
        );
        assert!(
            stdout.contains("flash write: no") || stdout.contains("flash write : no"),
            "got {stdout}"
        );
    }

    // -- fw flash, dry run and the refusal --------------------------------

    /// A structurally valid image for `profile`, stock constants everywhere.
    ///
    /// Built here rather than reusing `crate::testing::synthetic_image`, which is
    /// pinned to V1.13: this section needs the same shape at V1.12's length, and
    /// the two files have to differ from each other for a diff to mean anything.
    fn flash_image(profile: &FirmwareProfile, patched: bool) -> Vec<u8> {
        use m241_core::firmware::image::{
            DEVICE_TYPE_OFFSET, DEVICE_TYPE_SIZE, EPOCH_CONSTANT_OFFSETS, FAMILY, FAMILY_OFFSET,
            FAMILY_SIZE, FW_SIZE_OFFSET,
        };
        let mut data = vec![0xFFu8; profile.size];
        data[..4].copy_from_slice(&VECTOR);
        data[FAMILY_OFFSET..FAMILY_OFFSET + FAMILY_SIZE].fill(0);
        data[FAMILY_OFFSET..FAMILY_OFFSET + FAMILY.len()].copy_from_slice(FAMILY.as_bytes());
        data[DEVICE_TYPE_OFFSET..DEVICE_TYPE_OFFSET + DEVICE_TYPE_SIZE].fill(0);
        let device = profile.device_type.as_bytes();
        data[DEVICE_TYPE_OFFSET..DEVICE_TYPE_OFFSET + device.len()].copy_from_slice(device);
        let marker = profile.name.as_bytes();
        data[profile.version_marker..profile.version_marker + marker.len()].copy_from_slice(marker);
        for offset in EPOCH_CONSTANT_OFFSETS {
            data[offset..offset + 4].copy_from_slice(&GPS_EPOCH_UNIX.to_le_bytes());
        }
        data[FW_SIZE_OFFSET..FW_SIZE_OFFSET + 4]
            .copy_from_slice(&(profile.size as u32 - 4).to_le_bytes());
        if patched {
            let before = GPS_EPOCH_UNIX.to_le_bytes();
            let after = GPS_EPOCH_UNIX_WNRO.to_le_bytes();
            for (index, (old, new)) in before.iter().zip(after.iter()).enumerate() {
                if old != new {
                    data[EFFECTIVE_EPOCH_OFFSET + index] = *new;
                }
            }
            data[profile.version_marker + VERSION_MARKER_LEN - 1] = b'4';
        }
        data
    }

    /// The pinned vendor image, when `M241_VENDOR_IMAGE` points at it.
    ///
    /// The binary is not committed (`AGENTS.md` §6). Tests that need it return
    /// early when the variable is unset; the rejection tests run without it.
    fn vendor_image() -> Option<Vec<u8>> {
        let path = std::env::var_os("M241_VENDOR_IMAGE")?;
        Some(std::fs::read(path).expect("M241_VENDOR_IMAGE must be readable"))
    }

    /// Rebuild the documented 17-byte diff against a unit's own dump.
    ///
    /// The per-unit values here are synthetic, not a real unit's data
    /// (`AGENTS.md` §6). They let the fake unit match the backup while the
    /// vendor image stays the pinned vendor file; the count (14 per-unit +
    /// WNRO 2 + marker 1) is what the test pins.
    fn vendor_backup(vendor: &[u8]) -> Vec<u8> {
        assert_eq!(vendor.len(), PROFILE_V1_13.size);
        let mut backup = vendor.to_vec();
        backup[0x5C..0x60].copy_from_slice(&[0x12, 0x34, 0x56, 0x78]);
        backup[0xF8..0xFE].copy_from_slice(&[0x9A, 0xBC, 0xDE, 0xF0, 0x11, 0x22]);
        let after = GPS_EPOCH_UNIX_WNRO.to_le_bytes();
        backup[EFFECTIVE_EPOCH_OFFSET + 2] = after[2];
        backup[EFFECTIVE_EPOCH_OFFSET + 3] = after[3];
        backup[0x59C2C] = b'4';
        backup[0x63458..0x6345C].copy_from_slice(&[0xFF; 4]);
        let differing = backup
            .iter()
            .zip(vendor.iter())
            .filter(|(unit, vendor)| unit != vendor)
            .count();
        assert_eq!(
            differing, 17,
            "the documented vendor/backup diff is 17 bytes"
        );
        backup
    }

    /// Write `image` and `backup` into `scratch` and return their paths.
    fn flash_case(
        scratch: &crate::testing::Scratch,
        image: &[u8],
        backup: &[u8],
    ) -> (PathBuf, PathBuf) {
        let image_path = scratch.join("image.bin");
        let backup_path = scratch.join("backup.bin");
        fileio::write(&image_path, image).expect("write the image");
        fileio::write(&backup_path, backup).expect("write the backup");
        (image_path, backup_path)
    }

    fn flash_options(case: &(PathBuf, PathBuf)) -> FlashOptions {
        FlashOptions {
            image: case.0.clone(),
            backup: case.1.clone(),
            da: None,
            dry_run: true,
            recover: false,
            allow_vendor: false,
            allow_unverified: false,
            yes: false,
            assume_brom: false,
            port: String::from("auto"),
            json: false,
        }
    }

    #[test]
    fn flash_dry_run_accepts_a_patched_v1_13_backup_and_says_no_flash_was_written() {
        let scratch = crate::testing::Scratch::new("flash-dry-113");
        let backup = flash_image(&PROFILE_V1_13, false);
        let image = flash_image(&PROFILE_V1_13, true);
        let case = flash_case(&scratch, &image, &backup);
        let console = RecordingConsole::default();
        let links = FakePorts::empty();
        let session = Session::new(&links, &console, &AutoConfirm);
        flash(&flash_options(&case), session).expect("the patched backup is acceptable");
        let stdout = console.stdout();
        assert!(stdout.contains("profile     : V1.13"), "got {stdout}");
        assert!(stdout.contains("patched backup"), "got {stdout}");
        assert!(
            stdout.contains("406620"),
            "the V1.13 write length: {stdout}"
        );
        assert!(stdout.contains("1589"), "the packet count: {stdout}");
        assert!(
            stdout.contains("start       : 0x00000000 (settled DA-side base)"),
            "the settled start is shown, not a candidate list: {stdout}"
        );
        assert!(
            stdout.contains("actual write: permitted"),
            "the plan says the write would proceed: {stdout}"
        );
        assert!(
            stdout.contains("flash write : no"),
            "a dry run claims no write: {stdout}"
        );
        let stderr = console.stderr();
        assert!(
            stderr.contains("dry run: no port was opened and no command was sent"),
            "the note says nothing reached the wire: {stderr}"
        );
        assert!(
            stderr.contains("reads the range back before it reports success"),
            "the note says a real run verifies: {stderr}"
        );
    }

    #[test]
    fn flash_dry_run_accepts_a_patched_v1_12_backup_at_the_v1_12_length() {
        let scratch = crate::testing::Scratch::new("flash-dry-112");
        let backup = flash_image(&PROFILE_V1_12, false);
        let image = flash_image(&PROFILE_V1_12, true);
        let case = flash_case(&scratch, &image, &backup);
        let console = RecordingConsole::default();
        let links = FakePorts::empty();
        let session = Session::new(&links, &console, &AutoConfirm);
        flash(&flash_options(&case), session).expect("the V1.12 patched backup is acceptable");
        let stdout = console.stdout();
        assert!(stdout.contains("profile     : V1.12"), "got {stdout}");
        assert!(
            stdout.contains("406312"),
            "the V1.12 write length, not V1.13's: {stdout}"
        );
        assert!(
            stdout.contains("write range : 406312"),
            "a V1.12 plan must print V1.12's length: {stdout}"
        );
        assert!(
            stdout.contains("actual write: permitted"),
            "V1.12 is now writable via the variable-length strict path: {stdout}"
        );
    }

    #[test]
    fn flash_dry_run_accepts_a_v1_14_image_under_the_v1_13_profile() {
        // A patched V1.13 unit reports `V1.14`. `identify` accepts the marker,
        // and the plan names the V1.13 profile with its size and length.
        let scratch = crate::testing::Scratch::new("flash-dry-114");
        let backup = flash_image(&PROFILE_V1_13, false);
        let image = flash_image(&PROFILE_V1_13, true);
        let marker = FirmwareImage::new(&image)
            .expect("header")
            .field(PROFILE_V1_13.version_marker, VERSION_MARKER_LEN);
        assert_eq!(marker, "V1.14", "the fixture is a V1.14 image");
        let case = flash_case(&scratch, &image, &backup);
        let console = RecordingConsole::default();
        let links = FakePorts::empty();
        let session = Session::new(&links, &console, &AutoConfirm);
        flash(&flash_options(&case), session).expect("a V1.14 image plans under V1.13");
        let stdout = console.stdout();
        assert!(
            stdout.contains("profile     : V1.13 (01017-00F, 406620 bytes)"),
            "V1.14 is planned as V1.13's size and device: {stdout}"
        );
        assert!(stdout.contains("patched backup"), "got {stdout}");
    }

    #[test]
    fn flash_dry_run_calls_a_zero_diff_image_identical_to_the_backup() {
        let scratch = crate::testing::Scratch::new("flash-dry-identical");
        let backup = flash_image(&PROFILE_V1_13, false);
        let image = backup.clone();
        let case = flash_case(&scratch, &image, &backup);
        let console = RecordingConsole::default();
        let links = FakePorts::empty();
        let session = Session::new(&links, &console, &AutoConfirm);
        flash(&flash_options(&case), session).expect("an identical image is accepted");
        let stdout = console.stdout();
        assert!(
            stdout.contains("identical to the backup"),
            "a zero-diff image is not a patched backup: {stdout}"
        );
        assert!(
            !stdout.contains("patched backup"),
            "the label must not claim a patch was applied: {stdout}"
        );
    }

    #[test]
    fn flash_dry_run_opens_no_port_and_sends_no_opcode() {
        // The claim `--dry-run` makes. A fake DA is wired in, so if the handler
        // ever reached the port the fake would record it: the record starts at
        // zero opens and zero bytes, and both must still be zero afterwards.
        let scratch = crate::testing::Scratch::new("flash-dry-port");
        let backup = flash_image(&PROFILE_V1_13, false);
        let image = flash_image(&PROFILE_V1_13, true);
        let case = flash_case(&scratch, &image, &backup);
        let chip = fake_da::FakeChip::with_vector_at(PROBE_ADDRESSES[1]);
        let links = fake_da::FakeDaPorts::new(chip);
        let record = links.record();
        let console = RecordingConsole::default();
        let session = Session::new(&links, &console, &AutoConfirm).with_timings(Timings::fast());
        flash(&flash_options(&case), session).expect("dry run");

        let record = record.borrow();
        assert_eq!(record.opens, 0, "a dry run must open no port");
        assert_eq!(record.bytes_moved, 0, "a dry run must move no bytes");
        assert_eq!(record.opcodes, Vec::<u8>::new(), "no opcode may be sent");
        assert!(record.reads.is_empty(), "no 0xD6 read may happen");
        assert!(record.writes.is_empty(), "no 0xA1 frame may happen");
        assert!(record.jumps.is_empty(), "no 0xA8 jump may happen");
        assert!(
            !record.control_lines_asserted,
            "no reset line may be pulsed"
        );
    }

    #[test]
    fn flash_dry_run_refuses_an_image_that_is_neither_the_backup_nor_the_patch_offsets() {
        let scratch = crate::testing::Scratch::new("flash-dry-refuse");
        let backup = flash_image(&PROFILE_V1_13, false);
        let mut image = flash_image(&PROFILE_V1_13, true);
        image[0x8000] ^= 0xFF;
        let case = flash_case(&scratch, &image, &backup);
        let console = RecordingConsole::default();
        let links = FakePorts::empty();
        let session = Session::new(&links, &console, &AutoConfirm);
        let err = flash(&flash_options(&case), session).expect_err("neither rule holds");
        assert!(matches!(err, Error::Safety { .. }), "got {err}");
        assert_eq!(crate::exit::exit_code_for(&err), 5);
        assert!(
            console.stdout().is_empty(),
            "no plan is printed on a refusal"
        );
    }

    #[test]
    fn flash_dry_run_refuses_an_image_of_an_unknown_profile() {
        let scratch = crate::testing::Scratch::new("flash-dry-unknown");
        let backup = flash_image(&PROFILE_V1_13, false);
        let mut image = flash_image(&PROFILE_V1_13, true);
        // A marker no profile carries.
        image[PROFILE_V1_13.version_marker..PROFILE_V1_13.version_marker + 5]
            .copy_from_slice(b"V9.99");
        let case = flash_case(&scratch, &image, &backup);
        let console = RecordingConsole::default();
        let links = FakePorts::empty();
        let session = Session::new(&links, &console, &AutoConfirm);
        let err = flash(&flash_options(&case), session).expect_err("no profile matches");
        assert!(matches!(err, Error::Profile { .. }), "got {err}");
        assert_eq!(crate::exit::exit_code_for(&err), 4);
    }

    #[test]
    fn flash_dry_run_refuses_a_vendor_style_image_that_is_not_the_backup() {
        // A vendor release carries per-unit fields that differ from this unit's
        // dump. That difference is outside the patch table, and `AGENTS.md`
        // §2.3 gives no digest route around it.
        let scratch = crate::testing::Scratch::new("flash-dry-vendor");
        let backup = flash_image(&PROFILE_V1_13, false);
        let mut image = flash_image(&PROFILE_V1_13, true);
        image[0x1000] ^= 0xFF;
        let case = flash_case(&scratch, &image, &backup);
        let console = RecordingConsole::default();
        let links = FakePorts::empty();
        let session = Session::new(&links, &console, &AutoConfirm);
        let err = flash(&flash_options(&case), session).expect_err("not the backup");
        assert!(matches!(err, Error::Safety { .. }), "got {err}");
        let text = format!("{err}");
        assert!(
            text.contains("outside the V1.13 patch offsets"),
            "the only rule is named: {text}"
        );
        assert!(!text.contains("SHA-256"), "no digest bypass exists: {text}");
    }

    #[test]
    fn flash_without_yes_refuses_before_opening_a_port_and_before_the_da_file() {
        // `--yes` is checked before `check_layout`, the DA file and the port.
        // The DA path here does not exist: a handler that read it first would
        // fail with an I/O error instead of the confirmation refusal.
        let scratch = crate::testing::Scratch::new("flash-real-no-yes");
        let backup = flash_image(&PROFILE_V1_13, false);
        let image = flash_image(&PROFILE_V1_13, true);
        let case = flash_case(&scratch, &image, &backup);
        let chip = m241_flash::FakeDa::with_firmware(&backup);
        let links = da_write::DaWritePorts::new(chip);
        let ports = links.clone();
        let chip = links.chip();
        let console = RecordingConsole::default();
        let session = Session::new(&links, &console, &AutoConfirm).with_timings(Timings::fast());
        let mut options = flash_options(&case);
        options.dry_run = false;
        options.da = Some(scratch.join("absent.bin"));
        let err = flash(&options, session).expect_err("no --yes");
        assert!(matches!(err, Error::Safety { .. }), "got {err}");
        let text = format!("{err}");
        assert!(text.contains("--yes"), "the refusal names the flag: {text}");
        assert!(
            !text.contains("05-verification.md"),
            "the confirmation is asked for first: {text}"
        );
        assert_eq!(
            chip.borrow().read_bytes(),
            0,
            "no port may be opened before the confirmation"
        );
        assert_eq!(
            ports.opens(),
            0,
            "no port may be opened before the confirmation"
        );
    }

    #[test]
    fn flash_with_every_gate_passed_but_no_port_reports_a_port_error() {
        // The image, the confirmation, the layout, the acceptance rule and the
        // DA file all pass; the next step is resolving a port, and there is
        // none. This is what proves the dry-run-only refusal is gone without
        // touching a device.
        let scratch = crate::testing::Scratch::new("flash-real-noport");
        let backup = flash_image(&PROFILE_V1_13, false);
        let image = flash_image(&PROFILE_V1_13, true);
        let case = flash_case(&scratch, &image, &backup);
        let console = RecordingConsole::default();
        let links = FakePorts::empty();
        let session = Session::new(&links, &console, &AutoConfirm);
        let mut options = flash_options(&case);
        options.dry_run = false;
        options.yes = true;
        options.da = Some(da_file(scratch.path()));
        let err = flash(&options, session).expect_err("no port is available");
        assert!(matches!(err, Error::Port { .. }), "got {err}");
        assert_eq!(crate::exit::exit_code_for(&err), 3, "got {err}");
        assert!(
            !console.stderr().contains("writing "),
            "the port is resolved before the write is announced: {}",
            console.stderr()
        );
        assert!(
            console.stderr().contains("SHA-256"),
            "the DA file's full hash is shown before the port is opened: {}",
            console.stderr()
        );
    }

    #[test]
    fn flash_refuses_a_da_that_is_not_the_vendor_image_before_opening_a_port() {
        let scratch = crate::testing::Scratch::new("flash-real-bad-da");
        let backup = flash_image(&PROFILE_V1_13, false);
        let image = flash_image(&PROFILE_V1_13, true);
        let case = flash_case(&scratch, &image, &backup);
        let da = scratch.join("DA_v3.8.4.bin");
        fileio::write(&da, &vec![0u8; m241_flash::da::DA_IMAGE_SIZE])
            .expect("write a same-length file that is not the DA");
        let chip = m241_flash::FakeDa::with_firmware(&backup);
        let links = da_write::DaWritePorts::new(chip);
        let chip = links.chip();
        let console = RecordingConsole::default();
        let session = Session::new(&links, &console, &AutoConfirm).with_timings(Timings::fast());
        let mut options = flash_options(&case);
        options.dry_run = false;
        options.yes = true;
        options.da = Some(da);
        let err = flash(&options, session).expect_err("not the vendor DA");
        assert!(matches!(err, Error::Safety { .. }), "got {err}");
        assert!(
            format!("{err}").contains("A0 E1"),
            "the refusal names the first bytes it wanted: {err}"
        );
        assert_eq!(
            chip.borrow().read_bytes(),
            0,
            "the DA file is checked before the port is opened"
        );
    }

    #[test]
    fn flash_refuses_a_same_size_da_with_the_right_head_but_a_different_digest() {
        // The same-length, right-head, wrong-content file the head check cannot
        // see: `fw probe-da` has its own test for this, and `fw flash` reaches
        // the same gate through the same helper. Nothing is opened or sent.
        let scratch = crate::testing::Scratch::new("flash-real-lookalike-da");
        let backup = flash_image(&PROFILE_V1_13, false);
        let image = flash_image(&PROFILE_V1_13, true);
        let case = flash_case(&scratch, &image, &backup);
        let da = scratch.join("DA_v3.8.4.bin");
        let mut lookalike = da_fixture_bytes();
        let last = lookalike.last_mut().expect("a 14,896-byte image");
        *last ^= 0xFF;
        fileio::write(&da, &lookalike).expect("write a same-length lookalike");
        let chip = m241_flash::FakeDa::with_firmware(&backup);
        let links = da_write::DaWritePorts::new(chip);
        let chip = links.chip();
        let console = RecordingConsole::default();
        let session = Session::new(&links, &console, &AutoConfirm).with_timings(Timings::fast());
        let mut options = flash_options(&case);
        options.dry_run = false;
        options.yes = true;
        options.da = Some(da);
        let err = flash(&options, session).expect_err("a lookalike DA is not loaded");
        assert!(matches!(err, Error::Safety { .. }), "got {err}");
        assert!(
            format!("{err}").contains("SHA-256"),
            "the refusal names the digest check: {err}"
        );
        assert_eq!(
            chip.borrow().read_bytes(),
            0,
            "the DA file is checked before the port is opened"
        );
    }

    #[test]
    fn flash_refuses_a_unit_that_does_not_hold_the_backup_before_any_write_opcode() {
        // The whole point of `verify_device`: one byte different on the unit is
        // refused before the DA is loaded, so no `0xA1` frame, jump, `0xD3`,
        // `0xD5` or `0xD9` reaches the wire.
        let scratch = crate::testing::Scratch::new("flash-real-wrong-unit");
        let backup = flash_image(&PROFILE_V1_13, false);
        let image = flash_image(&PROFILE_V1_13, true);
        let case = flash_case(&scratch, &image, &backup);
        let mut unit = backup.clone();
        unit[0x1000] ^= 0xFF;
        let chip = m241_flash::FakeDa::with_firmware(&unit);
        let links = da_write::DaWritePorts::new(chip);
        let chip = links.chip();
        let console = RecordingConsole::default();
        let session = Session::new(&links, &console, &AutoConfirm).with_timings(Timings::fast());
        let mut options = flash_options(&case);
        options.dry_run = false;
        options.yes = true;
        options.da = Some(da_file(scratch.path()));
        let err = flash(&options, session).expect_err("the unit is not the backup");
        assert!(matches!(err, Error::Safety { .. }), "got {err}");
        let text = format!("{err}");
        assert!(
            text.contains("differs from the backup at image offset 0x1000"),
            "the refusal names the offset: {text}"
        );
        assert!(text.contains("Nothing was written"), "got {text}");
        let chip = chip.borrow();
        assert!(!chip.started(), "the DA was never started");
        assert_eq!(chip.bootrom_writes(), 0, "no 0xA1 frame was sent");
        for destructive in [0xD3u8, 0xD5, 0xD9] {
            assert!(
                !chip.opcodes().contains(&destructive),
                "0x{destructive:02X} must not go out before the device check passes: {:02X?}",
                chip.opcodes()
            );
        }
    }

    #[test]
    fn flash_recover_writes_the_backup_without_the_whole_unit_comparison() {
        // The unit is a partial write: one byte differs from the backup. The
        // normal path refuses it before the DA; recovery must still proceed,
        // because writing the backup back is exactly the repair.
        let scratch = crate::testing::Scratch::new("flash-recover-success");
        let backup = flash_image(&PROFILE_V1_13, false);
        let image = backup.clone();
        let case = flash_case(&scratch, &image, &backup);
        let mut unit = backup.clone();
        unit[0x1000] ^= 0xFF;
        let chip = m241_flash::FakeDa::with_firmware(&unit);
        let links = da_write::DaWritePorts::new(chip);
        let ports = links.clone();
        let chip = links.chip();
        let console = RecordingConsole::default();
        let session = Session::new(&links, &console, &AutoConfirm).with_timings(Timings::fast());
        let mut options = flash_options(&case);
        options.dry_run = false;
        options.recover = true;
        options.yes = true;
        options.da = Some(da_file(scratch.path()));

        flash(&options, session).expect("recovery skips verify_device");
        assert!(ports.opens() > 0, "the recovery run must reach the port");
        let chip = chip.borrow();
        assert!(chip.started(), "the DA was loaded and jumped to");
        assert!(chip.restarted(), "the read-back sent 0xD9");
        assert_eq!(
            &chip.memory()[..FLASH_WRITE_LENGTH],
            &backup[..],
            "the backup was written over the partial unit"
        );
        assert!(
            !chip
                .read16_requests()
                .iter()
                .any(|(_, bytes)| *bytes == FLASH_WRITE_LENGTH),
            "no single whole-span read may be issued: {:?}",
            chip.read16_requests()
        );
        assert!(
            chip.read16_requests()
                .iter()
                .all(|(_, bytes)| *bytes <= DUMP_CHUNK),
            "every BootROM frame stays within the chunk limit: {:?}",
            chip.read16_requests()
        );
    }

    #[test]
    fn flash_recover_refuses_an_image_that_is_not_the_backup() {
        // A WNRO-patched image is not the recovery payload: recovery writes the
        // user's own dump back, so the image must be byte-identical to it.
        let scratch = crate::testing::Scratch::new("flash-recover-patched");
        let backup = flash_image(&PROFILE_V1_13, false);
        let image = flash_image(&PROFILE_V1_13, true);
        let case = flash_case(&scratch, &image, &backup);
        let chip = m241_flash::FakeDa::with_firmware(&backup);
        let links = da_write::DaWritePorts::new(chip);
        let ports = links.clone();
        let chip = links.chip();
        let console = RecordingConsole::default();
        let session = Session::new(&links, &console, &AutoConfirm).with_timings(Timings::fast());
        let mut options = flash_options(&case);
        options.dry_run = false;
        options.recover = true;
        options.yes = true;
        options.da = Some(scratch.join("absent.bin"));

        let err = flash(&options, session).expect_err("not the backup");
        assert!(matches!(err, Error::Safety { .. }), "got {err}");
        let text = format!("{err}");
        assert!(
            text.contains("--recover"),
            "the refusal names the mode: {text}"
        );
        assert!(
            text.contains("byte-identical"),
            "the refusal says what recovery requires: {text}"
        );
        assert_eq!(ports.opens(), 0, "nothing may be opened for this refusal");
        assert_eq!(chip.borrow().read_bytes(), 0);
    }

    #[test]
    fn flash_dry_run_allow_vendor_warns_about_per_unit_data() {
        let Some(vendor) = vendor_image() else {
            return;
        };
        let backup = vendor_backup(&vendor);
        let scratch = crate::testing::Scratch::new("flash-dry-vendor");
        let case = flash_case(&scratch, &vendor, &backup);
        let console = RecordingConsole::default();
        let links = FakePorts::empty();
        let session = Session::new(&links, &console, &AutoConfirm);
        let mut options = flash_options(&case);
        options.allow_vendor = true;

        flash(&options, session).expect("the pinned vendor image is acceptable");
        let stdout = console.stdout();
        assert!(
            stdout.contains("official vendor image"),
            "the plan names the vendor path: {stdout}"
        );
        assert!(
            stdout.contains("14 bytes of per-unit data will be overwritten"),
            "the plan warns about the per-unit bytes: {stdout}"
        );
        assert!(
            stdout.contains("wnro patch") && stdout.contains("7168 days behind"),
            "the plan warns that the vendor image is unpatched: {stdout}"
        );
        assert!(
            console
                .stderr()
                .contains("warning: this is the official vendor image"),
            "the warning is on stderr: {}",
            console.stderr()
        );
        assert!(
            console.stderr().contains("7168 days behind"),
            "the warning says the WNRO fix is absent: {}",
            console.stderr()
        );
        assert!(
            stdout.contains("port opened : no"),
            "a dry run opens no port: {stdout}"
        );
    }

    #[test]
    fn flash_dry_run_vendor_without_allow_vendor_is_refused() {
        let Some(vendor) = vendor_image() else {
            return;
        };
        let backup = vendor_backup(&vendor);
        let scratch = crate::testing::Scratch::new("flash-dry-vendor-refused");
        let case = flash_case(&scratch, &vendor, &backup);
        let console = RecordingConsole::default();
        let links = FakePorts::empty();
        let session = Session::new(&links, &console, &AutoConfirm);

        let err = flash(&flash_options(&case), session).expect_err("no --allow-vendor");
        assert!(matches!(err, Error::Safety { .. }), "got {err}");
        assert!(
            format!("{err}").contains("outside the V1.13 patch offsets"),
            "the strict path refuses the vendor diff: {err}"
        );
        assert!(
            console.stdout().is_empty(),
            "no plan is printed on a refusal"
        );
    }

    #[test]
    fn flash_allow_vendor_writes_the_vendor_image_and_reads_it_back() {
        let Some(vendor) = vendor_image() else {
            return;
        };
        let backup = vendor_backup(&vendor);
        let scratch = crate::testing::Scratch::new("flash-vendor-e2e");
        let case = flash_case(&scratch, &vendor, &backup);
        let chip = m241_flash::FakeDa::with_firmware(&backup);
        let links = da_write::DaWritePorts::new(chip);
        let ports = links.clone();
        let chip = links.chip();
        let console = RecordingConsole::default();
        let session = Session::new(&links, &console, &AutoConfirm).with_timings(Timings::fast());
        let mut options = flash_options(&case);
        options.dry_run = false;
        options.allow_vendor = true;
        options.yes = true;
        options.da = Some(da_file(scratch.path()));

        flash(&options, session).expect("the vendor image is written and verified");
        assert!(ports.opens() > 0);
        {
            let chip = chip.borrow();
            assert!(chip.started(), "the DA was loaded and jumped to");
            assert!(chip.restarted(), "the read-back sent 0xD9");
            assert_eq!(
                &chip.memory()[..FLASH_WRITE_LENGTH],
                &vendor[..],
                "the vendor image is what the flash window holds"
            );
        }
        let stdout = console.stdout();
        assert!(
            stdout.contains("official vendor image"),
            "success names the vendor path: {stdout}"
        );
        assert!(
            stdout.contains("per-unit data were overwritten"),
            "success states the per-unit overwrite: {stdout}"
        );
        assert!(
            stdout.contains("wnro patch") && stdout.contains("7168 days behind"),
            "success states that the vendor image is unpatched: {stdout}"
        );
        assert!(
            console
                .stderr()
                .contains("warning: this is the official vendor image"),
            "the real path warns before the port is opened: {}",
            console.stderr()
        );
        assert!(
            console.stderr().contains("7168 days behind"),
            "the real warning says the WNRO fix is absent: {}",
            console.stderr()
        );
        assert!(stdout.contains("verified    : 406620"), "got {stdout}");
    }

    #[test]
    fn flash_allow_vendor_still_requires_the_unit_to_match_the_backup() {
        let Some(vendor) = vendor_image() else {
            return;
        };
        let backup = vendor_backup(&vendor);
        let mut unit = backup.clone();
        unit[0x1000] ^= 0xFF;
        let scratch = crate::testing::Scratch::new("flash-vendor-wrong-unit");
        let case = flash_case(&scratch, &vendor, &backup);
        let chip = m241_flash::FakeDa::with_firmware(&unit);
        let links = da_write::DaWritePorts::new(chip);
        let chip = links.chip();
        let console = RecordingConsole::default();
        let session = Session::new(&links, &console, &AutoConfirm).with_timings(Timings::fast());
        let mut options = flash_options(&case);
        options.dry_run = false;
        options.allow_vendor = true;
        options.yes = true;
        options.da = Some(da_file(scratch.path()));

        let err = flash(&options, session).expect_err("the unit does not match the backup");
        assert!(matches!(err, Error::Safety { .. }), "got {err}");
        assert!(
            format!("{err}").contains("differs from the backup at image offset 0x1000"),
            "verify_device is still required: {err}"
        );
        let chip = chip.borrow();
        assert!(!chip.started(), "the DA must not be loaded");
        assert!(
            !chip.opcodes().contains(&0xD3),
            "no write opcode may leave: {:02X?}",
            chip.opcodes()
        );
    }

    #[test]
    fn flash_allow_vendor_without_yes_refuses_before_opening_a_port() {
        let Some(vendor) = vendor_image() else {
            return;
        };
        let backup = vendor_backup(&vendor);
        let scratch = crate::testing::Scratch::new("flash-vendor-no-yes");
        let case = flash_case(&scratch, &vendor, &backup);
        let chip = m241_flash::FakeDa::with_firmware(&backup);
        let links = da_write::DaWritePorts::new(chip);
        let ports = links.clone();
        let chip = links.chip();
        let console = RecordingConsole::default();
        let session = Session::new(&links, &console, &AutoConfirm).with_timings(Timings::fast());
        let mut options = flash_options(&case);
        options.dry_run = false;
        options.allow_vendor = true;
        options.yes = false;
        options.da = Some(scratch.join("absent.bin"));

        let err = flash(&options, session).expect_err("no --yes");
        assert!(matches!(err, Error::Safety { .. }), "got {err}");
        assert!(format!("{err}").contains("--yes"), "got {err}");
        assert_eq!(ports.opens(), 0, "no port may be opened");
        assert_eq!(chip.borrow().read_bytes(), 0);
    }

    #[test]
    fn flash_allow_vendor_warns_before_a_missing_da_file() {
        // The warning must be before the DA file and the port, so a failure
        // still shows that the vendor image's per-unit/WNRO effects were known.
        let Some(vendor) = vendor_image() else {
            return;
        };
        let backup = vendor_backup(&vendor);
        let scratch = crate::testing::Scratch::new("flash-vendor-warn-before-da");
        let case = flash_case(&scratch, &vendor, &backup);
        let chip = m241_flash::FakeDa::with_firmware(&backup);
        let links = da_write::DaWritePorts::new(chip);
        let ports = links.clone();
        let chip = links.chip();
        let console = RecordingConsole::default();
        let session = Session::new(&links, &console, &AutoConfirm).with_timings(Timings::fast());
        let mut options = flash_options(&case);
        options.dry_run = false;
        options.allow_vendor = true;
        options.yes = true;
        options.da = Some(scratch.join("absent.bin"));

        let err = flash(&options, session).expect_err("the DA file is missing");
        assert!(matches!(err, Error::Io { .. }), "got {err}");
        assert!(
            console
                .stderr()
                .contains("warning: this is the official vendor image"),
            "the warning must precede the DA-file failure: {}",
            console.stderr()
        );
        assert!(
            console.stderr().contains("7168 days behind"),
            "the warning must name the WNRO effect: {}",
            console.stderr()
        );
        assert_eq!(ports.opens(), 0, "the port must not be opened");
        assert_eq!(chip.borrow().read_bytes(), 0);
    }

    #[test]
    fn flash_allow_vendor_rejects_a_non_vendor_digest() {
        let image = flash_image(&PROFILE_V1_13, false);
        let mut backup = image.clone();
        backup[0x1000] ^= 0xFF;
        let scratch = crate::testing::Scratch::new("flash-vendor-wrong-sha");
        let case = flash_case(&scratch, &image, &backup);
        let console = RecordingConsole::default();
        let links = FakePorts::empty();
        let session = Session::new(&links, &console, &AutoConfirm);
        let mut options = flash_options(&case);
        options.allow_vendor = true;

        let err = flash(&options, session).expect_err("not the pinned vendor digest");
        assert!(matches!(err, Error::Safety { .. }), "got {err}");
        let text = format!("{err}");
        assert!(
            text.contains("SHA-256") && text.contains("pinned V1.13 vendor digest"),
            "the refusal names the digest gate: {text}"
        );
    }

    #[test]
    fn flash_allow_vendor_rejects_a_non_vendor_v1_12_digest() {
        let image = flash_image(&PROFILE_V1_12, false);
        let mut backup = image.clone();
        backup[0x1000] ^= 0xFF;
        let scratch = crate::testing::Scratch::new("flash-vendor-v112");
        let case = flash_case(&scratch, &image, &backup);
        let console = RecordingConsole::default();
        let links = FakePorts::empty();
        let session = Session::new(&links, &console, &AutoConfirm);
        let mut options = flash_options(&case);
        options.allow_vendor = true;

        let err = flash(&options, session).expect_err("not the pinned V1.12 vendor digest");
        assert!(matches!(err, Error::Safety { .. }), "got {err}");
        let text = format!("{err}");
        assert!(
            text.contains("SHA-256") && text.contains(m241_flash::VENDOR_V1_12_SHA256),
            "the refusal names the pinned V1.12 vendor digest: {text}"
        );
    }

    #[test]
    fn flash_v1_12_patched_backup_writes_and_verifies() {
        let scratch = crate::testing::Scratch::new("flash-v112-e2e");
        let backup = flash_image(&PROFILE_V1_12, false);
        let image = flash_image(&PROFILE_V1_12, true);
        let case = flash_case(&scratch, &image, &backup);
        let chip = m241_flash::FakeDa::with_firmware(&backup);
        let links = da_write::DaWritePorts::new(chip);
        let chip = links.chip();
        let console = RecordingConsole::default();
        let session = Session::new(&links, &console, &AutoConfirm).with_timings(Timings::fast());
        let mut options = flash_options(&case);
        options.dry_run = false;
        options.yes = true;
        options.da = Some(da_file(scratch.path()));

        flash(&options, session).expect("V1.12 strict write succeeds");
        let chip = chip.borrow();
        assert_eq!(chip.recorded_range(), (0, (PROFILE_V1_12.size as u32) - 1));
        assert_eq!(
            &chip.memory()[..PROFILE_V1_12.size],
            &image[..],
            "flash holds the V1.12 patched image"
        );
        let stdout = console.stdout();
        assert!(stdout.contains("profile     : V1.12"), "got {stdout}");
        assert!(stdout.contains("verified    : 406312"), "got {stdout}");
    }

    #[test]
    fn flash_allow_unverified_rejects_physical_violations_before_opening_a_port() {
        let scratch = crate::testing::Scratch::new("flash-unverified-blocked");
        let backup = flash_image(&PROFILE_V1_13, false);
        for (label, bad) in [
            ("empty", Vec::new()),
            ("odd", vec![0xAAu8; 406_619]),
            (
                "too-large",
                vec![0xFFu8; m241_flash::FLASH_CAPACITY_BYTES + 2],
            ),
        ] {
            let image_path = scratch.join(&format!("{label}.bin"));
            let backup_path = scratch.join(&format!("{label}-bak.bin"));
            fileio::write(&image_path, &bad).expect("write bad image");
            fileio::write(&backup_path, &backup).expect("write backup");
            let console = RecordingConsole::default();
            let links = FakePorts::empty();
            let session = Session::new(&links, &console, &AutoConfirm);
            let mut options = flash_options(&(image_path, backup_path));
            options.allow_unverified = true;
            let err = flash(&options, session).expect_err("physical constraint must be refused");
            assert!(matches!(err, Error::Safety { .. }), "{label}: got {err}");
        }
    }

    #[test]
    fn flash_allow_unverified_dry_run_warns_when_arm_vector_is_absent() {
        let scratch = crate::testing::Scratch::new("flash-unverified-dry");
        let backup = flash_image(&PROFILE_V1_13, false);
        let custom = vec![0x5Au8; 1_024]; // valid even length, no ARM vector
        let case = flash_case(&scratch, &custom, &backup);
        let console = RecordingConsole::default();
        let links = FakePorts::empty();
        let session = Session::new(&links, &console, &AutoConfirm);
        let mut options = flash_options(&case);
        options.allow_unverified = true;

        flash(&options, session).expect("unverified dry-run succeeds");
        let stdout = console.stdout();
        assert!(stdout.contains("unverified (1024 bytes)"), "got {stdout}");
        assert!(stdout.contains("arm vector  : no"), "got {stdout}");
        assert!(
            console.stderr().contains("ABSENT"),
            "warning must highlight missing ARM vector: {}",
            console.stderr()
        );
    }

    #[test]
    fn flash_allow_unverified_writes_custom_length_and_tolerates_0x5c_restore() {
        let scratch = crate::testing::Scratch::new("flash-unverified-e2e");
        let backup = flash_image(&PROFILE_V1_13, false);
        let mut custom = vec![0x5Au8; 412_000];
        custom[..4].copy_from_slice(&VECTOR);
        custom[0x5C..0x60].copy_from_slice(&[0xFF; 4]);
        let case = flash_case(&scratch, &custom, &backup);
        let chip = m241_flash::FakeDa::with_firmware(&backup)
            .restoring_per_unit_bytes([0x91, 0x1B, 0xD4, 0xCF]);
        let links = da_write::DaWritePorts::new(chip);
        let chip = links.chip();
        let console = RecordingConsole::default();
        let session = Session::new(&links, &console, &AutoConfirm).with_timings(Timings::fast());
        let mut options = flash_options(&case);
        options.dry_run = false;
        options.allow_unverified = true;
        options.yes = true;
        options.json = true;
        options.da = Some(da_file(scratch.path()));

        flash(&options, session).expect("unverified custom write and 0x5C..0x60 restore succeed");
        {
            let chip = chip.borrow();
            assert_eq!(chip.recorded_range(), (0, 411_999));
            assert_eq!(chip.memory().len(), 412_000);
        }
        let document: serde_json::Value =
            serde_json::from_str(&console.stdout()).expect("one JSON object");
        assert_eq!(document["acceptance"], "unverified");
        assert_eq!(document["unverified_image"], true);
        assert_eq!(document["length"], 412_000);
        assert_eq!(document["verified"], 412_000);
        assert_eq!(document["restored_per_unit_bytes"], 4);
        assert_eq!(document["has_arm_vector"], true);
        assert!(
            console.stderr().contains("unverified image"),
            "warning must appear before port open: {}",
            console.stderr()
        );
    }

    #[test]
    fn flash_retries_once_when_d3_fails_before_d5() {
        let scratch = crate::testing::Scratch::new("flash-d3-retry");
        let backup = flash_image(&PROFILE_V1_13, false);
        let image = flash_image(&PROFILE_V1_13, true);
        let case = flash_case(&scratch, &image, &backup);
        let chip = m241_flash::FakeDa::with_firmware(&backup).refusing_next_ranges(1);
        let links = da_write::DaWritePorts::new(chip);
        let ports = links.clone();
        let chip = links.chip();
        let console = RecordingConsole::default();
        let session = Session::new(&links, &console, &AutoConfirm).with_timings(Timings::fast());
        let mut options = flash_options(&case);
        options.dry_run = false;
        options.yes = true;
        options.da = Some(da_file(scratch.path()));

        flash(&options, session).expect("the one retry succeeds");
        assert_eq!(
            ports.opens(),
            1,
            "the retry re-enters the BootROM on the same transport, without reopening the port"
        );
        let chip = chip.borrow();
        assert_eq!(
            chip.recorded_ranges(),
            &[(0, FLASH_WRITE_END), (0, FLASH_WRITE_END)],
            "both 0xD3 attempts use the pinned range"
        );
        assert!(chip.restarted(), "the failed D3 was followed by 0xD9");
        assert_eq!(
            chip.opcodes()
                .iter()
                .filter(|opcode| **opcode == 0xA8)
                .count(),
            2,
            "the first attempt loaded the DA once and the retry loaded it again"
        );
        assert_eq!(
            chip.pmtk180_requests(),
            2,
            "the retry re-entered the BootROM through the NMEA probe and $PMTK180 (the initial \
             entry found the fake already in the BootROM), and the read-back used the same route"
        );
        assert_eq!(chip.accepted_packets(), 1_589);
        let stderr = console.stderr();
        assert!(
            stderr.contains("0xD3 failed before 0xD5"),
            "the retry is explained: {stderr}"
        );
        assert!(
            stderr.contains("retrying the write once"),
            "the retry is bounded to one: {stderr}"
        );
    }

    #[test]
    fn flash_reports_no_battery_removal_after_two_d3_failures() {
        let scratch = crate::testing::Scratch::new("flash-d3-two-failures");
        let backup = flash_image(&PROFILE_V1_13, false);
        let image = flash_image(&PROFILE_V1_13, true);
        let case = flash_case(&scratch, &image, &backup);
        let chip = m241_flash::FakeDa::with_firmware(&backup).refusing_next_ranges(2);
        let links = da_write::DaWritePorts::new(chip);
        let chip = links.chip();
        let console = RecordingConsole::default();
        let session = Session::new(&links, &console, &AutoConfirm).with_timings(Timings::fast());
        let mut options = flash_options(&case);
        options.dry_run = false;
        options.yes = true;
        options.da = Some(da_file(scratch.path()));

        let err = flash(&options, session).expect_err("both D3 attempts are refused");
        assert!(matches!(err, Error::Rejected { .. }), "got {err}");
        let text = format!("{err}");
        assert!(
            text.contains("no battery removal is needed"),
            "the unit was restarted: {text}"
        );
        assert!(
            text.contains("automatic retry also failed"),
            "the retry is reported as attempted: {text}"
        );
        assert!(
            text.contains("nothing was written"),
            "both attempts stopped before 0xD5: {text}"
        );
        assert!(
            !text.contains("--recover"),
            "nothing was written, so a backup restore is not the next step: {text}"
        );
        assert!(text.contains("fw dump"), "the check is named: {text}");
        let chip = chip.borrow();
        assert_eq!(
            chip.recorded_ranges(),
            &[(0, FLASH_WRITE_END), (0, FLASH_WRITE_END)],
            "no third 0xD3 and no shifted range"
        );
        assert_eq!(
            chip.restart_requests(),
            2,
            "0xD9 is sent after each failed 0xD3"
        );
        assert_eq!(
            chip.pmtk180_requests(),
            1,
            "the retry re-entered the BootROM through the NMEA probe and $PMTK180"
        );
    }

    #[test]
    fn flash_does_not_retry_when_the_0xd5_header_cannot_be_sent() {
        // The boundary between "0xD3 failed" and "0xD5 may have started": the
        // header write itself fails. A failed write cannot prove that no byte
        // was sent, so the CLI must restart and stop, not retry as for a D3
        // failure. If `program_started` were set only after a successful header
        // write, this test would see two 0xD3 attempts.
        let scratch = crate::testing::Scratch::new("flash-d5-header-fails");
        let backup = flash_image(&PROFILE_V1_13, false);
        let image = flash_image(&PROFILE_V1_13, true);
        let case = flash_case(&scratch, &image, &backup);
        let chip = m241_flash::FakeDa::with_firmware(&backup).failing_write_command();
        let links = da_write::DaWritePorts::new(chip);
        let chip = links.chip();
        let console = RecordingConsole::default();
        let session = Session::new(&links, &console, &AutoConfirm).with_timings(Timings::fast());
        let mut options = flash_options(&case);
        options.dry_run = false;
        options.yes = true;
        options.da = Some(da_file(scratch.path()));

        let err = flash(&options, session).expect_err("the 0xD5 header write fails");
        let text = format!("{err}");
        assert!(
            text.contains("no battery removal is needed"),
            "the unit was restarted: {text}"
        );
        assert!(
            text.contains("not retried automatically"),
            "a failure after 0xD5 may have started is not retried: {text}"
        );
        assert!(
            text.contains("--recover"),
            "the recovery route is named: {text}"
        );
        assert!(text.contains("fw dump"), "the check is named: {text}");
        let chip = chip.borrow();
        assert_eq!(
            chip.recorded_ranges(),
            &[(0, FLASH_WRITE_END)],
            "only one 0xD3: the failure is not retried as a D3 failure"
        );
        assert_eq!(chip.restart_requests(), 1, "0xD9 is still sent");
        assert_eq!(
            chip.opcodes()
                .iter()
                .filter(|opcode| **opcode == 0xA8)
                .count(),
            1,
            "the DA is loaded once, not again for a retry"
        );
        assert_eq!(chip.accepted_packets(), 0, "no packet was programmed");
    }

    #[test]
    fn flash_retry_reentry_failure_does_not_claim_a_write_finished() {
        // After a failed 0xD3 nothing was written. If the re-entry for the retry
        // then fails, the error must not reuse the read-back wording ("the write
        // itself finished", "restore with --recover"): that would invite a
        // destructive recovery run for a unit whose flash is intact.
        let scratch = crate::testing::Scratch::new("flash-retry-no-reentry");
        let backup = flash_image(&PROFILE_V1_13, false);
        let image = flash_image(&PROFILE_V1_13, true);
        let case = flash_case(&scratch, &image, &backup);
        let chip = m241_flash::FakeDa::with_firmware(&backup)
            .refusing_next_ranges(1)
            .without_nmea_stream();
        let links = da_write::DaWritePorts::new(chip);
        let chip = links.chip();
        let console = RecordingConsole::default();
        let session = Session::new(&links, &console, &AutoConfirm).with_timings(Timings::fast());
        let mut options = flash_options(&case);
        options.dry_run = false;
        options.yes = true;
        options.da = Some(da_file(scratch.path()));

        let err = flash(&options, session).expect_err("the retry cannot re-enter");
        assert!(matches!(err, Error::State(_)), "got {err}");
        let text = format!("{err}");
        assert!(
            text.contains("did not stream NMEA"),
            "the cause is named: {text}"
        );
        assert!(
            text.contains("automatic retry was not attempted"),
            "the retry did not run: {text}"
        );
        assert!(
            text.contains("Nothing was written"),
            "the flash is intact: {text}"
        );
        assert!(
            !text.contains("The write itself finished"),
            "no write finished on this path: {text}"
        );
        assert!(
            !text.contains("read-back was not performed"),
            "this failure is not about a read-back: {text}"
        );
        assert!(
            !text.contains("--recover"),
            "nothing was written, so --recover is not the next step: {text}"
        );
        assert!(text.contains("fw dump"), "the check is named: {text}");
        let chip = chip.borrow();
        assert_eq!(
            chip.recorded_ranges(),
            &[(0, FLASH_WRITE_END)],
            "only the failed 0xD3 was sent"
        );
        assert_eq!(
            chip.restart_requests(),
            1,
            "the failed D3 is followed by one 0xD9"
        );
        assert_eq!(
            chip.pmtk180_requests(),
            0,
            "a silent line must not receive $PMTK180"
        );
    }

    #[test]
    fn flash_does_not_retry_after_d5_has_started() {
        let scratch = crate::testing::Scratch::new("flash-d5-no-retry");
        let backup = flash_image(&PROFILE_V1_13, false);
        let image = flash_image(&PROFILE_V1_13, true);
        let case = flash_case(&scratch, &image, &backup);
        let chip = m241_flash::FakeDa::with_firmware(&backup).refusing_packets_from(2, 1);
        let links = da_write::DaWritePorts::new(chip);
        let chip = links.chip();
        let console = RecordingConsole::default();
        let session = Session::new(&links, &console, &AutoConfirm).with_timings(Timings::fast());
        let mut options = flash_options(&case);
        options.dry_run = false;
        options.yes = true;
        options.da = Some(da_file(scratch.path()));

        let err = flash(&options, session).expect_err("the third packet is refused");
        let text = format!("{err}");
        assert!(
            text.contains("no battery removal is needed"),
            "the unit was restarted: {text}"
        );
        assert!(
            text.contains("not retried automatically"),
            "a partial write is not retried: {text}"
        );
        assert!(
            text.contains("--recover"),
            "the recovery route is named: {text}"
        );
        assert!(text.contains("fw dump"), "the check is named: {text}");
        let chip = chip.borrow();
        assert_eq!(
            chip.recorded_ranges(),
            &[(0, FLASH_WRITE_END)],
            "only one 0xD3, with the pinned range"
        );
        assert!(chip.restarted(), "0xD9 is still sent after a D5 failure");
        assert_eq!(
            chip.restart_requests(),
            1,
            "one restart, and no retry after 0xD5 started"
        );
    }

    #[test]
    fn flash_recover_retries_once_when_d3_fails_before_d5() {
        let scratch = crate::testing::Scratch::new("flash-recover-d3-retry");
        let backup = flash_image(&PROFILE_V1_13, false);
        let image = backup.clone();
        let case = flash_case(&scratch, &image, &backup);
        let chip = m241_flash::FakeDa::with_firmware(&backup).refusing_next_ranges(1);
        let links = da_write::DaWritePorts::new(chip);
        let chip = links.chip();
        let console = RecordingConsole::default();
        let session = Session::new(&links, &console, &AutoConfirm).with_timings(Timings::fast());
        let mut options = flash_options(&case);
        options.dry_run = false;
        options.recover = true;
        options.yes = true;
        options.da = Some(da_file(scratch.path()));

        flash(&options, session).expect("recovery also gets the one retry");
        let chip = chip.borrow();
        assert_eq!(
            chip.recorded_ranges(),
            &[(0, FLASH_WRITE_END), (0, FLASH_WRITE_END)]
        );
        assert!(chip.restarted());
    }

    #[test]
    fn flash_allow_vendor_retries_once_when_d3_fails_before_d5() {
        let Some(vendor) = vendor_image() else {
            return;
        };
        let backup = vendor_backup(&vendor);
        let scratch = crate::testing::Scratch::new("flash-vendor-d3-retry");
        let case = flash_case(&scratch, &vendor, &backup);
        let chip = m241_flash::FakeDa::with_firmware(&backup).refusing_next_ranges(1);
        let links = da_write::DaWritePorts::new(chip);
        let chip = links.chip();
        let console = RecordingConsole::default();
        let session = Session::new(&links, &console, &AutoConfirm).with_timings(Timings::fast());
        let mut options = flash_options(&case);
        options.dry_run = false;
        options.allow_vendor = true;
        options.yes = true;
        options.da = Some(da_file(scratch.path()));

        flash(&options, session).expect("the vendor path also gets the one retry");
        let chip = chip.borrow();
        assert_eq!(
            chip.recorded_ranges(),
            &[(0, FLASH_WRITE_END), (0, FLASH_WRITE_END)]
        );
        assert!(chip.restarted());
        assert_eq!(&chip.memory()[..FLASH_WRITE_LENGTH], &vendor[..]);
    }

    #[test]
    fn flash_writes_through_the_da_and_reports_the_verified_read_back() {
        // The end-to-end path against the write crate's own fake DA: entry,
        // `verify_device`, the RAM load, `0xD3` + `0xD5`, `0xD9`, the BootROM
        // re-entry and the read-back. A CLI that skipped `verify_written` would
        // have no report to print, and one that skipped `verify_device` would
        // get to `0xD3` instead of the mismatch test above.
        let scratch = crate::testing::Scratch::new("flash-real-e2e");
        let backup = flash_image(&PROFILE_V1_13, false);
        let image = flash_image(&PROFILE_V1_13, true);
        let case = flash_case(&scratch, &image, &backup);
        let chip = m241_flash::FakeDa::with_firmware(&backup);
        let links = da_write::DaWritePorts::new(chip);
        let chip = links.chip();
        let console = RecordingConsole::default();
        let session = Session::new(&links, &console, &AutoConfirm).with_timings(Timings::fast());
        let mut options = flash_options(&case);
        options.dry_run = false;
        options.yes = true;
        options.da = Some(da_file(scratch.path()));

        flash(&options, session).expect("the fake completes the write and the read-back");

        {
            let chip = chip.borrow();
            assert_eq!(
                chip.recorded_range(),
                (FLASH_WRITE_START, FLASH_WRITE_END),
                "0xD3 recorded the settled range"
            );
            assert_eq!(chip.accepted_packets(), 1589, "every 0xD5 packet landed");
            assert!(chip.restarted(), "the read-back sent 0xD9");
            assert_eq!(
                chip.mode(),
                m241_flash::simulator::Mode::BootRom,
                "the NMEA probe and $PMTK180 moved the chip back to the BootROM"
            );
            assert!(
                chip.pmtk180(),
                "the read-back required the hand-over sentence, not a set_baud shim"
            );
            assert!(
                chip.bauds().ends_with(&[NMEA_BAUD, DEFAULT_BOOTROM_BAUD]),
                "the re-entry must switch to NMEA baud and back to the BootROM rate: {:?}",
                chip.bauds()
            );
            assert!(
                chip.control_lines().is_empty(),
                "the re-entry must never assert DTR/RTS: {:?}",
                chip.control_lines()
            );
            assert_eq!(
                &chip.memory()[..FLASH_WRITE_LENGTH],
                &image[..],
                "the flash window now holds the image"
            );
            assert!(
                !chip.wrote_an_empty_range(),
                "no 0xD5 ever ran without a recorded range"
            );
        }
        assert_eq!(
            fileio::read(&case.1).expect("the backup is still there"),
            backup,
            "a read-only input is never modified"
        );

        let stdout = console.stdout();
        assert!(stdout.contains("port        : SIM0"), "got {stdout}");
        assert!(
            stdout.contains("start       : 0x00000000"),
            "the report names the range: {stdout}"
        );
        assert!(
            stdout.contains("length      : 406620"),
            "the report names the length: {stdout}"
        );
        assert!(
            stdout.contains("packets     : 1589"),
            "the report names the accepted packets: {stdout}"
        );
        assert!(
            stdout.contains("verified    : 406620 bytes read back and identical"),
            "success is the read-back, not the write: {stdout}"
        );
        assert!(
            stdout.contains("remove the battery for about ten seconds"),
            "the unit is left in the BootROM; the success output must say to power-cycle: {stdout}"
        );
        assert!(stdout.contains("flash write : yes"), "got {stdout}");
        let stderr = console.stderr();
        assert!(
            stderr.contains("the unit matches the backup"),
            "the pre-write device check is reported: {stderr}"
        );
        assert!(
            stderr.contains("reading the written range back"),
            "the read-back step is reported: {stderr}"
        );
    }

    #[test]
    fn flash_reports_a_read_back_that_could_not_be_reached() {
        // The write completes, then the restarted firmware never streams NMEA.
        // This must be reported as "the read-back could not be reached", not as
        // a mismatch and not as "the range is only partly programmed".
        let scratch = crate::testing::Scratch::new("flash-real-no-reentry");
        let backup = flash_image(&PROFILE_V1_13, false);
        let image = flash_image(&PROFILE_V1_13, true);
        let case = flash_case(&scratch, &image, &backup);
        let chip = m241_flash::FakeDa::with_firmware(&backup).without_nmea_stream();
        let links = da_write::DaWritePorts::new(chip);
        let chip = links.chip();
        let console = RecordingConsole::default();
        let session = Session::new(&links, &console, &AutoConfirm).with_timings(Timings::fast());
        let mut options = flash_options(&case);
        options.dry_run = false;
        options.yes = true;
        options.da = Some(da_file(scratch.path()));

        let err = flash(&options, session).expect_err("the re-entry has no NMEA stream");
        assert!(matches!(err, Error::State(_)), "got {err}");
        let text = format!("{err}");
        assert!(
            text.contains("did not stream NMEA"),
            "the cause is named: {text}"
        );
        assert!(
            text.contains("read-back could not be reached"),
            "the write completed but the read-back did not run: {text}"
        );
        assert!(
            !text.contains("read-back disagreed"),
            "a missing read-back is not a mismatch: {text}"
        );
        assert!(
            !text.contains("only partly erased"),
            "every packet was accepted, so the range is not partly programmed: {text}"
        );

        {
            let chip = chip.borrow();
            assert!(chip.restarted(), "0xD9 was sent");
            assert_eq!(chip.accepted_packets(), 1589, "every packet landed");
            assert_eq!(
                chip.mode(),
                m241_flash::simulator::Mode::Firmware,
                "no re-entry happened"
            );
            assert!(!chip.pmtk180(), "no $PMTK180 was sent");
            assert!(
                chip.control_lines().is_empty(),
                "the re-entry must never assert DTR/RTS: {:?}",
                chip.control_lines()
            );
        }
        assert!(
            console.stderr().contains("the run did not finish"),
            "the CLI still tells the user the backup was not modified: {}",
            console.stderr()
        );
        assert!(
            !console.stderr().contains("the write did not complete"),
            "the write completed; only the read-back did not run: {}",
            console.stderr()
        );
    }

    #[test]
    fn flash_reports_an_unusable_line_after_the_restart() {
        // Bytes arrive but never a complete NMEA sentence. Sending $PMTK180
        // blind is refused (AGENTS.md §2.2/§4.6), so the read-back must not run.
        let scratch = crate::testing::Scratch::new("flash-real-garbled-reentry");
        let backup = flash_image(&PROFILE_V1_13, false);
        let image = flash_image(&PROFILE_V1_13, true);
        let case = flash_case(&scratch, &image, &backup);
        let chip = m241_flash::FakeDa::with_firmware(&backup).with_garbled_stream();
        let links = da_write::DaWritePorts::new(chip);
        let chip = links.chip();
        let console = RecordingConsole::default();
        let session = Session::new(&links, &console, &AutoConfirm).with_timings(Timings::fast());
        let mut options = flash_options(&case);
        options.dry_run = false;
        options.yes = true;
        options.da = Some(da_file(scratch.path()));

        let err = flash(&options, session).expect_err("the line is not an NMEA sentence");
        assert!(matches!(err, Error::State(_)), "got {err}");
        let text = format!("{err}");
        assert!(
            text.contains("no complete NMEA sentence"),
            "the unusable line is named: {text}"
        );
        assert!(
            text.contains("read-back was not performed"),
            "the read-back did not run: {text}"
        );
        assert!(!text.contains("did not confirm"));
        assert!(
            !chip.borrow().pmtk180(),
            "no hand-over may be sent at an unclassified line"
        );
    }

    #[test]
    fn the_write_report_json_names_the_verified_count_and_the_flash_write() {
        let backup = flash_image(&PROFILE_V1_13, false);
        let image = flash_image(&PROFILE_V1_13, true);
        let plan = write_plan(&PROFILE_V1_13, &backup, &image).expect("plan");
        let report = WriteReport {
            start: FLASH_WRITE_START,
            length: FLASH_WRITE_LENGTH,
            packets: plan.packets,
            verified: FLASH_WRITE_LENGTH,
            restored_per_unit_bytes: 0,
        };
        let console = RecordingConsole::default();
        let options = FlashOptions {
            image: PathBuf::from("image.bin"),
            backup: PathBuf::from("backup.bin"),
            da: None,
            dry_run: false,
            recover: false,
            allow_vendor: false,
            allow_unverified: false,
            yes: true,
            assume_brom: false,
            port: String::from("auto"),
            json: true,
        };
        print_write_report(&options, &console, "SIM0", &plan, &report).expect("json");
        let document: serde_json::Value =
            serde_json::from_str(&console.stdout()).expect("one JSON object");
        assert_eq!(document["dry_run"], false);
        assert_eq!(document["start"], "0x00000000");
        assert_eq!(document["length"], 406_620);
        assert_eq!(document["packets"], 1_589);
        assert_eq!(document["verified"], 406_620);
        assert_eq!(document["verified_matches_image"], true);
        assert_eq!(document["flash_write"], true);
        assert_eq!(document["power_cycle_required"], true);
        assert_eq!(document["port"], "SIM0");
    }

    #[test]
    fn flash_json_is_one_object_that_says_no_port_was_opened() {
        let scratch = crate::testing::Scratch::new("flash-json");
        let backup = flash_image(&PROFILE_V1_13, false);
        let image = flash_image(&PROFILE_V1_13, true);
        let case = flash_case(&scratch, &image, &backup);
        let console = RecordingConsole::default();
        let links = FakePorts::empty();
        let session = Session::new(&links, &console, &AutoConfirm);
        let mut options = flash_options(&case);
        options.json = true;
        flash(&options, session).expect("dry run json");
        let document: serde_json::Value =
            serde_json::from_str(&console.stdout()).expect("one JSON object");
        assert_eq!(document["dry_run"], true);
        assert_eq!(document["profile"], "V1.13");
        assert_eq!(document["acceptance"], "patched backup");
        assert_eq!(document["size"], 406_620);
        assert_eq!(document["device_type"], "01017-00F");
        assert_eq!(document["write_length"], 406_620);
        assert_eq!(document["packets"], 1_589);
        assert_eq!(document["opens_port"], false);
        assert_eq!(document["wrote_flash"], false);
        assert_eq!(document["actual_write_refused"], false);
        assert_eq!(document["start"], "0x00000000");
        assert!(
            document["actual_write"]
                .as_str()
                .is_some_and(|line| line.starts_with("permitted")),
            "a V1.13 patched backup would be written: {}",
            document["actual_write"]
        );
        assert_eq!(document["differing_bytes"], 3);
    }

    // -- fw probe-da, end to end against a fake DA -------------------------

    /// The synthetic DA every CLI write test writes, and then reads back: the
    /// vendor's length and first four bytes, with every other byte distinct so a
    /// wrong relay shows up in the read-back.
    ///
    /// The vendor binary is never committed (`AGENTS.md` §6), so [`da_digest`]
    /// requires this fixture's own digest in the test build instead of the vendor
    /// value. It differs from `m241-flash`'s `da_image()` fixture in the bytes at
    /// `DA_REPORT_OFFSET` only: the fake DA there answers the 20-byte report from
    /// the image it kept in RAM, and the CLI's fake supplies its own. Both share
    /// the `index % 251` pattern and this head, so neither can be mistaken for
    /// the vendor file.
    ///
    /// `pub(super)`: [`da_digest`] in the parent module is the only caller
    /// outside this test module, and it needs the same bytes to hash.
    pub(super) fn da_fixture_bytes() -> Vec<u8> {
        let mut image: Vec<u8> = (0..m241_flash::da::DA_IMAGE_SIZE)
            .map(|index| (index % 251) as u8)
            .collect();
        if let Some(head) = image.get_mut(..m241_flash::da::DA_IMAGE_HEAD.len()) {
            head.copy_from_slice(&m241_flash::da::DA_IMAGE_HEAD);
        }
        image
    }

    fn da_file(dir: &Path) -> PathBuf {
        let path = dir.join("DA_v3.8.4.bin");
        fileio::write(&path, &da_fixture_bytes()).expect("write the DA");
        path
    }

    /// The opcodes the probe is allowed to send, once the BootROM frame bytes
    /// and the `0xD6` page conversation are separated from the rest.
    ///
    /// A BootROM frame contributes one recorded byte (`0xA1`, `0xAF`, `0xA8`);
    /// a DA command contributes its opcode, and the page conversation
    /// contributes its `0x5A` advances. Anything else — `0xD3`, `0xD4`,
    /// `0xD5`, `0xD9`, `0xA2`, or a stray byte — is not in this set.
    const ALLOWED: [u8; 6] = [
        m241_core::protocol::brom::OPCODE_WRITE16,
        m241_core::protocol::brom::OPCODE_READ32,
        m241_core::protocol::brom::OPCODE_JUMP,
        m241_flash::da::OPCODE_READ_RANGE,
        m241_flash::da::ACK,
        m241_flash::da::NAK,
    ];

    fn assert_no_destructive_opcode(record: &fake_da::Record) {
        for opcode in &record.opcodes {
            assert!(
                ALLOWED.contains(opcode),
                "0x{opcode:02X} is not part of loading the DA or reading through it; \
                 everything on the wire was {}",
                record.opcodes_hex()
            );
        }
        for destructive in [
            m241_flash::da::OPCODE_MEM_CMD, // 0xD3
            0xD4,                           // erase a range, never sent by this crate
            m241_flash::da::OPCODE_WRITE,   // 0xD5
            m241_flash::da::OPCODE_RESTART, // 0xD9
        ] {
            assert!(
                !record.opcodes.contains(&destructive),
                "fw probe-da must never send 0x{destructive:02X}: {}",
                record.opcodes_hex()
            );
        }
    }

    #[test]
    fn probe_da_reads_both_candidates_through_the_fake_and_sends_no_write_opcode() {
        // The safety claim for this command, checked on the code the command
        // actually runs: DA load, jump, two `0xD6` reads, verdict. A `0xD5`
        // slipped in beside `DownloadAgent::new` used to leave every gate green
        // because nothing drove `probe_da` past its port step.
        let scratch = Scratch::new("probe-fake");
        let da = da_file(scratch.path());
        // A running firmware, so the documented `$PMTK180` route is the one the
        // command takes. The first candidate carries only the vector's first
        // byte: a verdict that compared one byte would settle on `0x0020_0000`
        // at best (both match) or on the wrong address, and either fails below.
        let mut flash = vec![0xFFu8; 0x0020_0000 + 0x1000];
        if let Some(slot) = flash.get_mut(..4) {
            slot.copy_from_slice(&[VECTOR[0], 0x00, 0x00, 0x00]);
        }
        let at = PROBE_ADDRESSES[1] as usize;
        if let Some(slot) = flash.get_mut(at..at + VECTOR.len()) {
            slot.copy_from_slice(&VECTOR);
        }
        let chip = fake_da::FakeChip::new(fake_da::Mode::Firmware, flash);
        let links = fake_da::FakeDaPorts::new(chip.clone());
        let record = links.record();
        let console = RecordingConsole::default();
        // 500 ms, not `fast()`'s 20 ms: the fake DA answers synchronously but
        // loses replies when the workspace tests run in parallel.
        let session =
            Session::new(&links, &console, &AutoConfirm).with_timings(Timings::generous());
        let mut options = probe_options(&da);
        options.yes = true;
        options.json = true;

        probe_da(&options, session).expect("the fake DA answers");

        let record = record.borrow();
        assert_no_destructive_opcode(&record);
        assert!(
            record.pmtk180,
            "the stream was live, so the sentence went out"
        );
        assert_eq!(
            record.nmea_after, 0,
            "the fake switches on the sentence, so nothing may stream after it"
        );
        // One probe, not two. The fake gives each `clear_input`-delimited segment
        // one sentence and no segment more than one, and `probe_stream` returns
        // on the first sentence it sees. So a second probe — what
        // `enter_bootrom()` does internally where `enter_bootrom_after(state)`
        // does not — finds its segment already spent and the command refuses
        // instead of getting this far.
        assert_eq!(
            record.nmea_before, 1,
            "one sentence, one probe: {} before $PMTK180",
            record.nmea_before
        );

        // Both candidates were read, in order, at the page size the probe asks
        // for — dropping the second one must fail here.
        assert_eq!(
            record.reads,
            vec![
                (
                    u32::try_from(PROBE_PAGE_SIZE).unwrap(),
                    PROBE_ADDRESSES[0],
                    u32::try_from(PROBE_PAGE_SIZE).unwrap()
                ),
                (
                    u32::try_from(PROBE_PAGE_SIZE).unwrap(),
                    PROBE_ADDRESSES[1],
                    u32::try_from(PROBE_PAGE_SIZE).unwrap()
                ),
            ],
            "one 0xD6 per candidate, in PROBE_ADDRESSES order"
        );

        // The load itself: `0xA1` into RAM at the DA base, one `0xA8` back to
        // it with count zero, and nothing written outside the DA's span.
        let span = m241_flash::da::DA_LOAD_ADDRESS
            ..m241_flash::da::DA_LOAD_ADDRESS + m241_flash::da::DA_IMAGE_SIZE as u32;
        assert!(!record.writes.is_empty(), "the DA was streamed into RAM");
        for (address, payload) in &record.writes {
            assert!(
                span.contains(address),
                "0xA1 to 0x{address:08X} is outside the DA's RAM span"
            );
            let end = address.saturating_add(u32::try_from(payload.len()).unwrap());
            assert!(
                span.contains(&end.saturating_sub(1)),
                "the chunk ending at 0x{end:08X} runs past the DA's RAM span"
            );
        }
        assert_eq!(
            record.jumps,
            vec![(m241_flash::da::DA_LOAD_ADDRESS, 0)],
            "exactly one jump, to the DA base, with the documented count"
        );

        // The firmware streams and `$PMTK180` is what moved the chip to the
        // BootROM; no reset line was touched.
        assert!(record.pmtk180, "the documented entry route was used");
        assert!(
            !record.control_lines_asserted,
            "DTR/RTS must never be pulsed at a unit (AGENTS.md §2.1)"
        );

        let document: serde_json::Value =
            serde_json::from_str(&console.stdout()).expect("one JSON object");
        assert_eq!(document["conclusion"], "settled");
        assert_eq!(
            document["firmware_base"],
            format!("0x{:08X}", PROBE_ADDRESSES[1]),
            "the vector was at the second candidate, so that is the base"
        );
        assert_eq!(document["probe"][0]["read"], true);
        assert_eq!(
            document["probe"][0]["first_bytes"], "18 00 00 00",
            "the first candidate shares only the vector's first byte"
        );
        assert_eq!(document["probe"][0]["vector_match"], false);
        assert_eq!(document["probe"][1]["vector_match"], true);
        assert_eq!(document["wrote_flash"], false);
        assert_eq!(document["vector_verified_through_da"], false);
        assert_eq!(document["da"]["version"], "0x0308");
    }

    #[test]
    fn probe_da_with_the_vector_at_the_first_candidate_settles_on_it() {
        // The other direction: the verdict follows the bytes, not a preference
        // for either address.
        let scratch = Scratch::new("probe-fake-first");
        let da = da_file(scratch.path());
        let chip = fake_da::FakeChip::with_vector_at(PROBE_ADDRESSES[0]);
        let links = fake_da::FakeDaPorts::new(chip);
        let record = links.record();
        let console = RecordingConsole::default();
        // 500 ms, not `fast()`'s 20 ms: the fake DA answers synchronously but
        // loses replies when the workspace tests run in parallel.
        let session =
            Session::new(&links, &console, &AutoConfirm).with_timings(Timings::generous());
        let mut options = probe_options(&da);
        options.yes = true;
        options.json = true;

        probe_da(&options, session).expect("the fake DA answers");

        let record = record.borrow();
        assert_no_destructive_opcode(&record);
        assert_eq!(record.reads.len(), 2, "both candidates are still read");
        let document: serde_json::Value =
            serde_json::from_str(&console.stdout()).expect("one JSON object");
        assert_eq!(
            document["firmware_base"],
            format!("0x{:08X}", PROBE_ADDRESSES[0])
        );
        assert_eq!(document["probe"][0]["vector_match"], true);
        assert_eq!(document["probe"][1]["vector_match"], false);
    }

    #[test]
    fn probe_da_uses_silence_and_assume_brom_without_touching_the_reset_lines() {
        // A chip already parked in the BootROM: the stream probe finds nothing,
        // the silence route is taken, and still nothing is pulsed.
        let scratch = Scratch::new("probe-fake-silent");
        let da = da_file(scratch.path());
        let chip = fake_da::FakeChip::with_vector_at(PROBE_ADDRESSES[1]);
        let links = fake_da::FakeDaPorts::new(chip);
        let record = links.record();
        let console = RecordingConsole::default();
        // 500 ms, not `fast()`'s 20 ms: the fake DA answers synchronously but
        // loses replies when the workspace tests run in parallel.
        let session =
            Session::new(&links, &console, &AutoConfirm).with_timings(Timings::generous());
        let mut options = probe_options(&da);
        options.yes = true;

        probe_da(&options, session).expect("the fake BootROM answers the sync");

        let record = record.borrow();
        assert!(!record.pmtk180, "no stream, so no sentence");
        assert!(
            !record.control_lines_asserted,
            "silence must not be read as 'pulse the reset lines'"
        );
        assert_eq!(record.reads.len(), 2);
        assert_no_destructive_opcode(&record);
        assert!(
            console.stderr().contains("no NMEA stream"),
            "the silence route is reported: {}",
            console.stderr()
        );
    }

    #[test]
    fn da_restart_sends_one_d9_without_reading_a_reply_or_touching_the_control_lines() {
        let backup = flash_image(&PROFILE_V1_13, false);
        let chip = m241_flash::FakeDa::with_firmware(&backup);
        let links = da_write::DaWritePorts::new(chip);
        let chip = links.chip();
        let before = chip.borrow().memory().to_vec();
        let console = RecordingConsole::default();
        let session = Session::new(&links, &console, &AutoConfirm);
        let options = DaRestartOptions {
            port: String::from("auto"),
        };

        da_restart(&options, session).expect("the restart byte is written");

        assert_eq!(links.opens(), 1, "one port open");
        let chip = chip.borrow();
        assert_eq!(chip.restart_requests(), 1, "exactly one 0xD9");
        assert!(chip.restarted(), "the fake saw the restart");
        assert_eq!(chip.read_polls(), 0, "0xD9 has no reply to read");
        assert!(chip.opcodes().is_empty(), "no 0xD3 / 0xD5 / 0xD6");
        assert!(chip.recorded_ranges().is_empty(), "no range was recorded");
        assert!(chip.packets().is_empty(), "no write packet was sent");
        assert!(
            chip.control_lines().is_empty(),
            "DTR/RTS must never be touched"
        );
        assert_eq!(chip.bauds(), &[DEFAULT_BOOTROM_BAUD], "opened at 115200");
        assert_eq!(chip.memory(), before.as_slice(), "flash contents unchanged");
        assert!(
            console.stdout().contains(
                "DA restart sent; no flash was written. 0xD9 has no reply, so the restart cannot \
                 be confirmed from this command: run `m241 fw dump` to check the unit answers."
            ),
            "the result says what happened and what cannot be confirmed: {}",
            console.stdout()
        );
    }
}
