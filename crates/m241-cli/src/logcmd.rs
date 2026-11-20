//! `m241 log info`.
//!
//! What a dump contains, without converting it. The point is to answer "did the
//! download work" and "how much of this log is damaged" before spending time on
//! an export, so the summary counts what the parser had to skip as well as what
//! it kept.

use m241_core::log::{ParseReport, export, parse_dump};
use m241_core::{LogSummary, Result};

use crate::Session;
use crate::cli::LogInfoOptions;
use crate::fileio;
use crate::output::Console;

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
    use crate::testing::{AutoConfirm, FakePorts, RecordingConsole, synthetic_dump};
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
}
