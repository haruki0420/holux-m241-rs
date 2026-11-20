//! `m241 log info`.
//!
//! What a dump contains, without converting it. The point is to answer "did the
//! download work" and "how much of this log is damaged" before spending time on
//! an export, so the summary counts what the parser had to skip as well as what
//! it kept.

use m241_core::log::{ParseReport, export, parse_dump};
use m241_core::protocol::pmtk::{NMEA_BAUD, PmtkClient};
use m241_core::{LogSummary, Result};

use crate::Session;
use crate::cli::{DownloadOptions, LogInfoOptions};
use crate::fileio;
use crate::link;
use crate::output::Console;

/// Read the track log off the device and save the raw dump.
///
/// Logging is paused for the transfer and resumed by
/// [`PmtkClient::download_log`] however the transfer ends: a unit left with
/// recording off loses the next trip, which is worse than the failed download.
///
/// # Errors
///
/// * [`Error::Safety`] when the output exists and `--force` was not given;
/// * [`Error::Port`] when no plausible port can be opened;
/// * [`Error::Timeout`] / [`Error::Nmea`] when the transfer fails;
/// * [`Error::Log`] when the bytes that came back are not a dump.
pub fn download(options: &DownloadOptions, session: Session<'_>) -> Result<()> {
    fileio::ensure_free(&options.output, options.force)?;
    let port = link::resolve(session.links, &options.port)?;
    let transport = session.links.open(&port, NMEA_BAUD)?;
    let mut client = PmtkClient::new(transport).with_reply_timeout(session.timings.reply);

    session
        .console
        .note(&format!("reading the track log from {port}"));
    let mut last_reported = 0usize;
    let bytes = client.download_log(&mut |done, total| {
        // One line per 5% rather than per chunk: a 200 KB log is about 100 round
        // trips, and a line apiece would be a wall of text.
        let step = (total / 20).max(1);
        if done == total || done.saturating_sub(last_reported) >= step {
            last_reported = done;
            session.console.progress(done, total);
        }
    })?;
    session.console.progress_done();

    // Written only after the transfer returned: a failed download must not leave
    // a partial dump where a complete one is expected.
    fileio::write(&options.output, &bytes)?;

    let (track, report) = parse_dump(&bytes)?;
    let summary = report.summary(&track);
    warn_about_damage(session.console, &summary, &report);

    if options.json {
        return session.console.json(&serde_json::json!({
            "port": port,
            "output": options.output.display().to_string(),
            "bytes": bytes.len(),
            "summary": summary_document(summary),
        }));
    }
    session.console.result(&format!("{:<12}: {port}", "port"));
    session
        .console
        .result(&format!("{:<12}: {}", "output", options.output.display()));
    session
        .console
        .result(&format!("{:<12}: {}", "bytes", bytes.len()));
    session
        .console
        .result(&format!("{:<12}: {}", "records", summary.records));
    session
        .console
        .result(&format!("{:<12}: {}", "fixes", summary.fixes));
    session.console.note(&format!(
        "next: m241 log read {} --format gpx -o track.gpx",
        options.output.display()
    ));
    Ok(())
}

/// Summarise a dumped track log.
///
/// # Errors
///
/// [`m241_core::Error::Io`] when the file cannot be read,
/// [`m241_core::Error::Log`] when it is not a dump at all.
pub fn info(options: &LogInfoOptions, session: Session<'_>) -> Result<()> {
    let bytes = fileio::read(&options.file)?;
    let (track, report) = parse_dump(&bytes)?;
    let summary = report.summary(&track);
    warn_about_damage(session.console, &summary, &report);

    if options.json {
        return session.console.json(&serde_json::json!({
            "file": options.file.display().to_string(),
            "size": bytes.len(),
            "summary": summary_document(summary),
            "parse": parse_document(&report),
        }));
    }
    session
        .console
        .result(&format!("{:<18}: {}", "file", options.file.display()));
    session
        .console
        .result(&format!("{:<18}: {}", "size", bytes.len()));
    session
        .console
        .result(&format!("{:<18}: {}", "records", summary.records));
    session
        .console
        .result(&format!("{:<18}: {}", "fixes", summary.fixes));
    session
        .console
        .result(&format!("{:<18}: {}", "blocks", report.blocks));
    session.console.result(&format!(
        "{:<18}: {}",
        "damaged blocks", summary.damaged_block_count
    ));
    session.console.result(&format!(
        "{:<18}: {}",
        "bad checksums", report.bad_checksums
    ));
    session.console.result(&format!(
        "{:<18}: {}",
        "truncated records", report.truncated_records
    ));
    session.console.result(&format!(
        "{:<18}: {}",
        "start",
        timestamp(summary.started_at)
    ));
    session
        .console
        .result(&format!("{:<18}: {}", "end", timestamp(summary.ended_at)));
    Ok(())
}

/// One summary as JSON, with the timestamps rendered as ISO-8601 as well.
fn summary_document(summary: LogSummary) -> serde_json::Value {
    serde_json::json!({
        "records": summary.records,
        "fixes": summary.fixes,
        "damaged_block_count": summary.damaged_block_count,
        "started_at": summary.started_at,
        "started_at_iso": summary.started_at.map(export::utc_iso),
        "ended_at": summary.ended_at,
        "ended_at_iso": summary.ended_at.map(export::utc_iso),
    })
}

/// What the parser had to skip, as JSON.
fn parse_document(report: &ParseReport) -> serde_json::Value {
    serde_json::json!({
        "blocks": report.blocks,
        "empty_blocks": report.empty_blocks,
        "damaged_blocks": report.damaged_blocks,
        "records": report.records,
        "bad_checksums": report.bad_checksums,
        "truncated_records": report.truncated_records,
        "used_bytes": report.used_bytes,
    })
}

/// `1970-01-01T00:00:00Z`, or `-` when the log has no timed record.
fn timestamp(utc: Option<u32>) -> String {
    utc.map_or_else(|| String::from("-"), export::utc_iso)
}

/// Say on stderr what the parser had to skip.
///
/// A warning, never an error: one damaged block is a track worth keeping, and
/// refusing the whole dump would throw the trip away.
fn warn_about_damage(console: &dyn Console, summary: &LogSummary, report: &ParseReport) {
    if summary.damaged_block_count > 0 {
        console.warn(&format!(
            "{} of {} blocks failed their header checksum; their records are still exported",
            summary.damaged_block_count, report.blocks
        ));
    }
    if report.truncated_records > 0 {
        console.warn(&format!(
            "{} records were cut short (the log was probably still being written); they were \
             skipped",
            report.truncated_records
        ));
    }
    if report.bad_checksums > 0 {
        console.warn(&format!(
            "{} records failed their per-record checksum; they are exported and marked",
            report.bad_checksums
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Timings;
    use crate::link::PortEntry;
    use crate::testing::{AutoConfirm, DeviceHandle, FakePorts, RecordingConsole, synthetic_dump};
    use m241_core::Error;
    use m241_core::protocol::simulator::FakeDevice;
    use std::path::PathBuf;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("m241-cli-log-{}-{name}", std::process::id()));
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

    fn write_dump(scratch: &Scratch, name: &str, points: usize) -> PathBuf {
        let path = scratch.join(name);
        fileio::write(&path, &synthetic_dump(points)).expect("write dump");
        path
    }

    /// A session whose `FakeDevice` replies land without waiting out a timeout.
    fn fast<'a>(
        links: &'a FakePorts,
        console: &'a RecordingConsole,
        confirm: &'a AutoConfirm,
    ) -> Session<'a> {
        Session::new(links, console, confirm).with_timings(Timings::fast())
    }

    #[test]
    fn info_summarises_a_dump_from_a_file() {
        let scratch = Scratch::new("info");
        let path = write_dump(&scratch, "track.bin", 3);
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        info(
            &LogInfoOptions {
                file: path,
                json: false,
            },
            Session::new(&links, &console, &confirm),
        )
        .expect("info");
        let text = console.stdout();
        assert!(text.contains("records           : 3"), "got {text}");
        assert!(text.contains("fixes             : 3"), "got {text}");
        assert!(text.contains("2023-11-14T22:13:20Z"), "got {text}");
    }

    #[test]
    fn info_json_is_one_object_carrying_the_summary() {
        let scratch = Scratch::new("info-json");
        let path = write_dump(&scratch, "track.bin", 2);
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        info(
            &LogInfoOptions {
                file: path,
                json: true,
            },
            Session::new(&links, &console, &confirm),
        )
        .expect("info");
        let document: serde_json::Value =
            serde_json::from_str(&console.stdout()).expect("one JSON object");
        assert_eq!(document["summary"]["records"], 2);
        assert_eq!(document["summary"]["fixes"], 2);
        assert_eq!(document["parse"]["blocks"], 1);
    }

    #[test]
    fn info_refuses_a_file_that_is_not_a_log() {
        let scratch = Scratch::new("info-garbage");
        let path = scratch.join("garbage.bin");
        fileio::write(&path, &[0xAB; 4096]).expect("write");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        let err = info(
            &LogInfoOptions {
                file: path,
                json: false,
            },
            Session::new(&links, &console, &confirm),
        )
        .expect_err("not a dump");
        assert_eq!(crate::exit::exit_code_for(&err), 1, "got {err}");
        assert!(
            format!("{err}").contains("not an M-241 log dump"),
            "got {err}"
        );
    }
    #[test]
    fn download_saves_the_dump_and_resumes_logging() {
        let scratch = Scratch::new("download");
        let output = scratch.join("track.bin");
        let handle = DeviceHandle::new(|| FakeDevice::new().with_log(&synthetic_dump(3)));
        let links = FakePorts::shared(handle.clone());
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        download(
            &DownloadOptions {
                port: String::from("auto"),
                output: output.clone(),
                force: false,
                json: false,
            },
            fast(&links, &console, &confirm),
        )
        .expect("download");
        assert_eq!(fileio::read(&output).expect("dump"), synthetic_dump(3));
        assert!(
            handle.logging_enabled(),
            "logging must be on again after the transfer"
        );
        assert!(
            console.stdout().contains("records"),
            "got {}",
            console.stdout()
        );
    }

    #[test]
    fn a_failed_download_still_resumes_logging() {
        // The documented failure: a dropped reply mid-transfer must not leave the
        // unit with recording off.
        let scratch = Scratch::new("download-fail");
        let output = scratch.join("track.bin");
        let handle = DeviceHandle::new(|| {
            FakeDevice::new()
                .with_log(&synthetic_dump(3))
                .dropping_log_replies(1)
        });
        let links = FakePorts::shared(handle.clone());
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        let err = download(
            &DownloadOptions {
                port: String::from("auto"),
                output: output.clone(),
                force: false,
                json: false,
            },
            fast(&links, &console, &confirm),
        )
        .expect_err("the transfer fails");
        assert!(matches!(err, Error::Timeout { .. }), "got {err}");
        assert!(
            handle.logging_enabled(),
            "logging must be resumed even when the transfer fails"
        );
        assert!(
            !output.exists(),
            "a failed transfer must not leave a partial dump"
        );
    }

    #[test]
    fn download_refuses_to_overwrite_an_existing_dump() {
        let scratch = Scratch::new("download-clobber");
        let output = scratch.join("track.bin");
        fileio::write(&output, b"previous").expect("write");
        let handle = DeviceHandle::new(|| FakeDevice::new().with_log(&synthetic_dump(1)));
        let links = FakePorts::shared(handle);
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        let err = download(
            &DownloadOptions {
                port: String::from("auto"),
                output: output.clone(),
                force: false,
                json: false,
            },
            fast(&links, &console, &confirm),
        )
        .expect_err("must refuse");
        assert!(matches!(err, Error::Safety { .. }), "got {err}");
        assert_eq!(fileio::read(&output).expect("still there"), b"previous");
    }

    #[test]
    fn download_says_so_when_several_ports_could_be_the_logger() {
        let scratch = Scratch::new("download-ambiguous");
        let links = FakePorts::with_ports(vec![
            PortEntry {
                name: String::from("/dev/ttyUSB0"),
                cable: Some(crate::link::Cable::Cp210x),
                likely: true,
                description: None,
            },
            PortEntry {
                name: String::from("/dev/ttyUSB1"),
                cable: Some(crate::link::Cable::Cp210x),
                likely: true,
                description: None,
            },
        ]);
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        let err = download(
            &DownloadOptions {
                port: String::from("auto"),
                output: scratch.join("track.bin"),
                force: false,
                json: false,
            },
            fast(&links, &console, &confirm),
        )
        .expect_err("ambiguous");
        assert_eq!(crate::exit::exit_code_for(&err), 3, "got {err}");
        assert!(format!("{err}").contains("--port"), "got {err}");
    }
}
