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
//! [`exif::write_gps`], which appends rather than relocates. The guarantee is
//! per photo: a run that fails on the fourth photo leaves the first three
//! written, so a failed folder run is recoverable, not rolled back.
//!
//! # Shifted time is written, not remembered
//!
//! [`GeotagOptions::photo_shift_seconds`] moves the photo's clock as well as
//! the match: [`apply`] writes the shifted value into `DateTimeOriginal`
//! (`07-eztour-spec.ja.md` §6.1-7, §6.2), so the photo says when it was really
//! taken and a later run matches it without being told about the shift again.
//! [`plan_with_shift`] is the reviewable half of that operation — it returns
//! the re-matched plans and writes nothing.
//!
//! # Shifting in the units the dialog offers, and only some photos
//!
//! `07-eztour-spec.ja.md` §6.1-7 records that ezTour's `Shift photo time…`
//! dialog shifts by days, hours, minutes and seconds, and offers three scopes:
//! every photo (`allPhotos`), the selected photos (`selPhotos`) and the photos
//! from one camera (`byCamera`). [`PhotoTimeShift`] is the first and
//! [`PhotoShiftScope`] the second, taken by [`plan_with_time_shift`].
//! [`plan_with_shift`] and [`GeotagOptions::photo_shift_seconds`] remain the
//! all-photos, one-total path, so a caller that predates the scopes is
//! unchanged. The spec does not say which Exif tags identify "one camera" or how
//! two spellings of a camera fold together, so [`CameraModel`] and
//! [`PhotoShiftScope::SelectedPhotos`] compare exact values and the matching
//! rule is recorded as unconfirmed in `docs/dev/12-progress-summary.md`.
//!
//! # Not verified on camera files
//!
//! Nothing in this module has been run against a real photo: the tests build
//! their JPEGs byte by byte. See the same caveat in [`exif`], which produces the
//! bytes this module writes.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::error::{Error, Result};
use crate::log::exif::{self, GpsPosition};
use crate::log::kmz::KmzPhoto;
use crate::types::{Track, TrackPoint};

/// The gap used when the caller did not choose one: five minutes.
///
/// A logger recording once a second and a camera whose clock is a few seconds
/// off are still well inside this; a photo from a different day is obviously
/// outside it. The number is a judgement, which is why it is configurable.
pub const DEFAULT_MAX_GAP_SECONDS: u32 = 300;

/// How closely a photo and a track point have to agree, and how matching
/// behaves.
///
/// [`plan`] never writes to any file; a `--dry-run` caller stops after [`plan`]
/// and skips [`apply`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GeotagOptions {
    /// Largest accepted difference between the capture time and the track point,
    /// in whole seconds. A photo whose nearest point is further away is skipped
    /// rather than tagged with a coordinate it cannot support.
    pub max_gap_seconds: u32,
    /// When `true`, [`apply`] writes a sibling `<photo>_original` backup before
    /// replacing a photo (if that backup does not already exist).
    ///
    /// The backup is a *suffix* on purpose, so it sits beside the photo where a
    /// user who typed `ls` or opened the folder will see it and can restore it
    /// by hand. The rewrite's own temporary file uses the opposite convention (a
    /// hidden `.name.m241-tmp`) because nobody should ever open that one: it
    /// exists for milliseconds and only survives a failure, where the error
    /// names its path.
    pub keep_backup: bool,
    /// Offset in seconds added to each photo's capture time before matching
    /// against the track (for camera clock drift or timezone correction).
    ///
    /// This offset is **also written into the photo** by [`apply`]: the
    /// `DateTimeOriginal` a tagged photo ends up with is its old value plus
    /// this offset, so the tag and the match agree and a later run without the
    /// shift still matches. See [`apply_with_rematch`] for where the shift is
    /// applied.
    pub photo_shift_seconds: i64,
    /// Optional haversine distance threshold in metres for grouping matched
    /// photos into the same [`GeotagPlan::group_id`] when `Some(d)` with `d > 0.0`.
    ///
    /// `None` is this crate's default and it does **no grouping**: every matched
    /// photo is its own group. `07-eztour-spec.ja.md` §6 records that ezTour
    /// ships a non-zero default for `groupPhotoDistance`, but the distribution
    /// does not say what that value is, and a guessed number would silently
    /// merge photos the user did not ask to merge. A caller that wants ezTour's
    /// behaviour must pass the value its UI shows.
    pub group_distance_meters: Option<f64>,
    /// Optional largest long edge, in pixels, for a photo embedded in a KMZ
    /// (`kmzMaxPhotoEdgeSize`, `07-eztour-spec.ja.md` §6.3).
    ///
    /// Only meaningful when the plan is turned into a [`KmzPhoto`] (see
    /// [`GeotagPlan::to_kmz_photo`]). It is carried here rather than added as
    /// an argument to the conversion so the option a user sets once travels
    /// with the rest of the geotag settings. [`None`] embeds the original bytes.
    pub max_photo_edge_pixels: Option<u32>,
}

impl Default for GeotagOptions {
    fn default() -> Self {
        Self {
            max_gap_seconds: DEFAULT_MAX_GAP_SECONDS,
            keep_backup: false,
            photo_shift_seconds: 0,
            group_distance_meters: None,
            max_photo_edge_pixels: None,
        }
    }
}

/// A shift of a photo's clock, in the units ezTour's dialog offers.
///
/// `07-eztour-spec.ja.md` §6.1-7 records that `Shift photo time…` takes days,
/// hours, minutes and seconds. A day here is 24 hours (86,400 seconds), not a
/// calendar day: the value being moved is EXIF's zone-less wall clock, and
/// whether ezTour steps across a daylight-saving change by the calendar rather
/// than by the clock is unconfirmed.
///
/// Every field is signed and they are summed, so `days: -1, hours: 2` moves the
/// clock 22 hours back. [`Self::total_seconds`] computes the sum with checked
/// arithmetic and reports an overflow rather than wrapping it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PhotoTimeShift {
    /// Whole days, each counted as 86,400 seconds.
    pub days: i64,
    /// Whole hours, each counted as 3,600 seconds.
    pub hours: i64,
    /// Whole minutes, each counted as 60 seconds.
    pub minutes: i64,
    /// Seconds.
    pub seconds: i64,
}

impl PhotoTimeShift {
    /// A shift expressed as one number of seconds: the shape
    /// [`GeotagOptions::photo_shift_seconds`] already has.
    #[must_use]
    pub const fn from_seconds(seconds: i64) -> Self {
        Self {
            days: 0,
            hours: 0,
            minutes: 0,
            seconds,
        }
    }

    /// The shift as one number of seconds, or `None` when the fields add up to
    /// something outside the range `i64` can hold.
    #[must_use]
    pub fn total_seconds(self) -> Option<i64> {
        self.days
            .checked_mul(86_400)?
            .checked_add(self.hours.checked_mul(3_600)?)?
            .checked_add(self.minutes.checked_mul(60)?)?
            .checked_add(self.seconds)
    }

    /// Whether the shift moves nothing.
    #[must_use]
    pub fn is_zero(self) -> bool {
        self.days == 0 && self.hours == 0 && self.minutes == 0 && self.seconds == 0
    }
}

/// The camera a photo came from, as its Exif `Make` and `Model` tags carry it.
///
/// [`PhotoShiftScope::ByCamera`] compares this value exactly. The spec records
/// that ezTour can shift "by camera" but not which tag it groups on, nor how two
/// spellings of one camera are folded together, so no normalisation is invented
/// here: the tags have their trailing NUL and space padding trimmed and are
/// otherwise compared byte for byte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CameraModel {
    /// Exif IFD0 `Make` (tag 0x010F), trailing padding trimmed. `None` when the
    /// photo does not name a maker.
    pub make: Option<String>,
    /// Exif IFD0 `Model` (tag 0x0110), trailing padding trimmed. `None` when the
    /// photo does not name a model.
    pub model: Option<String>,
}

impl CameraModel {
    /// Read the camera identity from a JPEG's Exif block.
    ///
    /// `None` when the bytes are not a JPEG, carry no Exif block this function
    /// can walk, or name neither tag. A photo with no readable camera is never
    /// selected by [`PhotoShiftScope::ByCamera`]; it is still planned, at its
    /// unshifted time.
    #[must_use]
    pub fn from_jpeg(jpeg: &[u8]) -> Option<Self> {
        let tiff = exif_tiff_block(jpeg)?;
        let make = ifd0_ascii(tiff, TAG_MAKE);
        let model = ifd0_ascii(tiff, TAG_MODEL);
        (make.is_some() || model.is_some()).then_some(Self { make, model })
    }
}

/// Which photos a clock shift applies to.
///
/// `07-eztour-spec.ja.md` §6.1-7 names ezTour's three scopes: `allPhotos`,
/// `selPhotos` and `byCamera`. [`Self::AllPhotos`] is the default and is also
/// the behaviour of [`plan_with_shift`] and
/// [`GeotagOptions::photo_shift_seconds`]; the narrower scopes are applied by
/// [`plan_with_time_shift`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum PhotoShiftScope {
    /// Every photo: ezTour's `allPhotos`, and the default.
    #[default]
    AllPhotos,
    /// Only the photos whose path equals one in this list: ezTour's
    /// `selPhotos`.
    ///
    /// The comparison is exact rather than a canonicalised one: the list is the
    /// caller's own selection, and silently matching a different spelling of
    /// one path would hide the typo that made them differ.
    SelectedPhotos(Vec<PathBuf>),
    /// Only photos whose Exif names this camera: ezTour's `byCamera`.
    ByCamera(CameraModel),
}

impl PhotoShiftScope {
    /// Whether this scope moves `photo`'s clock.
    ///
    /// `camera` is the photo's identity from [`CameraModel::from_jpeg`]. Only
    /// [`Self::ByCamera`] reads it; a caller using another scope passes `None`,
    /// and one that already read the photo can pass it for any scope.
    #[must_use]
    pub fn includes(&self, photo: &Path, camera: Option<&CameraModel>) -> bool {
        match self {
            Self::AllPhotos => true,
            Self::SelectedPhotos(selection) => {
                selection.iter().any(|selected| selected.as_path() == photo)
            }
            Self::ByCamera(wanted) => camera == Some(wanted),
        }
    }
}

/// When the photo was taken, and which clock that answer came from.
///
/// Kept as a named type rather than a bare `u32` so callers reviewing a
/// [`GeotagPlan`] can tell when a timestamp came from the filesystem rather
/// than the camera shutter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureTime {
    /// `DateTimeOriginal`, read as UTC because EXIF has no zone.
    Exif(u32),
    /// The file's modification time, for a photo whose Exif holds no capture
    /// time.
    FileModified(u32),
}

impl CaptureTime {
    /// The capture moment as Unix seconds.
    #[must_use]
    pub fn utc(self) -> u32 {
        match self {
            Self::Exif(utc) | Self::FileModified(utc) => utc,
        }
    }
}

/// What [`plan`] decided for one photo.
#[derive(Debug, Clone, PartialEq)]
pub struct GeotagPlan {
    /// The photo the decision is about.
    pub photo: PathBuf,
    /// When the photo was taken (after applying `photo_shift_seconds`) and
    /// which clock that time came from. `None` when no usable capture time
    /// could be obtained (for example, a non-JPEG or corrupt Exif).
    pub capture_time: Option<CaptureTime>,
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
    /// Whether [`apply`] should preserve a `<photo>_original` backup before
    /// replacing the photo.
    ///
    /// A copy of [`GeotagOptions::keep_backup`] taken by [`plan`]; the plan
    /// carries it so [`apply`] does not need the options again. Treat it as
    /// read-only once [`plan`] has returned — writing it changes what a reviewed
    /// plan does, which is exactly what the review is meant to prevent.
    pub keep_backup: bool,
    /// Cluster identifier assigned by [`group_matched_plans`] when distance
    /// grouping is enabled.
    pub group_id: Option<usize>,
}

impl GeotagPlan {
    /// Whether [`apply`] will write to this photo.
    #[must_use]
    pub fn will_write(&self) -> bool {
        self.point.is_some()
    }

    /// This photo as a KMZ entry, keeping the capture time and its provenance.
    ///
    /// The coordinates are the ones [`Self::point`] carries, so the photo is
    /// referenced rather than duplicated: `jpeg_bytes` is empty and the caller
    /// fills it from [`Self::photo`]. [`None`] when the plan has no point.
    ///
    /// `max_edge_pixels` is `07-eztour-spec.ja.md` §6.3's
    /// `kmzMaxPhotoEdgeSize`; see [`crate::log::kmz::KmzPhoto::max_edge_pixels`]
    /// for why reduction is a request the exporter reports it could not meet
    /// rather than something this conversion performs.
    #[must_use]
    pub fn to_kmz_photo(&self, max_edge_pixels: Option<u32>) -> Option<KmzPhoto> {
        let point = self.point?;
        Some(KmzPhoto {
            name: self
                .photo
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| self.photo.display().to_string()),
            lat: point.latitude?,
            lon: point.longitude?,
            altitude_m: point.height_m,
            utc: self.capture_time,
            max_edge_pixels,
            jpeg_bytes: Vec::new(),
        })
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
    plan_each(photos, track, options, |_, _| options.photo_shift_seconds)
}

/// Plan each photo, with `shift_for` deciding its clock shift from its path and
/// its bytes.
///
/// The one loop [`plan`] and [`plan_with_time_shift`] share: read, choose the
/// shift, decide, then group. Keeping it in one place is what makes a dry run
/// and a real run take the same decisions.
fn plan_each(
    photos: &[PathBuf],
    track: &Track,
    options: &GeotagOptions,
    shift_for: impl Fn(&Path, &[u8]) -> i64,
) -> Result<Vec<GeotagPlan>> {
    let mut plans = Vec::with_capacity(photos.len());
    for photo in photos {
        let bytes = std::fs::read(photo)
            .map_err(|source| Error::io(photo.display().to_string(), source))?;
        // `options` is copied per photo with only the shift replaced, so every
        // decision below (gap, backup, grouping) still comes from one options
        // value the caller set once.
        let shifted = GeotagOptions {
            photo_shift_seconds: shift_for(photo, &bytes),
            ..*options
        };
        plans.push(plan_one(photo, &bytes, track, &shifted));
    }
    if let Some(max_distance_meters) = options.group_distance_meters
        && max_distance_meters.is_finite()
        && max_distance_meters > 0.0
    {
        group_matched_plans(&mut plans, max_distance_meters);
    }
    Ok(plans)
}

/// Group matched plans so that two photos less than `max_distance_meters`
/// apart always share a [`GeotagPlan::group_id`].
///
/// "Less than" is applied to every pair, not to a running anchor: the relation
/// is treated as an equivalence, so if A is 10 m from B and B is 10 m from C,
/// A, B and C are one group even when A and C are 20 m apart. That is what
/// `07-eztour-spec.ja.md` §6.1-5 asks for ("two photos closer than the
/// threshold go on the same placemark"), and it makes the answer independent of
/// the order the photos are listed in — a different listing order is a
/// different upload, not a different place.
///
/// This is a union-find over the matched plans: identical to transitive
/// closure, computed in one pass over the pairs rather than by repeatedly
/// merging sets. It costs O(n²) distance computations; a set of photos large
/// enough for that to matter is a problem for its own reason.
///
/// Group ids are assigned in the order the groups first appear in `plans`, so
/// the first matched photo is always group `0`. Unmatched plans (or plans
/// without a finite coordinate) keep `group_id = None`. If
/// `max_distance_meters` is not finite or not positive, every `group_id` is
/// cleared to `None`.
pub fn group_matched_plans(plans: &mut [GeotagPlan], max_distance_meters: f64) {
    for plan in plans.iter_mut() {
        plan.group_id = None;
    }
    if !max_distance_meters.is_finite() || max_distance_meters <= 0.0 {
        return;
    }

    // The matched plans' coordinates, in listing order.
    let positions: Vec<(f64, f64)> = plans
        .iter()
        .filter_map(|plan| {
            let point = plan.point?;
            let (lat, lon) = (point.latitude?, point.longitude?);
            (lat.is_finite() && lon.is_finite()).then_some((lat, lon))
        })
        .collect();

    let mut sets = DisjointSets::new(positions.len());
    for (i, &(lat_i, lon_i)) in positions.iter().enumerate() {
        for (offset, &(lat_j, lon_j)) in positions.iter().skip(i + 1).enumerate() {
            if haversine_meters(lat_i, lon_i, lat_j, lon_j) < max_distance_meters {
                sets.union(i, i + 1 + offset);
            }
        }
    }

    // Walk the matched plans again, in the same order, giving each root the
    // next free id: a group keeps the id of whichever of its members came
    // first.
    let mut group_of_root: Vec<Option<usize>> = vec![None; positions.len()];
    let mut next_group = 0usize;
    // Which positions index each plan holds, `None` for an unmatched plan. The
    // first pass and this one must agree on that set, so it is computed once.
    let mut indices: Vec<Option<usize>> = Vec::with_capacity(plans.len());
    let mut next_index = 0usize;
    for plan in plans.iter() {
        let matched = plan
            .point
            .and_then(|point| Some((point.latitude?, point.longitude?)))
            .is_some_and(|(lat, lon)| lat.is_finite() && lon.is_finite());
        if matched {
            indices.push(Some(next_index));
            next_index += 1;
        } else {
            indices.push(None);
        }
    }
    for (plan, &index) in plans.iter_mut().zip(&indices) {
        let Some(index) = index else {
            continue;
        };
        let root = sets.find(index);
        let Some(slot) = group_of_root.get_mut(root) else {
            continue;
        };
        let id = *slot.get_or_insert_with(|| {
            let id = next_group;
            next_group += 1;
            id
        });
        plan.group_id = Some(id);
    }
}

/// Union-find with path halving; `find` returns the representative of a set.
struct DisjointSets {
    parent: Vec<usize>,
}

impl DisjointSets {
    fn new(len: usize) -> Self {
        Self {
            parent: (0..len).collect(),
        }
    }

    fn find(&mut self, mut node: usize) -> usize {
        loop {
            let Some(&parent) = self.parent.get(node) else {
                return node;
            };
            if parent == node {
                return node;
            }
            let Some(&grandparent) = self.parent.get(parent) else {
                return parent;
            };
            if grandparent == parent {
                return parent;
            }
            // Path halving: point this node at its grandparent and continue
            // from there, so a chain gets shorter every visit.
            if let Some(slot) = self.parent.get_mut(node) {
                *slot = grandparent;
            }
            node = grandparent;
        }
    }

    fn union(&mut self, a: usize, b: usize) {
        let root_a = self.find(a);
        let root_b = self.find(b);
        if root_a != root_b {
            // The smaller root stays the representative, which keeps the ids
            // stable when the same pairs arrive in another order.
            let (keep, drop) = if root_a < root_b {
                (root_a, root_b)
            } else {
                (root_b, root_a)
            };
            if let Some(slot) = self.parent.get_mut(drop) {
                *slot = keep;
            }
        }
    }
}

/// Great-circle distance in metres between two WGS84-ish lat/lon points using
/// the mean Earth radius (`6_371_000` m).
#[must_use]
pub(crate) fn haversine_meters(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    const EARTH_RADIUS_M: f64 = 6_371_000.0;
    let dlat = (lat2 - lat1).to_radians();
    let dlon = (lon2 - lon1).to_radians();
    let lat1_rad = lat1.to_radians();
    let lat2_rad = lat2.to_radians();
    let sin_dlat = (dlat / 2.0).sin();
    let sin_dlon = (dlon / 2.0).sin();
    let a = sin_dlat * sin_dlat + lat1_rad.cos() * lat2_rad.cos() * sin_dlon * sin_dlon;
    let c = 2.0 * a.sqrt().min(1.0).asin();
    EARTH_RADIUS_M * c
}

/// Apply every plan that carries a point, and return the photos written.
///
/// Photos are written in the order the plans list them; a plan with no point is
/// skipped. The photo is re-read here rather than carried in the plan, because a
/// reviewed plan is a decision, not a copy of the file: if the photo changed
/// between planning and applying, the change is what gets modified, and every
/// tag this module does not own is still preserved by [`exif::write_gps`].
///
/// When [`GeotagPlan::keep_backup`] is `true`, a sibling `<photo>_original`
/// backup of the pre-modification bytes is written if that backup file does not
/// already exist.
///
/// When [`GeotagPlan::capture_time`] is [`CaptureTime::Exif`], that value —
/// which [`plan`] already shifted by [`GeotagOptions::photo_shift_seconds`] — is
/// written into the photo's `DateTimeOriginal` in the same rewrite as the
/// position. A [`CaptureTime::FileModified`] time is **not** written: it is the
/// file's own mtime, not the camera's claim, and stamping it into the photo
/// would turn a guess into evidence.
///
/// This is one photo at a time. A run that fails on the fourth photo leaves the
/// first three written and the rest untouched, with the `_original` backups of
/// the three present, so the folder is recoverable but not rolled back.
///
/// # Errors
///
/// [`Error::Io`] when a photo cannot be read or replaced (or its backup cannot
/// be written), and whatever [`exif::write_gps`] reports when a photo's Exif
/// cannot be rewritten (most often: the new block no longer fits a JPEG
/// segment). The photo is untouched when an error is returned for it.
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
        // The shift moves the match; the same shift must move the photo's own
        // clock, or the tag and the time it was chosen for disagree and the
        // next run starts from the un-shifted time again.
        let capture_time = match plan.capture_time {
            Some(CaptureTime::Exif(utc)) => Some(utc),
            Some(CaptureTime::FileModified(_)) | None => None,
        };
        let bytes = std::fs::read(&plan.photo)
            .map_err(|source| Error::io(plan.photo.display().to_string(), source))?;
        let tagged = exif::write_gps(&bytes, &position)?;
        let tagged = match capture_time {
            Some(utc) => exif::write_datetime_original(&tagged, utc)?,
            None => tagged,
        };
        if plan.keep_backup {
            write_backup_if_missing(&plan.photo, &bytes)?;
        }
        replace_file(&plan.photo, &tagged)?;
        written.push(plan.photo.clone());
    }
    Ok(written)
}

/// Shift every photo's capture time and match it again, returning a fresh plan
/// for each photo.
///
/// `07-eztour-spec.ja.md` §6.1-7 requires that shifting the photo time
/// re-matches automatically, and §6.2 records that ezTour does it by rewriting
/// `DateTimeOriginal`. This function is the whole of that operation and it
/// stops before writing: it computes, for every photo, the capture time it
/// would have after `shift_seconds`, matches that time against `track`, and
/// returns the plans a review step shows and [`apply`] then writes. Writing is
/// [`apply`]'s job, so a dry run and a real shift take exactly the same
/// decisions — run this, show the plans, and either stop or call [`apply`].
///
/// The shift is not stored anywhere: it lives in the returned plans. A caller
/// that only shows the plans and never applies them has moved nothing, which is
/// what makes shifting safe to try.
///
/// The returned plans use `options` with [`GeotagOptions::photo_shift_seconds`]
/// replaced by `shift_seconds`; distance grouping is applied as `options` asks.
///
/// This is the all-photos form of [`plan_with_time_shift`], kept so the
/// seconds-only API that predates the day/hour/minute/second dialog and the
/// scopes does not change.
///
/// # Errors
///
/// [`Error::Io`] when a photo cannot be read.
pub fn plan_with_shift(
    photos: &[PathBuf],
    track: &Track,
    options: &GeotagOptions,
    shift_seconds: i64,
) -> Result<Vec<GeotagPlan>> {
    plan_with_time_shift(
        photos,
        track,
        options,
        PhotoTimeShift::from_seconds(shift_seconds),
        &PhotoShiftScope::AllPhotos,
    )
}

/// Shift the clock of the photos `scope` selects by `shift`, and match every
/// photo again; writes nothing.
///
/// This is ezTour's `Shift photo time…` expressed as a plan
/// (`07-eztour-spec.ja.md` §6.1-7): the shift lives in the returned plans and is
/// written into `DateTimeOriginal` by [`apply`], so a dry run and a real shift
/// take exactly the same decisions. A photo outside `scope` is planned at its
/// existing capture time, because only the selected photos' clocks are meant to
/// move.
///
/// [`GeotagOptions::photo_shift_seconds`] is replaced rather than added to: the
/// shift argument is the operation, and `options` supplies the rest of the
/// policy (gap, grouping, backup, KMZ limit). [`plan_with_shift`] does the same
/// for every photo.
///
/// # Errors
///
/// [`Error::State`] when the fields of `shift` do not add up to a number of
/// seconds `i64` can hold — checked before any photo is read, so a nonsense
/// shift cannot leave a folder half-planned — and [`Error::Io`] when a photo
/// cannot be read.
pub fn plan_with_time_shift(
    photos: &[PathBuf],
    track: &Track,
    options: &GeotagOptions,
    shift: PhotoTimeShift,
    scope: &PhotoShiftScope,
) -> Result<Vec<GeotagPlan>> {
    let shift_seconds = shift.total_seconds().ok_or_else(|| {
        Error::State(format!(
            "the photo clock shift {shift:?} does not fit in a number of seconds"
        ))
    })?;
    // Reading Make/Model for every photo is only needed by `byCamera`; doing it
    // for the other scopes would be work whose answer is then ignored.
    let reads_camera = matches!(scope, PhotoShiftScope::ByCamera(_));
    plan_each(photos, track, options, |photo, bytes| {
        let camera = if reads_camera {
            CameraModel::from_jpeg(bytes)
        } else {
            None
        };
        if scope.includes(photo, camera.as_ref()) {
            shift_seconds
        } else {
            0
        }
    })
}

/// Write `<photo>_original` if it does not already exist.
///
/// Uses `create_new(true)` so an existing `_original` backup is never
/// overwritten, preserving the true un-tagged original across repeated runs.
fn write_backup_if_missing(photo: &Path, original_bytes: &[u8]) -> Result<()> {
    use std::io::Write as _;

    let Some(file_name) = photo.file_name() else {
        return Err(Error::State(format!(
            "{} has no file name to back up",
            photo.display()
        )));
    };
    let mut backup_name = file_name.to_os_string();
    backup_name.push("_original");
    let backup = photo.with_file_name(backup_name);
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&backup)
    {
        Ok(mut file) => file
            .write_all(original_bytes)
            .and_then(|()| file.flush())
            .map_err(|source| Error::io(backup.display().to_string(), source)),
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(err) => Err(Error::io(backup.display().to_string(), err)),
    }
}

/// The decision for one photo, from its bytes and the track.
fn plan_one(photo: &Path, bytes: &[u8], track: &Track, options: &GeotagOptions) -> GeotagPlan {
    let raw_captured = match capture_time(photo, bytes) {
        Ok(captured) => captured,
        Err(reason) => return skipped(photo, None, None, options.keep_backup, reason),
    };
    let Some(captured) = shift_capture_time(raw_captured, options.photo_shift_seconds) else {
        return skipped(
            photo,
            None,
            None,
            options.keep_backup,
            format!(
                "shifting the capture time by {} s would put it outside the Unix \
                 range 1970-2106",
                options.photo_shift_seconds
            ),
        );
    };
    let Some((point, gap)) = nearest_positioned_point(track, captured.utc()) else {
        return skipped(
            photo,
            Some(captured),
            None,
            options.keep_backup,
            String::from("the track has no point with both a time and a position to compare with"),
        );
    };
    if gap > options.max_gap_seconds {
        return skipped(
            photo,
            Some(captured),
            Some(gap),
            options.keep_backup,
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
            Some(captured),
            Some(gap),
            options.keep_backup,
            String::from("the chosen track point has no complete coordinate"),
        );
    };
    if let Err(err) = exif::write_gps(bytes, &position) {
        return skipped(
            photo,
            Some(captured),
            Some(gap),
            options.keep_backup,
            format!("the photo's Exif block cannot be rewritten: {err}"),
        );
    }
    GeotagPlan {
        photo: photo.to_path_buf(),
        capture_time: Some(captured),
        point: Some(point),
        gap_seconds: Some(gap),
        skipped: None,
        keep_backup: options.keep_backup,
        group_id: None,
    }
}

fn shift_capture_time(captured: CaptureTime, shift_seconds: i64) -> Option<CaptureTime> {
    let shifted = i64::from(captured.utc()).checked_add(shift_seconds)?;
    let utc = u32::try_from(shifted).ok()?;
    Some(match captured {
        CaptureTime::Exif(_) => CaptureTime::Exif(utc),
        CaptureTime::FileModified(_) => CaptureTime::FileModified(utc),
    })
}

/// Whether `captured`, moved by `shift_seconds`, still lands inside the range
/// the plan and the EXIF writer can represent.
///
/// This is the check [`plan_one`] makes before it uses a shift, kept as its own
/// function so the failure can be exercised without a file: a shift that leaves
/// the Unix `u32` range would otherwise silently match against a wrapped time.
#[cfg(test)]
fn shift_fits(captured: CaptureTime, shift_seconds: i64) -> bool {
    shift_capture_time(captured, shift_seconds).is_some()
}

/// A plan for a photo that will not be tagged.
fn skipped(
    photo: &Path,
    capture_time: Option<CaptureTime>,
    gap_seconds: Option<u32>,
    keep_backup: bool,
    reason: String,
) -> GeotagPlan {
    GeotagPlan {
        photo: photo.to_path_buf(),
        capture_time,
        point: None,
        gap_seconds,
        skipped: Some(reason),
        keep_backup,
        group_id: None,
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
        // The modification time is deliberately *not* shifted by
        // `photo_shift_seconds`: `apply` would write a shifted mtime into
        // `DateTimeOriginal`, turning "when the file was copied" into "when the
        // shutter fired". The shift is for the camera's clock, and only a time
        // the camera wrote can be that.
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
    bytes.starts_with(&[0xFF, exif::JPEG_MARKER_SOI])
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

// ---------------------------------------------------------------------------
// Camera identity (Exif IFD0 `Make` / `Model`)
// ---------------------------------------------------------------------------

// `exif` owns the TIFF walk, but its reader is private to that module and its
// public functions read only the GPS IFD and `DateTimeOriginal`. `Make` and
// `Model` live in IFD0, so [`PhotoShiftScope::ByCamera`] would have no way to
// resolve a photo's camera without a small read-only walk here. It reads only
// those two tags and returns `None` on anything it cannot follow: a photo whose
// camera is unreadable must not make a whole folder run fail. A future change
// should move this into `exif` beside the other readers so the TIFF walk has one
// implementation.

/// TIFF field type 2: a NUL-terminated ASCII string.
const TIFF_TYPE_ASCII: u16 = 2;
/// Exif IFD0 tag holding the camera maker.
const TAG_MAKE: u16 = 0x010F;
/// Exif IFD0 tag holding the camera model.
const TAG_MODEL: u16 = 0x0110;

/// The byte order a TIFF header declares.
#[derive(Debug, Clone, Copy)]
enum Endian {
    /// `II`: little-endian.
    Little,
    /// `MM`: big-endian.
    Big,
}

impl Endian {
    fn read_u16(self, bytes: [u8; 2]) -> u16 {
        match self {
            Self::Little => u16::from_le_bytes(bytes),
            Self::Big => u16::from_be_bytes(bytes),
        }
    }

    fn read_u32(self, bytes: [u8; 4]) -> u32 {
        match self {
            Self::Little => u32::from_le_bytes(bytes),
            Self::Big => u32::from_be_bytes(bytes),
        }
    }
}

/// The TIFF block inside the first Exif APP1 segment, or `None`.
///
/// Walks JPEG segments the way [`crate::log::kmz`] walks them for the frame
/// size: `FF <marker>` then, for every marker that carries one, a two-byte
/// big-endian length that includes itself. The walk stops at the first Exif
/// APP1, at start-of-scan, or at end-of-image.
fn exif_tiff_block(jpeg: &[u8]) -> Option<&[u8]> {
    let mut cursor = jpeg.strip_prefix(&[0xFF, exif::JPEG_MARKER_SOI])?;
    loop {
        let marker = loop {
            let next = *cursor.first()?;
            cursor = cursor.get(1..)?;
            if next != 0xFF {
                break next;
            }
        };
        match marker {
            // Restart markers and TEM stand alone, without a length.
            0x01 | 0xD0..=exif::JPEG_MARKER_SOI => {}
            // Start of scan or end of image: no Exif segment can follow.
            0xDA | exif::JPEG_MARKER_EOI => return None,
            _ => {
                let length = u16::from_be_bytes([*cursor.first()?, *cursor.get(1)?]);
                let payload_len = usize::from(length).checked_sub(2)?;
                let payload = cursor.get(2..2usize.checked_add(payload_len)?)?;
                if marker == 0xE1
                    && let Some(tiff) = payload.strip_prefix(exif::EXIF_HEADER)
                {
                    return Some(tiff);
                }
                cursor = cursor.get(usize::from(length)..)?;
            }
        }
    }
}

/// The first IFD0 entry `wanted`, when it is an ASCII string.
///
/// `tiff` starts at the TIFF header (`II`/`MM`). Only the fields needed to reach
/// that one value are read; a value longer than four bytes lives at an offset
/// from the header, which is the layout cameras use for `Make` and `Model`.
fn ifd0_ascii(tiff: &[u8], wanted: u16) -> Option<String> {
    let endian = if tiff.starts_with(b"II") {
        Endian::Little
    } else if tiff.starts_with(b"MM") {
        Endian::Big
    } else {
        return None;
    };
    let ifd_offset = usize::try_from(endian.read_u32(tiff.get(4..8)?.try_into().ok()?)).ok()?;
    let count_at = tiff.get(ifd_offset..ifd_offset.checked_add(2)?)?;
    let entry_count = usize::from(endian.read_u16(count_at.try_into().ok()?));
    for index in 0..entry_count {
        let at = ifd_offset
            .checked_add(2)?
            .checked_add(index.checked_mul(12)?)?;
        let entry = tiff.get(at..at.checked_add(12)?)?;
        if endian.read_u16(entry.get(..2)?.try_into().ok()?) != wanted {
            continue;
        }
        if endian.read_u16(entry.get(2..4)?.try_into().ok()?) != TIFF_TYPE_ASCII {
            return None;
        }
        let value_len = usize::try_from(endian.read_u32(entry.get(4..8)?.try_into().ok()?)).ok()?;
        let field = entry.get(8..12)?;
        let value = if value_len <= 4 {
            field.get(..value_len)?
        } else {
            let value_offset = usize::try_from(endian.read_u32(field.try_into().ok()?)).ok()?;
            tiff.get(value_offset..value_offset.checked_add(value_len)?)?
        };
        let text = String::from_utf8_lossy(value);
        let trimmed = text.trim_end_matches(['\0', ' ']).trim();
        return if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_owned())
        };
    }
    None
}

/// Write `bytes` over `path` through a sibling temporary file.
///
/// A photo is the one file this crate modifies that cannot be regenerated, and a
/// crash halfway through writing it in place would leave a truncated JPEG at
/// the photo's own name. The temporary file is written first and renamed into
/// place, which is atomic on Unix; Windows refuses to rename over an existing
/// file, so only on Windows does this fall back to removing the old name first
/// (and if the follow-up rename fails, the temporary file is preserved on disk
/// and its path is included in the returned error so the photo is never lost).
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
    replace_from_temp(&temp, path)
}

#[cfg(not(windows))]
fn replace_from_temp(temp: &Path, path: &Path) -> Result<()> {
    std::fs::rename(temp, path).map_err(|source| {
        // On Unix, a failed rename leaves the original file at `path` intact,
        // so removing the temporary file cannot lose data.
        let _ = std::fs::remove_file(temp);
        Error::io(path.display().to_string(), source)
    })
}

#[cfg(windows)]
fn replace_from_temp(temp: &Path, path: &Path) -> Result<()> {
    // The closure is not cosmetic: passing `std::fs::rename` directly makes
    // rustc 1.100 on Windows fail with "implementation of FnMut is not general
    // enough" (the higher-ranked lifetimes of the function item are not
    // inferred through the generic parameter). Wrapping it fixes the inference
    // without weakening the helper's contract.
    replace_with_remove_fallback(temp, path, |from, to| std::fs::rename(from, to))
}

#[cfg(any(windows, test))]
fn replace_with_remove_fallback(
    temp: &Path,
    path: &Path,
    mut rename: impl FnMut(&Path, &Path) -> std::io::Result<()>,
) -> Result<()> {
    if rename(temp, path).is_ok() {
        return Ok(());
    }
    if let Err(source) = std::fs::remove_file(path) {
        // `path` was not removed, so the original photo is still intact.
        let _ = std::fs::remove_file(temp);
        return Err(Error::io(path.display().to_string(), source));
    }
    // `path` is already gone: `temp` is now the only remaining copy of the
    // photo and must NEVER be deleted if the rename fails.
    rename(temp, path).map_err(|source| {
        Error::io(
            format!(
                "{} (the rewritten photo is preserved at {})",
                path.display(),
                temp.display()
            ),
            source,
        )
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

    /// A synthetic JPEG taken at `utc` whose IFD0 names `make` and `model`.
    fn photo_taken_by(utc: u32, make: &str, model: &str) -> Vec<u8> {
        let stamp = format_datetime(utc);
        fixture::exif_jpeg(
            ByteOrder::Little,
            &[
                fixture::Entry::ascii(TAG_MAKE, make),
                fixture::Entry::ascii(TAG_MODEL, model),
            ],
            Some(&[
                fixture::Entry::ascii(exif::tag::GPS_LATITUDE_REF, "N"),
                fixture::Entry::ascii(exif::tag::GPS_LONGITUDE_REF, "E"),
            ]),
            Some(&stamp),
        )
    }

    /// A shift in the four units ezTour's dialog offers.
    fn time_shift(days: i64, hours: i64, minutes: i64, seconds: i64) -> PhotoTimeShift {
        PhotoTimeShift {
            days,
            hours,
            minutes,
            seconds,
        }
    }

    /// A camera identity from the two Exif strings.
    fn camera_of(make: Option<&str>, model: Option<&str>) -> CameraModel {
        CameraModel {
            make: make.map(str::to_owned),
            model: model.map(str::to_owned),
        }
    }

    /// `"YYYY:MM:DD HH:MM:SS"` for a Unix time, without pulling in a date crate.
    fn format_datetime(utc: u32) -> String {
        let days = i64::from(utc / 86_400);
        let seconds = utc % 86_400;
        let (year, month, day) = exif::civil_from_days(days);
        format!(
            "{year:04}:{month:02}:{day:02} {:02}:{:02}:{:02}",
            seconds / 3_600,
            (seconds % 3_600) / 60,
            seconds % 60
        )
    }

    #[test]
    fn the_nearest_point_in_time_is_the_one_chosen() {
        // A point before and a point after the shutter, neither exact.
        let track = track_at(&[SHUTTER - 12, SHUTTER + 4]);
        let dir = TempDir::new("nearest");
        let photo = dir.photo("nearer.jpg", &photo_taken_at(SHUTTER));

        let plans = plan(&[photo], &track, &GeotagOptions::default()).expect("plan");
        assert_eq!(plans.len(), 1);
        let chosen = &plans[0];
        assert!(chosen.skipped.is_none(), "skipped: {:?}", chosen.skipped);
        assert_eq!(chosen.capture_time, Some(CaptureTime::Exif(SHUTTER)));
        assert_eq!(chosen.gap_seconds, Some(4), "the later point is nearer");
        assert_eq!(chosen.point.expect("a point").utc, Some(SHUTTER + 4));
        assert!(chosen.will_write());
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
        assert!(
            !decision.will_write(),
            "an over-gap photo must not be written"
        );
        assert_eq!(decision.capture_time, Some(CaptureTime::Exif(SHUTTER)));
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
            ..GeotagOptions::default()
        };
        assert!(
            plan(std::slice::from_ref(&decision.photo), &track, &wider)
                .expect("plan")
                .first()
                .is_some_and(GeotagPlan::will_write)
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
        assert!(!decision.will_write());
        assert_eq!(decision.gap_seconds, None);
    }

    #[test]
    fn a_dry_run_leaves_the_photo_byte_for_byte_unchanged() {
        let track = track_at(&[SHUTTER]);
        let dir = TempDir::new("dry");
        let original = photo_taken_at(SHUTTER);
        let photo = dir.photo("dry.jpg", &original);
        let options = GeotagOptions::default();

        let plans = plan(std::slice::from_ref(&photo), &track, &options).expect("plan");
        assert!(plans.first().is_some_and(GeotagPlan::will_write));
        assert_eq!(
            std::fs::read(&photo).expect("read back"),
            original,
            "planning must not touch the photo"
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
        assert!(decision.will_write(), "skipped: {:?}", decision.skipped);
        assert_eq!(
            decision.capture_time,
            Some(CaptureTime::FileModified(SHUTTER))
        );
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
    fn a_non_jpeg_and_photos_with_unreadable_capture_time_are_skipped_by_name() {
        let dir = TempDir::new("mixed");
        let png = dir.photo("picture.png", b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR");
        let corrupt_time = dir.photo(
            "bad-time.jpg",
            &fixture::exif_jpeg(ByteOrder::Little, &[], None, Some("not-a-date")),
        );
        let pre_epoch_time = dir.photo(
            "pre-epoch.jpg",
            &fixture::exif_jpeg(ByteOrder::Little, &[], None, Some("1969:12:31 23:59:59")),
        );
        // Set mtime to SHUTTER to prove an out-of-range DateTimeOriginal does
        // not silently fall back to mtime.
        set_mtime(&pre_epoch_time, SHUTTER);
        let timed = dir.photo("timed.jpg", &photo_taken_at(SHUTTER));
        let track = track_at(&[SHUTTER]);

        let plans = plan(
            &[png, corrupt_time, pre_epoch_time, timed],
            &track,
            &GeotagOptions::default(),
        )
        .expect("plan");
        assert_eq!(plans.len(), 4);

        let png_plan = plans.first().expect("the PNG plan");
        assert!(!png_plan.will_write());
        assert_eq!(png_plan.capture_time, None);
        let reason = png_plan.skipped.as_deref().expect("a reason");
        assert!(
            reason.contains("not a JPEG"),
            "the skip reason must name the format: {reason}"
        );

        let corrupt_plan = plans.get(1).expect("the corrupt-time plan");
        assert!(!corrupt_plan.will_write());
        assert_eq!(corrupt_plan.capture_time, None);
        let reason = corrupt_plan.skipped.as_deref().expect("a reason");
        assert!(
            reason.contains("Exif could not be read"),
            "corrupt DateTimeOriginal should be skipped with Exif error: {reason}"
        );

        let pre_epoch_plan = plans.get(2).expect("the pre-epoch plan");
        assert!(!pre_epoch_plan.will_write());
        assert_eq!(pre_epoch_plan.capture_time, None);
        let reason = pre_epoch_plan.skipped.as_deref().expect("a reason");
        assert!(
            reason.contains("Exif could not be read"),
            "out-of-range DateTimeOriginal must not fall back to mtime: {reason}"
        );

        // Bad files do not stop the folder: the good photo is still planned.
        let good_plan = plans.get(3).expect("the timed photo plan");
        assert!(good_plan.will_write());
        assert_eq!(good_plan.capture_time, Some(CaptureTime::Exif(SHUTTER)));
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
    fn the_default_options_ask_for_five_minutes() {
        let options = GeotagOptions::default();
        // The user-visible number, not a copy of the constant: changing the
        // policy is meant to fail this line and be a decision.
        assert_eq!(options.max_gap_seconds, 300);
        assert_eq!(options.photo_shift_seconds, 0);
        assert_eq!(options.group_distance_meters, None);
        assert_eq!(options.max_photo_edge_pixels, None);
    }

    #[test]
    fn replace_file_preserves_target_and_cleans_temp_when_rename_fails() {
        let dir = TempDir::new("replace-dir");
        // Renaming a regular file over an existing directory fails on POSIX
        // (EISDIR); `replace_file` must leave the target untouched and clean up
        // its sibling temporary file.
        let target_dir = dir.0.join("not-a-file.jpg");
        std::fs::create_dir(&target_dir).expect("target directory");
        let temp = dir.0.join(".not-a-file.jpg.m241-tmp");

        let err = replace_file(&target_dir, b"new photo bytes")
            .expect_err("replacing a directory must fail");
        assert!(matches!(err, Error::Io { .. }), "got {err:?}");
        assert!(target_dir.is_dir(), "target must remain untouched");
        assert!(
            !temp.exists(),
            "temporary file should be cleaned up when original is untouched"
        );
    }

    #[test]
    fn remove_fallback_never_deletes_temp_after_removing_original() {
        let dir = TempDir::new("replace-preserve");
        let original = dir.photo("photo.jpg", b"old photo");
        let temp = dir.photo(".photo.jpg.m241-tmp", b"new rewritten photo");

        let err = replace_with_remove_fallback(&temp, &original, |_from, _to| {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "simulated rename failure",
            ))
        })
        .expect_err("second rename failure must return an error");

        assert!(matches!(err, Error::Io { .. }), "got {err:?}");
        assert!(
            !original.exists(),
            "test assumes remove_file removed the original"
        );
        assert!(
            temp.exists(),
            "temporary photo MUST be preserved when original is already removed"
        );
        assert_eq!(
            std::fs::read(&temp).expect("read preserved temp"),
            b"new rewritten photo"
        );
        let text = err.to_string();
        assert!(
            text.contains(&temp.display().to_string()),
            "error must report the preserved temporary path for recovery: {text}"
        );
    }

    #[test]
    fn remove_fallback_replaces_when_second_rename_succeeds() {
        let dir = TempDir::new("replace-retry");
        let original = dir.photo("photo.jpg", b"old photo");
        let temp = dir.photo(".photo.jpg.m241-tmp", b"new rewritten photo");
        let mut attempts = 0usize;

        replace_with_remove_fallback(&temp, &original, |from, to| {
            attempts += 1;
            if attempts == 1 {
                Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "simulated Windows rename-over-existing failure",
                ))
            } else {
                std::fs::rename(from, to)
            }
        })
        .expect("second rename succeeds after removing original");

        assert_eq!(attempts, 2);
        assert!(!temp.exists());
        assert_eq!(
            std::fs::read(&original).expect("read replaced photo"),
            b"new rewritten photo"
        );
    }

    #[test]
    fn keep_backup_writes_original_once_and_never_overwrites_existing_backup() {
        let dir = TempDir::new("backup");
        let original_bytes = photo_taken_at(SHUTTER);
        let photo = dir.photo("photo.jpg", &original_bytes);
        let backup = dir.0.join("photo.jpg_original");

        let track = track_at(&[SHUTTER]);
        let opts = GeotagOptions {
            keep_backup: true,
            ..GeotagOptions::default()
        };
        let plans = plan(std::slice::from_ref(&photo), &track, &opts).expect("plan");
        assert!(!backup.exists(), "plan must not write the backup");

        apply(&plans).expect("first apply");
        assert!(backup.exists(), "_original backup must be created");
        assert_eq!(
            std::fs::read(&backup).expect("read backup"),
            original_bytes,
            "_original must match the pre-tag photo"
        );
        let first_tagged = std::fs::read(&photo).expect("read tagged photo");
        assert_ne!(first_tagged, original_bytes);

        // Re-applying against a shifted track must NOT overwrite the existing _original.
        let moved_track = Track {
            points: vec![TrackPoint {
                utc: Some(SHUTTER),
                latitude: Some(34.693_7),
                longitude: Some(135.502_3),
                height_m: Some(20.0),
                speed_kmh: None,
                rcr: None,
            }],
            damaged_blocks: Vec::new(),
        };
        let plans2 = plan(std::slice::from_ref(&photo), &moved_track, &opts).expect("plan2");
        apply(&plans2).expect("second apply");
        assert_eq!(
            std::fs::read(&backup).expect("read preserved backup"),
            original_bytes,
            "existing _original backup must never be overwritten"
        );
        let tags = exif::read_gps(&std::fs::read(&photo).expect("read re-tagged"))
            .expect("read")
            .expect("Exif");
        assert!((tags.latitude.expect("lat") - 34.693_7).abs() < 1e-6);
    }

    #[test]
    fn distance_grouping_is_transitive_so_a_chain_is_one_group() {
        // A-B 10 m, B-C 10 m, A-C 20 m, threshold 15 m: two photos closer than
        // the threshold are always in the same group, so the chain is one group
        // (spec §6.1-5). A running anchor would answer "A alone, then B and C",
        // which is the behaviour this test was written to catch.
        let dir = TempDir::new("chain");
        let jpeg = photo_taken_at(SHUTTER);
        let (a, b, c) = (
            dir.photo("a.jpg", &jpeg),
            dir.photo("b.jpg", &jpeg),
            dir.photo("c.jpg", &jpeg),
        );
        // 1e-4 degrees of latitude is about 11 m, so these are ~11 m apart.
        assert!(
            (10.0..=12.0).contains(&haversine_meters(35.0, 139.0, 35.000_1, 139.0)),
            "the fixture steps must be about 11 m"
        );
        let point_at = |utc: u32, lat: f64| TrackPoint {
            utc: Some(utc),
            latitude: Some(lat),
            longitude: Some(139.0),
            ..TrackPoint::default()
        };
        let track = Track {
            points: vec![
                point_at(SHUTTER, 35.0),
                point_at(SHUTTER + 10, 35.000_1),
                point_at(SHUTTER + 20, 35.000_2),
            ],
            damaged_blocks: Vec::new(),
        };
        let opts = GeotagOptions {
            group_distance_meters: Some(15.0),
            ..GeotagOptions::default()
        };

        let plans = plan(&[a.clone(), b.clone(), c.clone()], &track, &opts).expect("plan");
        let ids: Vec<Option<usize>> = plans.iter().map(|plan| plan.group_id).collect();
        assert_eq!(
            ids,
            vec![Some(0), Some(0), Some(0)],
            "A-B and B-C are both under 15 m, so A and C share a group"
        );

        // The same three photos listed in the reverse order get the same
        // answer: grouping is a property of the places, not of the upload
        // order. Ids are renumbered by first appearance, so compare partitions.
        let reversed = plan(&[c, b, a], &track, &opts).expect("plan reversed");
        let ids: Vec<Option<usize>> = reversed.iter().map(|plan| plan.group_id).collect();
        assert_eq!(ids, vec![Some(0), Some(0), Some(0)]);
    }

    #[test]
    fn distance_grouping_keeps_two_separate_places_apart_in_either_order() {
        // Two clusters far apart: order must not merge them or split them.
        let dir = TempDir::new("clusters");
        // Each photo is nearest in time to one point: two at the same place,
        // one a kilometre east.
        let near_a = dir.photo("near-a.jpg", &photo_taken_at(SHUTTER));
        let far = dir.photo("far.jpg", &photo_taken_at(SHUTTER + 10));
        let near_b = dir.photo("near-b.jpg", &photo_taken_at(SHUTTER + 20));
        let point_at = |utc: u32, lon: f64| TrackPoint {
            utc: Some(utc),
            latitude: Some(35.0),
            longitude: Some(lon),
            ..TrackPoint::default()
        };
        // ~1 km east, ~11 m east: three points, two places.
        let track = Track {
            points: vec![
                point_at(SHUTTER, 139.0),
                point_at(SHUTTER + 10, 139.008),
                point_at(SHUTTER + 20, 139.000_1),
            ],
            damaged_blocks: Vec::new(),
        };
        let opts = GeotagOptions {
            group_distance_meters: Some(50.0),
            ..GeotagOptions::default()
        };
        let plans = plan(&[near_a, far, near_b], &track, &opts).expect("plan");
        assert_eq!(plans[0].group_id, Some(0));
        assert_eq!(plans[1].group_id, Some(1));
        assert_eq!(plans[2].group_id, Some(0));
    }

    #[test]
    fn a_photo_taken_near_the_epoch_is_skipped_when_shifted_before_it() {
        // `shift_capture_time` must refuse a shift that leaves the `u32` range
        // rather than wrapping. A photo shot at 00:00:40 on 1970-01-01 shifted
        // back by 100 s is the smallest reachable version of that: the shift is
        // valid, the shifted time is not.
        let dir = TempDir::new("shift-below-epoch");
        let photo = dir.photo("epoch.jpg", &photo_taken_at(40));
        let options = GeotagOptions {
            photo_shift_seconds: -100,
            ..GeotagOptions::default()
        };
        let plans = plan(
            std::slice::from_ref(&photo),
            &track_at(&[SHUTTER]),
            &options,
        )
        .expect("plan");
        assert!(
            !plans[0].will_write(),
            "a shift below the epoch must not produce a photo tagged at 2106"
        );
        assert!(
            plans[0].capture_time.is_none(),
            "no shifted time exists to report"
        );
        let reason = plans[0].skipped.as_deref().expect("a reason");
        assert!(
            reason.contains("-100") && reason.contains("outside"),
            "the reason must name the shift and the range: {reason}"
        );
    }

    #[test]
    fn the_longest_possible_shift_above_is_still_computed_exactly() {
        // The other end: a shift that stays in range must not be clamped or
        // wrapped on the way through `i64`.
        let shifted = shift_capture_time(CaptureTime::Exif(SHUTTER), 86_400)
            .expect("a day forward is representable");
        assert_eq!(shifted, CaptureTime::Exif(SHUTTER + 86_400));
    }

    #[test]
    fn a_shift_is_refused_when_it_leaves_the_representable_range() {
        // Mutation guard for `shift_capture_time`'s failure path: a shift below
        // the epoch must be reported, not wrapped into 2106.
        assert!(!shift_fits(CaptureTime::Exif(100), -200));
        assert!(!shift_fits(CaptureTime::FileModified(0), -1));
        assert!(!shift_fits(CaptureTime::Exif(u32::MAX), 1));
        assert!(
            shift_fits(CaptureTime::Exif(100), -100),
            "epoch itself is valid"
        );
        assert!(shift_fits(CaptureTime::Exif(1_700_000_000), 3_600));

        // And the same through `plan`, where the refusal becomes a reason the
        // caller can show.
        let dir = TempDir::new("shift-range");
        let photo = dir.photo("early.jpg", &photo_taken_at(100));
        let options = GeotagOptions {
            photo_shift_seconds: -200,
            ..GeotagOptions::default()
        };
        let plans = plan(
            std::slice::from_ref(&photo),
            &track_at(&[SHUTTER]),
            &options,
        )
        .expect("plan");
        assert!(!plans[0].will_write());
        let reason = plans[0].skipped.as_deref().expect("a reason");
        assert!(
            reason.contains("-200") && reason.contains("outside"),
            "the reason must name the shift and the range: {reason}"
        );
    }

    #[test]
    fn a_shift_is_written_into_the_photo_and_re_matches_without_it() {
        // Spec §6.1-7: after shifting, re-match automatically; §6.2: the shift
        // is done by rewriting DateTimeOriginal. The photo's clock was an hour
        // slow, so its 21:13:20 reading belongs at the track's 22:13:20.
        let dir = TempDir::new("shift-write");
        let photo = dir.photo("slow.jpg", &photo_taken_at(SHUTTER - 3_600));
        let track = track_at(&[SHUTTER]);

        let unshifted = plan(
            std::slice::from_ref(&photo),
            &track,
            &GeotagOptions::default(),
        )
        .expect("plan");
        assert!(
            !unshifted[0].will_write(),
            "an hour early is outside the gap"
        );

        let shifted = plan_with_shift(
            std::slice::from_ref(&photo),
            &track,
            &GeotagOptions::default(),
            3_600,
        )
        .expect("shifted plan");
        assert_eq!(shifted[0].capture_time, Some(CaptureTime::Exif(SHUTTER)));
        assert_eq!(shifted[0].gap_seconds, Some(0));
        assert_eq!(
            std::fs::read(&photo).expect("read"),
            photo_taken_at(SHUTTER - 3_600),
            "planning a shift must not write the photo"
        );

        apply(&shifted).expect("apply the shifted plan");
        let written = std::fs::read(&photo).expect("read the tagged photo");
        assert_eq!(
            exif::read_datetime_original(&written).expect("read DateTimeOriginal"),
            Some(SHUTTER),
            "the shifted capture time must be in the photo"
        );
        assert!(
            exif::read_gps(&written)
                .expect("read GPS")
                .is_some_and(|tags| tags.latitude.is_some()),
            "the position is tagged in the same rewrite"
        );

        // The point of writing it: a later run with no shift still matches.
        let plain = plan(
            std::slice::from_ref(&photo),
            &track,
            &GeotagOptions::default(),
        )
        .expect("plan without a shift");
        assert!(
            plain[0].will_write(),
            "the photo must no longer look an hour early: {:?}",
            plain[0].skipped
        );
        assert_eq!(plain[0].gap_seconds, Some(0));
    }

    #[test]
    fn a_shift_never_rewrites_a_modification_time_into_the_photo() {
        // The mtime is not a camera claim. Shifting it and writing it into
        // DateTimeOriginal would turn "when the file was copied" into "when the
        // shutter fired", so the shift moves the match and nothing else.
        let dir = TempDir::new("shift-mtime");
        let original = fixture::jpeg_without_exif();
        let photo = dir.photo("plain.jpg", &original);
        set_mtime(&photo, SHUTTER - 3_600);
        let track = track_at(&[SHUTTER]);

        let plans = plan_with_shift(
            std::slice::from_ref(&photo),
            &track,
            &GeotagOptions::default(),
            3_600,
        )
        .expect("shifted plan");
        assert_eq!(
            plans[0].capture_time,
            Some(CaptureTime::FileModified(SHUTTER))
        );
        apply(&plans).expect("apply");
        let written = std::fs::read(&photo).expect("read");
        assert_eq!(
            exif::read_datetime_original(&written).expect("read"),
            None,
            "a modified time must not be promoted to a capture time"
        );
        assert!(
            exif::read_gps(&written)
                .expect("read GPS")
                .is_some_and(|tags| tags.latitude.is_some()),
            "the position is still written"
        );
    }

    #[test]
    fn a_plan_becomes_a_kmz_photo_that_keeps_its_capture_time_provenance() {
        let dir = TempDir::new("to-kmz");
        let photo = dir.photo("holiday.jpg", &photo_taken_at(SHUTTER));
        let plans = plan(
            std::slice::from_ref(&photo),
            &track_at(&[SHUTTER + 5]),
            &GeotagOptions::default(),
        )
        .expect("plan");
        let entry = plans[0]
            .to_kmz_photo(Some(2048))
            .expect("a matched plan converts");
        assert_eq!(entry.name, "holiday.jpg");
        assert_eq!(entry.utc, Some(CaptureTime::Exif(SHUTTER)));
        assert_eq!(entry.max_edge_pixels, Some(2048));
        assert!((entry.lat - TOKYO_LAT).abs() < 1e-9);
        assert!((entry.lon - TOKYO_LON).abs() < 1e-9);
        assert_eq!(entry.altitude_m, Some(80.0));
        assert!(
            entry.jpeg_bytes.is_empty(),
            "the conversion does not copy the image; the caller reads the file"
        );

        // A skipped plan has no place to put a photo on the map.
        let far = plan(
            std::slice::from_ref(&photo),
            &track_at(&[SHUTTER + 10_000]),
            &GeotagOptions::default(),
        )
        .expect("plan");
        assert!(far[0].to_kmz_photo(None).is_none());
    }

    #[test]
    fn photo_shift_seconds_adjusts_capture_time_before_matching() {
        let dir = TempDir::new("shift");
        // Camera clock was 1 hour slow: photo says SHUTTER - 3600, track is at SHUTTER.
        let photo = dir.photo("slow-clock.jpg", &photo_taken_at(SHUTTER - 3_600));
        let track = track_at(&[SHUTTER]);

        let unshifted = plan(
            std::slice::from_ref(&photo),
            &track,
            &GeotagOptions::default(),
        )
        .expect("unshifted plan");
        assert!(!unshifted[0].will_write());

        let shifted_opts = GeotagOptions {
            photo_shift_seconds: 3_600,
            ..GeotagOptions::default()
        };
        let shifted = plan(std::slice::from_ref(&photo), &track, &shifted_opts).expect("shifted");
        assert!(shifted[0].will_write());
        assert_eq!(shifted[0].capture_time, Some(CaptureTime::Exif(SHUTTER)));
        assert_eq!(shifted[0].gap_seconds, Some(0));

        // Negative shift works too: camera 1 hour fast.
        let fast_photo = dir.photo("fast-clock.jpg", &photo_taken_at(SHUTTER + 3_600));
        let neg_opts = GeotagOptions {
            photo_shift_seconds: -3_600,
            ..GeotagOptions::default()
        };
        let neg = plan(&[fast_photo], &track, &neg_opts).expect("neg shifted");
        assert!(neg[0].will_write());
        assert_eq!(neg[0].gap_seconds, Some(0));

        // Shifting below Unix epoch skips cleanly instead of panicking or wrapping.
        let epoch_photo = dir.photo("early.jpg", &photo_taken_at(100));
        let underflow_opts = GeotagOptions {
            photo_shift_seconds: -200,
            ..GeotagOptions::default()
        };
        let underflow = plan(&[epoch_photo], &track, &underflow_opts).expect("underflow plan");
        assert!(!underflow[0].will_write());
        assert!(underflow[0].skipped.is_some());
    }

    #[test]
    fn the_day_hour_minute_and_second_fields_add_up_to_one_total() {
        assert_eq!(
            time_shift(1, 2, 3, 4).total_seconds(),
            Some(86_400 + 7_200 + 180 + 4)
        );
        assert_eq!(time_shift(0, 0, 0, 0).total_seconds(), Some(0));
        assert!(time_shift(0, 0, 0, 0).is_zero());
        assert!(!time_shift(0, 0, 0, 1).is_zero());
        assert!(PhotoTimeShift::default().is_zero());
        // The seconds-only form the old API carried is still exact.
        assert_eq!(PhotoTimeShift::from_seconds(-90).total_seconds(), Some(-90));
    }

    #[test]
    fn negative_shift_components_subtract_from_the_total() {
        assert_eq!(
            time_shift(-1, 2, -3, 4).total_seconds(),
            Some(-86_400 + 7_200 - 180 + 4)
        );
        assert_eq!(time_shift(0, -1, 0, 0).total_seconds(), Some(-3_600));
    }

    #[test]
    fn a_shift_that_overflows_seconds_is_refused_before_any_photo_is_planned() {
        let shift = PhotoTimeShift {
            days: i64::MAX,
            ..PhotoTimeShift::default()
        };
        assert_eq!(shift.total_seconds(), None, "the sum must report overflow");

        let dir = TempDir::new("shift-overflow");
        let photo = dir.photo("a.jpg", &photo_taken_at(SHUTTER));
        let err = plan_with_time_shift(
            std::slice::from_ref(&photo),
            &track_at(&[SHUTTER]),
            &GeotagOptions::default(),
            shift,
            &PhotoShiftScope::AllPhotos,
        )
        .expect_err("an overflowing shift must be refused");
        assert!(matches!(err, Error::State(_)), "got {err:?}");
        assert!(
            err.to_string().contains("does not fit"),
            "the error must say why: {err}"
        );
    }

    #[test]
    fn a_day_shift_moves_the_match_by_twenty_four_hours() {
        // The day field is 86,400 seconds: the photo a day early matches the
        // track once the clock is moved forward by one day.
        let dir = TempDir::new("shift-days");
        let original = photo_taken_at(SHUTTER - 86_400);
        let photo = dir.photo("a-day-early.jpg", &original);
        let track = track_at(&[SHUTTER]);

        let plans = plan_with_time_shift(
            std::slice::from_ref(&photo),
            &track,
            &GeotagOptions::default(),
            time_shift(1, 0, 0, 0),
            &PhotoShiftScope::AllPhotos,
        )
        .expect("plan");
        assert!(plans[0].will_write(), "skipped: {:?}", plans[0].skipped);
        assert_eq!(plans[0].capture_time, Some(CaptureTime::Exif(SHUTTER)));
        assert_eq!(plans[0].gap_seconds, Some(0));
        assert_eq!(
            std::fs::read(&photo).expect("read"),
            original,
            "planning a shift must not write the photo"
        );
    }

    #[test]
    fn an_all_photos_scope_shifts_every_photo() {
        let dir = TempDir::new("scope-all");
        let early = dir.photo("early.jpg", &photo_taken_at(SHUTTER - 3_600));
        let late = dir.photo("late.jpg", &photo_taken_at(SHUTTER + 3_600));
        let track = track_at(&[SHUTTER, SHUTTER + 7_200]);

        // Without a shift neither photo is within the gap: one is an hour
        // before its point and the other an hour after its own.
        let unshifted = plan(
            &[early.clone(), late.clone()],
            &track,
            &GeotagOptions::default(),
        )
        .expect("unshifted plan");
        assert!(unshifted.iter().all(|plan| !plan.will_write()));

        let plans = plan_with_time_shift(
            &[early, late],
            &track,
            &GeotagOptions::default(),
            time_shift(0, 1, 0, 0),
            &PhotoShiftScope::AllPhotos,
        )
        .expect("plan");
        assert!(
            plans.iter().all(GeotagPlan::will_write),
            "both clocks move under allPhotos: {:?}",
            plans
                .iter()
                .map(|plan| plan.skipped.clone())
                .collect::<Vec<_>>()
        );
        assert_eq!(plans[0].capture_time, Some(CaptureTime::Exif(SHUTTER)));
        assert_eq!(
            plans[1].capture_time,
            Some(CaptureTime::Exif(SHUTTER + 7_200))
        );
        assert_eq!(plans[0].gap_seconds, Some(0));
        assert_eq!(plans[1].gap_seconds, Some(0));
    }

    #[test]
    fn a_selected_photos_scope_shifts_only_the_listed_photos() {
        let dir = TempDir::new("scope-selected");
        let early = dir.photo("early.jpg", &photo_taken_at(SHUTTER - 3_600));
        let late = dir.photo("late.jpg", &photo_taken_at(SHUTTER + 3_600));
        let track = track_at(&[SHUTTER, SHUTTER + 7_200]);

        let scope = PhotoShiftScope::SelectedPhotos(vec![early.clone()]);
        let plans = plan_with_time_shift(
            &[early, late],
            &track,
            &GeotagOptions::default(),
            time_shift(0, 1, 0, 0),
            &scope,
        )
        .expect("plan");
        assert!(plans[0].will_write());
        assert_eq!(plans[0].capture_time, Some(CaptureTime::Exif(SHUTTER)));
        assert_eq!(
            plans[1].capture_time,
            Some(CaptureTime::Exif(SHUTTER + 3_600)),
            "the unlisted photo keeps its own clock"
        );
        assert!(
            !plans[1].will_write(),
            "the unshifted late photo stays outside the gap"
        );
    }

    #[test]
    fn a_by_camera_scope_shifts_only_photos_from_that_camera() {
        let dir = TempDir::new("scope-camera");
        let from_test_cam = dir.photo(
            "a.jpg",
            &photo_taken_by(SHUTTER - 3_600, "TestCam", "Model X"),
        );
        let from_other = dir.photo(
            "b.jpg",
            &photo_taken_by(SHUTTER + 3_600, "OtherCam", "Model Y"),
        );
        let track = track_at(&[SHUTTER, SHUTTER + 7_200]);
        let scope = PhotoShiftScope::ByCamera(camera_of(Some("TestCam"), Some("Model X")));

        let plans = plan_with_time_shift(
            &[from_test_cam, from_other],
            &track,
            &GeotagOptions::default(),
            time_shift(0, 1, 0, 0),
            &scope,
        )
        .expect("plan");
        assert!(plans[0].will_write());
        assert_eq!(plans[0].capture_time, Some(CaptureTime::Exif(SHUTTER)));
        assert_eq!(
            plans[1].capture_time,
            Some(CaptureTime::Exif(SHUTTER + 3_600)),
            "the other camera's photo keeps its own clock"
        );
        assert!(
            !plans[1].will_write(),
            "the unshifted other-camera photo stays outside the gap"
        );
    }

    #[test]
    fn a_photo_without_a_camera_tag_is_not_selected_by_a_camera_scope() {
        let dir = TempDir::new("scope-no-camera");
        let anonymous = dir.photo("anonymous.jpg", &photo_taken_at(SHUTTER - 3_600));
        let scope = PhotoShiftScope::ByCamera(camera_of(Some("TestCam"), Some("Model X")));

        let plans = plan_with_time_shift(
            std::slice::from_ref(&anonymous),
            &track_at(&[SHUTTER]),
            &GeotagOptions::default(),
            time_shift(0, 1, 0, 0),
            &scope,
        )
        .expect("plan");
        assert_eq!(
            plans[0].capture_time,
            Some(CaptureTime::Exif(SHUTTER - 3_600)),
            "a photo naming no camera must keep its clock"
        );
        assert!(!plans[0].will_write());
    }

    #[test]
    fn plan_with_shift_is_the_all_photos_form_of_the_scoped_plan() {
        // Regression: the seconds-only entry point that predates the scopes
        // must still produce the same plans, grouping included.
        let dir = TempDir::new("shift-regression");
        let early = dir.photo("early.jpg", &photo_taken_at(SHUTTER - 3_600));
        let late = dir.photo("late.jpg", &photo_taken_at(SHUTTER + 3_600));
        let track = track_at(&[SHUTTER, SHUTTER + 7_200]);
        let options = GeotagOptions {
            group_distance_meters: Some(50.0),
            ..GeotagOptions::default()
        };

        let legacy = plan_with_shift(&[early.clone(), late.clone()], &track, &options, 3_600)
            .expect("legacy plan");
        let scoped = plan_with_time_shift(
            &[early, late],
            &track,
            &options,
            time_shift(0, 1, 0, 0),
            &PhotoShiftScope::AllPhotos,
        )
        .expect("scoped plan");
        assert_eq!(
            legacy, scoped,
            "the seconds-only call must be exactly the all-photos form"
        );
    }

    #[test]
    fn the_camera_model_is_read_from_ifd0_make_and_model_in_both_byte_orders() {
        for order in [ByteOrder::Little, ByteOrder::Big] {
            let jpeg = fixture::exif_jpeg(
                order,
                &[
                    fixture::Entry::ascii(TAG_MAKE, "TestCam"),
                    fixture::Entry::ascii(TAG_MODEL, "Model X"),
                ],
                None,
                None,
            );
            assert_eq!(
                CameraModel::from_jpeg(&jpeg),
                Some(camera_of(Some("TestCam"), Some("Model X"))),
                "{order:?}: both tags must be read from the value area"
            );
        }
    }

    #[test]
    fn short_camera_names_that_fit_inline_are_read_too() {
        // A value of four bytes or fewer lives in the entry itself, which is a
        // different branch of the walk than the offset one above.
        let jpeg = fixture::exif_jpeg(
            ByteOrder::Little,
            &[
                fixture::Entry::ascii(TAG_MAKE, "AB"),
                fixture::Entry::ascii(TAG_MODEL, "M"),
            ],
            None,
            None,
        );
        assert_eq!(
            CameraModel::from_jpeg(&jpeg),
            Some(camera_of(Some("AB"), Some("M")))
        );
    }

    #[test]
    fn a_photo_that_names_no_camera_has_no_camera_model() {
        assert_eq!(
            CameraModel::from_jpeg(&photo_taken_at(SHUTTER)),
            None,
            "GPS and DateTimeOriginal do not name a camera"
        );
        assert_eq!(CameraModel::from_jpeg(&fixture::jpeg_without_exif()), None);
        assert_eq!(CameraModel::from_jpeg(b"not a JPEG"), None);
        let jpeg = photo_taken_by(SHUTTER, "TestCam", "Model X");
        assert_eq!(
            CameraModel::from_jpeg(&jpeg[..20]),
            None,
            "a truncated Exif block must be unknown, not a panic"
        );
    }

    #[test]
    fn distance_grouping_clusters_nearby_matched_photos() {
        let dir = TempDir::new("group");
        let p1 = dir.photo("p1.jpg", &photo_taken_at(SHUTTER));
        let p2 = dir.photo("p2.jpg", &photo_taken_at(SHUTTER + 10));
        let p3 = dir.photo("p3.jpg", &photo_taken_at(SHUTTER + 20));
        let p_skip = dir.photo("p_skip.jpg", &photo_taken_at(SHUTTER + 10_000));

        // p1 at Tokyo Station, p2 ~15m away, p3 in Osaka (~400km away).
        let track = Track {
            points: vec![
                TrackPoint {
                    utc: Some(SHUTTER),
                    latitude: Some(TOKYO_LAT),
                    longitude: Some(TOKYO_LON),
                    height_m: Some(10.0),
                    ..TrackPoint::default()
                },
                TrackPoint {
                    utc: Some(SHUTTER + 10),
                    latitude: Some(TOKYO_LAT + 0.000_1),
                    longitude: Some(TOKYO_LON + 0.000_1),
                    height_m: Some(10.0),
                    ..TrackPoint::default()
                },
                TrackPoint {
                    utc: Some(SHUTTER + 20),
                    latitude: Some(34.693_7),
                    longitude: Some(135.502_3),
                    height_m: Some(20.0),
                    ..TrackPoint::default()
                },
            ],
            damaged_blocks: Vec::new(),
        };

        let opts = GeotagOptions {
            group_distance_meters: Some(50.0),
            ..GeotagOptions::default()
        };
        let plans = plan(&[p1, p2, p3, p_skip], &track, &opts).expect("plan");
        assert_eq!(plans[0].group_id, Some(0));
        assert_eq!(plans[1].group_id, Some(0));
        assert_eq!(plans[2].group_id, Some(1));
        assert_eq!(plans[3].group_id, None);
    }
}
