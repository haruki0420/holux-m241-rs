//! Reading the log off a connected unit: the `log download` worker.
//!
//! The transfer is `m241_core::protocol::PmtkClient::download_log`, the same
//! PMTK182 path the CLI uses: logging is paused for the transfer and always
//! resumed by the core, so a failed download cannot leave the unit not recording.
//! The work runs on a thread and reports through a channel because a download is
//! about a hundred round trips and an immediate-mode UI must keep drawing.
//!
//! **The GUI download is unverified on hardware.** The CLI has run this path on a
//! unit (`docs/dev/05-verification.md`); this screen has not, and the note the
//! download panel shows says so. The core calls are the same, which makes the
//! risk small, not zero.
//!
//! Nothing here writes to the device except the core's stop/resume pair.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, TryRecvError};

use m241_core::Track;
use m241_core::log::{ParseReport, parse_dump};
use m241_core::protocol::Port;
use m241_core::protocol::pmtk::{NMEA_BAUD, PmtkClient};

/// What a download thread reports to the screen.
#[derive(Debug)]
pub enum DownloadMsg {
    /// Bytes received of the total the unit announced.
    Progress { received: usize, total: usize },
    /// The transfer finished, with the raw dump or the failure's sentence.
    Done(Result<Vec<u8>, String>),
}

/// What the bytes will be used for: a new log on the screen, or the copy that
/// must be saved before an erase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadPurpose {
    /// `log download`: save the dump and show it.
    Dump,
    /// `log erase` step 1: save the copy, then stop at the confirmation.
    Backup,
}

/// A running download.
#[derive(Debug)]
pub struct DownloadJob {
    purpose: DownloadPurpose,
    receiver: Receiver<DownloadMsg>,
}

impl DownloadJob {
    /// Which action started it.
    pub fn purpose(&self) -> DownloadPurpose {
        self.purpose
    }

    /// Take the next message, if any.
    ///
    /// The `Receiver` stays private so the state the screen exposes is "a job"
    /// rather than "a channel"; the console is the only reader.
    pub fn try_recv(&self) -> Result<DownloadMsg, TryRecvError> {
        self.receiver.try_recv()
    }
}

/// Start a download on its own thread and return the job the screen polls.
pub fn start_download(port: String, purpose: DownloadPurpose) -> DownloadJob {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = download_from(&port, &sender);
        let _sent = sender.send(DownloadMsg::Done(result));
    });
    DownloadJob { purpose, receiver }
}

/// Open `port` and transfer the whole log, reporting each chunk.
fn download_from(port: &str, sender: &Sender<DownloadMsg>) -> Result<Vec<u8>, String> {
    let name = resolve_port_name(port)?;
    let opened =
        Port::open(&name, NMEA_BAUD).map_err(|err| format!("{name} を開けません: {err}"))?;
    let mut client = PmtkClient::new(opened);
    client
        .download_log(&mut |received, total| {
            let _sent = sender.send(DownloadMsg::Progress { received, total });
        })
        .map_err(|err| format!("{name} からログを吸い出せません: {err}"))
}

/// Turn `auto` (or an empty field) into the one plausible port, or name what was
/// found instead.
///
/// The CLI resolves `--port auto` by USB ids and description
/// (`crates/m241-cli/src/link.rs::identify`): a port is `likely` when its
/// bridge is a CP210x / PL2303 / FTDI / CH340, or when the description names a
/// USB-serial adapter. `m241-core` exposes port **names** only, so this mirrors
/// the filter with the OS's naming conventions ([`looks_like_cable`]) rather
/// than claiming the USB-id classification. An explicitly typed name bypasses
/// the filter, exactly as it does in the CLI. The intended end state is one
/// classification used by both front ends, in core.
///
/// # Errors
///
/// No cable-shaped port, several of them, or an enumeration failure. A refusal
/// lists every port the OS did report, so the user can type one rather than
/// guess.
pub fn resolve_port_name(text: &str) -> Result<String, String> {
    let trimmed = text.trim();
    if !trimmed.is_empty() && !trimmed.eq_ignore_ascii_case("auto") {
        return Ok(trimmed.to_owned());
    }
    let ports = Port::list().map_err(|err| format!("ポートを列挙できません: {err}"))?;
    let candidates: Vec<&String> = ports.iter().filter(|name| looks_like_cable(name)).collect();
    match candidates.as_slice() {
        [only] => Ok((*only).clone()),
        [] => Err(format!(
            "M-241 のケーブルらしいポートが見つかりません（見つかったポート: {}）。\
             端末を USB でつなぎ、電源を入れて「一覧」で更新するか、使うポート名を入力してください",
            if ports.is_empty() {
                String::from("なし")
            } else {
                ports.join("、")
            }
        )),
        many => Err(format!(
            "ケーブルらしいポートが複数あります（{}）。使うポートを入力してください",
            many.iter()
                .map(|name| name.as_str())
                .collect::<Vec<_>>()
                .join("、")
        )),
    }
}

/// Whether an OS port name looks like a USB-serial adapter the logger can be on.
///
/// These are naming conventions, not identification: the genuine Holux cable is a
/// CP210x bridge and appears as `ttyUSB*` / `cu.SLAB_USBtoUART` / a `COM`
/// port, and a Bluetooth or pseudo-terminal port never does. A platform whose
/// convention is not listed here gets a refusal that lists the ports, which is
/// the CLI's behaviour when no port is `likely`.
fn looks_like_cable(name: &str) -> bool {
    let name = name.trim();
    // Linux: USB serial bridges are ttyUSBn / ttyACMn. Bluetooth is rfcommN and
    // a pseudo terminal is pts/N, neither of which matches.
    if name.starts_with("/dev/ttyUSB") || name.starts_with("/dev/ttyACM") {
        return true;
    }
    // macOS: a USB-serial bridge is a `cu.*` device whose name carries the
    // adapter (SLAB = Silicon Labs, wchusbserial = CH34x, usbserial / FTDI /
    // usbmodem). `cu.Bluetooth-Incoming-Port` and `cu.debug-console` do not.
    if let Some(stem) = name.strip_prefix("/dev/cu.") {
        let stem = stem.to_ascii_lowercase();
        return ["usbserial", "slab", "wchusbserial", "usbmodem", "ftdi"]
            .iter()
            .any(|marker| stem.contains(marker));
    }
    // Windows: a COM port is the only shape a serial adapter takes, and the name
    // carries no chip information, so every COM port is a candidate — the CLI
    // reaches the same result when the description is all the OS gives.
    let upper = name.to_ascii_uppercase();
    upper
        .strip_prefix("COM")
        .is_some_and(|digits| !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()))
}

/// A dump that was saved and parsed.
#[derive(Debug)]
pub struct SavedDump {
    /// Where the raw bytes went.
    pub path: PathBuf,
    /// How many bytes were saved.
    pub bytes: usize,
    /// The decoded track.
    pub track: Track,
    /// The parser's damage report.
    pub report: ParseReport,
}

/// What came back from the unit.
#[derive(Debug)]
pub enum DumpOutcome {
    /// A unit with nothing recorded: nothing was written.
    Empty,
    /// A saved dump, parsed.
    Saved(SavedDump),
}

/// Save the raw bytes and parse them, refusing a silent overwrite.
///
/// The file is written before parsing: a dump whose parser refuses is still the
/// only copy of the transfer, and the error names where it went.
///
/// # Errors
///
/// An empty destination, an existing file without `force`, an I/O failure, or a
/// saved dump `parse_dump` cannot read.
pub fn save_and_parse(bytes: &[u8], destination: &str, force: bool) -> Result<DumpOutcome, String> {
    if bytes.is_empty() {
        return Ok(DumpOutcome::Empty);
    }
    let path = write_dump(destination, bytes, force)?;
    let (track, report) = parse_dump(bytes).map_err(|err| {
        format!(
            "{} に保存しましたが、ログとして解析できません: {err}",
            path.display()
        )
    })?;
    Ok(DumpOutcome::Saved(SavedDump {
        path,
        bytes: bytes.len(),
        track,
        report,
    }))
}

/// Write a raw dump, creating its parent directory.
///
/// # Errors
///
/// The same refusals as [`save_and_parse`], without the parse.
pub fn write_dump(destination: &str, bytes: &[u8], force: bool) -> Result<PathBuf, String> {
    let trimmed = destination.trim();
    if trimmed.is_empty() {
        return Err(String::from("保存先を入力してください"));
    }
    let path = Path::new(trimmed);
    if path.exists() && !force {
        return Err(format!(
            "{trimmed} はすでに存在します。上書きする場合は「上書き」を入れてください"
        ));
    }
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("{} を作れません: {err}", parent.display()))?;
    }
    std::fs::write(path, bytes).map_err(|err| format!("{trimmed} に書けません: {err}"))?;
    Ok(path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{ScratchDir, synthetic_dump};
    use super::*;

    #[test]
    fn an_explicit_port_name_is_kept_as_typed() {
        assert_eq!(resolve_port_name("COM3"), Ok(String::from("COM3")));
        assert_eq!(
            resolve_port_name(" /dev/ttyUSB0 "),
            Ok(String::from("/dev/ttyUSB0"))
        );
    }

    #[test]
    fn auto_only_considers_a_cable_shaped_port() {
        // `--port auto` is "the only plausible cable" in the CLI too; core gives
        // names only, so this pins the naming conventions that stand in for the
        // USB-id classification.
        for name in [
            "/dev/ttyUSB0",
            "/dev/ttyACM1",
            "/dev/cu.SLAB_USBtoUART",
            "/dev/cu.usbserial-1420",
            "/dev/cu.wchusbserial1420",
            "/dev/cu.usbmodem14201",
            "COM3",
            "com12",
        ] {
            assert!(looks_like_cable(name), "{name} should be a cable candidate");
        }
        for name in [
            "/dev/ttyS0",
            "/dev/rfcomm0",
            "/dev/pts/3",
            "/dev/cu.Bluetooth-Incoming-Port",
            "/dev/cu.debug-console",
            "LPT1",
            "COM",
            "COMx",
        ] {
            assert!(!looks_like_cable(name), "{name} is not a cable candidate");
        }
    }

    #[test]
    fn an_empty_log_writes_nothing() {
        let scratch = ScratchDir::new("empty-dump");
        let path = scratch.join("empty.bin");
        let outcome = save_and_parse(&[], path.to_str().unwrap_or_default(), false);
        assert!(matches!(outcome, Ok(DumpOutcome::Empty)), "{outcome:?}");
        assert!(!path.exists(), "an empty log must not write a file");
    }

    #[test]
    fn a_saved_dump_is_parsed_into_its_points() {
        let scratch = ScratchDir::new("dump");
        let bytes = synthetic_dump(&[
            (1_700_000_000, 35.68, 139.76, 10.0),
            (1_700_000_060, 35.68, 139.76, 12.0),
        ]);
        let path = scratch.join("track.bin");
        let outcome = save_and_parse(&bytes, path.to_str().unwrap_or_default(), false)
            .expect("save and parse");
        let DumpOutcome::Saved(saved) = outcome else {
            panic!("expected a saved dump");
        };
        assert_eq!(saved.track.points.len(), 2);
        assert_eq!(saved.bytes, bytes.len());
        assert_eq!(saved.path, path);
        assert_eq!(saved.report.records, 2);
    }

    #[test]
    fn an_existing_file_is_refused_without_force_and_kept_intact() {
        let scratch = ScratchDir::new("force");
        let path = scratch.join("track.bin");
        std::fs::write(&path, b"keep").expect("write the existing file");
        let error = write_dump(path.to_str().unwrap_or_default(), b"new", false)
            .expect_err("an existing file is refused");
        assert!(error.contains("上書き"), "got {error}");
        assert_eq!(std::fs::read(&path).expect("read it back"), b"keep");
        write_dump(path.to_str().unwrap_or_default(), b"new", true).expect("force overwrites");
        assert_eq!(std::fs::read(&path).expect("read it back"), b"new");
    }

    #[test]
    fn a_missing_parent_directory_is_created() {
        let scratch = ScratchDir::new("parents");
        let path = scratch.join("nested/deeper/track.bin");
        write_dump(path.to_str().unwrap_or_default(), b"dump", false).expect("write");
        assert!(path.exists());
    }
}
