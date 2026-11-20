//! `m241 fw dump` and `m241 fw patch`.
//!
//! The two halves of the repair, deliberately asymmetric. [`dump`] touches the
//! device and is read-only: it reads the image twice, comparing hashes, so a
//! read that silently truncated cannot be mistaken for success. [`patch`] never
//! opens a port at all — it rewrites a file on this machine, and writing to the
//! device is a different command.
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
use m241_core::firmware::profiles::{Evidence, PROFILE_V1_12, PROFILE_V1_13, VERSION_MARKER_LEN};
use m241_core::firmware::{self, sha256_hex};
use m241_core::protocol::brom::{BASE_ADDR_FIRMWARE, DEFAULT_BOOTROM_BAUD, DUMP_CHUNK, MtkBootRom};
use m241_core::protocol::pmtk::{NMEA_BAUD, StreamState};
use m241_core::protocol::port::Transport;
use m241_core::{Error, FirmwareProfile, PatchPlan, Result};

use crate::Session;
use crate::cli::{DumpOptions, PatchOptions};
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
    let transport = enter_bootrom(options, session, port)?;
    let mut rom = MtkBootRom::new(transport);
    rom.sync()?;
    session
        .console
        .note("BootROM sync answered; reading the firmware image");
    read_flash(&mut rom, options.length, session.console)
}

/// Open `port` and put the transport at BootROM baud using the safe entry route.
fn enter_bootrom(options: &DumpOptions, session: Session<'_>, port: &str) -> Result<Opened> {
    if options.assume_brom {
        // Asked to assume the chip is already listening. Nothing is asserted:
        // this path exists for a unit that a failed write left in the BootROM.
        let mut transport = session.links.open(port, NMEA_BAUD)?;
        transport.set_baud(DEFAULT_BOOTROM_BAUD)?;
        return Ok(transport);
    }

    let mut client = link::open_pmtk(session.links, port, session.timings)?;
    match client.probe_stream()? {
        StreamState::Streaming => {
            session
                .console
                .note("NMEA is streaming; sending $PMTK180 to hand over to the BootROM");
            client.enter_bootrom()?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Timings;
    use crate::testing::{AutoConfirm, DeviceHandle, FakePorts, RecordingConsole, synthetic_image};
    use m241_core::protocol::simulator::FakeDevice;
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
}
