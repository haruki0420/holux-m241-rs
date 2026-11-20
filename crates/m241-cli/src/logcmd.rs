//! `m241 log download`, `m241 log info`, `m241 log read`, and `m241 log geotag`.
//!
//! Downloading talks PMTK182 to the logger and saves the raw flash blocks;
//! `info` summarises a dump without converting it so the user can check damage
//! before exporting; `read` converts a dump to GPX, KML, CSV, NMEA or JSON,
//! optionally shifting WNRO-stale timestamps; `geotag` matches photos by capture
//! time and writes GPS EXIF tags.

use std::path::PathBuf;

use m241_core::log::geotag::{self, CaptureTime, GeotagOptions, GeotagPlan};
use m241_core::log::{ParseReport, export, parse_dump};
use m241_core::{Error, LogSummary, Result, Track};

use crate::Session;
use crate::cal::OffsetSpec;
use crate::cli::{DownloadOptions, LogGeotagOptions, LogInfoOptions, LogReadOptions};
use crate::fileio;
use crate::link;
use crate::output::Console;

/// Read the track log off the device and save the raw dump.
///
/// Logging is paused for the transfer and resumed by
/// [`m241_core::protocol::PmtkClient::download_log`] however the transfer ends:
/// a unit left with recording off loses the next trip, which is worse than the
/// failed download.
///
/// # Errors
///
/// * [`m241_core::Error::Safety`] when the output exists and `--force` was not given;
/// * [`m241_core::Error::Port`] when no plausible port can be opened;
/// * [`m241_core::Error::Timeout`] / [`m241_core::Error::Nmea`] /
///   [`m241_core::Error::Rejected`] when the transfer fails;
/// * [`m241_core::Error::Log`] when the bytes that came back are not a dump.
pub fn download(options: &DownloadOptions, session: Session<'_>) -> Result<()> {
    fileio::ensure_free(&options.output, options.force)?;
    let port = link::resolve(session.links, &options.port)?;
    let mut client = link::open_pmtk(session.links, &port, session.timings)?;

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
    // a partial dump where a complete one is expected. If parsing then fails,
    // keep the raw capture on disk and tell the user where it was written.
    fileio::write(&options.output, &bytes)?;

    let (track, report) = match parse_dump(&bytes) {
        Ok(parsed) => parsed,
        Err(err) => {
            session.console.warn(&format!(
                "saved {} raw bytes to {}, but parsing the log failed",
                bytes.len(),
                options.output.display()
            ));
            return Err(err);
        }
    };
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

/// Convert a dumped log to GPX, KML, CSV, NMEA or JSON.
///
/// `--utc-offset-days auto` shifts only a log whose newest record is stale — one
/// written by an unpatched unit, whose every timestamp is 7168 days in the past.
/// An explicit number is applied as given, including negative values.
///
/// # Errors
///
/// [`m241_core::Error::Io`] when the file cannot be read or the output written,
/// [`m241_core::Error::Log`] when it is not a dump at all.
pub fn read(options: &LogReadOptions, session: Session<'_>) -> Result<()> {
    let bytes = fileio::read(&options.file)?;
    let (mut track, report) = parse_dump(&bytes)?;
    let summary = report.summary(&track);
    warn_about_damage(session.console, &summary, &report);

    let _shift = apply_utc_offset(&mut track, options.utc_offset_days, session.console);

    match &options.output {
        Some(path) => {
            fileio::write_text(path, &export(&track, options.format))?;
            session.console.note(&format!(
                "wrote {} {} to {}",
                track.points.len(),
                if track.points.len() == 1 {
                    "point"
                } else {
                    "points"
                },
                path.display()
            ));
        }
        None => session.console.text(&export(&track, options.format)),
    }
    Ok(())
}

/// Which day, in days since the Unix epoch, the log's newest record falls on.
///
/// Uses the maximum timestamp across all records rather than the final record
/// in file order, because a wrapped ring buffer places older blocks after newer
/// ones.
fn track_days(track: &Track) -> Option<i64> {
    track
        .points
        .iter()
        .filter_map(|point| point.utc)
        .max()
        .map(|utc| i64::from(utc / 86_400))
}

/// Add `days` to every timestamp the track carries.
fn shift_track(track: &mut Track, days: i32) {
    for point in &mut track.points {
        point.utc = point.utc.map(|utc| crate::cal::shift_seconds(utc, days));
    }
}

/// Apply `--utc-offset-days` (`auto` or explicit) to `track` and return the applied day shift.
fn apply_utc_offset(track: &mut Track, spec: OffsetSpec, console: &dyn Console) -> i32 {
    let shift = track_days(track).map_or(0, |days| spec.shift_for(days));
    if shift != 0 {
        shift_track(track, shift);
        let reason = if matches!(spec, OffsetSpec::Auto) {
            "the log was written by a unit whose clock is still on the old GPS epoch"
        } else {
            "--utc-offset-days"
        };
        console.note(&format!(
            "shifted every timestamp by {shift:+} days: {reason}"
        ));
    }
    shift
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

/// Match photos to track points by capture time and write GPS EXIF tags.
///
/// # Errors
///
/// * [`Error::Io`] when `<LOG>`, a directory, or a photo cannot be read or replaced;
/// * [`Error::Log`] when `<LOG>` is not a valid M-241 dump.
pub fn geotag(options: &LogGeotagOptions, session: Session<'_>) -> Result<()> {
    let bytes = fileio::read(&options.log)?;
    let (mut track, report) = parse_dump(&bytes)?;
    let summary = report.summary(&track);
    warn_about_damage(session.console, &summary, &report);

    let offset_days = apply_utc_offset(&mut track, options.utc_offset_days, session.console);
    let photo_paths = expand_photo_inputs(&options.photos)?;

    let tz_delta_seconds = i64::from(options.tz_offset_minutes) * 60;
    let mut matching_track = track;
    if tz_delta_seconds != 0 {
        shift_track_seconds(&mut matching_track, tz_delta_seconds);
    }

    let geotag_options = GeotagOptions {
        max_gap_seconds: options.max_gap,
    };
    let mut plans = geotag::plan(&photo_paths, &matching_track, &geotag_options)?;
    if tz_delta_seconds != 0 {
        for plan in &mut plans {
            if let Some(point) = &mut plan.point {
                point.utc = point
                    .utc
                    .map(|utc| shift_timestamp_seconds(utc, -tz_delta_seconds));
            }
        }
    }

    let written_paths = if options.dry_run {
        Vec::new()
    } else {
        geotag::apply(&plans)?
    };
    let matched_count = plans.iter().filter(|plan| plan.will_write()).count();
    let skipped_count = plans.len().saturating_sub(matched_count);
    let written_count = if options.dry_run {
        0
    } else {
        written_paths.len()
    };

    if options.json {
        return session.console.json(&geotag_document(
            options,
            offset_days,
            &plans,
            matched_count,
            written_count,
            skipped_count,
        ));
    }

    for plan in &plans {
        if let Some(point) = &plan.point {
            let action = if options.dry_run { "plan" } else { "tag" };
            session.console.result(&format!(
                "{:<8}: {} -> {:.6}, {:.6} ({}, gap {} s)",
                action,
                plan.photo.display(),
                point.latitude.unwrap_or(0.0),
                point.longitude.unwrap_or(0.0),
                timestamp(point.utc),
                plan.gap_seconds.unwrap_or(0),
            ));
        } else {
            let reason = plan.skipped.as_deref().unwrap_or("skipped");
            session.console.result(&format!(
                "{:<8}: {} ({reason})",
                "skip",
                plan.photo.display()
            ));
        }
    }
    session
        .console
        .result(&format!("{:<8}: {offset_days}", "offset"));
    session
        .console
        .result(&format!("{:<8}: {matched_count}", "matched"));
    session
        .console
        .result(&format!("{:<8}: {written_count}", "written"));
    session
        .console
        .result(&format!("{:<8}: {skipped_count}", "skipped"));
    if options.dry_run {
        session
            .console
            .note("dry run: no photos were modified. Drop --dry-run to write GPS EXIF tags.");
    }
    Ok(())
}

/// Expand photo file and directory arguments into a flat list of regular file paths.
///
/// Directories have their immediate regular file entries collected and sorted
/// deterministically by path; explicit file paths stay in the order given.
fn expand_photo_inputs(inputs: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut photos = Vec::new();
    for input in inputs {
        if input.is_dir() {
            let entries = std::fs::read_dir(input)
                .map_err(|source| Error::io(input.display().to_string(), source))?;
            let mut children = Vec::new();
            for entry in entries {
                let entry =
                    entry.map_err(|source| Error::io(input.display().to_string(), source))?;
                let file_type = entry
                    .file_type()
                    .map_err(|source| Error::io(entry.path().display().to_string(), source))?;
                if file_type.is_file() {
                    children.push(entry.path());
                }
            }
            children.sort();
            photos.extend(children);
        } else {
            photos.push(input.clone());
        }
    }
    Ok(photos)
}

/// Shift every timestamp in `track` by `delta_seconds`, clamping to `u32`.
fn shift_track_seconds(track: &mut Track, delta_seconds: i64) {
    for point in &mut track.points {
        point.utc = point
            .utc
            .map(|utc| shift_timestamp_seconds(utc, delta_seconds));
    }
}

/// Shift one Unix timestamp by `delta_seconds`, clamped to `0..=u32::MAX`.
fn shift_timestamp_seconds(utc: u32, delta_seconds: i64) -> u32 {
    let shifted = i64::from(utc) + delta_seconds;
    let clamped = shifted.clamp(0, i64::from(u32::MAX));
    u32::try_from(clamped).unwrap_or(utc)
}

/// Render `m241 log geotag --json` as a single JSON object.
fn geotag_document(
    options: &LogGeotagOptions,
    offset_days: i32,
    plans: &[GeotagPlan],
    matched_count: usize,
    written_count: usize,
    skipped_count: usize,
) -> serde_json::Value {
    let entries: Vec<serde_json::Value> = plans
        .iter()
        .map(|plan| {
            let status = if plan.will_write() {
                if options.dry_run {
                    "planned"
                } else {
                    "written"
                }
            } else {
                "skipped"
            };
            let capture_utc = plan.capture_time.map(CaptureTime::utc);
            let capture_source = plan.capture_time.map(|capture| match capture {
                CaptureTime::Exif(_) => "exif",
                CaptureTime::FileModified(_) => "mtime",
            });
            let matched_utc = plan.point.and_then(|point| point.utc);
            serde_json::json!({
                "photo": plan.photo.display().to_string(),
                "status": status,
                "will_write": plan.will_write(),
                "skip_reason": plan.skipped,
                "capture_utc": capture_utc,
                "capture_iso": capture_utc.map(export::utc_iso),
                "capture_source": capture_source,
                "matched_utc": matched_utc,
                "matched_iso": matched_utc.map(export::utc_iso),
                "gap_seconds": plan.gap_seconds,
                "latitude": plan.point.and_then(|point| point.latitude),
                "longitude": plan.point.and_then(|point| point.longitude),
                "height_m": plan.point.and_then(|point| point.height_m),
            })
        })
        .collect();
    serde_json::json!({
        "log": options.log.display().to_string(),
        "dry_run": options.dry_run,
        "offset_days": offset_days,
        "tz_offset_minutes": options.tz_offset_minutes,
        "max_gap_seconds": options.max_gap,
        "matched": matched_count,
        "written": written_count,
        "skipped": skipped_count,
        "plans": entries,
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

    #[test]
    fn read_exports_gpx_from_a_dump() {
        let scratch = Scratch::new("read-gpx");
        let path = write_dump(&scratch, "track.bin", 2);
        let output = scratch.join("track.gpx");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        read(
            &LogReadOptions {
                file: path,
                format: m241_core::ExportFormat::Gpx,
                output: Some(output.clone()),
                utc_offset_days: crate::cal::OffsetSpec::Days(0),
            },
            fast(&links, &console, &confirm),
        )
        .expect("read");
        let text = fileio::read_text(&output).expect("gpx");
        assert!(text.starts_with("<?xml version=\"1.0\""), "got {text}");
        assert!(text.contains("35.681"), "got {text}");
    }

    #[test]
    fn read_writes_to_stdout_when_no_output_is_given() {
        let scratch = Scratch::new("read-stdout");
        let path = write_dump(&scratch, "track.bin", 1);
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        read(
            &LogReadOptions {
                file: path,
                format: m241_core::ExportFormat::Csv,
                output: None,
                utc_offset_days: crate::cal::OffsetSpec::Days(0),
            },
            fast(&links, &console, &confirm),
        )
        .expect("read");
        assert!(
            console.stdout().starts_with("index,utc,lat,lon"),
            "got {}",
            console.stdout()
        );
    }

    #[test]
    fn an_explicit_offset_shifts_every_timestamp() {
        let scratch = Scratch::new("read-shift");
        let path = write_dump(&scratch, "track.bin", 1);
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        read(
            &LogReadOptions {
                file: path,
                format: m241_core::ExportFormat::Json,
                output: None,
                utc_offset_days: crate::cal::OffsetSpec::Days(1),
            },
            fast(&links, &console, &confirm),
        )
        .expect("read");
        let document: serde_json::Value =
            serde_json::from_str(&console.stdout()).expect("one JSON object");
        assert_eq!(
            document["points"][0]["utc"],
            1_700_000_000u32 + 86_400,
            "one day was added"
        );
        assert!(
            console.stderr().contains("+1 days"),
            "got {}",
            console.stderr()
        );
    }

    #[test]
    fn auto_shift_leaves_a_modern_log_alone() {
        let scratch = Scratch::new("read-auto");
        let path = write_dump(&scratch, "track.bin", 1);
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        read(
            &LogReadOptions {
                file: path,
                format: m241_core::ExportFormat::Json,
                output: None,
                utc_offset_days: crate::cal::OffsetSpec::Auto,
            },
            fast(&links, &console, &confirm),
        )
        .expect("read");
        let document: serde_json::Value =
            serde_json::from_str(&console.stdout()).expect("one JSON object");
        assert_eq!(document["points"][0]["utc"], 1_700_000_000u32);
        assert!(
            !console.stderr().contains("shifted"),
            "got {}",
            console.stderr()
        );
    }

    #[test]
    fn an_empty_file_is_an_empty_dump_not_an_error() {
        let scratch = Scratch::new("read-empty");
        let path = scratch.join("empty.bin");
        fileio::write(&path, b"").expect("write");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        read(
            &LogReadOptions {
                file: path,
                format: m241_core::ExportFormat::Csv,
                output: None,
                utc_offset_days: crate::cal::OffsetSpec::Auto,
            },
            fast(&links, &console, &confirm),
        )
        .expect("empty dump");
        assert!(
            console.stdout().starts_with("index,utc,lat,lon"),
            "the header is still written: {}",
            console.stdout()
        );
    }

    #[test]
    fn damage_is_warned_about_on_stderr_only() {
        let scratch = Scratch::new("read-damage");
        // Corrupt one record's checksum byte inside the block.
        let mut dump = synthetic_dump(2);
        if let Some(byte) = dump.get_mut(0x200 + 15) {
            *byte ^= 0xFF;
        }
        let path = scratch.join("damaged.bin");
        fileio::write(&path, &dump).expect("write dump");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        read(
            &LogReadOptions {
                file: path,
                format: m241_core::ExportFormat::Csv,
                output: None,
                utc_offset_days: crate::cal::OffsetSpec::Days(0),
            },
            fast(&links, &console, &confirm),
        )
        .expect("read");
        assert!(
            console.stderr().contains("checksum"),
            "the warning is on stderr: {}",
            console.stderr()
        );
        assert!(
            !console.stdout().contains("warning"),
            "stdout must stay machine-readable: {}",
            console.stdout()
        );
    }

    /// Build a one-block dump whose records carry the given UTC timestamps.
    fn dump_with_timestamps(timestamps: &[u32]) -> Vec<u8> {
        use m241_core::log::block::{
            BLOCK_HEADER_SIZE, BLOCK_SIZE, POLICY_LOG_ENABLED, xor_checksum,
        };

        const MASK: u32 = 0x8000_001D;
        const LAT: f32 = 35.681_236;
        const LON: f32 = 139.767_13;

        let mut block = vec![0xFFu8; BLOCK_SIZE];
        block[0..2].copy_from_slice(&(timestamps.len() as u16).to_le_bytes());
        block[2..6].copy_from_slice(&MASK.to_le_bytes());
        block[6..8].copy_from_slice(&POLICY_LOG_ENABLED.to_le_bytes());
        block[8..12].copy_from_slice(&10u32.to_le_bytes());

        let mut cursor = BLOCK_HEADER_SIZE;
        for &utc in timestamps {
            let mut record = Vec::new();
            record.extend_from_slice(&utc.to_le_bytes());
            record.extend_from_slice(&LAT.to_le_bytes());
            record.extend_from_slice(&LON.to_le_bytes());
            record.extend_from_slice(&[0x00, 0xA0, 0x42]);
            let checksum = xor_checksum(&record);
            record.push(checksum);
            block[cursor..cursor + record.len()].copy_from_slice(&record);
            cursor += record.len();
        }
        block
    }

    #[test]
    fn auto_shift_advances_a_stale_log_by_one_rollover_cycle() {
        let scratch = Scratch::new("read-auto-stale");
        let stale_utc = (crate::cal::days_from_civil(2006, 9, 17) * 86_400) as u32;
        let path = scratch.join("stale.bin");
        fileio::write(&path, &dump_with_timestamps(&[stale_utc, stale_utc + 60])).expect("write");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        read(
            &LogReadOptions {
                file: path,
                format: m241_core::ExportFormat::Json,
                output: None,
                utc_offset_days: OffsetSpec::Auto,
            },
            fast(&links, &console, &confirm),
        )
        .expect("read");
        let document: serde_json::Value =
            serde_json::from_str(&console.stdout()).expect("one JSON object");
        let expected = crate::cal::shift_seconds(stale_utc, crate::cal::WNRO_DAYS);
        assert_eq!(document["points"][0]["utc"], expected);
        assert!(
            console.stderr().contains("old GPS epoch"),
            "auto-detected shift should explain itself: {}",
            console.stderr()
        );
    }

    #[test]
    fn auto_shift_checks_the_newest_record_not_the_last_one() {
        // Wrapped ring buffer: a modern record appears before an older stale record.
        // Auto-detection must inspect the newest record (`max`), not the last one.
        let scratch = Scratch::new("read-wrapped");
        let modern_utc = 1_700_000_000u32;
        let stale_utc = (crate::cal::days_from_civil(2006, 9, 17) * 86_400) as u32;
        let path = scratch.join("wrapped.bin");
        fileio::write(&path, &dump_with_timestamps(&[modern_utc, stale_utc])).expect("write");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        read(
            &LogReadOptions {
                file: path,
                format: m241_core::ExportFormat::Json,
                output: None,
                utc_offset_days: OffsetSpec::Auto,
            },
            fast(&links, &console, &confirm),
        )
        .expect("read");
        let document: serde_json::Value =
            serde_json::from_str(&console.stdout()).expect("one JSON object");
        assert_eq!(document["points"][0]["utc"], modern_utc);
        assert_eq!(document["points"][1]["utc"], stale_utc);
        assert!(
            !console.stderr().contains("shifted"),
            "newest record is modern, so auto must not shift: {}",
            console.stderr()
        );
    }

    #[test]
    fn explicit_7168_offset_is_not_labelled_as_auto_detected() {
        let scratch = Scratch::new("read-explicit-7168");
        let stale_utc = (crate::cal::days_from_civil(2006, 9, 17) * 86_400) as u32;
        let path = scratch.join("stale.bin");
        fileio::write(&path, &dump_with_timestamps(&[stale_utc])).expect("write");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        read(
            &LogReadOptions {
                file: path,
                format: m241_core::ExportFormat::Json,
                output: None,
                utc_offset_days: OffsetSpec::Days(crate::cal::WNRO_DAYS),
            },
            fast(&links, &console, &confirm),
        )
        .expect("read");
        let stderr = console.stderr();
        assert!(stderr.contains("--utc-offset-days"), "got {stderr}");
        assert!(
            !stderr.contains("old GPS epoch"),
            "explicit offset must not claim auto-detection: {stderr}"
        );
    }

    #[test]
    fn download_json_is_one_object_with_clean_stdout() {
        let scratch = Scratch::new("download-json");
        let output = scratch.join("track.bin");
        let handle = DeviceHandle::new(|| FakeDevice::new().with_log(&synthetic_dump(2)));
        let links = FakePorts::shared(handle);
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        download(
            &DownloadOptions {
                port: String::from("auto"),
                output: output.clone(),
                force: false,
                json: true,
            },
            fast(&links, &console, &confirm),
        )
        .expect("download --json");
        let stdout = console.stdout();
        let document: serde_json::Value =
            serde_json::from_str(&stdout).expect("stdout must be a single valid JSON object");
        assert_eq!(document["bytes"], synthetic_dump(2).len());
        assert_eq!(document["summary"]["records"], 2);
        assert_eq!(document["output"], output.display().to_string());
        assert!(
            !console.stderr().is_empty(),
            "progress/notes belong on stderr"
        );
    }

    #[test]
    fn download_exits_code_one_when_the_device_rejects_pmtk182() {
        use std::collections::VecDeque;

        use m241_core::protocol::pmtk::line as pmtk_line;
        use m241_core::protocol::port::Transport;

        #[derive(Debug)]
        struct RejectingStopTransport {
            pending: Vec<u8>,
            output: VecDeque<u8>,
        }

        impl Transport for RejectingStopTransport {
            fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
                let mut count = 0;
                while count < buf.len() {
                    let Some(byte) = self.output.pop_front() else {
                        break;
                    };
                    buf[count] = byte;
                    count += 1;
                }
                Ok(count)
            }

            fn write_all(&mut self, buf: &[u8]) -> Result<()> {
                self.pending.extend_from_slice(buf);
                while let Some(pos) = self.pending.iter().position(|&b| b == b'\n') {
                    let raw: Vec<u8> = self.pending.drain(..=pos).collect();
                    let text = String::from_utf8_lossy(&raw);
                    if text.contains("PMTK182,2,8") {
                        self.output.extend(pmtk_line("PMTK182,3,8,100").as_bytes());
                    } else if text.contains("PMTK182,5") {
                        // Status 2 = action failed / rejected by the device.
                        self.output.extend(pmtk_line("PMTK001,182,5,2").as_bytes());
                    }
                }
                Ok(())
            }

            fn flush(&mut self) -> Result<()> {
                Ok(())
            }

            fn clear_input(&mut self) -> Result<()> {
                self.output.clear();
                Ok(())
            }

            fn set_baud(&mut self, _baud: u32) -> Result<()> {
                Ok(())
            }

            fn set_control_lines(&mut self, _asserted: bool) -> Result<()> {
                Ok(())
            }
        }

        struct RejectingLinks;

        impl crate::Links for RejectingLinks {
            fn ports(&self) -> Result<Vec<PortEntry>> {
                Ok(vec![PortEntry {
                    name: String::from("SIM0"),
                    cable: Some(crate::link::Cable::Cp210x),
                    likely: true,
                    description: None,
                }])
            }

            fn open(&self, _name: &str, _baud: u32) -> Result<crate::link::Opened> {
                Ok(crate::link::Opened::new(Box::new(RejectingStopTransport {
                    pending: Vec::new(),
                    output: VecDeque::new(),
                })))
            }
        }

        let scratch = Scratch::new("download-rejected");
        let output = scratch.join("track.bin");
        let links = RejectingLinks;
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        let err = download(
            &DownloadOptions {
                port: String::from("auto"),
                output: output.clone(),
                force: false,
                json: false,
            },
            Session::new(&links, &console, &confirm).with_timings(Timings::fast()),
        )
        .expect_err("device rejected $PMTK182,5");
        assert!(matches!(err, Error::Rejected { .. }), "got {err}");
        assert_eq!(crate::exit::exit_code_for(&err), 1, "got {err}");
        assert_eq!(
            crate::exit::exit_code_of(&Err(crate::exit::CliError::from(err))),
            1
        );
        assert!(!output.exists());
    }

    #[test]
    fn download_reports_the_saved_path_when_parsing_fails() {
        let scratch = Scratch::new("download-unparseable");
        let output = scratch.join("raw.bin");
        let garbage = vec![0xABu8; 4096];
        let handle = DeviceHandle::new(move || FakeDevice::new().with_log(&garbage));
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
        .expect_err("garbage bytes are not a valid log dump");
        assert!(matches!(err, Error::Log { .. }), "got {err}");
        assert!(output.exists(), "raw capture is preserved on disk");
        assert!(
            console.stderr().contains(&output.display().to_string()),
            "stderr must tell the user where the raw dump was saved: {}",
            console.stderr()
        );
    }

    #[test]
    fn read_exports_kml_and_nmea_from_a_dump() {
        let scratch = Scratch::new("read-kml-nmea");
        let path = write_dump(&scratch, "track.bin", 2);
        let links = FakePorts::empty();
        let confirm = AutoConfirm;

        let kml_console = RecordingConsole::default();
        read(
            &LogReadOptions {
                file: path.clone(),
                format: m241_core::ExportFormat::Kml,
                output: None,
                utc_offset_days: OffsetSpec::Days(0),
            },
            fast(&links, &kml_console, &confirm),
        )
        .expect("kml export");
        assert!(
            kml_console.stdout().contains("<kml") && kml_console.stdout().contains("139.767"),
            "got {}",
            kml_console.stdout()
        );

        let nmea_console = RecordingConsole::default();
        read(
            &LogReadOptions {
                file: path,
                format: m241_core::ExportFormat::Nmea,
                output: None,
                utc_offset_days: OffsetSpec::Days(0),
            },
            fast(&links, &nmea_console, &confirm),
        )
        .expect("nmea export");
        assert!(
            nmea_console.stdout().contains("$GPRMC"),
            "got {}",
            nmea_console.stdout()
        );
    }

    /// Build a minimal valid JPEG containing an APP1 Exif block with `DateTimeOriginal`.
    fn synthetic_jpeg_with_datetime(datetime: &str) -> Vec<u8> {
        let mut tiff = Vec::new();
        // Little-endian TIFF header: "II", 42, IFD0 at offset 8.
        tiff.extend_from_slice(b"II");
        tiff.extend_from_slice(&42u16.to_le_bytes());
        tiff.extend_from_slice(&8u32.to_le_bytes());

        // IFD0 at offset 8: 1 entry (ExifIFDPointer), next IFD = 0.
        // IFD0 size = 2 + 12 + 4 = 18 bytes -> ends at offset 26.
        let exif_ifd_offset = 26u32;
        tiff.extend_from_slice(&1u16.to_le_bytes());
        tiff.extend_from_slice(&0x8769u16.to_le_bytes()); // EXIF_IFD_POINTER
        tiff.extend_from_slice(&4u16.to_le_bytes()); // LONG
        tiff.extend_from_slice(&1u32.to_le_bytes());
        tiff.extend_from_slice(&exif_ifd_offset.to_le_bytes());
        tiff.extend_from_slice(&0u32.to_le_bytes());

        // Exif SubIFD at offset 26: 1 entry (DateTimeOriginal), next IFD = 0.
        // SubIFD size = 2 + 12 + 4 = 18 bytes -> ends at offset 44.
        let mut dt_bytes = datetime.as_bytes().to_vec();
        dt_bytes.push(0);
        let value_offset = 44u32;
        tiff.extend_from_slice(&1u16.to_le_bytes());
        tiff.extend_from_slice(&0x9003u16.to_le_bytes()); // DATE_TIME_ORIGINAL
        tiff.extend_from_slice(&2u16.to_le_bytes()); // ASCII
        tiff.extend_from_slice(&(dt_bytes.len() as u32).to_le_bytes());
        tiff.extend_from_slice(&value_offset.to_le_bytes());
        tiff.extend_from_slice(&0u32.to_le_bytes());
        tiff.extend_from_slice(&dt_bytes);

        let mut app1 = Vec::from(b"Exif\0\0".as_slice());
        app1.extend_from_slice(&tiff);
        let app1_len = (app1.len() + 2) as u16;

        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE1];
        jpeg.extend_from_slice(&app1_len.to_be_bytes());
        jpeg.extend_from_slice(&app1);
        jpeg.extend_from_slice(&[0xFF, 0xD9]);
        jpeg
    }

    fn format_exif_utc(utc: u32) -> String {
        let days = i64::from(utc / 86_400);
        let secs = utc % 86_400;
        let (year, month, day) = crate::cal::civil_from_days(days);
        format!(
            "{year:04}:{month:02}:{day:02} {:02}:{:02}:{:02}",
            secs / 3_600,
            (secs % 3_600) / 60,
            secs % 60
        )
    }

    #[test]
    fn geotag_dry_run_leaves_photo_untouched_and_apply_writes_with_wnro_auto_shift() {
        let scratch = Scratch::new("geotag-wnro");
        let stale_utc = (crate::cal::days_from_civil(2006, 9, 17) * 86_400 + 3600) as u32;
        let shifted_utc = crate::cal::shift_seconds(stale_utc, crate::cal::WNRO_DAYS);

        let log_path = scratch.join("stale.bin");
        fileio::write(&log_path, &dump_with_timestamps(&[stale_utc])).expect("write dump");

        let original_jpeg = synthetic_jpeg_with_datetime(&format_exif_utc(shifted_utc));
        let photo_path = scratch.join("shot.jpg");
        fileio::write(&photo_path, &original_jpeg).expect("write jpeg");

        let links = FakePorts::empty();
        let confirm = AutoConfirm;

        // Dry run must leave the JPEG byte-for-byte unchanged.
        let dry_console = RecordingConsole::default();
        geotag(
            &LogGeotagOptions {
                log: log_path.clone(),
                photos: vec![photo_path.clone()],
                max_gap: 300,
                utc_offset_days: OffsetSpec::Auto,
                tz_offset_minutes: 0,
                dry_run: true,
                json: true,
            },
            Session::new(&links, &dry_console, &confirm),
        )
        .expect("dry run");
        assert_eq!(
            fileio::read(&photo_path).expect("photo"),
            original_jpeg,
            "dry-run must not touch the JPEG on disk"
        );
        let dry_doc: serde_json::Value =
            serde_json::from_str(&dry_console.stdout()).expect("one JSON object");
        assert_eq!(dry_doc["dry_run"], true);
        assert_eq!(dry_doc["offset_days"], crate::cal::WNRO_DAYS);
        assert_eq!(dry_doc["written"], 0);
        assert_eq!(dry_doc["skipped"], 0);
        assert_eq!(dry_doc["plans"][0]["status"], "planned");

        // Real run writes GPS EXIF tags verified by exif::read_gps.
        let apply_console = RecordingConsole::default();
        geotag(
            &LogGeotagOptions {
                log: log_path,
                photos: vec![photo_path.clone()],
                max_gap: 300,
                utc_offset_days: OffsetSpec::Auto,
                tz_offset_minutes: 0,
                dry_run: false,
                json: false,
            },
            Session::new(&links, &apply_console, &confirm),
        )
        .expect("apply geotag");
        let tagged = fileio::read(&photo_path).expect("tagged photo");
        assert_ne!(tagged, original_jpeg);
        let gps = m241_core::log::exif::read_gps(&tagged)
            .expect("parse exif")
            .expect("gps tags written");
        assert!((gps.latitude.expect("lat") - 35.681_236).abs() < 1e-4);
        assert!((gps.longitude.expect("lon") - 139.767_13).abs() < 1e-4);
        assert!(apply_console.stdout().contains("written : 1"));
    }

    #[test]
    fn geotag_expands_directories_handles_skips_and_applies_tz_offset() {
        let scratch = Scratch::new("geotag-dir");
        let track_utc = 1_700_000_000u32;
        let log_path = write_dump(&scratch, "track.bin", 2);

        let photos_dir = scratch.join("photos");
        let nested_dir = photos_dir.join("nested");
        std::fs::create_dir_all(&nested_dir).expect("create dir");

        // Camera clock set to UTC+9 (540 minutes ahead of UTC).
        let local_camera_utc = track_utc + 540 * 60;
        let good_photo = photos_dir.join("01_good.jpg");
        fileio::write(
            &good_photo,
            &synthetic_jpeg_with_datetime(&format_exif_utc(local_camera_utc)),
        )
        .expect("write good jpg");

        // Photo taken outside max_gap.
        let far_photo = photos_dir.join("02_far.jpg");
        fileio::write(
            &far_photo,
            &synthetic_jpeg_with_datetime(&format_exif_utc(local_camera_utc + 1000)),
        )
        .expect("write far jpg");

        // Non-JPEG file in the same directory must be skipped cleanly.
        let png_file = photos_dir.join("03_note.png");
        fileio::write(&png_file, b"\x89PNG\r\n\x1a\n").expect("write png");

        // Subdirectory file must NOT be collected (only immediate regular files).
        let nested_bytes = synthetic_jpeg_with_datetime(&format_exif_utc(local_camera_utc));
        let nested_photo = nested_dir.join("ignored.jpg");
        fileio::write(&nested_photo, &nested_bytes).expect("write nested");

        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        geotag(
            &LogGeotagOptions {
                log: log_path,
                photos: vec![photos_dir],
                max_gap: 300,
                utc_offset_days: OffsetSpec::Days(0),
                tz_offset_minutes: 540,
                dry_run: false,
                json: true,
            },
            Session::new(&links, &console, &confirm),
        )
        .expect("geotag directory");

        let doc: serde_json::Value =
            serde_json::from_str(&console.stdout()).expect("one JSON object");
        assert_eq!(doc["offset_days"], 0);
        assert_eq!(doc["written"], 1);
        assert_eq!(doc["skipped"], 2);
        let plans = doc["plans"].as_array().expect("plans array");
        assert_eq!(plans.len(), 3, "only immediate regular files are included");
        assert_eq!(plans[0]["status"], "written");
        // matched_utc must be true UTC (not shifted by camera tz offset!).
        assert_eq!(plans[0]["matched_utc"], track_utc);
        assert_eq!(plans[1]["status"], "skipped");
        assert_eq!(plans[2]["status"], "skipped");
        assert!(
            plans[2]["skip_reason"]
                .as_str()
                .unwrap_or("")
                .contains("not a JPEG")
        );

        let good_gps =
            m241_core::log::exif::read_gps(&fileio::read(&good_photo).expect("read good"))
                .expect("read exif")
                .expect("gps tags written");
        assert!((good_gps.latitude.expect("lat") - 35.681_236).abs() < 1e-4);

        // Nested photo was not touched.
        assert_eq!(
            fileio::read(&nested_photo).expect("read nested"),
            nested_bytes
        );
        let nested_gps = m241_core::log::exif::read_gps(&nested_bytes)
            .expect("read exif")
            .expect("exif present without gps");
        assert_eq!(nested_gps.latitude, None);
        assert_eq!(nested_gps.longitude, None);
    }
}
