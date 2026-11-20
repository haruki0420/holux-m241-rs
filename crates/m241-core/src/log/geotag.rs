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
//! `DateTimeOriginal` carries no timezone (`exif::read_datetime_original` reads
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
//! [`crate::log::exif::write_gps`], which appends rather than relocates.

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
/// tag this module does not own is still preserved by
/// [`crate::log::exif::write_gps`].
///
/// # Errors
///
/// [`Error::Io`] when a photo cannot be read or replaced, and whatever
/// [`crate::log::exif::write_gps`] reports when a photo's Exif cannot be
/// rewritten (most often: the new block no longer fits a JPEG segment).
pub fn apply(plans: &[GeotagPlan]) -> Result<Vec<PathBuf>> {
    let mut written = Vec::new();
    for plan in plans {
        let Some(point) = plan.point else {
            continue;
        };
        // `plan` never chooses a point without a position; a hand-built plan
        // that does gets no coordinate rather than a made-up (0, 0).
        let (Some(latitude), Some(longitude)) = (point.latitude, point.longitude) else {
            continue;
        };
        let position = GpsPosition {
            latitude,
            longitude,
            altitude_m: point.height_m,
            utc: point.utc,
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
    let Some(captured) = capture_utc(photo, bytes) else {
        return GeotagPlan {
            photo: photo.to_path_buf(),
            point: None,
            gap_seconds: None,
            skipped: Some(capture_problem(photo, bytes)),
        };
    };
    let Some((point, gap)) = nearest_positioned_point(track, captured) else {
        return GeotagPlan {
            photo: photo.to_path_buf(),
            point: None,
            gap_seconds: None,
            skipped: Some(String::from(
                "the track has no point with both a time and a position to compare with",
            )),
        };
    };
    if gap > options.max_gap_seconds {
        return GeotagPlan {
            photo: photo.to_path_buf(),
            point: None,
            gap_seconds: Some(gap),
            skipped: Some(format!(
                "the nearest track point is {gap} s from the capture time, past the \
                 {} s limit",
                options.max_gap_seconds
            )),
        };
    }
    GeotagPlan {
        photo: photo.to_path_buf(),
        point: Some(point),
        gap_seconds: Some(gap),
        skipped: None,
    }
}

/// The capture time, or `None` when the photo has none.
///
/// `DateTimeOriginal` first, then the modification time. Read through
/// [`capture_problem`]'s counterpart so the reason for a missing time is not
/// recomputed differently in two places.
fn capture_utc(photo: &Path, bytes: &[u8]) -> Option<u32> {
    match exif::read_datetime_original(bytes) {
        Ok(Some(utc)) => Some(utc),
        // `Ok(None)`: no Exif segment, no Exif sub-IFD, or no
        // `DateTimeOriginal` tag. `Err`: the file is not a JPEG, or its
        // structure is damaged. Both fall back to the modification time, which
        // is a weaker time but still a time — except for a non-JPEG, which
        // `write_gps` would refuse; that case is rejected before it gets here by
        // `is_jpeg` in `capture_problem`.
        _ => file_mtime_utc(photo),
    }
}

/// Why [`capture_utc`] returned `None`, in words the caller can show.
fn capture_problem(photo: &Path, bytes: &[u8]) -> String {
    // A non-JPEG has no EXIF by definition, and the modification time of a PNG
    // says nothing about when its picture was taken: naming the real problem is
    // more useful than a vague "no capture time".
    if !is_jpeg(bytes) {
        return format!(
            "{} is not a JPEG, so it has no Exif block to write a position into",
            photo.display()
        );
    }
    match exif::read_datetime_original(bytes) {
        Ok(Some(_)) => String::from("no track point could be chosen"),
        Ok(None) => String::from(
            "the photo has no DateTimeOriginal, and its modification time is not a \
             usable capture time",
        ),
        Err(err) => format!("the photo's Exif could not be read: {err}"),
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
    std::fs::write(&temp, bytes)
        .map_err(|source| Error::io(temp.display().to_string(), source))?;
    if std::fs::rename(&temp, path).is_ok() {
        return Ok(());
    }
    std::fs::remove_file(path)
        .map_err(|source| Error::io(path.display().to_string(), source))?;
    std::fs::rename(&temp, path).map_err(|source| {
        let _ = std::fs::remove_file(&temp);
        Error::io(path.display().to_string(), source)
    })
}
