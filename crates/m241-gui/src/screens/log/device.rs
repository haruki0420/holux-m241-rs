//! Reading the log off a connected unit: the `log download` worker.
//!
//! The transfer is `m241_core::protocol::PmtkClient::download_log`, the same
//! PMTK182 path the CLI uses: logging is paused for the transfer and always
//! resumed by the core, so a failed download cannot leave the unit not recording.
//! The work runs on a thread and reports through a channel because a download is
//! about a hundred round trips and an immediate-mode UI must keep drawing.
//!
//! Nothing here writes to the device except the core's stop/resume pair.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};

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
    /// Which action started it.
    pub purpose: DownloadPurpose,
    /// Messages from the worker.
    pub receiver: Receiver<DownloadMsg>,
}

/// Start a download on its own thread.
pub fn start_download(port: String, sender: Sender<DownloadMsg>) {
    std::thread::spawn(move || {
        let result = download_from(&port, &sender);
        let _sent = sender.send(DownloadMsg::Done(result));
    });
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
/// # Errors
///
/// No port, several ports, or an enumeration failure. A refusal names the ports
/// so the user can type one rather than guess.
pub fn resolve_port_name(text: &str) -> Result<String, String> {
    let trimmed = text.trim();
    if !trimmed.is_empty() && !trimmed.eq_ignore_ascii_case("auto") {
        return Ok(trimmed.to_owned());
    }
    let ports = Port::list().map_err(|err| format!("ポートを列挙できません: {err}"))?;
    match ports.as_slice() {
        [only] => Ok(only.clone()),
        [] => Err(String::from(
            "ポートが見つかりません。端末を USB でつなぎ、「一覧」で更新してください",
        )),
        many => Err(format!(
            "ポートが複数あります（{}）。使うポートを入力してください",
            many.join("、")
        )),
    }
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
