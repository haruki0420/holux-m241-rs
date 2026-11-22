//! Convert, edit and bundle a downloaded track log.
//!
//! `m241 log download`, `m241 log info`, `m241 log read`, `m241 log edit`,
//! `m241 log erase`, `m241 log geotag` and `m241 log kmz`.
//!
//! Downloading talks PMTK182 to the logger and saves the raw flash blocks;
//! `info` summarises a dump without converting it so the user can check damage
//! before exporting; `read` converts a dump to GPX, KML, CSV, NMEA or JSON,
//! optionally shifting WNRO-stale timestamps; `edit` splits, merges or trims a
//! dump and exports the result in the same formats; `erase` saves the log and
//! then destroys it on the device through `m241-flash`; `geotag` matches photos
//! by capture time and writes GPS EXIF tags; `kmz` bundles the track and the
//! matched photos into an archive for a map viewer.

use std::path::{Path, PathBuf};

use m241_core::log::edit::TrackEditor;
use m241_core::log::geotag::{self, CaptureTime, GeotagOptions, GeotagPlan};
use m241_core::log::kmz::{KmzPhoto, export_kmz};
use m241_core::log::{ParseReport, export, parse_dump};
use m241_core::protocol::PmtkClient;
use m241_core::protocol::port::Transport;
use m241_core::{Error, ExportFormat, LogSummary, Result, Track};
use m241_flash::erase::{BackupProof, LogEraser};

use crate::Session;
use crate::cal::OffsetSpec;
use crate::cli::{
    DownloadOptions, EraseOptions, LogEditOptions, LogGeotagOptions, LogInfoOptions, LogKmzOptions,
    LogReadOptions, ShiftScopeArg,
};
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
    let bytes = read_log(&mut client, session.console)?;

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
    warn_about_damage(session.console, &summary, &report, None);

    if options.json {
        return session.console.json(&serde_json::json!({
            "port": port,
            "output": options.output.display().to_string(),
            "bytes": bytes.len(),
            "summary": summary_document(summary),
        }));
    }
    print_download(
        session.console,
        &port,
        &options.output,
        bytes.len(),
        summary,
    );
    session.console.note(&format!(
        "next: m241 log read {} --format gpx -o track.gpx",
        options.output.display()
    ));
    Ok(())
}

/// Read the whole log into memory, reporting progress on stderr.
///
/// Shared by `log download` and `log erase` so the erase path shows the same
/// five-percent steps rather than growing a second progress loop.
fn read_log<T: Transport>(client: &mut PmtkClient<T>, console: &dyn Console) -> Result<Vec<u8>> {
    let mut last_reported = 0usize;
    let bytes = client.download_log(&mut |done, total| {
        // One line per 5% rather than per chunk: a 200 KB log is about 100 round
        // trips, and a line apiece would be a wall of text.
        let step = (total / 20).max(1);
        if done == total || done.saturating_sub(last_reported) >= step {
            last_reported = done;
            console.progress(done, total);
        }
    })?;
    console.progress_done();
    Ok(bytes)
}

/// Save the track log, then erase it on the device.
///
/// The copy is written **before** the erase is considered, and the
/// [`BackupProof`] handed to [`LogEraser::erase`] is made from the bytes that
/// were just written. If the device reports an empty log (`0` bytes), nothing
/// is written or erased and an [`Error::Safety`] is returned. A failed write
/// also returns before the erase sentence is built, so there is no path from
/// "the backup could not be saved" to a destructive command. Without `--yes`
/// the download still runs and is saved: the refusal says where the copy is,
/// and the next invocation can erase.
///
/// # Errors
///
/// * [`Error::Safety`] when the output exists and `--force` was not given, when
///   the device reports an empty log (`0` bytes), or when `--yes` is absent
///   (exit code 5);
/// * [`Error::Port`] when no plausible port can be opened;
/// * [`Error::Timeout`] / [`Error::Nmea`] / [`Error::Rejected`] when the
///   transfer or the erase fails;
/// * [`Error::Io`] when the backup cannot be written.
pub fn erase(options: &EraseOptions, session: Session<'_>) -> Result<()> {
    fileio::ensure_free(&options.output, options.force)?;
    let port = link::resolve(session.links, &options.port)?;
    let mut client = link::open_pmtk(session.links, &port, session.timings)?;

    session
        .console
        .note(&format!("reading the track log from {port}"));
    let bytes = read_log(&mut client, session.console)?;

    if bytes.is_empty() {
        return Err(Error::Safety {
            reason: String::from(
                "the device reported an empty log (0 bytes); nothing was written or erased",
            ),
        });
    }

    // The file is the only copy this tool can offer once `$PMTK182,6` has run.
    // Writing it first, and returning here if the write fails, is what makes
    // the proof below evidence rather than a formality.
    fileio::write(&options.output, &bytes)?;

    if !options.yes {
        session.console.note(&format!(
            "saved {} bytes to {}; the log on the device was not erased",
            bytes.len(),
            options.output.display()
        ));
        return Err(Error::Safety {
            reason: format!(
                "saved {} bytes to {}, but the log was not erased; re-run with --yes (and \
                 --force to reuse this path) to erase it",
                bytes.len(),
                options.output.display()
            ),
        });
    }

    let proof = BackupProof::of_bytes(&bytes);
    // Logging must be stopped (`$PMTK182,5`) before `$PMTK182,6,1` and always
    // resumed (`$PMTK182,4`) afterwards, even when the stop's acknowledgement is
    // lost or the erase fails — otherwise a failed command leaves the unit
    // silently not recording. A failed stop also skips the erase: the unit's
    // state is unknown, and the erase sentence must not be sent blind. The stop
    // error is kept as the primary failure; if the resume fails too, both are
    // reported.
    let (mut client, outcome) = match client.log_stop() {
        Ok(()) => {
            let mut eraser =
                LogEraser::new(client.into_transport()).with_reply_timeout(session.timings.erase);
            let outcome = eraser.erase(proof);
            let client = PmtkClient::new(eraser.into_transport())
                .with_reply_timeout(session.timings.reply)
                .with_stream_window(session.timings.stream);
            (client, outcome)
        }
        Err(err) => (client, Err(err)),
    };
    m241_core::protocol::pmtk::resume_after(&mut client, outcome)?;

    if options.json {
        return session.console.json(&serde_json::json!({
            "port": port,
            "output": options.output.display().to_string(),
            "bytes": bytes.len(),
            "erased": true,
        }));
    }
    print_erase(session.console, &port, &options.output, bytes.len());
    Ok(())
}

/// The labelled lines `log erase` prints on stdout.
fn print_erase(console: &dyn Console, port: &str, output: &Path, bytes: usize) {
    console.result(&format!("{:<12}: {port}", "port"));
    console.result(&format!("{:<12}: {}", "output", output.display()));
    console.result(&format!("{:<12}: {bytes}", "bytes"));
    console.result(&format!("{:<12}: yes", "erased"));
}

/// The labelled lines `log download` prints on stdout.
fn print_download(
    console: &dyn Console,
    port: &str,
    output: &Path,
    bytes: usize,
    summary: LogSummary,
) {
    console.result(&format!("{:<12}: {port}", "port"));
    console.result(&format!("{:<12}: {}", "output", output.display()));
    console.result(&format!("{:<12}: {bytes}", "bytes"));
    console.result(&format!("{:<12}: {}", "records", summary.records));
    console.result(&format!("{:<12}: {}", "fixes", summary.fixes));
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
    warn_about_damage(session.console, &summary, &report, None);

    let shift = apply_utc_offset(&mut track, options.utc_offset_days, session.console);

    // The shift is already announced on stderr by `apply_utc_offset`. A JSON
    // reader gets it as a field too, so `--format json` carries the same fact as
    // `geotag --json` rather than leaving the number in a human message.
    let exported = export(&track, options.format);
    let exported = if options.format == ExportFormat::Json && shift != 0 {
        match serde_json::from_str::<serde_json::Value>(&exported) {
            Ok(serde_json::Value::Object(mut object)) => {
                object.insert(String::from("offset_days"), serde_json::Value::from(shift));
                format!("{}\n", serde_json::Value::Object(object))
            }
            // Not an object: leave the exporter's text exactly as it wrote it.
            _ => exported,
        }
    } else {
        exported
    };

    match &options.output {
        Some(path) => {
            fileio::write_text(path, &exported)?;
            session.console.note(&format!(
                "wrote {} to {}",
                count_label(track.points.len(), "point"),
                path.display()
            ));
        }
        None => session.console.text(&exported),
    }
    Ok(())
}

/// `1 point`, `2 points`, `1 photo`, `2 photos` — a count as it reads in a
/// sentence.
fn count_label(count: usize, singular: &str) -> String {
    format!("{count} {singular}{}", if count == 1 { "" } else { "s" })
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

/// Add whole `days` to every timestamp the track carries.
///
/// `cal::shift_seconds` takes its second argument in *days* despite the name
/// (it is the one place the epoch-to-seconds conversion happens); this wrapper
/// keeps the unit in its own name so a caller cannot pass a day count where a
/// second count was meant. `shift_track_seconds` below is the second-counting
/// sibling.
fn shift_track_days(track: &mut Track, days: i32) {
    for point in &mut track.points {
        point.utc = point.utc.map(|utc| crate::cal::shift_seconds(utc, days));
    }
}

/// Apply `--utc-offset-days` (`auto` or explicit) to `track` and return the applied day shift.
fn apply_utc_offset(track: &mut Track, spec: OffsetSpec, console: &dyn Console) -> i32 {
    let shift = track_days(track).map_or(0, |days| spec.shift_for(days));
    if shift != 0 {
        shift_track_days(track, shift);
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

/// Split, merge or trim a dumped track log, then export the result.
///
/// The output keeps `read`'s formats. A split leaves two tracks, so the first
/// half goes to `-o` and the second to a sibling named from it
/// (`track-2.gpx`); merge and delete leave one track and write only `-o`. The
/// input dump is read, never written.
///
/// # Errors
///
/// [`Error::Io`] when a log cannot be read or an output written; [`Error::Log`]
/// when an input is not a dump, a named point does not exist, or a split would
/// leave a half empty; [`Error::State`] when the edit does not leave one or two
/// tracks; [`Error::Safety`] when the split's sibling path already exists,
/// because that file is not one the user named.
pub fn edit(options: &LogEditOptions, session: Session<'_>) -> Result<()> {
    let bytes = fileio::read(&options.log)?;
    let (track, report) = parse_dump(&bytes)?;
    warn_about_damage(session.console, &report.summary(&track), &report, None);
    let mut editor = TrackEditor::from_track(track);

    match requested_edit(options) {
        Some(LogEdit::Split(split_at)) => editor.split(FIRST_TRACK, split_at)?,
        Some(LogEdit::Merge(paths)) => {
            editor = merge_in_time_order(editor.tracks(), &paths, session.console)?;
        }
        Some(LogEdit::Delete(indices)) => delete_named_points(&mut editor, &indices)?,
        // `cli.rs`'s `ArgGroup` requires one operation, so this arm is
        // unreachable from a parsed command line. It is an error rather than a
        // panic because a library caller can build `LogEditOptions` without
        // going through clap.
        None => {
            return Err(Error::State(String::from(
                "no edit was selected; the argument grammar requires exactly one",
            )));
        }
    }

    write_edited(&editor, options, session.console)
}

/// The index of the only track an editor built from one dump holds.
const FIRST_TRACK: usize = 0;

/// The one edit `cli.rs`'s `ArgGroup` lets a command line ask for.
///
/// Spelled out instead of inferred from "`--merge` is empty, so this must be
/// `--delete`": the exclusivity is a fact about the grammar in another file, and
/// a reader of this handler should not have to reconstruct it.
enum LogEdit {
    Split(usize),
    Merge(Vec<PathBuf>),
    Delete(Vec<usize>),
}

/// The operation `options` asks for, when the grammar has admitted one.
fn requested_edit(options: &LogEditOptions) -> Option<LogEdit> {
    if let Some(split_at) = options.split_at {
        Some(LogEdit::Split(split_at))
    } else if !options.merge.is_empty() {
        Some(LogEdit::Merge(options.merge.clone()))
    } else if !options.delete.is_empty() {
        Some(LogEdit::Delete(options.delete.clone()))
    } else {
        None
    }
}

/// Write the edited tracks and report where they went.
fn write_edited(
    editor: &TrackEditor,
    options: &LogEditOptions,
    console: &dyn Console,
) -> Result<()> {
    match editor.tracks() {
        [only] => {
            write_track(&options.output, only, options.format)?;
            console.note(&format!(
                "wrote {} to {}",
                count_label(only.points.len(), "point"),
                options.output.display()
            ));
        }
        [first, second] => {
            let second_path = second_sibling(&options.output);
            // The sibling is a path the user did not name, so replacing a file
            // that is already there would destroy something this command was
            // never asked about. Checked before either half is written, so a
            // refusal leaves no part of the edit on disk.
            fileio::ensure_free(&second_path, false)?;
            write_track(&options.output, first, options.format)?;
            if let Err(err) = write_track(&second_path, second, options.format) {
                console.warn(&format!(
                    "{} was written before the failure",
                    options.output.display()
                ));
                return Err(err);
            }
            console.note(&format!(
                "wrote {} to {} and {} to {}",
                count_label(first.points.len(), "point"),
                options.output.display(),
                count_label(second.points.len(), "point"),
                second_path.display()
            ));
        }
        tracks => {
            return Err(Error::State(format!(
                "the edit produced {} tracks; one or two were expected",
                tracks.len()
            )));
        }
    }
    Ok(())
}

/// Write one track in `format` to `path`.
fn write_track(path: &Path, track: &Track, format: ExportFormat) -> Result<()> {
    fileio::write_text(path, &export(track, format))
}

/// Delete `indices`, refusing any the log does not have.
///
/// `TrackEditor::delete_points` ignores out-of-range indices. That is right for
/// a UI deleting a stale selection but wrong for a typed command, where a typo
/// would silently delete nothing; here it is an error naming the index and the
/// valid range.
fn delete_named_points(editor: &mut TrackEditor, indices: &[usize]) -> Result<()> {
    let points = editor
        .tracks()
        .first()
        .map_or(0, |track| track.points.len());
    // The range is inclusive (`0..=2` holds three indices, and `0..=0` holds
    // one): an exclusive `0..3` would read as though 3 were valid, and with no
    // points `0..0` would read as though index 0 existed.
    let range = if points == 0 {
        String::from("the log has no points")
    } else {
        format!("{points} point(s) are indexed 0..={}", points - 1)
    };
    for &index in indices {
        if index >= points {
            return Err(Error::Log {
                message: format!("point index {index} does not exist: {range}"),
                offset: None,
            });
        }
    }
    editor.delete_points(FIRST_TRACK, indices)
}

/// Read the `--merge` dumps and join them with `tracks` in capture-time order.
///
/// [`TrackEditor::merge`] concatenates in the order the indices are listed, so
/// they are sorted by each track's earliest timestamp first. A track with no
/// timestamp sorts after the timed ones rather than in front of them: it cannot
/// claim a place among dumps that carry one.
fn merge_in_time_order(
    tracks: &[Track],
    merge_paths: &[PathBuf],
    console: &dyn Console,
) -> Result<TrackEditor> {
    let mut all = tracks.to_vec();
    for path in merge_paths {
        let bytes = fileio::read(path)?;
        let (track, report) = parse_dump(&bytes)?;
        warn_about_damage(console, &report.summary(&track), &report, Some(path));
        all.push(track);
    }
    let mut indices: Vec<usize> = (0..all.len()).collect();
    indices.sort_by_key(|&index| {
        all.get(index)
            .and_then(|track| track.points.iter().filter_map(|point| point.utc).min())
            .unwrap_or(u32::MAX)
    });
    let mut editor = TrackEditor::new(all);
    editor.merge(&indices)?;
    Ok(editor)
}

/// `track.gpx` -> `track-2.gpx`, `track` -> `track-2`.
fn second_sibling(path: &Path) -> PathBuf {
    const SECOND: u32 = 2;
    let stem = path.file_stem().map_or_else(
        || String::from("track"),
        |stem| stem.to_string_lossy().into_owned(),
    );
    match path.extension() {
        Some(extension) => {
            path.with_file_name(format!("{stem}-{SECOND}.{}", extension.to_string_lossy()))
        }
        None => path.with_file_name(format!("{stem}-{SECOND}")),
    }
}

/// Bundle a track log and its photos into a KMZ archive.
///
/// Photos are matched to track points by capture time through [`geotag::plan`],
/// so `--utc-offset-days auto` matters here as much as it does in `log geotag`:
/// nothing lines up with a log that is still 7168 days in the past. A photo the
/// planner leaves out is left out with the planner's own reason rather than a
/// guess, because "no point inside `--max-gap`" is only one of them — a non-JPEG,
/// an unreadable capture time and an undecodable EXIF block are others — and the
/// reason is what tells the user which next step would help.
///
/// The archive's layout is checked by this project's own ZIP reader in
/// `m241-core`; Google Earth and `unzip` have not been run on the output, so it
/// is not claimed to open in an external viewer.
///
/// # Errors
///
/// [`Error::Io`] when the log, a photo or the output cannot be read or written;
/// [`Error::Log`] when the log is not a dump, or when the archive would exceed
/// the ZIP32 limits; [`Error::State`] when a photo over `--max-photo-edge`
/// cannot be decoded to be reduced.
pub fn kmz(options: &LogKmzOptions, session: Session<'_>) -> Result<()> {
    let bytes = fileio::read(&options.log)?;
    let (mut track, report) = parse_dump(&bytes)?;
    warn_about_damage(session.console, &report.summary(&track), &report, None);
    apply_utc_offset(&mut track, options.utc_offset_days, session.console);

    let photo_paths = expand_photo_inputs(&options.photos)?;
    let geotag_options = GeotagOptions {
        max_gap_seconds: options.max_gap,
        ..GeotagOptions::default()
    };
    let plans = plan_kmz_photos(
        &photo_paths,
        &track,
        &geotag_options,
        options.tz_offset_minutes,
    )?;
    let photos = materialize_kmz_photos(&plans, options.max_photo_edge, session.console)?;

    let archive = export_kmz(&track, &photos)?;
    fileio::write(&options.output, &archive)?;
    session.console.note(&format!(
        "wrote {} and {} to {}",
        count_label(track.points.len(), "point"),
        count_label(photos.len(), "photo"),
        options.output.display()
    ));
    Ok(())
}

/// Match `photos` to `track` in the camera's clock, then put the capture times
/// back on UTC for the archive.
///
/// `tz_offset_minutes` is the camera's offset from UTC, for the same reason
/// `log geotag` has the flag: `DateTimeOriginal` is the camera's local time, so a
/// camera set to UTC+9 would otherwise look nine hours away from every point.
/// Matching happens on a shifted copy of the track; the archive's track stays
/// UTC, and so do the times recorded for the photos, because a `<TimeStamp>` in
/// local time would sit at a different moment from the track it is drawn on.
fn plan_kmz_photos(
    photos: &[PathBuf],
    track: &Track,
    options: &GeotagOptions,
    tz_offset_minutes: i32,
) -> Result<Vec<GeotagPlan>> {
    let tz_delta_seconds = i64::from(tz_offset_minutes) * 60;
    let mut matching_track = track.clone();
    if tz_delta_seconds != 0 {
        shift_track_seconds(&mut matching_track, tz_delta_seconds);
    }
    let mut plans = geotag::plan(photos, &matching_track, options)?;
    if tz_delta_seconds != 0 {
        for plan in &mut plans {
            plan.capture_time = plan
                .capture_time
                .map(|capture| shift_capture_time(capture, -tz_delta_seconds));
        }
    }
    Ok(plans)
}

/// `capture` moved by `delta_seconds`, clamped like every other time shift here.
fn shift_capture_time(capture: CaptureTime, delta_seconds: i64) -> CaptureTime {
    match capture {
        CaptureTime::Exif(utc) => CaptureTime::Exif(shift_timestamp_seconds(utc, delta_seconds)),
        CaptureTime::FileModified(utc) => {
            CaptureTime::FileModified(shift_timestamp_seconds(utc, delta_seconds))
        }
    }
}

/// Turn the plans into archive entries, reading each accepted photo's bytes.
fn materialize_kmz_photos(
    plans: &[GeotagPlan],
    max_edge: Option<u32>,
    console: &dyn Console,
) -> Result<Vec<KmzPhoto>> {
    let mut photos = Vec::with_capacity(plans.len());
    for plan in plans {
        let Some(mut photo) = plan.to_kmz_photo(max_edge) else {
            // `to_kmz_photo` is `None` for every reason the planner rejected the
            // photo, not only a gap over the limit; the plan carries that reason,
            // and it names the next step where a guess here would mislead.
            let reason = plan.skipped.as_deref().unwrap_or("skipped");
            console.warn(&format!(
                "{} ({reason}); it was not bundled",
                plan.photo.display()
            ));
            continue;
        };
        photo.jpeg_bytes = fileio::read(&plan.photo)?;
        photos.push(photo);
    }
    Ok(photos)
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
    warn_about_damage(session.console, &summary, &report, None);

    if options.json {
        return session.console.json(&serde_json::json!({
            "file": options.file.display().to_string(),
            "size": bytes.len(),
            "summary": summary_document(summary),
            "parse": parse_document(&report),
        }));
    }
    print_info(
        session.console,
        &options.file,
        bytes.len(),
        summary,
        &report,
    );
    Ok(())
}

/// The labelled lines `log info` prints on stdout.
fn print_info(
    console: &dyn Console,
    file: &Path,
    size: usize,
    summary: LogSummary,
    report: &ParseReport,
) {
    let lines = [
        ("file", file.display().to_string()),
        ("size", size.to_string()),
        ("records", summary.records.to_string()),
        ("fixes", summary.fixes.to_string()),
        ("blocks", report.blocks.to_string()),
        ("damaged blocks", summary.damaged_block_count.to_string()),
        ("bad checksums", report.bad_checksums.to_string()),
        ("truncated records", report.truncated_records.to_string()),
        ("start", timestamp(summary.started_at)),
        ("end", timestamp(summary.ended_at)),
    ];
    for (label, value) in lines {
        console.result(&format!("{label:<18}: {value}"));
    }
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
/// The four `--shift-*` values move the clock of the photos `--scope` selects
/// and re-match them. Every photo is still planned: one outside the scope keeps
/// its clock and is tagged at its unshifted time, the contract
/// [`geotag::plan_with_time_shift`] implements. [`geotag::apply`] writes the
/// shifted value into `DateTimeOriginal`, so a later run needs no shift.
///
/// # Errors
///
/// * [`Error::Io`] when `<LOG>`, a directory, or a photo cannot be read or replaced;
/// * [`Error::Log`] when `<LOG>` is not a valid M-241 dump;
/// * [`Error::State`] when the four `--shift-*` values do not add up to a number
///   of seconds `i64` can hold.
pub fn geotag(options: &LogGeotagOptions, session: Session<'_>) -> Result<()> {
    let bytes = fileio::read(&options.log)?;
    let (mut track, report) = parse_dump(&bytes)?;
    let summary = report.summary(&track);
    warn_about_damage(session.console, &summary, &report, None);

    let offset_days = apply_utc_offset(&mut track, options.utc_offset_days, session.console);
    let photo_paths = expand_photo_inputs(&options.photos)?;

    // Each of these belongs to one scope; reporting the extra one beats doing
    // nothing silently when the user spelled `--scope` wrong.
    if options.scope != ShiftScopeArg::Selected && !options.photo.is_empty() {
        session
            .console
            .warn("--photo is only read with --scope selected; it is ignored here");
    }
    if options.scope != ShiftScopeArg::ByCamera
        && (options.camera_make.is_some() || options.camera_model.is_some())
    {
        session.console.warn(
            "--camera-make/--camera-model are only read with --scope by-camera; \
             they are ignored here",
        );
    }
    let shift = options.shift();
    let scope = options.shift_scope();

    // The camera's clock, not the log's. `DateTimeOriginal` is written from the
    // camera's local time, so a camera set to UTC+9 records a capture nine hours
    // later than the moment it happened. Matching against the UTC track therefore
    // moves the *track* forward by the same amount — both sides are now on the
    // camera's clock and the nearest point is the right one — and then moves the
    // chosen point's `utc` back, because the coordinate that gets written is a
    // real UTC instant and must not carry the camera's error. Doing it the other
    // way (moving only the photo, or only the match) would tag the right place
    // with the wrong time, which is harder to notice than a miss.
    let tz_delta_seconds = i64::from(options.tz_offset_minutes) * 60;
    let mut matching_track = track;
    if tz_delta_seconds != 0 {
        shift_track_seconds(&mut matching_track, tz_delta_seconds);
    }

    let geotag_options = GeotagOptions {
        max_gap_seconds: options.max_gap,
        ..GeotagOptions::default()
    };
    // One path for both cases: an all-photos, zero-second shift is exactly
    // what `plan` does.
    let mut plans = geotag::plan_with_time_shift(
        &photo_paths,
        &matching_track,
        &geotag_options,
        shift,
        &scope,
    )?;
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
    if let Some(shift_seconds) = shift.total_seconds().filter(|&seconds| seconds != 0) {
        session.console.result(&format!(
            "{:<8}: {shift_seconds} s ({})",
            "shift",
            options.scope.label()
        ));
    }
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
    let clamped = (i64::from(utc) + delta_seconds).clamp(0, i64::from(u32::MAX));
    // The conversion cannot fail — `clamped` is inside `0..=u32::MAX` by
    // construction — so `unwrap_or(utc)` used to stand here as a silent
    // fallback that nothing could reach. The bit-cast says the same thing
    // without an unreachable branch: the clamp above is the only guard, and
    // `shifting_seconds_clamps_at_the_ends_of_the_range` in `cal` pins it.
    u32::try_from(clamped).unwrap_or_else(|_| {
        debug_assert!(false, "the clamp above keeps the value in the u32 range");
        utc
    })
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
        "shift_seconds": options.shift().total_seconds(),
        "shift_scope": options.scope.label(),
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
fn warn_about_damage(
    console: &dyn Console,
    summary: &LogSummary,
    report: &ParseReport,
    source: Option<&Path>,
) {
    let source = source.map_or_else(String::new, |path| format!("{}: ", path.display()));
    if summary.damaged_block_count > 0 {
        console.warn(&format!(
            "{source}{} of {} blocks failed their header checksum; their records are still exported",
            summary.damaged_block_count, report.blocks
        ));
    }
    if report.truncated_records > 0 {
        console.warn(&format!(
            "{source}{} records were cut short (the log was probably still being written); they \
             were skipped",
            report.truncated_records
        ));
    }
    if report.bad_checksums > 0 {
        console.warn(&format!(
            "{source}{} records failed their per-record checksum; they are exported and marked",
            report.bad_checksums
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Timings;
    use crate::link::PortEntry;
    use crate::testing::{
        AutoConfirm, DeviceHandle, FakePorts, RecordingConsole, Scratch, ScriptedLinks,
        ScriptedTransport, synthetic_dump,
    };
    use m241_core::Error;
    use m241_core::protocol::simulator::FakeDevice;
    use std::cell::Cell;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;

    fn write_dump(scratch: &Scratch, name: &str, points: usize) -> PathBuf {
        let path = scratch.join(name);
        fileio::write(&path, &synthetic_dump(points)).expect("write dump");
        path
    }

    /// `log geotag` options with every flag at its documented default, for the
    /// tests to override only the ones they are about.
    fn geotag_options(log: PathBuf, photos: Vec<PathBuf>) -> LogGeotagOptions {
        LogGeotagOptions {
            log,
            photos,
            max_gap: 300,
            utc_offset_days: OffsetSpec::Auto,
            tz_offset_minutes: 0,
            shift_days: 0,
            shift_hours: 0,
            shift_minutes: 0,
            shift_seconds: 0,
            scope: ShiftScopeArg::All,
            photo: Vec::new(),
            camera_make: None,
            camera_model: None,
            dry_run: false,
            json: false,
        }
    }

    /// A session whose `FakeDevice` replies land without waiting out a timeout.
    fn fast<'a>(
        links: &'a FakePorts,
        console: &'a RecordingConsole,
        confirm: &'a AutoConfirm,
    ) -> Session<'a> {
        Session::new(links, console, confirm).with_timings(Timings::generous())
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
            Session::new(&links, &console, &confirm).with_timings(Timings::fast()),
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
        // One day, expressed through the same conversion the shift uses rather
        // than a bare 86,400 whose unit a reader has to work out.
        let one_day_later = crate::cal::shift_seconds(1_700_000_000, 1);
        assert_eq!(one_day_later, 1_700_000_000 + 86_400, "one day is 86,400 s");
        assert_eq!(
            document["points"][0]["utc"], one_day_later,
            "one day was added"
        );
        assert_eq!(document["offset_days"], 1, "the JSON names the shift");
        assert!(
            console.stderr().contains("+1 days"),
            "got {}",
            console.stderr()
        );
    }

    #[test]
    fn read_json_without_a_shift_carries_no_offset_field() {
        // The field is added only when something was shifted, so a reader can
        // tell "no shift asked for" from "shifted by zero".
        let scratch = Scratch::new("read-json-no-offset");
        let path = write_dump(&scratch, "track.bin", 1);
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        read(
            &LogReadOptions {
                file: path,
                format: m241_core::ExportFormat::Json,
                output: None,
                utc_offset_days: OffsetSpec::Days(0),
            },
            fast(&links, &console, &confirm),
        )
        .expect("read");
        let document: serde_json::Value =
            serde_json::from_str(&console.stdout()).expect("one JSON object");
        assert!(document.get("offset_days").is_none(), "got {document}");
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

    /// Build a one-block dump whose records carry a position but no timestamp.
    ///
    /// The same fields as [`dump_with_timestamps`] minus bit 0 (UTC): the block
    /// mask is per block, so an untimed dump cannot be mixed into a timed one.
    fn dump_without_timestamps(records: usize) -> Vec<u8> {
        use m241_core::log::block::{
            BLOCK_HEADER_SIZE, BLOCK_SIZE, POLICY_LOG_ENABLED, xor_checksum,
        };

        const MASK: u32 = 0x8000_001C;
        const LAT: f32 = 35.681_236;
        const LON: f32 = 139.767_13;

        let mut block = vec![0xFFu8; BLOCK_SIZE];
        block[0..2].copy_from_slice(&(records as u16).to_le_bytes());
        block[2..6].copy_from_slice(&MASK.to_le_bytes());
        block[6..8].copy_from_slice(&POLICY_LOG_ENABLED.to_le_bytes());
        block[8..12].copy_from_slice(&10u32.to_le_bytes());

        let mut cursor = BLOCK_HEADER_SIZE;
        for _ in 0..records {
            let mut record = Vec::new();
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
        use m241_core::protocol::pmtk::line as pmtk_line;

        // A unit that reports a log length and then refuses to stop logging.
        let links = ScriptedLinks::new(|| {
            ScriptedTransport::new(|request, output| {
                if request.contains("PMTK182,2,8") {
                    output.extend(pmtk_line("PMTK182,3,8,100").as_bytes());
                } else if request.contains("PMTK182,5") {
                    // Status 2 = action failed / rejected by the device.
                    output.extend(pmtk_line("PMTK001,182,5,2").as_bytes());
                }
            })
        });

        let scratch = Scratch::new("download-rejected");
        let output = scratch.join("track.bin");
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

    /// A fake logger that answers the erase with `reply`, plus the backup path
    /// and a cell that says whether the backup existed when the erase arrived.
    fn erase_fixture(scratch: &Scratch, reply: &str) -> (DeviceHandle, Rc<Cell<bool>>, PathBuf) {
        let output = scratch.join("track.bin");
        let handle = DeviceHandle::new(|| FakeDevice::new().with_log(&synthetic_dump(3)))
            .with_erase_reply(reply);
        let saved_at_erase = Rc::new(Cell::new(false));
        let saved = Rc::clone(&saved_at_erase);
        let backup = output.clone();
        handle.on_erase(move || saved.set(backup.exists()));
        (handle, saved_at_erase, output)
    }

    fn erase_options(output: &Path) -> EraseOptions {
        EraseOptions {
            port: String::from("auto"),
            output: output.to_path_buf(),
            force: false,
            yes: true,
            json: false,
        }
    }

    #[test]
    fn erase_saves_the_log_then_sends_the_erase_sentence_once() {
        let scratch = Scratch::new("erase");
        let (handle, saved_at_erase, output) = erase_fixture(&scratch, "PMTK001,182,6,3");
        let links = FakePorts::shared(handle.clone());
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;

        erase(&erase_options(&output), fast(&links, &console, &confirm)).expect("erase");

        assert_eq!(
            fileio::read(&output).expect("backup"),
            synthetic_dump(3),
            "the saved bytes must be the dump the device sent"
        );
        assert!(
            saved_at_erase.get(),
            "the backup must be on disk before $PMTK182,6 is sent"
        );
        let commands = handle.commands();
        assert_eq!(
            commands
                .iter()
                .filter(|command| command.as_str() == "PMTK182,6,1")
                .count(),
            1,
            "exactly one erase sentence: {commands:?}"
        );
        let erase_at = commands
            .iter()
            .position(|command| command == "PMTK182,6,1")
            .expect("the erase was recorded");
        assert_eq!(
            commands.get(erase_at.saturating_sub(1)).map(String::as_str),
            Some("PMTK182,5"),
            "logging must be stopped immediately before the erase: {commands:?}"
        );
        assert_eq!(
            commands.get(erase_at + 1).map(String::as_str),
            Some("PMTK182,4"),
            "logging must be resumed immediately after the erase: {commands:?}"
        );
        assert!(
            handle.logging_enabled(),
            "a completed erase leaves the unit recording"
        );
        let stdout = console.stdout();
        assert!(stdout.contains("bytes"), "got {stdout}");
        assert!(stdout.contains("erased"), "got {stdout}");
    }

    #[test]
    fn erase_without_yes_saves_the_log_and_sends_no_erase() {
        let scratch = Scratch::new("erase-no-yes");
        let (handle, _, output) = erase_fixture(&scratch, "PMTK001,182,6,3");
        let links = FakePorts::shared(handle.clone());
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        let mut options = erase_options(&output);
        options.yes = false;

        let err = erase(&options, fast(&links, &console, &confirm)).expect_err("--yes is required");

        assert!(matches!(err, Error::Safety { .. }), "got {err}");
        assert_eq!(crate::exit::exit_code_for(&err), 5, "got {err}");
        assert_eq!(
            fileio::read(&output).expect("the backup is still written"),
            synthetic_dump(3)
        );
        // The fake device records a bare body when the checksum is right, so a
        // malformed sentence is not hidden behind the prefix check.
        assert!(
            !handle.commands().iter().any(
                |command| command.starts_with("PMTK182,6") || command.starts_with("$PMTK182,6")
            ),
            "no erase may be sent without --yes: {:?}",
            handle.commands()
        );
        assert!(format!("{err}").contains("--yes"), "got {err}");
        assert!(handle.logging_enabled());
    }

    #[test]
    fn erase_sends_the_checksummed_sentence_and_leaves_logging_on() {
        // The device-level test in `m241-flash` proves the bytes `LogEraser`
        // writes; this one proves they survive the CLI's own transport, which
        // is where a wrong checksum would be dropped by the fake firmware.
        let scratch = Scratch::new("erase-sentence");
        let (handle, _, output) = erase_fixture(&scratch, "PMTK001,182,6,3");
        let links = FakePorts::shared(handle.clone());
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;

        erase(&erase_options(&output), fast(&links, &console, &confirm)).expect("erase");

        let commands = handle.commands();
        assert!(
            commands.iter().any(|command| command == "PMTK182,6,1"),
            "the erase must be recognised as the checksummed $PMTK182,6,1*3E: {commands:?}"
        );
        assert!(
            !commands
                .iter()
                .any(|command| command.contains("$PMTK182,6") || command.contains("*3E")),
            "a raw sentence in the recorded body means the line was not parsed and framed, so \
             the checksum was not checked: {commands:?}"
        );
        assert!(
            handle.logging_enabled(),
            "logging must be on again once the erase finished"
        );
    }

    #[test]
    fn a_timed_out_erase_still_resumes_logging() {
        // The erase itself fails; the resume that follows it must still run, or
        // the unit is left silently not recording.
        let scratch = Scratch::new("erase-timeout-resume");
        let (handle, _, output) = erase_fixture(&scratch, "PMTK001,182,6,3");
        handle.set_erase_reply(None);
        let links = FakePorts::shared(handle.clone());
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        let mut options = erase_options(&output);
        // Set before the erase: assigning it afterwards would make the
        // "no success object on failure" assertion pass without exercising the
        // JSON branch at all.
        options.json = true;
        let timings = Timings {
            reply: std::time::Duration::from_millis(50),
            stream: std::time::Duration::from_millis(1),
            entry: m241_core::protocol::pmtk::EntryTiming::fast(),
            erase: std::time::Duration::from_millis(20),
        };

        let err = erase(
            &options,
            Session::new(&links, &console, &confirm).with_timings(timings),
        )
        .expect_err("a silent erase must time out");

        assert!(matches!(err, Error::Timeout { .. }), "got {err}");
        let commands = handle.commands();
        assert!(
            commands.iter().any(|command| command == "PMTK182,4"),
            "the resume must be sent after a failed erase: {commands:?}"
        );
        assert!(
            handle.logging_enabled(),
            "logging must be on again after a timed-out erase"
        );
        assert!(
            console.stdout().is_empty(),
            "a failed erase must not print a success object: {}",
            console.stdout()
        );
    }

    #[test]
    fn erase_resumes_logging_when_the_stop_ack_is_lost() {
        use m241_core::protocol::pmtk::line as pmtk_line;

        // The `$PMTK182,5` sent immediately before the erase reaches the unit,
        // but its acknowledgement is lost. The unit is now not recording, so the
        // resume must still be sent — and the erase must not be, because the
        // client cannot tell whether the stop took effect.
        let stops = Rc::new(Cell::new(0u32));
        let resumed_after_lost_stop = Rc::new(Cell::new(false));
        let erased = Rc::new(Cell::new(false));
        let links = ScriptedLinks::new({
            let stops = Rc::clone(&stops);
            let resumed_after_lost_stop = Rc::clone(&resumed_after_lost_stop);
            let erased = Rc::clone(&erased);
            move || {
                let stops = Rc::clone(&stops);
                let resumed_after_lost_stop = Rc::clone(&resumed_after_lost_stop);
                let erased = Rc::clone(&erased);
                ScriptedTransport::new(move |request, output| {
                    if request.contains("PMTK182,2,8") {
                        output.extend(pmtk_line("PMTK182,3,8,10").as_bytes());
                    } else if request.contains("PMTK182,5") {
                        let count = stops.get() + 1;
                        stops.set(count);
                        // The first stop (the transfer's) is answered; the one
                        // before the erase is seen by the unit but its reply is
                        // lost.
                        if count == 1 {
                            output.extend(pmtk_line("PMTK001,182,5,3").as_bytes());
                        }
                    } else if request.contains("PMTK182,7,") {
                        let reply = format!("PMTK182,8,0,{}", "00".repeat(0x10));
                        output.extend(pmtk_line(&reply).as_bytes());
                    } else if request.contains("PMTK182,6,1") {
                        erased.set(true);
                        output.extend(pmtk_line("PMTK001,182,6,3").as_bytes());
                    } else if request.contains("PMTK182,4") {
                        if stops.get() == 2 {
                            resumed_after_lost_stop.set(true);
                        }
                        output.extend(pmtk_line("PMTK001,182,4,3").as_bytes());
                    }
                })
            }
        });

        let scratch = Scratch::new("erase-lost-stop");
        let output = scratch.join("track.bin");
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;

        let err = erase(
            &erase_options(&output),
            Session::new(&links, &console, &confirm).with_timings(Timings::fast()),
        )
        .expect_err("the stop acknowledgement was lost");

        assert!(matches!(err, Error::Timeout { .. }), "got {err}");
        assert!(
            format!("{err}").contains("PMTK182,5"),
            "the error must name the stop: {err}"
        );
        assert!(
            resumed_after_lost_stop.get(),
            "log_resume() must still be sent after a lost stop acknowledgement"
        );
        assert!(
            !erased.get(),
            "the erase must not be sent when the stop was not confirmed"
        );
        assert_eq!(
            fileio::read(&output)
                .expect("the backup is still saved")
                .len(),
            0x10,
            "the backup is written before the stop"
        );
    }

    #[test]
    fn a_failed_backup_write_sends_no_erase_or_stop() {
        // The output path is the scratch directory itself, so the write fails
        // with EISDIR after a successful download. Nothing destructive may be
        // sent: the erase would destroy the only copy the user has.
        let scratch = Scratch::new("erase-write-fails");
        let handle = DeviceHandle::new(|| FakeDevice::new().with_log(&synthetic_dump(3)))
            .with_erase_reply("PMTK001,182,6,3");
        let links = FakePorts::shared(handle.clone());
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        let mut options = erase_options(scratch.path());
        options.force = true;

        let err = erase(&options, fast(&links, &console, &confirm))
            .expect_err("writing over a directory must fail");

        assert!(matches!(err, Error::Io { .. }), "got {err}");
        let commands = handle.commands();
        assert!(
            !commands
                .iter()
                .any(|command| command.starts_with("PMTK182,6")),
            "no erase may follow a failed backup write: {commands:?}"
        );
        assert_eq!(
            commands
                .iter()
                .filter(|command| *command == "PMTK182,5")
                .count(),
            1,
            "only the download's own stop may have been sent: {commands:?}"
        );
        assert!(
            handle.logging_enabled(),
            "the download's resume must still leave the unit recording"
        );
    }

    #[test]
    fn erase_refuses_an_existing_output_before_opening_a_port() {
        let scratch = Scratch::new("erase-clobber");
        let output = scratch.join("track.bin");
        fileio::write(&output, b"previous").expect("write");
        // No device at all: a handler that opened the port first would fail
        // with Error::Port instead of the safety refusal under test.
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;

        let err = erase(&erase_options(&output), fast(&links, &console, &confirm))
            .expect_err("must refuse to overwrite");

        assert!(matches!(err, Error::Safety { .. }), "got {err}");
        assert_eq!(crate::exit::exit_code_for(&err), 5, "got {err}");
        assert_eq!(fileio::read(&output).expect("still there"), b"previous");
    }

    #[test]
    fn a_failed_erase_keeps_the_saved_backup() {
        let scratch = Scratch::new("erase-refused");
        // Status 0 is a refusal: the device answered, but did not erase.
        let (handle, saved_at_erase, output) = erase_fixture(&scratch, "PMTK001,182,6,0");
        let links = FakePorts::shared(handle.clone());
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;

        let err = erase(&erase_options(&output), fast(&links, &console, &confirm))
            .expect_err("the device refuses the erase");

        assert!(matches!(err, Error::Rejected { .. }), "got {err}");
        assert!(saved_at_erase.get(), "the backup was written first");
        assert_eq!(
            fileio::read(&output).expect("the backup survives the failure"),
            synthetic_dump(3)
        );
        assert_eq!(
            handle
                .commands()
                .iter()
                .filter(|command| command.as_str() == "PMTK182,6,1")
                .count(),
            1
        );
        assert!(
            handle.logging_enabled(),
            "logging must be resumed even when the erase is refused"
        );
    }

    #[test]
    fn erase_json_is_one_object_saying_the_log_is_gone() {
        let scratch = Scratch::new("erase-json");
        let (handle, _, output) = erase_fixture(&scratch, "PMTK001,182,6,3");
        let links = FakePorts::shared(handle);
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        let mut options = erase_options(&output);
        options.json = true;

        erase(&options, fast(&links, &console, &confirm)).expect("erase --json");

        let document: serde_json::Value =
            serde_json::from_str(&console.stdout()).expect("one JSON object");
        assert_eq!(document["port"], crate::testing::FAKE_PORT);
        assert_eq!(document["output"], output.display().to_string());
        assert_eq!(document["bytes"], synthetic_dump(3).len());
        assert_eq!(document["erased"], true);
    }

    #[test]
    fn erase_refuses_an_empty_log_without_writing_or_sending_erase() {
        let scratch = Scratch::new("erase-empty");
        let output = scratch.join("track.bin");
        let handle = DeviceHandle::new(|| FakeDevice::new().with_log(&[]))
            .with_erase_reply("PMTK001,182,6,3");
        let links = FakePorts::shared(handle.clone());
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;

        let err = erase(&erase_options(&output), fast(&links, &console, &confirm))
            .expect_err("an empty log has nothing to back up or erase");

        assert!(matches!(err, Error::Safety { .. }), "got {err}");
        assert_eq!(crate::exit::exit_code_for(&err), 5, "got {err}");
        assert!(format!("{err}").contains("0 bytes"), "got {err}");
        assert!(!output.exists(), "no zero-byte file may be written");
        let commands = handle.commands();
        assert!(
            !commands.iter().any(|command| matches!(
                command.as_str(),
                "PMTK182,5" | "PMTK182,6,1" | "PMTK182,4"
            )),
            "no stop/erase/resume may be sent for an empty log: {commands:?}"
        );
    }

    #[test]
    fn erase_uses_timings_erase_not_timings_reply_for_erase_timeout() {
        let scratch = Scratch::new("erase-timeout-budget");
        let output = scratch.join("track.bin");
        // No `with_erase_reply`: the erase sentence is left silent while the
        // surrounding PMTK182 transfer/stop/resume steps answer immediately.
        let handle = DeviceHandle::new(|| FakeDevice::new().with_log(&synthetic_dump(1)));
        let links = FakePorts::shared(handle.clone());
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        let timings = Timings {
            reply: std::time::Duration::from_secs(1),
            stream: std::time::Duration::from_millis(1),
            entry: m241_core::protocol::pmtk::EntryTiming::fast(),
            erase: std::time::Duration::from_millis(1),
        };

        let err = erase(
            &erase_options(&output),
            Session::new(&links, &console, &confirm).with_timings(timings),
        )
        .expect_err("silent erase must time out");

        match err {
            Error::Timeout { timeout, .. } => {
                assert_eq!(timeout.as_std(), timings.erase);
                assert_ne!(timeout.as_std(), timings.reply);
            }
            other => panic!("expected Error::Timeout, got {other}"),
        }
        assert!(
            handle.logging_enabled(),
            "logging must be resumed after an erase timeout"
        );
    }

    #[test]
    fn erase_preserves_its_error_when_log_resume_also_fails() {
        use m241_core::protocol::pmtk::line as pmtk_line;

        let saw_erase = Rc::new(Cell::new(false));
        let resumed_after_erase = Rc::new(Cell::new(false));
        let links = ScriptedLinks::new({
            let saw_erase = Rc::clone(&saw_erase);
            let resumed_after_erase = Rc::clone(&resumed_after_erase);
            move || {
                let saw_erase = Rc::clone(&saw_erase);
                let resumed_after_erase = Rc::clone(&resumed_after_erase);
                ScriptedTransport::new(move |request, output| {
                    if request.contains("PMTK182,2,8") {
                        output.extend(pmtk_line("PMTK182,3,8,10").as_bytes());
                    } else if request.contains("PMTK182,5") {
                        output.extend(pmtk_line("PMTK001,182,5,3").as_bytes());
                    } else if request.contains("PMTK182,7,") {
                        let reply = format!("PMTK182,8,0,{}", "00".repeat(0x10));
                        output.extend(pmtk_line(&reply).as_bytes());
                    } else if request.contains("PMTK182,6,1") {
                        saw_erase.set(true);
                        // Status 0 = refusal from the device.
                        output.extend(pmtk_line("PMTK001,182,6,0").as_bytes());
                    } else if request.contains("PMTK182,4") {
                        if saw_erase.get() {
                            // Leave the post-erase resume silent so `log_resume()` times out.
                            resumed_after_erase.set(true);
                        } else {
                            output.extend(pmtk_line("PMTK001,182,4,3").as_bytes());
                        }
                    }
                })
            }
        });

        let scratch = Scratch::new("erase-resume-also-fails");
        let output = scratch.join("track.bin");
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;

        let err = erase(
            &erase_options(&output),
            Session::new(&links, &console, &confirm).with_timings(Timings::fast()),
        )
        .expect_err("erase was refused");

        assert!(
            resumed_after_erase.get(),
            "log_resume() must still be attempted after a failed erase"
        );
        assert!(
            matches!(err, Error::Rejected { .. }),
            "erase error must not be masked by resume timeout: got {err}"
        );
        assert!(
            format!("{err}").contains("PMTK182,6"),
            "returned error must describe the erase refusal: got {err}"
        );
        assert!(
            format!("{err}").contains("could not be resumed")
                && format!("{err}").contains("PMTK182,4"),
            "a failed resume must be reported beside the erase error: got {err}"
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

    /// Build a JPEG whose IFD0 names `make` and `model` and whose Exif sub-IFD
    /// carries `DateTimeOriginal`, laid out the way a camera writes it: IFD0
    /// entries in ascending tag order and the strings that do not fit inline in
    /// a value area after the last IFD.
    fn synthetic_jpeg_with_camera(datetime: &str, make: &str, model: &str) -> Vec<u8> {
        const TYPE_ASCII: u16 = 2;
        const TYPE_LONG: u16 = 4;
        const TAG_MAKE: u16 = 0x010F;
        const TAG_MODEL: u16 = 0x0110;
        const TAG_EXIF_POINTER: u16 = 0x8769;
        const TAG_DATE_TIME_ORIGINAL: u16 = 0x9003;

        fn ascii(text: &str) -> Vec<u8> {
            let mut bytes = text.as_bytes().to_vec();
            bytes.push(0);
            bytes
        }

        fn push_entry(out: &mut Vec<u8>, tag: u16, field_type: u16, count: u32, field: u32) {
            out.extend_from_slice(&tag.to_le_bytes());
            out.extend_from_slice(&field_type.to_le_bytes());
            out.extend_from_slice(&count.to_le_bytes());
            out.extend_from_slice(&field.to_le_bytes());
        }

        let datetime_bytes = ascii(datetime);
        let make_bytes = ascii(make);
        let model_bytes = ascii(model);

        // Header (8) + IFD0 (3 entries) + Exif IFD (1 entry), then a 4-aligned
        // value area holding the three strings.
        let ifd0_offset = 8usize;
        let exif_ifd_offset = ifd0_offset + 2 + 12 * 3 + 4;
        let exif_ifd_size = 2 + 12 + 4;
        let value_area = (exif_ifd_offset + exif_ifd_size + 3) & !3;
        let datetime_at = value_area;
        let make_at = datetime_at + datetime_bytes.len();
        let model_at = make_at + make_bytes.len();

        let mut tiff = Vec::new();
        tiff.extend_from_slice(b"II");
        tiff.extend_from_slice(&42u16.to_le_bytes());
        tiff.extend_from_slice(&(ifd0_offset as u32).to_le_bytes());

        tiff.extend_from_slice(&3u16.to_le_bytes());
        push_entry(
            &mut tiff,
            TAG_MAKE,
            TYPE_ASCII,
            make_bytes.len() as u32,
            make_at as u32,
        );
        push_entry(
            &mut tiff,
            TAG_MODEL,
            TYPE_ASCII,
            model_bytes.len() as u32,
            model_at as u32,
        );
        push_entry(
            &mut tiff,
            TAG_EXIF_POINTER,
            TYPE_LONG,
            1,
            exif_ifd_offset as u32,
        );
        tiff.extend_from_slice(&0u32.to_le_bytes());
        assert_eq!(
            tiff.len(),
            exif_ifd_offset,
            "the IFD0 offset was miscounted"
        );

        tiff.extend_from_slice(&1u16.to_le_bytes());
        push_entry(
            &mut tiff,
            TAG_DATE_TIME_ORIGINAL,
            TYPE_ASCII,
            datetime_bytes.len() as u32,
            datetime_at as u32,
        );
        tiff.extend_from_slice(&0u32.to_le_bytes());
        while tiff.len() < value_area {
            tiff.push(0);
        }
        tiff.extend_from_slice(&datetime_bytes);
        tiff.extend_from_slice(&make_bytes);
        tiff.extend_from_slice(&model_bytes);

        let mut app1 = Vec::from(b"Exif\0\0".as_slice());
        app1.extend_from_slice(&tiff);
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE1];
        jpeg.extend_from_slice(&((app1.len() + 2) as u16).to_be_bytes());
        jpeg.extend_from_slice(&app1);
        jpeg.extend_from_slice(&[0xFF, 0xD9]);
        jpeg
    }

    /// The `DateTimeOriginal` a photo file carries, as EXIF's `YYYY:MM:DD HH:MM:SS`.
    fn photo_datetime(path: &Path) -> String {
        let bytes = fileio::read(path).expect("photo bytes");
        let utc = m241_core::log::exif::read_datetime_original(&bytes)
            .expect("parse exif")
            .expect("a capture time");
        format_exif_utc(utc)
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
                dry_run: true,
                json: true,
                ..geotag_options(log_path.clone(), vec![photo_path.clone()])
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
            &geotag_options(log_path, vec![photo_path.clone()]),
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
                utc_offset_days: OffsetSpec::Days(0),
                tz_offset_minutes: 540,
                json: true,
                ..geotag_options(log_path, vec![photos_dir])
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

    #[test]
    fn a_selected_scope_shift_moves_only_the_named_photos_clock() {
        let scratch = Scratch::new("geotag-selected");
        let track_utc = 1_700_000_000u32;
        // The chosen photo is moved onto the second track point, so both
        // photos still find a point inside the gap after the shift.
        let shifted_utc = track_utc + 3_600;
        let log_path = scratch.join("track.bin");
        fileio::write(&log_path, &dump_with_timestamps(&[track_utc, shifted_utc]))
            .expect("write dump");

        let chosen = scratch.join("chosen.jpg");
        let chosen_bytes = synthetic_jpeg_with_datetime(&format_exif_utc(track_utc));
        fileio::write(&chosen, &chosen_bytes).expect("write chosen photo");
        let other = scratch.join("other.jpg");
        let other_bytes = synthetic_jpeg_with_datetime(&format_exif_utc(track_utc));
        fileio::write(&other, &other_bytes).expect("write other photo");

        let links = FakePorts::empty();
        let confirm = AutoConfirm;

        // The dry run reports the shifted capture time and writes nothing.
        let dry_console = RecordingConsole::default();
        geotag(
            &LogGeotagOptions {
                scope: ShiftScopeArg::Selected,
                photo: vec![chosen.clone()],
                shift_hours: 1,
                dry_run: true,
                json: true,
                ..geotag_options(log_path.clone(), vec![chosen.clone(), other.clone()])
            },
            Session::new(&links, &dry_console, &confirm),
        )
        .expect("dry run of a selected shift");
        assert_eq!(
            fileio::read(&chosen).expect("chosen photo"),
            chosen_bytes,
            "a dry run must not touch the selected photo"
        );
        assert_eq!(
            fileio::read(&other).expect("other photo"),
            other_bytes,
            "a dry run must not touch an out-of-scope photo either"
        );
        let dry_doc: serde_json::Value =
            serde_json::from_str(&dry_console.stdout()).expect("one JSON object");
        assert_eq!(dry_doc["shift_seconds"], 3_600);
        assert_eq!(dry_doc["shift_scope"], "selected");
        assert_eq!(dry_doc["plans"][0]["capture_utc"], shifted_utc);
        assert_eq!(dry_doc["plans"][1]["capture_utc"], track_utc);

        // The real run writes the shifted clock only into the selected photo,
        // and still tags both photos at a position.
        let console = RecordingConsole::default();
        geotag(
            &LogGeotagOptions {
                scope: ShiftScopeArg::Selected,
                photo: vec![chosen.clone()],
                shift_hours: 1,
                json: true,
                ..geotag_options(log_path, vec![chosen.clone(), other.clone()])
            },
            Session::new(&links, &console, &confirm),
        )
        .expect("a selected shift");
        assert_eq!(photo_datetime(&chosen), format_exif_utc(shifted_utc));
        assert_eq!(
            photo_datetime(&other),
            format_exif_utc(track_utc),
            "a photo outside the scope keeps its clock"
        );
        for photo in [&chosen, &other] {
            let gps = m241_core::log::exif::read_gps(&fileio::read(photo).expect("photo"))
                .expect("parse exif")
                .expect("gps tags written");
            assert!(
                gps.latitude.is_some(),
                "{} was dropped instead of tagged",
                photo.display()
            );
        }
        let doc: serde_json::Value =
            serde_json::from_str(&console.stdout()).expect("one JSON object");
        assert_eq!(doc["written"], 2);
        assert_eq!(doc["skipped"], 0);
    }

    #[test]
    fn a_by_camera_scope_shift_moves_only_photos_from_that_camera() {
        let scratch = Scratch::new("geotag-by-camera");
        let track_utc = 1_700_000_000u32;
        let shifted_utc = track_utc + 3_600;
        let log_path = scratch.join("track.bin");
        fileio::write(&log_path, &dump_with_timestamps(&[track_utc, shifted_utc]))
            .expect("write dump");

        let holux = scratch.join("holux.jpg");
        fileio::write(
            &holux,
            &synthetic_jpeg_with_camera(&format_exif_utc(track_utc), "HOLUX", "M-241"),
        )
        .expect("write HOLUX photo");
        let phone = scratch.join("phone.jpg");
        fileio::write(
            &phone,
            &synthetic_jpeg_with_camera(&format_exif_utc(track_utc), "Other", "Phone"),
        )
        .expect("write phone photo");

        let links = FakePorts::empty();
        let confirm = AutoConfirm;
        let console = RecordingConsole::default();
        geotag(
            &LogGeotagOptions {
                scope: ShiftScopeArg::ByCamera,
                camera_make: Some(String::from("HOLUX")),
                camera_model: Some(String::from("M-241")),
                shift_seconds: 3_600,
                json: true,
                ..geotag_options(log_path, vec![holux.clone(), phone.clone()])
            },
            Session::new(&links, &console, &confirm),
        )
        .expect("a by-camera shift");

        assert_eq!(photo_datetime(&holux), format_exif_utc(shifted_utc));
        assert_eq!(
            photo_datetime(&phone),
            format_exif_utc(track_utc),
            "a photo from another camera keeps its clock"
        );
        let doc: serde_json::Value =
            serde_json::from_str(&console.stdout()).expect("one JSON object");
        assert_eq!(doc["shift_scope"], "by-camera");
        assert_eq!(doc["written"], 2);
        assert_eq!(doc["skipped"], 0);
    }

    /// The UTC values of a JSON export, in file order, with `None` for a point
    /// the dump recorded without a timestamp.
    fn exported_utc_options(path: &Path) -> Vec<Option<u32>> {
        let text = fileio::read_text(path).expect("read export");
        let document: serde_json::Value = serde_json::from_str(&text).expect("JSON export");
        document["points"]
            .as_array()
            .expect("points array")
            .iter()
            .map(|point| {
                point["utc"]
                    .as_u64()
                    .map(|utc| u32::try_from(utc).expect("u32"))
            })
            .collect()
    }

    /// The UTC values of a JSON export whose every point is timed, in file order.
    fn exported_utcs(path: &Path) -> Vec<u32> {
        exported_utc_options(path)
            .into_iter()
            .map(|utc| utc.expect("every point in this export has a timestamp"))
            .collect()
    }

    /// A JPEG whose frame header claims `width` x `height` and whose Exif holds
    /// `capture_utc`, but which has no scan data, so measuring it works and
    /// decoding it cannot.
    fn jpeg_with_frame_size(width: u16, height: u16, capture_utc: u32) -> Vec<u8> {
        let mut jpeg = synthetic_jpeg_with_datetime(&format_exif_utc(capture_utc));
        // Drop the EOI the helper ends with and put a SOF0 in front of it.
        jpeg.truncate(jpeg.len() - 2);
        jpeg.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x0C, 0x08]);
        jpeg.extend_from_slice(&height.to_be_bytes());
        jpeg.extend_from_slice(&width.to_be_bytes());
        jpeg.extend_from_slice(&[0x01, 0x01, 0x11, 0x00]);
        jpeg.extend_from_slice(&[0xFF, 0xD9]);
        jpeg
    }

    fn kmz_options(log: PathBuf, photos: Vec<PathBuf>, output: &Path) -> LogKmzOptions {
        LogKmzOptions {
            log,
            photos,
            output: output.to_path_buf(),
            max_photo_edge: None,
            max_gap: m241_core::log::geotag::DEFAULT_MAX_GAP_SECONDS,
            utc_offset_days: OffsetSpec::Days(0),
            tz_offset_minutes: 0,
        }
    }

    #[test]
    fn kmz_bundles_the_track_and_a_photo_whose_capture_time_matches() {
        let scratch = Scratch::new("kmz");
        let log = write_dump(&scratch, "track.bin", 2);
        let photo = scratch.join("shot.jpg");
        fileio::write(
            &photo,
            &synthetic_jpeg_with_datetime(&format_exif_utc(1_700_000_000)),
        )
        .expect("write photo");
        let output = scratch.join("track.kmz");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;

        kmz(
            &kmz_options(log, vec![photo], &output),
            fast(&links, &console, &confirm),
        )
        .expect("kmz");

        let archive = fileio::read(&output).expect("archive");
        assert!(
            archive.starts_with(b"PK\x03\x04"),
            "the output must be a ZIP archive"
        );
        for name in [&b"doc.kml"[..], &b"photo/shot.jpg"[..]] {
            assert!(
                archive.windows(name.len()).any(|window| window == name),
                "the archive must carry {:?}",
                String::from_utf8_lossy(name)
            );
        }
        assert!(
            console.stderr().contains("1 photo"),
            "the note must say what was bundled: {}",
            console.stderr()
        );
    }

    #[test]
    fn kmz_leaves_out_a_photo_with_no_track_point_inside_the_gap() {
        let scratch = Scratch::new("kmz-gap");
        let log = write_dump(&scratch, "track.bin", 2);
        let photo = scratch.join("late.jpg");
        // The track is at 2023-11-14T22:13:20Z; this photo claims 2020.
        fileio::write(
            &photo,
            &synthetic_jpeg_with_datetime(&format_exif_utc(1_600_000_000)),
        )
        .expect("write photo");
        let output = scratch.join("track.kmz");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;

        kmz(
            &kmz_options(log, vec![photo], &output),
            fast(&links, &console, &confirm),
        )
        .expect("kmz");

        let archive = fileio::read(&output).expect("archive");
        assert!(
            !archive
                .windows(b"photo/".len())
                .any(|window| window == b"photo/"),
            "a photo with no point in the gap must not be bundled"
        );
        assert!(
            console.stderr().contains("not bundled"),
            "the skip must be said on stderr: {}",
            console.stderr()
        );
    }

    #[test]
    fn kmz_reports_the_planners_reason_for_a_photo_it_leaves_out() {
        // A non-JPEG is skipped long before any track point is looked at, so the
        // warning must say that instead of blaming the gap: "no track point
        // within N s" would send the user to `--max-gap`, which cannot help.
        let scratch = Scratch::new("kmz-reason");
        let log = write_dump(&scratch, "track.bin", 2);
        let photo = scratch.join("notes.png");
        fileio::write(&photo, b"\x89PNG\r\n\x1a\n").expect("write png");
        let output = scratch.join("track.kmz");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;

        kmz(
            &kmz_options(log, vec![photo], &output),
            fast(&links, &console, &confirm),
        )
        .expect("kmz");

        let stderr = console.stderr();
        assert!(
            stderr.contains("not a JPEG"),
            "the warning must carry the planner's reason: {stderr}"
        );
        assert!(
            !stderr.contains("no track point within"),
            "the gap must not be blamed for a non-JPEG: {stderr}"
        );
    }

    #[test]
    fn kmz_applies_the_utc_offset_before_matching_photos() {
        // A log from an unpatched unit is 7168 days behind the photo's real
        // capture time. The match can only be found when the shift is applied
        // before `geotag::plan`, not to the track the archive is built from
        // afterwards.
        let scratch = Scratch::new("kmz-offset");
        let stale_utc = (crate::cal::days_from_civil(2006, 9, 17) * 86_400 + 3600) as u32;
        let real_utc = crate::cal::shift_seconds(stale_utc, crate::cal::WNRO_DAYS);
        let log = scratch.join("stale.bin");
        fileio::write(&log, &dump_with_timestamps(&[stale_utc])).expect("write dump");
        let photo = scratch.join("shot.jpg");
        fileio::write(
            &photo,
            &synthetic_jpeg_with_datetime(&format_exif_utc(real_utc)),
        )
        .expect("write photo");
        let output = scratch.join("track.kmz");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        let mut options = kmz_options(log, vec![photo], &output);
        options.utc_offset_days = OffsetSpec::Auto;

        kmz(&options, fast(&links, &console, &confirm)).expect("kmz");

        let archive = fileio::read(&output).expect("archive");
        assert!(
            archive
                .windows(b"photo/shot.jpg".len())
                .any(|window| window == b"photo/shot.jpg"),
            "the photo must match the shifted track"
        );
        assert!(
            console.stderr().contains("1 photo"),
            "the note must say the photo was bundled: {}",
            console.stderr()
        );
    }

    #[test]
    fn kmz_matches_a_photo_from_a_local_time_camera_with_the_tz_offset() {
        // `DateTimeOriginal` is the camera's local clock. A camera set to UTC+9
        // records a capture nine hours after the real moment, so matching only
        // works when the track is moved into the camera's clock first.
        let scratch = Scratch::new("kmz-tz");
        let track_utc = 1_700_000_000u32;
        let log = write_dump(&scratch, "track.bin", 2);
        let photo = scratch.join("local.jpg");
        let camera_utc = track_utc + 540 * 60;
        fileio::write(
            &photo,
            &synthetic_jpeg_with_datetime(&format_exif_utc(camera_utc)),
        )
        .expect("write photo");
        let output = scratch.join("track.kmz");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        let mut options = kmz_options(log, vec![photo], &output);
        options.tz_offset_minutes = 540;

        kmz(&options, fast(&links, &console, &confirm)).expect("kmz");

        let archive = fileio::read(&output).expect("archive");
        assert!(
            archive
                .windows(b"photo/local.jpg".len())
                .any(|window| window == b"photo/local.jpg"),
            "the photo must match the camera-clock track"
        );
        assert!(
            console.stderr().contains("1 photo"),
            "the note must say the photo was bundled: {}",
            console.stderr()
        );
    }

    #[test]
    fn kmz_bundles_a_track_when_no_photos_are_given() {
        let scratch = Scratch::new("kmz-track-only");
        let log = write_dump(&scratch, "track.bin", 2);
        let output = scratch.join("track.kmz");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;

        kmz(
            &kmz_options(log, Vec::new(), &output),
            fast(&links, &console, &confirm),
        )
        .expect("kmz without photos");

        let archive = fileio::read(&output).expect("archive");
        assert!(
            archive
                .windows(b"doc.kml".len())
                .any(|window| window == b"doc.kml"),
            "the track-only archive must still carry the KML"
        );
        assert!(
            !archive
                .windows(b"photo/".len())
                .any(|window| window == b"photo/"),
            "no photos means no photo entries"
        );
        assert!(
            console.stderr().contains("0 photos"),
            "the note must say no photo was bundled: {}",
            console.stderr()
        );
    }

    #[test]
    fn kmz_keeps_a_photo_within_the_edge_limit_byte_for_byte() {
        // A frame header within the limit is never decoded, so bytes with no
        // scan data pass through untouched; that is only true when the limit is
        // compared rather than applied.
        let scratch = Scratch::new("kmz-edge-keep");
        let log = write_dump(&scratch, "track.bin", 2);
        let photo = scratch.join("small.jpg");
        let bytes = jpeg_with_frame_size(800, 600, 1_700_000_000);
        fileio::write(&photo, &bytes).expect("write photo");
        let output = scratch.join("track.kmz");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        let mut options = kmz_options(log, vec![photo], &output);
        options.max_photo_edge = Some(1024);

        kmz(&options, fast(&links, &console, &confirm)).expect("kmz");

        let archive = fileio::read(&output).expect("archive");
        assert!(
            archive.windows(bytes.len()).any(|window| window == bytes),
            "a photo under the limit must keep its exact bytes"
        );
    }

    #[test]
    fn the_photo_edge_limit_reaches_the_kmz_exporter() {
        // 4000x3000 with no scan data: the only way this command could succeed
        // is to ignore the limit. It must instead report the decode failure,
        // which is what proves `--max-photo-edge` arrived at the exporter.
        let scratch = Scratch::new("kmz-edge");
        let log = write_dump(&scratch, "track.bin", 2);
        let photo = scratch.join("big.jpg");
        fileio::write(&photo, &jpeg_with_frame_size(4000, 3000, 1_700_000_000))
            .expect("write photo");
        let output = scratch.join("track.kmz");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        let mut options = kmz_options(log, vec![photo], &output);
        options.max_photo_edge = Some(1024);

        let err = kmz(&options, fast(&links, &console, &confirm)).expect_err("undecodable");
        let message = err.to_string();
        assert!(
            message.contains("could not be decoded") && message.contains("1024"),
            "the limit must be the one that was asked for: {message}"
        );
    }

    fn edit_options(output: &Path) -> LogEditOptions {
        LogEditOptions {
            log: PathBuf::new(),
            split_at: None,
            merge: Vec::new(),
            delete: Vec::new(),
            format: ExportFormat::Json,
            output: output.to_path_buf(),
        }
    }

    #[test]
    fn edit_split_writes_the_two_halves_to_their_own_files() {
        let scratch = Scratch::new("edit-split");
        let log = write_dump(&scratch, "track.bin", 4);
        let output = scratch.join("first.json");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        let mut options = edit_options(&output);
        options.log = log;
        options.split_at = Some(2);

        edit(&options, fast(&links, &console, &confirm)).expect("edit split");

        assert_eq!(
            exported_utcs(&output),
            vec![1_700_000_000, 1_700_000_001],
            "the first half is the first N points"
        );
        let second = scratch.join("first-2.json");
        assert!(second.exists(), "the second half is {}", second.display());
        assert_eq!(exported_utcs(&second), vec![1_700_000_002, 1_700_000_003]);
        let stderr = console.stderr();
        assert!(
            stderr.contains("first.json") && stderr.contains("first-2.json"),
            "both written paths must be reported: {stderr}"
        );
    }

    #[test]
    fn edit_merge_joins_dumps_in_capture_time_order() {
        let scratch = Scratch::new("edit-merge");
        let log = write_dump(&scratch, "track.bin", 2);
        let earlier = scratch.join("earlier.bin");
        fileio::write(&earlier, &dump_with_timestamps(&[1_600_000_000])).expect("write dump");
        let output = scratch.join("merged.json");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        let mut options = edit_options(&output);
        options.log = log;
        options.merge = vec![earlier];

        edit(&options, fast(&links, &console, &confirm)).expect("edit merge");

        assert_eq!(
            exported_utcs(&output),
            vec![1_600_000_000, 1_700_000_000, 1_700_000_001],
            "the earlier dump goes first even though it was listed second"
        );
    }

    #[test]
    fn edit_delete_removes_the_named_points_and_keeps_the_rest() {
        let scratch = Scratch::new("edit-delete");
        let log = write_dump(&scratch, "track.bin", 4);
        let output = scratch.join("trimmed.json");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        let mut options = edit_options(&output);
        options.log = log;
        options.delete = vec![1, 3];

        edit(&options, fast(&links, &console, &confirm)).expect("edit delete");

        assert_eq!(
            exported_utcs(&output),
            vec![1_700_000_000, 1_700_000_002],
            "only the named indices are gone"
        );
    }

    #[test]
    fn edit_refuses_a_point_index_the_log_does_not_have() {
        let scratch = Scratch::new("edit-bad-index");
        let log = write_dump(&scratch, "track.bin", 3);
        let output = scratch.join("trimmed.json");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        let mut options = edit_options(&output);
        options.log = log;
        options.delete = vec![7];

        let err = edit(&options, fast(&links, &console, &confirm)).expect_err("out of range");
        assert_eq!(crate::exit::exit_code_for(&err), 1, "got {err}");
        let message = err.to_string();
        assert!(
            message.contains('7') && message.contains("3 point(s)") && message.contains("0..=2"),
            "the error must name the index and the inclusive range: {message}"
        );
        assert!(!output.exists(), "nothing is written on a refused edit");
    }

    #[test]
    fn edit_does_not_modify_the_input_dump() {
        // `docs/reference/02-cli.md` promises the input dump is never modified.
        // Both operations read it, so both are checked against the original
        // bytes; a merge input is checked too, because a write there would be
        // just as destructive.
        let scratch = Scratch::new("edit-input");
        let log = write_dump(&scratch, "track.bin", 4);
        let original = fileio::read(&log).expect("read dump");
        let earlier = scratch.join("earlier.bin");
        fileio::write(&earlier, &dump_with_timestamps(&[1_600_000_000])).expect("write dump");
        let earlier_original = fileio::read(&earlier).expect("read dump");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;

        let mut split = edit_options(&scratch.join("split.json"));
        split.log = log.clone();
        split.split_at = Some(2);
        edit(&split, fast(&links, &console, &confirm)).expect("split");
        assert_eq!(
            fileio::read(&log).expect("read dump"),
            original,
            "a split must not touch the input dump"
        );

        let mut merge = edit_options(&scratch.join("merged.json"));
        merge.log = log.clone();
        merge.merge = vec![earlier.clone()];
        edit(&merge, fast(&links, &console, &confirm)).expect("merge");
        assert_eq!(
            fileio::read(&log).expect("read dump"),
            original,
            "a merge must not touch its own dump"
        );
        assert_eq!(
            fileio::read(&earlier).expect("read dump"),
            earlier_original,
            "a merge must not touch the dump it joins"
        );
    }

    #[test]
    fn edit_merge_puts_a_dump_without_timestamps_last() {
        // A dump with no time cannot claim a place among dumps that carry one,
        // so it goes last; `--merge` repeated twice also joins both dumps.
        let scratch = Scratch::new("edit-merge-untimed");
        let log = write_dump(&scratch, "track.bin", 2);
        let earlier = scratch.join("earlier.bin");
        fileio::write(&earlier, &dump_with_timestamps(&[1_600_000_000])).expect("write dump");
        let untimed = scratch.join("untimed.bin");
        fileio::write(&untimed, &dump_without_timestamps(1)).expect("write dump");
        let output = scratch.join("merged.json");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        let mut options = edit_options(&output);
        options.log = log;
        options.merge = vec![untimed, earlier];

        edit(&options, fast(&links, &console, &confirm)).expect("edit merge");

        assert_eq!(
            exported_utc_options(&output),
            vec![
                Some(1_600_000_000),
                Some(1_700_000_000),
                Some(1_700_000_001),
                None
            ],
            "timed dumps keep capture-time order and the untimed dump comes last"
        );
    }

    #[test]
    fn edit_refuses_a_split_that_would_leave_an_empty_half() {
        let scratch = Scratch::new("edit-bad-split");
        let log = write_dump(&scratch, "track.bin", 4);
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;

        for split_at in [0, 4, 9] {
            let output = scratch.join(&format!("split-{split_at}.json"));
            let mut options = edit_options(&output);
            options.log = log.clone();
            options.split_at = Some(split_at);

            let err = edit(&options, fast(&links, &console, &confirm)).expect_err("empty half");
            assert_eq!(
                crate::exit::exit_code_for(&err),
                1,
                "split at {split_at}: {err}"
            );
            assert!(
                err.to_string().contains("cannot split"),
                "split at {split_at}: {err}"
            );
            assert!(
                !output.exists(),
                "split at {split_at} must not write an output"
            );
        }
    }

    #[test]
    fn edit_split_refuses_to_overwrite_a_sibling_it_was_not_given() {
        let scratch = Scratch::new("edit-sibling");
        let log = write_dump(&scratch, "track.bin", 4);
        let output = scratch.join("first.json");
        let sibling = scratch.join("first-2.json");
        let existing = b"a file the user already had";
        fileio::write(&sibling, existing).expect("write sibling");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        let mut options = edit_options(&output);
        options.log = log;
        options.split_at = Some(2);

        let err = edit(&options, fast(&links, &console, &confirm)).expect_err("sibling exists");
        assert_eq!(crate::exit::exit_code_for(&err), 5, "got {err}");
        assert!(
            err.to_string().contains("first-2.json"),
            "the refusal must name the path it will not replace: {err}"
        );
        assert!(
            !output.exists(),
            "a refused split must not write the first half"
        );
        assert_eq!(
            fileio::read(&sibling).expect("sibling"),
            existing,
            "the existing sibling must be untouched"
        );
    }

    #[test]
    fn edit_warns_about_a_damaged_dump() {
        let scratch = Scratch::new("edit-damage");
        // Corrupt one record's checksum byte inside the block.
        let mut damaged = synthetic_dump(2);
        if let Some(byte) = damaged.get_mut(0x200 + 15) {
            *byte ^= 0xFF;
        }
        let log = scratch.join("damaged.bin");
        fileio::write(&log, &damaged).expect("write dump");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        let mut options = edit_options(&scratch.join("trimmed.json"));
        options.log = log.clone();
        options.delete = vec![0];

        edit(&options, fast(&links, &console, &confirm)).expect("edit");
        assert!(
            console.stderr().contains("checksum"),
            "a damaged input must be announced, as `log read` does: {}",
            console.stderr()
        );

        // A damaged `--merge` input names its path, because with more than one
        // input the message has to say which one it is about.
        let clean = write_dump(&scratch, "clean.bin", 2);
        let merge_input = scratch.join("merge-damaged.bin");
        fileio::write(&merge_input, &damaged).expect("write dump");
        let mut merge = edit_options(&scratch.join("merged.json"));
        merge.log = clean;
        merge.merge = vec![merge_input];
        edit(&merge, fast(&links, &console, &confirm)).expect("edit merge");
        let stderr = console.stderr();
        assert!(
            stderr.contains("merge-damaged.bin") && stderr.contains("checksum"),
            "the warning must name the damaged merge input: {stderr}"
        );
    }
}
