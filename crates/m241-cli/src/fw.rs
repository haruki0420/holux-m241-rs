//! `m241 fw dump`, `m241 fw patch` and `m241 fw probe-da`.
//!
//! The halves of the repair, deliberately asymmetric. [`dump`] touches the
//! device and is read-only: it reads the image twice, comparing hashes, so a
//! read that silently truncated cannot be mistaken for success. [`patch`] never
//! opens a port at all — it rewrites a file on this machine, and writing to the
//! device is a different command. [`probe_da`] loads the DA into the chip's RAM
//! and reads two candidate addresses through it; it writes no flash either, and
//! exists because the address `fw flash` refuses on cannot be settled any other
//! way.
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
    Confirmation, DaLoadId, DaLoader, DownloadAgent, FLASH_WRITE_START, PROBE_ADDRESSES,
    PROBE_PAGE_SIZE, VECTOR, WritePlan, WriteReport, check_da_image, check_image, check_layout,
    read_candidates, write_plan,
};

use crate::Session;
use crate::cli::{DumpOptions, FlashOptions, PatchOptions, ProbeDaOptions};
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
/// runs [`write_plan`] — the same acceptance rule [`DownloadAgent::write`] runs —
/// and prints what a write would do. It **opens no port and sends nothing**.
///
/// A real run does everything decidable without a device first, in this order:
/// the `--yes` confirmation, [`check_layout`], the driver's own acceptance rule
/// against the pinned V1.13 range ([`check_image`]), and the DA file
/// ([`check_da_image`]). Only then is a port opened. On the device it:
///
/// 1. enters the BootROM ([`enter_bootrom`]) and syncs;
/// 2. calls [`DownloadAgent::verify_device`], so a unit that does not hold
///    `backup` byte for byte is refused **before** anything is erased;
/// 3. loads the DA into the chip's RAM ([`DaLoader::load`], RAM only);
/// 4. programs the image ([`DownloadAgent::write`], `0xD3` + `0xD5`);
/// 5. restarts the chip (`0xD9`), re-enters the BootROM through the same NMEA
///    probe and `$PMTK180*3B` hand-over the initial entry used, and reads the
///    range back ([`PendingWrite::verify_written`]), reporting success only
///    when every byte matches.
///
/// The read-back after `0xD9` can fail: the unit boots its firmware, and if
/// that firmware does not stream NMEA the BootROM cannot be re-entered. That is
/// reported as a read-back that could not be reached, with the power-cycle and
/// `fw dump` checks; this function never upgrades a failed read-back to success.
/// ⚠️ 未検証 on hardware: the write path, including the post-`0xD9` re-entry,
/// has run against the fake only.
///
/// # Errors
///
/// * [`Error::Io`] when a file cannot be read.
/// * [`Error::Profile`] when the image or the backup matches no known profile.
/// * [`Error::Firmware`] when the image is too short to hold a header.
/// * [`Error::Safety`] when the image is not acceptable ([`write_plan`] /
///   [`check_image`]), when `--yes` was not given, from [`check_layout`], or
///   from [`check_da_image`] / the device check.
/// * [`Error::Port`] when no plausible port can be opened.
/// * [`Error::State`] when the BootROM cannot be re-entered after `0xD9`.
/// * [`Error::Timeout`] / [`Error::Framing`] / [`Error::Rejected`] from the
///   BootROM, the DA, the write or the read-back; each carries the crate's
///   recovery paragraph once anything has been sent.
pub fn flash(options: &FlashOptions, session: Session<'_>) -> Result<()> {
    let backup = fileio::read(&options.backup)?;
    let image = fileio::read(&options.image)?;

    // Identify both. The image decides the profile a plan is made against; the
    // backup is identified only for the message, because a backup of another
    // length is refused by the plan with a precise reason.
    let image_view = FirmwareImage::new(&image)?;
    let profile = identify(&image_view)?.profile;
    let plan = write_plan(profile, &backup, &image)?;

    // The plan may describe a V1.12 or V1.14 image, but the driver programs the
    // pinned V1.13 range. Run its own acceptance rule now so a dry run can say a
    // write would be refused, and a real run stops before opening a port.
    let writable = check_image(&PROFILE_V1_13, &backup, &image);

    if options.dry_run {
        print_write_plan(
            options,
            session.console,
            &plan,
            writable.as_ref().err().map(ToString::to_string).as_deref(),
        )?;
        session.console.note(&dry_run_note());
        return Ok(());
    }

    // A real write. Everything that can be decided without a port is decided
    // first, in the order a person would want to be told: the confirmation,
    // then the layout and the files the write needs.
    let confirmation = Confirmation::of_command_line(options.yes)?;
    check_layout()?;
    writable?;

    // clap requires `--da` unless `--dry-run`, but the handler does not lean on
    // the grammar: a missing file must not fall through to "load nothing".
    let Some(da_path) = options.da.as_deref() else {
        return Err(Error::State(String::from(
            "no DA image: pass --da FILE (the vendor DA_v3.8.4.bin; not needed with --dry-run)",
        )));
    };
    let da = fileio::read(da_path)?;
    check_da_image(&da)?;
    session.console.note(&format!(
        "DA file {} is {} bytes with SHA-256 {}; `check_da_image` checks only the length and \
         the first four bytes (W2a adds the digest). AGENTS.md §2.5 requires the full hash to \
         match the vendor value before --yes is approved",
        da_path.display(),
        da.len(),
        sha256_hex(&da)
    ));

    // The port is opened only after every refusal above, so a dry run, a
    // missing confirmation and a bad file never touch a cable.
    let port = link::resolve(session.links, &options.port)?;
    session.console.note(&format!(
        "writing {} bytes to {port}; the backup at {} is not modified",
        plan.write_length,
        options.backup.display()
    ));

    let transport = enter_bootrom(options.assume_brom, session, &port)?;
    let mut rom = MtkBootRom::new(transport);
    rom.sync()?;
    session
        .console
        .note("BootROM sync answered; comparing the whole unit with the backup before any erase");
    DownloadAgent::verify_device(&mut rom, &backup)?;

    session.console.note(
        "the unit matches the backup; loading the DA into the chip's RAM (no flash write yet)",
    );
    let mut loader = DaLoader::new(rom.transport_mut());
    let id = loader.load(confirmation, &da)?;
    session.console.note(&format!(
        "DA v{}.{} answered; programming {} bytes in {} packets",
        id.version >> 8,
        id.version & 0xFF,
        plan.write_length,
        plan.packets
    ));

    let mut da_session = DownloadAgent::new(rom.into_transport());
    let pending = match da_session.write(confirmation, &backup, &image) {
        Ok(pending) => pending,
        Err(err) => {
            note_write_failure(session.console, &options.backup);
            return Err(err);
        }
    };
    session.console.note(
        "every packet was acknowledged; restarting the unit and reading the written range back",
    );

    let report = match pending.verify_written(da_session, &image, FLASH_WRITE_START, |transport| {
        reenter_bootrom(transport, session)
    }) {
        Ok(report) => report,
        Err(err) => {
            note_write_failure(session.console, &options.backup);
            return Err(err);
        }
    };

    print_write_report(options, session.console, &port, &plan, &report)
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
/// [`Error::Io`] from the transport.
fn reenter_bootrom<T: Transport>(transport: T, session: Session<'_>) -> Result<T> {
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
            session.console.note(
                "NMEA is streaming again after the restart; sending $PMTK180 to re-enter the \
                 BootROM for the read-back",
            );
            client.enter_bootrom_after(StreamState::Streaming)?;
            Ok(client.into_transport())
        }
        StreamState::Silent => Err(Error::State(String::from(
            "the unit did not stream NMEA after the restart, so the BootROM could not be \
             re-entered and the read-back was not performed. The write itself finished; do not \
             assume it failed. Power-cycle the unit and run `m241 fw dump` to see what it holds, \
             then write your own backup back if needed",
        ))),
        StreamState::Unknown => Err(Error::State(String::from(
            "the line carried bytes but no complete NMEA sentence after the restart, so the \
             BootROM could not be re-entered and the read-back was not performed. Power-cycle \
             the unit and run `m241 fw dump` to see what it holds",
        ))),
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
        "the write did not complete. The backup at {} was not modified by this command; follow \
         the recovery steps in the error above. A power-cycle followed by `m241 fw dump` shows \
         what the unit actually holds — do not treat the write as successful without that check.",
        backup.display()
    ));
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
            "acceptance": "patched backup",
            "start": format!("0x{:08X}", report.start),
            "length": report.length,
            "packets": report.packets,
            "verified": report.verified,
            "verified_matches_image": true,
            "flash_write": true,
        }));
    }
    console.result(&format!("{:<12}: {port}", "port"));
    console.result(&format!("{:<12}: {}", "image", options.image.display()));
    console.result(&format!("{:<12}: {}", "backup", options.backup.display()));
    console.result(&format!(
        "{:<12}: {} ({}, {} bytes)",
        "profile", plan.profile.name, plan.profile.device_type, plan.size
    ));
    console.result(&format!("{:<12}: 0x{:08X}", "start", report.start));
    console.result(&format!("{:<12}: {}", "length", report.length));
    console.result(&format!("{:<12}: {}", "packets", report.packets));
    console.result(&format!(
        "{:<12}: {} bytes read back and identical",
        "verified", report.verified
    ));
    console.result(&format!("{:<12}: yes", "flash write"));
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
            "acceptance": "patched backup",
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
    console.result(&format!("{:<12}: {}", "accepted as", "patched backup"));
    console.result(&format!("{:<12}: {}", "device", plan.device_type));
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
/// `0` returned the vector and DA address `0x00200000` returned the
/// unmapped-bus pattern. A 64-byte page emits 64 data bytes plus the two-byte
/// trailer; multi-page reads and the advance after the last page remain
/// unmeasured (`docs/reference/01-da-protocol.md` §5.5, §7 item 4). A run whose
/// cause is unknown therefore produces *unread*, not *no image here*, and the
/// two are never folded together.
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
                     DA-side vector check itself is still unverified",
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
/// * [`Error::Safety`] without `--yes`, or for a DA file that is not the DA
///   (wrong size, wrong first bytes). Both run before a port is opened.
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
    check_da_image(&da)?;

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
    let id = loader.load(confirmation, &da)?;
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
            let session = Session::new(&links, &console, &confirm).with_timings(Timings::fast());
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
    fn probe_da_with_a_valid_da_reaches_the_port_step_and_refuses_without_one() {
        // The DA path itself is exercised against the fake in `m241-flash`; here
        // the point is that the command gets past its own refusals and then
        // reports the missing port as a port error, not a safety one.
        let scratch = Scratch::new("probe-port");
        let path = scratch.join("DA_v3.8.4.bin");
        let mut image = vec![0u8; 14_896];
        image
            .get_mut(..4)
            .expect("four bytes")
            .copy_from_slice(&m241_flash::da::DA_IMAGE_HEAD);
        fileio::write(&path, &image).expect("write");
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
            stdout.contains("actual write: refused"),
            "the driver writes only the pinned V1.13 range, so the plan must not promise a V1.12 \
             write: {stdout}"
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
    fn flash_dry_run_refuses_an_image_that_is_neither_the_backup_nor_the_vendor_release() {
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
        }
        assert!(
            console.stderr().contains("the write did not complete"),
            "the CLI still tells the user the backup was not modified: {}",
            console.stderr()
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
        };
        let console = RecordingConsole::default();
        let options = FlashOptions {
            image: PathBuf::from("image.bin"),
            backup: PathBuf::from("backup.bin"),
            da: None,
            dry_run: false,
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

    /// The DA file `probe_da` reads: the vendor length and first bytes, with
    /// every other byte distinct so a wrong relay shows up in the read-back.
    fn da_file(dir: &Path) -> PathBuf {
        let path = dir.join("DA_v3.8.4.bin");
        let mut image: Vec<u8> = (0..m241_flash::da::DA_IMAGE_SIZE)
            .map(|index| (index % 251) as u8)
            .collect();
        if let Some(head) = image.get_mut(..m241_flash::da::DA_IMAGE_HEAD.len()) {
            head.copy_from_slice(&m241_flash::da::DA_IMAGE_HEAD);
        }
        fileio::write(&path, &image).expect("write the DA");
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
        let session = Session::new(&links, &console, &AutoConfirm).with_timings(Timings::fast());
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
        let session = Session::new(&links, &console, &AutoConfirm).with_timings(Timings::fast());
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
        let session = Session::new(&links, &console, &AutoConfirm).with_timings(Timings::fast());
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
}
