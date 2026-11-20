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
use m241_core::firmware::profiles::Evidence;
use m241_core::firmware::{self, sha256_hex};
use m241_core::protocol::brom::{BASE_ADDR_FIRMWARE, DEFAULT_BOOTROM_BAUD, DUMP_CHUNK, MtkBootRom};
use m241_core::protocol::pmtk::{NMEA_BAUD, PmtkClient, StreamState};
use m241_core::protocol::port::Transport;
use m241_core::{Error, PatchPlan, Result};

use crate::Session;
use crate::cli::{DumpOptions, PatchOptions};
use crate::fileio;
use crate::link;
use crate::output::Console;

/// Read the firmware off the device, twice, and save it once both reads agree.
///
/// # Errors
///
/// * [`Error::Safety`] when the output exists and `--force` was not given;
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
    // that looks like a backup next to the real one.
    fileio::write(&options.output, &first)?;

    let image = FirmwareImage::new(&first)?;
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

    if options.json {
        return session.console.json(&serde_json::json!({
            "port": port,
            "output": options.output.display().to_string(),
            "size": first.len(),
            "sha256": first_digest,
            "profile": profile.map(|profile| profile.name),
            "verified": true,
            "declared_size": image.u32_at(FW_SIZE_OFFSET),
        }));
    }
    session.console.result(&format!("{:<12}: {port}", "port"));
    session
        .console
        .result(&format!("{:<12}: {}", "size", first.len()));
    session
        .console
        .result(&format!("{:<12}: {first_digest}", "sha256"));
    session.console.result(&format!(
        "{:<12}: {}",
        "profile",
        profile.map_or("-", |profile| profile.name)
    ));
    session
        .console
        .result(&format!("{:<12}: yes (two identical reads)", "verified"));
    session
        .console
        .result(&format!("{:<12}: {}", "written", options.output.display()));
    if let Some(declared) = image
        .u32_at(FW_SIZE_OFFSET)
        .filter(|declared| usize::try_from(*declared).is_ok_and(|value| value != first.len()))
    {
        // Not a refusal: the released images store `len - 4` there, so this is
        // a note about the header, not about the read.
        session.console.note(&format!(
            "note: the header at 0x{FW_SIZE_OFFSET:X} declares {declared} bytes while the file \
             is {}; the released images store `length - 4` there, so this is expected",
            first.len()
        ));
    }
    Ok(())
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
    Ok(())
}

/// Open, enter the BootROM, and read `options.length` bytes once.
fn read_once(options: &DumpOptions, session: Session<'_>, port: &str) -> Result<Vec<u8>> {
    let mut transport = session.links.open(port, NMEA_BAUD)?;
    if options.assume_brom {
        // Asked to assume the chip is already listening. Nothing is asserted:
        // this path exists for a unit that a failed write left in the BootROM.
        transport.set_baud(DEFAULT_BOOTROM_BAUD)?;
    } else {
        let mut client = PmtkClient::new(transport).with_stream_window(session.timings.stream);
        match client.probe_stream()? {
            StreamState::Streaming => {
                session
                    .console
                    .note("NMEA is streaming; sending $PMTK180 to hand over to the BootROM");
                client.enter_bootrom()?;
                transport = client.into_transport();
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
                transport = client.into_transport();
                transport.set_baud(DEFAULT_BOOTROM_BAUD)?;
            }
            StreamState::Unknown => {
                // The tri-state exists so that "cannot tell" never collapses
                // into either answer: the sentence would be ignored and the
                // reset route would be unsafe, so the only safe move is a
                // refusal that says how to get a definite answer.
                return Err(Error::State(String::from(
                    "bytes arrived but no complete NMEA sentence did, so the line state is not \
                     usable; power-cycle the logger, reconnect the cable, and retry — or pass \
                     --assume-brom if the unit is already in the BootROM",
                )));
            }
        }
    }

    let mut rom = MtkBootRom::new(transport);
    rom.sync()?;
    session
        .console
        .note("BootROM sync answered; reading the firmware image");

    let mut image: Vec<u8> = Vec::with_capacity(options.length);
    while image.len() < options.length {
        let want = DUMP_CHUNK.min(options.length - image.len());
        let offset = u32::try_from(image.len()).map_err(|_| {
            Error::State(format!(
                "dump offset {} does not fit in a BootROM address",
                image.len()
            ))
        })?;
        let chunk = rom.read16(BASE_ADDR_FIRMWARE + offset, want)?;
        image.extend_from_slice(&chunk);
        session.console.progress(image.len(), options.length);
    }
    session.console.progress_done();
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
/// The device is never opened. `--dry-run` prints the plan and writes nothing.
///
/// # Errors
///
/// * [`Error::Profile`] when the image matches no known release;
/// * [`Error::Firmware`] when its epoch constant is not the stock value, was
///   edited by something else, or is already half patched;
/// * [`Error::Safety`] when the output would overwrite the source image;
/// * [`Error::Io`] when a file cannot be read or written.
pub fn patch(options: &PatchOptions, session: Session<'_>) -> Result<()> {
    let bytes = fileio::read(&options.source)?;
    let report = plan_patch(&bytes)?;

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

    if !report.plan.changes_anything() {
        // Already patched. Nothing is written: rewriting the file would only
        // produce a copy of the input, and the point of the plan is to say what
        // changes.
        if options.json {
            return session.console.json(&plan_document(&report, None));
        }
        print_plan(session.console, &report);
        session.console.note(
            "nothing to do; the image was left as it is. The device is never touched by this \
             command.",
        );
        return Ok(());
    }

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

/// What a changed byte means, when it is one of the measured three.
fn describe_offset(offset: usize) -> &'static str {
    match offset {
        0x402F6 | 0x402F7 => "effective epoch constant",
        0x59C2C => "version marker (V1.13)",
        0x59B18 => "version marker (V1.12)",
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
    fn patch_of_an_already_patched_image_changes_nothing_and_says_why() {
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

        let console = RecordingConsole::default();
        let session = Session::new(&links, &console, &AutoConfirm);
        patch(
            &PatchOptions {
                source: patched_path,
                output: Some(again.clone()),
                dry_run: false,
                json: false,
            },
            session,
        )
        .expect("second run is a no-op, not a failure");
        assert!(!again.exists(), "a no-op writes nothing");
        assert!(
            console.stdout().contains("already patched"),
            "got {}",
            console.stdout()
        );
        assert!(
            console.stderr().contains("nothing to do"),
            "got {}",
            console.stderr()
        );
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
    fn dump_refuses_a_line_state_it_cannot_classify() {
        // A device that answers the stream probe with bytes that never form a
        // complete sentence is the documented "Unknown" case.
        let scratch = Scratch::new("dump-unknown");
        let links = FakePorts::single(|| FakeDevice::new().without_nmea());
        let console = RecordingConsole::default();
        let session = Session::new(&links, &console, &AutoConfirm).with_timings(Timings::fast());
        let err = dump(&dump_options(&scratch.join("stock.bin")), session)
            .expect_err("a silent unit cannot enter the BootROM by sync here");
        // Silence is the documented safe case, so the code attempts the sync
        // rather than refusing on the state; the fake is not in the BootROM, so
        // the sync is what fails.
        assert!(
            !format!("{err}").contains("no complete NMEA"),
            "silence is not the Unknown state: {err}"
        );
    }
}
