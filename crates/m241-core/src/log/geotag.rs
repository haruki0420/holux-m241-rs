//! Choosing which track point a photo was taken at, and writing that answer in.
//!
//! A track log records a position once a second; a camera records the moment a
//! photo was taken. Matching one to the other is a nearest-neighbour choice in
//! time, and this module makes that choice explicit rather than doing it inline
//! in a command: [`plan`] reads the photos and the track and returns one
//! [`GeotagPlan`] per photo, [`apply`] writes the plans that carry a point. The
//! split is the same one `firmware::patch` uses, and it exists so a `--dry-run`
//! and a real run compute the answers on exactly the same code path — a preview
//! that disagrees with the run is worse than no preview.
//!
//! # No interpolation
//!
//! When the nearest point is a few seconds away the tempting move is to
//! interpolate between the two surrounding points. This module does not, because
//! the interpolated coordinate is a *computation*, not evidence: it is the
//! position the logger would have had if it had been recording, and putting it
//! in a photo makes the photo claim a place nobody measured. The nearest
//! recorded point is evidence, and the gap is reported so the caller can decide
//! whether it is good enough.
//!
//! # Which points can be chosen
//!
//! A candidate needs both a time and a position. A record written while the unit
//! had no satellite lock has a time but no coordinates; it is skipped, because a
//! placeholder coordinate would tag the photo at (0, 0). Records without a time
//! cannot be compared at all and are skipped too.
//!
//! # Wall-clock time
//!
//! `DateTimeOriginal` carries no timezone ([`exif::read_datetime_original`] reads
//! it as if it were UTC, and says so). This module therefore compares it against
//! the track's UTC directly. A camera left on local time is off by its UTC
//! offset; correcting that is the caller's job, because only the caller knows
//! which zone the camera was set to. The alternative — guessing a zone from the
//! machine running the command — would produce a plausible wrong answer.
//!
//! The modification time is the fallback when a photo has no
//! `DateTimeOriginal`, which is what a phone or an editor may leave behind after
//! stripping metadata. It is the moment the file was last written, not the
//! moment the shutter fired, so it is only as good as the copy operation that
//! produced the file.
//!
//! # A photo is not replaceable
//!
//! Every rewrite goes through a sibling temporary file and a rename, so an
//! interrupted run leaves either the old photo or the new one at the original
//! name, never a half-written JPEG. The bytes themselves are produced by
//! [`exif::write_gps`], which appends rather than relocates.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::error::{Error, Result};
use crate::log::exif::{self, GpsPosition};
use crate::types::{Track, TrackPoint};

/// The gap used when the caller did not choose one: five minutes.
///
/// A logger recording once a second and a camera whose clock is a few seconds
/// off are still well inside this; a photo from a different day is obviously
/// outside it. The number is a judgement, which is why it is configurable.
pub const DEFAULT_MAX_GAP_SECONDS: u32 = 300;

/// How closely a photo and a track point have to agree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GeotagOptions {
    /// Largest accepted difference between the capture time and the track point,
    /// in whole seconds. A photo whose nearest point is further away is skipped
    /// rather than tagged with a coordinate it cannot support.
    pub max_gap_seconds: u32,
    /// Whether the caller should stop after [`plan`].
    ///
    /// [`plan`] never writes in either mode; this flag is the caller's own
    /// decision, kept in the same value as the gap so there is one place that
    /// answers "would this run write?". [`Self::writes`] spells that out, and
    /// [`apply`] is the only function that touches the files.
    pub dry_run: bool,
}

impl Default for GeotagOptions {
    fn default() -> Self {
        Self {
            max_gap_seconds: DEFAULT_MAX_GAP_SECONDS,
            dry_run: false,
        }
    }
}

impl GeotagOptions {
    /// Whether a caller following these options should go on to [`apply`].
    #[must_use]
    pub fn writes(&self) -> bool {
        !self.dry_run
    }
}

/// What [`plan`] decided for one photo.
#[derive(Debug, Clone, PartialEq)]
pub struct GeotagPlan {
    /// The photo the decision is about.
    pub photo: PathBuf,
    /// The point to write, when one was found inside the gap.
    pub point: Option<TrackPoint>,
    /// How far the nearest usable track point is from the capture time, in
    /// seconds. `Some` even when the point was rejected for exceeding the gap,
    /// because "how far" is what tells the caller whether to widen the limit;
    /// `None` when the track had no point with both a time and a position.
    pub gap_seconds: Option<u32>,
    /// Why the photo will not be tagged. `Some` exactly when [`Self::point`] is
    /// `None`.
    pub skipped: Option<String>,
}

impl GeotagPlan {
    /// Whether applying this plan would write to the photo.
    #[must_use]
    pub fn writes(&self) -> bool {
        self.point.is_some()
    }
}

/// Decide what to write into each photo.
///
/// Returns one plan per photo, in the order given, so the caller can report the
/// decisions the user is about to accept. Nothing is written.
///
/// A photo that cannot supply a capture time, is not a JPEG, or whose Exif is
/// corrupt is skipped with the reason in [`GeotagPlan::skipped`]: a folder of
/// photos should still be geotagged when one file in it is damaged. A photo that
/// cannot be *read* at all is an error, because that is a path problem and
/// silently returning "0 photos tagged" would hide it.
///
/// # Errors
///
/// [`Error::Io`] when a photo cannot be read.
pub fn plan(photos: &[PathBuf], track: &Track, options: &GeotagOptions) -> Result<Vec<GeotagPlan>> {
    let mut plans = Vec::with_capacity(photos.len());
    for photo in photos {
        let bytes = std::fs::read(photo)
            .map_err(|source| Error::io(photo.display().to_string(), source))?;
        plans.push(plan_one(photo, &bytes, track, options));
    }
    Ok(plans)
}

/// Apply every plan that carries a point, and return the photos written.
///
/// Photos are written in the order the plans list them; a plan with no point is
/// skipped. The photo is re-read here rather than carried in the plan, because a
/// reviewed plan is a decision, not a copy of the file: if the photo changed
/// between planning and applying, the change is what gets modified, and every
/// tag this module does not own is still preserved by [`exif::write_gps`].
///
/// # Errors
///
/// [`Error::Io`] when a photo cannot be read or replaced, and whatever
/// [`exif::write_gps`] reports when a photo's Exif cannot be rewritten (most
/// often: the new block no longer fits a JPEG segment).
pub fn apply(plans: &[GeotagPlan]) -> Result<Vec<PathBuf>> {
    let mut written = Vec::new();
    for plan in plans {
        let Some(point) = plan.point else {
            continue;
        };
        // `plan` never chooses a point without a position, and it validates the
        // rewrite before promising one; a hand-built plan that lacks a
        // coordinate gets no tag rather than a made-up (0, 0).
        let Some(position) = position_of(&point) else {
            continue;
        };
        let bytes = std::fs::read(&plan.photo)
            .map_err(|source| Error::io(plan.photo.display().to_string(), source))?;
        let tagged = exif::write_gps(&bytes, &position)?;
        replace_file(&plan.photo, &tagged)?;
        written.push(plan.photo.clone());
    }
    Ok(written)
}

/// The decision for one photo, from its bytes and the track.
fn plan_one(photo: &Path, bytes: &[u8], track: &Track, options: &GeotagOptions) -> GeotagPlan {
    let captured = match capture_time(photo, bytes) {
        Ok(captured) => captured,
        Err(reason) => return skipped(photo, None, reason),
    };
    let Some((point, gap)) = nearest_positioned_point(track, captured.utc()) else {
        return skipped(
            photo,
            None,
            String::from("the track has no point with both a time and a position to compare with"),
        );
    };
    if gap > options.max_gap_seconds {
        return skipped(
            photo,
            Some(gap),
            format!(
                "the nearest track point is {gap} s from the capture time, past the \
                 {} s limit",
                options.max_gap_seconds
            ),
        );
    }
    // Validate the rewrite now, on the bytes we already have. A plan the user
    // reviews then fails in `apply` is worse than no plan: it would abort a run
    // halfway through a folder.
    let Some(position) = position_of(&point) else {
        return skipped(
            photo,
            Some(gap),
            String::from("the chosen track point has no complete coordinate"),
        );
    };
    if let Err(err) = exif::write_gps(bytes, &position) {
        return skipped(
            photo,
            Some(gap),
            format!("the photo's Exif block cannot be rewritten: {err}"),
        );
    }
    GeotagPlan {
        photo: photo.to_path_buf(),
        point: Some(point),
        gap_seconds: Some(gap),
        skipped: None,
    }
}

/// A plan for a photo that will not be tagged.
fn skipped(photo: &Path, gap_seconds: Option<u32>, reason: String) -> GeotagPlan {
    GeotagPlan {
        photo: photo.to_path_buf(),
        point: None,
        gap_seconds,
        skipped: Some(reason),
    }
}

/// The position a track point would be written as, when it has both coordinates.
fn position_of(point: &TrackPoint) -> Option<GpsPosition> {
    Some(GpsPosition {
        latitude: point.latitude?,
        longitude: point.longitude?,
        altitude_m: point.height_m,
        utc: point.utc,
    })
}

/// When the photo was taken, and which clock that answer came from.
///
/// Kept as a named type rather than a bare `u32` so the reason a time is weak
/// (it came from the filesystem, not the shutter) stays visible at the use site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaptureTime {
    /// `DateTimeOriginal`, read as UTC because EXIF has no zone.
    Exif(u32),
    /// The file's modification time, for a photo whose Exif holds no capture
    /// time.
    FileModified(u32),
}

impl CaptureTime {
    /// The capture moment as Unix seconds.
    fn utc(self) -> u32 {
        match self {
            Self::Exif(utc) | Self::FileModified(utc) => utc,
        }
    }
}

/// Read the capture time, or say why there is none.
///
/// A damaged Exif block is *not* treated as "no timestamp": appending to a block
/// this module cannot parse would fail in [`apply`], after the user had already
/// reviewed a plan that promised a tag.
fn capture_time(photo: &Path, bytes: &[u8]) -> std::result::Result<CaptureTime, String> {
    // A non-JPEG has no EXIF by definition, and the modification time of a PNG
    // says nothing about when its picture was taken: naming the real problem is
    // more useful than a vague "no capture time".
    if !is_jpeg(bytes) {
        return Err(format!(
            "{} is not a JPEG, so it has no Exif block to write a position into",
            photo.display()
        ));
    }
    match exif::read_datetime_original(bytes) {
        Ok(Some(utc)) => Ok(CaptureTime::Exif(utc)),
        // `Ok(None)` is the lean case: no Exif segment, no Exif sub-IFD, or no
        // `DateTimeOriginal` tag. What is left is when the file was last
        // written, which is what a stripped or re-exported photo still carries.
        Ok(None) => file_mtime_utc(photo)
            .map(CaptureTime::FileModified)
            .ok_or_else(|| {
                String::from(
                    "the photo has no DateTimeOriginal, and its modification time is not a \
                 usable capture time",
                )
            }),
        Err(err) => Err(format!(
            "the photo's Exif could not be read, so it cannot be rewritten: {err}"
        )),
    }
}

/// Whether the bytes start with the JPEG start-of-image marker.
fn is_jpeg(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xFF, 0xD8])
}

/// The file's modification time as Unix seconds, when it can be represented.
fn file_mtime_utc(photo: &Path) -> Option<u32> {
    let modified = std::fs::metadata(photo).ok()?.modified().ok()?;
    let since_epoch = modified.duration_since(UNIX_EPOCH).ok()?;
    u32::try_from(since_epoch.as_secs()).ok()
}

/// The usable track point with the time closest to `utc`, and the gap.
///
/// Ties keep the earlier point in the log, which is the one the device wrote
/// first. Points without a time cannot be compared; points without a position
/// cannot tag anything and are passed over rather than chosen and rejected.
fn nearest_positioned_point(track: &Track, utc: u32) -> Option<(TrackPoint, u32)> {
    let mut best: Option<(TrackPoint, u32)> = None;
    for point in &track.points {
        let (Some(point_utc), true) = (point.utc, point.has_position()) else {
            continue;
        };
        let gap = point_utc.abs_diff(utc);
        if best.is_none_or(|(_, best_gap)| gap < best_gap) {
            best = Some((*point, gap));
        }
    }
    best
}

/// Write `bytes` over `path` through a sibling temporary file.
///
/// A photo is the one file this crate modifies that cannot be regenerated, and a
/// crash halfway through writing it in place would leave a truncated JPEG at
/// the photo's own name. The temporary file is written first and renamed into
/// place, which is atomic where the platform allows it; Windows refuses to
/// rename over an existing file, so that case removes the old name first.
fn replace_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let Some(file_name) = path.file_name() else {
        return Err(Error::State(format!(
            "{} has no file name to replace",
            path.display()
        )));
    };
    let mut temp_name = std::ffi::OsString::from(".");
    temp_name.push(file_name);
    temp_name.push(".m241-tmp");
    let temp = path.with_file_name(temp_name);
    std::fs::write(&temp, bytes).map_err(|source| Error::io(temp.display().to_string(), source))?;
    if std::fs::rename(&temp, path).is_ok() {
        return Ok(());
    }
    std::fs::remove_file(path).map_err(|source| Error::io(path.display().to_string(), source))?;
    std::fs::rename(&temp, path).map_err(|source| {
        let _ = std::fs::remove_file(&temp);
        Error::io(path.display().to_string(), source)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::exif::{self, ByteOrder};
    use crate::log::fixture;

    /// The repository's example coordinate. Never a real place anyone stood.
    const TOKYO_LAT: f64 = 35.681_236;
    const TOKYO_LON: f64 = 139.767_125;
    /// 2023-11-14 22:13:20 UTC.
    const SHUTTER: u32 = 1_700_000_000;

    /// A scratch directory that removes itself when the test ends.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(label: &str) -> Self {
            // `process::id` plus the label keeps parallel tests apart; the
            // counter covers a test that is somehow run twice in one process.
            use std::sync::atomic::{AtomicU32, Ordering};
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let serial = NEXT.fetch_add(1, Ordering::Relaxed);
            let mut path = std::env::temp_dir();
            path.push(format!(
                "m241-geotag-{}-{label}-{serial}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).expect("a scratch directory");
            Self(path)
        }

        /// Write `bytes` and return the path.
        fn photo(&self, name: &str, bytes: &[u8]) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, bytes).expect("a scratch photo");
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A point at `utc`, at the example coordinate.
    fn point(utc: u32) -> TrackPoint {
        TrackPoint {
            utc: Some(utc),
            latitude: Some(TOKYO_LAT),
            longitude: Some(TOKYO_LON),
            height_m: Some(80.0),
            speed_kmh: None,
            rcr: None,
        }
    }

    /// A track whose points are at the given times, all with a position.
    fn track_at(times: &[u32]) -> Track {
        Track {
            points: times.iter().copied().map(point).collect(),
            damaged_blocks: Vec::new(),
        }
    }

    /// A synthetic JPEG with `DateTimeOriginal` set to `utc`.
    fn photo_taken_at(utc: u32) -> Vec<u8> {
        let stamp = format_datetime(utc);
        fixture::exif_jpeg(
            ByteOrder::Little,
            &[],
            Some(&[
                fixture::Entry::ascii(exif::tag::GPS_LATITUDE_REF, "N"),
                fixture::Entry::ascii(exif::tag::GPS_LONGITUDE_REF, "E"),
            ]),
            Some(&stamp),
        )
    }

    /// `"YYYY:MM:DD HH:MM:SS"` for a Unix time, without pulling in a date crate.
    fn format_datetime(utc: u32) -> String {
        let days = i64::from(utc / 86_400);
        let seconds = utc % 86_400;
        let (year, month, day) = civil(days);
        format!(
            "{year:04}:{month:02}:{day:02} {:02}:{:02}:{:02}",
            seconds / 3_600,
            (seconds % 3_600) / 60,
            seconds % 60
        )
    }

    /// Days since 1970-01-01 as a Gregorian date; Hinnant's `civil_from_days`.
    fn civil(days: i64) -> (i64, u32, u32) {
        let shifted = days + 719_468;
        let era = if shifted >= 0 {
            shifted
        } else {
            shifted - 146_096
        } / 146_097;
        let day_of_era = shifted - era * 146_097;
        let year_of_era =
            (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
        let year = year_of_era + era * 400;
        let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
        let month_prime = (5 * day_of_year + 2) / 153;
        let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
        let month = if month_prime < 10 {
            month_prime + 3
        } else {
            month_prime - 9
        };
        (
            if month <= 2 { year + 1 } else { year },
            u32::try_from(month).expect("a month"),
            u32::try_from(day).expect("a day"),
        )
    }

    #[test]
    fn the_nearest_point_in_time_is_the_one_chosen() {
        // A point before and a point after the shutter, neither exact.
        let track = track_at(&[SHUTTER - 12, SHUTTER + 4]);
        let dir = TempDir::new("nearest");
        let photo = dir.photo("nearer.jpg", &photo_taken_at(SHUTTER));

        let plans = plan(
            std::slice::from_ref(&photo),
            &track,
            &GeotagOptions::default(),
        )
        .expect("plan");
        assert_eq!(plans.len(), 1);
        let chosen = &plans[0];
        assert!(chosen.skipped.is_none(), "skipped: {:?}", chosen.skipped);
        assert_eq!(chosen.gap_seconds, Some(4), "the later point is nearer");
        assert_eq!(chosen.point.expect("a point").utc, Some(SHUTTER + 4));
        assert!(chosen.writes());
        assert!(
            plan(&[photo], &track, &GeotagOptions::default())
                .expect("plan")
                .first()
                .is_some_and(GeotagPlan::writes)
        );
    }

    #[test]
    fn a_photo_further_than_the_maximum_gap_is_skipped_with_the_reason() {
        let track = track_at(&[SHUTTER - 400]);
        let dir = TempDir::new("gap");
        let photo = dir.photo("late.jpg", &photo_taken_at(SHUTTER));
        let options = GeotagOptions::default();
        assert_eq!(options.max_gap_seconds, DEFAULT_MAX_GAP_SECONDS);

        let plans = plan(&[photo], &track, &options).expect("plan");
        let decision = plans.first().expect("one plan");
        assert!(!decision.writes(), "an over-gap photo must not be written");
        assert_eq!(decision.gap_seconds, Some(400), "the gap is still reported");
        let reason = decision.skipped.as_deref().expect("a reason");
        assert!(
            reason.contains("400 s") && reason.contains("300"),
            "the reason must name the gap and the limit: {reason}"
        );

        // The same photo and track with a wider limit is inside it: the refusal
        // is the policy, not a broken match.
        let wider = GeotagOptions {
            max_gap_seconds: 600,
            ..options
        };
        assert!(
            plan(std::slice::from_ref(&decision.photo), &track, &wider)
                .expect("plan")
                .first()
                .is_some_and(GeotagPlan::writes)
        );
    }

    #[test]
    fn track_points_without_a_position_are_passed_over() {
        // The closest point in time has no fix at all; the next one is further
        // away but is the only one that can tag anything.
        let track = Track {
            points: vec![
                TrackPoint {
                    utc: Some(SHUTTER + 1),
                    latitude: None,
                    longitude: None,
                    ..TrackPoint::default()
                },
                point(SHUTTER - 20),
            ],
            damaged_blocks: Vec::new(),
        };
        let dir = TempDir::new("nofix");
        let photo = dir.photo("nofix.jpg", &photo_taken_at(SHUTTER));

        let plans = plan(&[photo], &track, &GeotagOptions::default()).expect("plan");
        let decision = plans.first().expect("one plan");
        let chosen = decision.point.expect("the positioned point");
        assert_eq!(chosen.utc, Some(SHUTTER - 20));
        assert_eq!(decision.gap_seconds, Some(20));

        // A track with no positioned point at all skips the photo rather than
        // choosing the time-only record.
        let no_fix_only = Track {
            points: vec![TrackPoint {
                utc: Some(SHUTTER),
                ..TrackPoint::default()
            }],
            damaged_blocks: Vec::new(),
        };
        let plans = plan(
            std::slice::from_ref(&decision.photo),
            &no_fix_only,
            &GeotagOptions::default(),
        )
        .expect("plan");
        let decision = plans.first().expect("one plan");
        assert!(!decision.writes());
        assert_eq!(decision.gap_seconds, None);
    }

    #[test]
    fn a_dry_run_leaves_the_photo_byte_for_byte_unchanged() {
        let track = track_at(&[SHUTTER]);
        let dir = TempDir::new("dry");
        let original = photo_taken_at(SHUTTER);
        let photo = dir.photo("dry.jpg", &original);
        let options = GeotagOptions {
            dry_run: true,
            ..GeotagOptions::default()
        };
        assert!(!options.writes());

        let plans = plan(std::slice::from_ref(&photo), &track, &options).expect("plan");
        assert!(plans.first().is_some_and(GeotagPlan::writes));
        assert_eq!(
            std::fs::read(&photo).expect("read back"),
            original,
            "planning must not touch the photo, in a dry run or otherwise"
        );

        // And the plan that a dry run showed is the plan a real run applies.
        let written = apply(&plans).expect("apply");
        assert_eq!(written, vec![photo.clone()]);
        let tagged = std::fs::read(&photo).expect("read the tagged photo");
        assert_ne!(tagged, original, "apply must have written something");
        let tags = exif::read_gps(&tagged)
            .expect("read the new Exif")
            .expect("Exif");
        assert!((tags.latitude.expect("a latitude") - TOKYO_LAT).abs() < 1e-9);
        assert!((tags.longitude.expect("a longitude") - TOKYO_LON).abs() < 1e-9);
    }

    #[test]
    fn a_photo_without_a_capture_time_falls_back_to_its_modification_time() {
        // No Exif at all: the mtime is the only clock left. Setting it to the
        // track's own time is how a copied-but-unstripped photo behaves.
        let dir = TempDir::new("mtime");
        let photo = dir.photo("plain.jpg", &fixture::jpeg_without_exif());
        set_mtime(&photo, SHUTTER);

        let track = track_at(&[SHUTTER]);
        let plans = plan(
            std::slice::from_ref(&photo),
            &track,
            &GeotagOptions::default(),
        )
        .expect("plan");
        let decision = plans.first().expect("one plan");
        assert!(decision.writes(), "skipped: {:?}", decision.skipped);
        assert_eq!(decision.gap_seconds, Some(0));
        assert!(apply(&plans).expect("apply").contains(&photo));
    }

    /// Set a file's modification time to `utc`, to test the mtime fallback.
    fn set_mtime(path: &Path, utc: u32) {
        use std::fs::FileTimes;
        let since_epoch = std::time::Duration::from_secs(u64::from(utc));
        let file = std::fs::File::options()
            .write(true)
            .open(path)
            .expect("open for a timestamp");
        let mut times = FileTimes::new();
        times = times.set_modified(UNIX_EPOCH + since_epoch);
        file.set_times(times).expect("set the modification time");
    }

    #[test]
    fn a_non_jpeg_and_a_photo_with_no_time_are_skipped_by_name() {
        let dir = TempDir::new("mixed");
        let png = dir.photo("picture.png", b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR");
        let timed = dir.photo("timed.jpg", &photo_taken_at(SHUTTER));
        let track = track_at(&[SHUTTER]);

        let plans = plan(&[png, timed], &track, &GeotagOptions::default()).expect("plan");
        assert_eq!(plans.len(), 2);
        let first = plans.first().expect("the PNG plan");
        assert!(!first.writes());
        let reason = first.skipped.as_deref().expect("a reason");
        assert!(
            reason.contains("not a JPEG"),
            "the skip reason must name the format: {reason}"
        );
        // One bad file does not stop the folder: the good photo is still planned.
        assert!(plans.get(1).is_some_and(GeotagPlan::writes));
    }

    #[test]
    fn a_photo_that_cannot_be_read_is_an_error_not_a_silent_skip() {
        let dir = TempDir::new("missing");
        let missing = dir.0.join("not-there.jpg");
        let err = plan(&[missing], &track_at(&[SHUTTER]), &GeotagOptions::default())
            .expect_err("a missing photo is an error");
        assert!(matches!(err, Error::Io { .. }), "got {err:?}");
    }

    #[test]
    fn the_default_options_ask_for_five_minutes_and_a_real_run() {
        let options = GeotagOptions::default();
        assert_eq!(options.max_gap_seconds, 300);
        assert!(!options.dry_run);
        assert!(options.writes());
        assert_eq!(DEFAULT_MAX_GAP_SECONDS, 300);
    }
}
