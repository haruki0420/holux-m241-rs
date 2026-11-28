//! Offline log documents: one loaded track, its summary, its photos and its
//! exports.
//!
//! The Log screen never opens a serial port. A document arrives as an absolute
//! path the operator typed (or as the built-in sample), is read here, and stays
//! in memory while the screen asks for point slices, day rows, photo matches and
//! an export. Keeping the parsed [`Track`] in one place is what lets
//! `open_log` return a small summary instead of the whole point list, and what
//! makes a later `log_points` call cheap.
//!
//! # Why the path must be absolute
//!
//! The frontend cannot see a working directory that means anything to the user,
//! so a relative path would mean "relative to whatever the binary was started
//! from". Requiring an absolute path makes the command's contract independent of
//! the process's cwd, and makes it impossible to open a file by guessing.
//!
//! # Formats
//!
//! `.bin` is the raw dump (`parse_dump`); `.gpx`, `.kml` and `.kmz` go through
//! the matching importer. The extension is the dispatch, because the formats
//! share no reliable magic byte and the CLI splits them the same way.
//!
//! # The sample
//!
//! The sample is a deterministic synthetic track around the public coordinate
//! `35.6812360, 139.7671250` (Tokyo Station), with one waypoint per session. No
//! real track and no personal data is embedded: a sample that was somebody's
//! actual walk could not be shipped in a public repository.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use image::ImageEncoder as _;
use image::codecs::jpeg::JpegEncoder;

use m241_core::log::export::utc_date;
use m241_core::log::geotag::{self, GeotagOptions, GeotagPlan};
use m241_core::log::{
    CaptureTime, KmzPhoto, ParseReport, TrackPoint, export_kmz, import_gpx, import_kml, import_kmz,
    parse_dump, read_datetime_original, to_html, to_rtf,
};
use m241_core::{ExportFormat, Track, TrackStats};

/// The `source` value that selects the built-in sample instead of a path.
pub const SAMPLE_SOURCE: &str = "sample";

/// Largest document the screen will read, matching the egui Log screen's limit.
pub const MAX_LOG_BYTES: u64 = 16 * 1024 * 1024;

/// Largest number of points one `log_points` call may return.
///
/// The screen asks for the whole track in one go; this bound only stops a caller
/// from turning the IPC channel into a way to move an arbitrary amount of JSON.
pub const MAX_POINT_SLICE: usize = 50_000;

/// Largest photo the loader will read into memory.
const MAX_PHOTO_BYTES: usize = 24 * 1024 * 1024;

/// Longest edge of the JPEG thumbnail sent to the dock.
const THUMBNAIL_EDGE: u32 = 640;

/// The application state Tauri keeps for the Log screen.
///
/// The document is behind an `Arc<Mutex<…>>` so a blocking command can take its
/// own handle into `spawn_blocking` and keep the webview thread free. The
/// command still does not share the document with the frontend: it copies only
/// the slice it was asked for.
#[derive(Debug, Default)]
pub struct AppState {
    /// The document the screen is showing, if one has been opened.
    pub log: Arc<Mutex<Option<LoadedLog>>>,
}

/// One loaded document: the track plus everything derived from it.
#[derive(Debug)]
pub struct LoadedLog {
    /// Absolute path, or [`SAMPLE_SOURCE`].
    pub source: String,
    /// Display name (`file.bin` or the sample's name).
    pub name: String,
    /// The format the document was read from.
    pub format: DocumentFormat,
    /// The track itself.
    pub track: Track,
    /// What the parser had to skip (all zero for an importer).
    pub report: ParseReport,
    /// Photos the operator loaded for this track, matched by capture time.
    pub photos: Vec<LoadedPhoto>,
    /// Videos and other files from the same folder.
    pub media: Vec<MediaItem>,
}

/// The document formats the Log screen can open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentFormat {
    /// A raw `.bin` dump.
    Bin,
    /// A GPX document.
    Gpx,
    /// A KML document.
    Kml,
    /// A KMZ archive.
    Kmz,
    /// The built-in sample.
    Sample,
}

impl DocumentFormat {
    /// The lowercase label the frontend shows.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Bin => "bin",
            Self::Gpx => "gpx",
            Self::Kml => "kml",
            Self::Kmz => "kmz",
            Self::Sample => "sample",
        }
    }
}

/// One photo the operator loaded alongside the track.
#[derive(Debug, Clone)]
pub struct LoadedPhoto {
    /// Absolute path on this machine.
    pub path: PathBuf,
    /// File name for display.
    pub name: String,
    /// Capture time as Unix seconds.
    pub captured_at: Option<u32>,
    /// Where the time came from: `exif` or `file`.
    pub time_source: Option<&'static str>,
    /// Index of the nearest timed track point, when the photo is close enough.
    pub point_index: Option<usize>,
    /// Latitude of the matched point.
    pub lat: Option<f64>,
    /// Longitude of the matched point.
    pub lon: Option<f64>,
    /// A JPEG data URL, when the file could be decoded.
    pub thumbnail: Option<String>,
    /// Cluster id when distance grouping is on; `None` for an ungrouped photo.
    pub group_id: Option<usize>,
}

/// How the photo loader corrects clocks and groups nearby photos.
///
/// `07-eztour-spec.ja.md` §6.1 asks for four rules: nearest point, a maximum time
/// difference, timezone conversion and grouping by distance. The first two were
/// already here; this struct carries the other two, and [`match_photos`] turns
/// them into the same decisions `m241_core::log::geotag::GeotagOptions` makes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhotoOptions {
    /// Minutes added to each photo's capture time before matching.
    ///
    /// EXIF `DateTimeOriginal` has no timezone and [`read_datetime_original`]
    /// reads it as UTC, so a camera set to UTC+9 is nine hours away from the
    /// track. `540` is that camera; `0` keeps the old behaviour. The sign is
    /// "camera clock minus UTC", the same convention as the CLI's
    /// `--tz-offset-minutes`.
    pub camera_offset_minutes: i32,
    /// Whole days added to every track timestamp before matching.
    ///
    /// The same meaning as `m241 log geotag --utc-offset-days`: a dump from an
    /// unpatched unit is 7168 days behind, and a photo cannot be matched against
    /// a track that has not been moved. `0` leaves the track as parsed.
    pub log_offset_days: i32,
    /// Photos closer than this many metres share a group.
    ///
    /// `0` (the default) does no grouping, matching ezTour's configurable
    /// `groupPhotoDistance` rather than guessing its distribution default.
    pub group_distance_meters: f64,
    /// Largest accepted difference between the capture time and the nearest
    /// point, in seconds (ezTour's "Max photo/track diff time", in minutes,
    /// expressed here in seconds).
    pub max_gap_seconds: u32,
}

impl Default for PhotoOptions {
    fn default() -> Self {
        Self {
            camera_offset_minutes: 0,
            log_offset_days: 0,
            group_distance_meters: 0.0,
            max_gap_seconds: PHOTO_MAX_GAP_SECONDS,
        }
    }
}

/// A video or other file from the photo folder.
#[derive(Debug, Clone)]
pub struct MediaItem {
    /// File name.
    pub name: String,
    /// Size in bytes.
    pub bytes: u64,
    /// Modification time as Unix seconds.
    pub modified_at: Option<u32>,
    /// `video` or `other`.
    pub kind: &'static str,
}

/// A geographic box, as the frontend's `fitBounds` takes it.
#[derive(Debug, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Bounds {
    /// Western edge.
    pub west: f64,
    /// Southern edge.
    pub south: f64,
    /// Eastern edge.
    pub east: f64,
    /// Northern edge.
    pub north: f64,
}

/// One day of the track, for the track list.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DaySummary {
    /// `YYYY-MM-DD` in UTC.
    pub date: String,
    /// Points recorded that day.
    pub points: usize,
    /// Points with a position.
    pub fixes: usize,
    /// Distance in metres.
    pub distance_m: f64,
    /// Time between the first and last timestamp, in seconds.
    pub duration_seconds: Option<u32>,
    /// First timestamp of the day.
    pub first_utc: Option<u32>,
    /// Last timestamp of the day.
    pub last_utc: Option<u32>,
}

/// What the parser had to skip, flattened for the frontend.
#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportDto {
    /// Blocks fully or partly read.
    pub blocks: usize,
    /// Blocks never written.
    pub empty_blocks: usize,
    /// Blocks with a damaged record.
    pub damaged_blocks: usize,
    /// Records that became points.
    pub records: usize,
    /// Records whose checksum failed.
    pub bad_checksums: usize,
    /// Records skipped because the bytes ran out.
    pub truncated_records: usize,
    /// Settings-journal items skipped.
    pub journal_items: usize,
    /// Blocks whose header mask disagreed with the record length.
    pub layout_mismatched_blocks: usize,
    /// Bytes no record could explain.
    pub unparsed_bytes: usize,
}

/// The summary `open_log` returns. It deliberately carries no point list.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogSummary {
    /// Absolute path, or `sample`.
    pub source: String,
    /// Display name.
    pub name: String,
    /// Format label.
    pub format: String,
    /// Whether this is the built-in sample.
    pub sample: bool,
    /// Total records.
    pub points: usize,
    /// Records with a position.
    pub fixes: usize,
    /// Records flagged as manual waypoints.
    pub waypoints: usize,
    /// First timestamp.
    pub started_at: Option<u32>,
    /// Last timestamp.
    pub ended_at: Option<u32>,
    /// Bounding box of the positioned points.
    pub bounds: Option<Bounds>,
    /// Total distance in metres.
    pub distance_m: f64,
    /// Duration in seconds.
    pub duration_seconds: Option<u32>,
    /// Distance divided by duration.
    pub average_speed_kmh: Option<f64>,
    /// Largest speed any record carries.
    pub max_speed_kmh: Option<f32>,
    /// Lowest altitude.
    pub min_altitude_m: Option<f32>,
    /// Highest altitude.
    pub max_altitude_m: Option<f32>,
    /// One row per UTC day.
    pub days: Vec<DaySummary>,
    /// Parser diagnostics.
    pub report: ReportDto,
}

/// One point as the frontend draws and selects it.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PointDto {
    /// Index in the full track, so selection survives a slice.
    pub index: usize,
    /// UTC as Unix seconds.
    pub utc: Option<u32>,
    /// Latitude.
    pub lat: Option<f64>,
    /// Longitude.
    pub lon: Option<f64>,
    /// Height in metres.
    pub height_m: Option<f32>,
    /// Speed in km/h.
    pub speed_kmh: Option<f32>,
    /// Whether the recorder flagged this point as a manual waypoint.
    pub waypoint: bool,
}

/// One matched photo, as the dock and the map consume it.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoDto {
    /// Absolute path.
    pub path: String,
    /// File name.
    pub name: String,
    /// Capture time.
    pub captured_at: Option<u32>,
    /// `exif`, `file` or `null`.
    pub time_source: Option<String>,
    /// Index of the matched track point.
    pub point_index: Option<usize>,
    /// Latitude of the matched point.
    pub lat: Option<f64>,
    /// Longitude of the matched point.
    pub lon: Option<f64>,
    /// JPEG data URL, when the photo could be decoded.
    pub thumbnail: Option<String>,
    /// Cluster id when distance grouping is on.
    pub group_id: Option<usize>,
}

/// One media file row.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaItemDto {
    /// File name.
    pub name: String,
    /// Size in bytes.
    pub bytes: u64,
    /// Modification time.
    pub modified_at: Option<u32>,
    /// `video` or `other`.
    pub kind: String,
}

/// What `load_photos` returns.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoLoad {
    /// The folder that was read.
    pub directory: String,
    /// Photos, ordered by capture time.
    pub photos: Vec<PhotoDto>,
    /// Videos and other files in the same folder.
    pub media: Vec<MediaItemDto>,
    /// Files skipped, with the reason.
    pub skipped: Vec<String>,
}

/// What `export_log` returns.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportOutcome {
    /// The file that was written.
    pub destination: String,
    /// Format label.
    pub format: String,
    /// Bytes written.
    pub bytes: u64,
}

/// Photo matching options from the Log screen IPC contract.
#[derive(Debug, Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoOptionsDto {
    /// Minutes added to each photo's capture time relative to UTC.
    pub camera_offset_minutes: i32,
    /// Whole days added to every track timestamp before matching.
    pub log_offset_days: i32,
    /// Photos closer than this many metres share a group (`0` disables).
    pub group_distance_meters: f64,
    /// Maximum allowed gap between capture time and nearest track point, in seconds.
    #[serde(default = "default_photo_max_gap_seconds")]
    pub max_gap_seconds: u32,
}

fn default_photo_max_gap_seconds() -> u32 {
    PHOTO_MAX_GAP_SECONDS
}

impl From<PhotoOptionsDto> for PhotoOptions {
    fn from(dto: PhotoOptionsDto) -> Self {
        Self {
            camera_offset_minutes: dto.camera_offset_minutes,
            log_offset_days: dto.log_offset_days,
            group_distance_meters: dto.group_distance_meters,
            max_gap_seconds: dto.max_gap_seconds,
        }
    }
}

/// Options accepted by `geotag_preview` and `geotag_apply`.
#[derive(Debug, Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeotagOptionsDto {
    /// Minutes added to each photo's capture time relative to UTC.
    pub camera_offset_minutes: i32,
    /// Whole days added to every track timestamp before matching.
    pub log_offset_days: i32,
    /// Photos closer than this many metres share a group (`0` disables).
    pub group_distance_meters: f64,
    /// Maximum allowed gap between capture time and nearest track point, in seconds.
    pub max_gap_seconds: u32,
    /// Whether `geotag_apply` writes a `<photo>_original` backup before rewriting.
    pub keep_backup: bool,
    /// Whether `geotag_apply` rewrites `DateTimeOriginal` for EXIF-timed photos.
    pub rewrite_datetime_original: bool,
}

/// One photo row in a `geotag_preview` response.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GeotagItemDto {
    /// Absolute path to the photo.
    pub path: String,
    /// Display file name.
    pub name: String,
    /// Capture time as Unix seconds.
    pub captured_at: Option<u32>,
    /// `exif`, `file` or `null`.
    pub time_source: Option<String>,
    /// Matched track point index in the loaded track.
    pub point_index: Option<usize>,
    /// Matched latitude.
    pub lat: Option<f64>,
    /// Matched longitude.
    pub lon: Option<f64>,
    /// Gap in seconds to the nearest point.
    pub gap_seconds: Option<u32>,
    /// Cluster id when distance grouping is enabled.
    pub group_id: Option<usize>,
    /// Optional JPEG thumbnail data URL.
    pub thumbnail: Option<String>,
    /// Reason the photo was skipped, when `point_index` is `None`.
    pub skipped_reason: Option<String>,
}

/// Response returned by `geotag_preview`.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GeotagPreviewDto {
    /// Canonical directory path inspected.
    pub directory: String,
    /// Number of photos matched to a track point.
    pub matched: usize,
    /// Number of photos skipped.
    pub skipped: usize,
    /// Per-photo plan items.
    pub items: Vec<GeotagItemDto>,
}

/// Response returned by `geotag_apply`.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GeotagApplyOutcome {
    /// Number of photos whose EXIF tags were updated.
    pub written: usize,
    /// Number of photos skipped or failed.
    pub skipped: usize,
    /// Number of `<photo>_original` backups newly created by this run.
    pub backups_created: usize,
    /// Per-file error messages, if any write failed.
    pub errors: Vec<String>,
}

/// Read `source` into a loaded document.
///
/// `source` is [`SAMPLE_SOURCE`] or an absolute path ending in `.bin`, `.gpx`,
/// `.kml` or `.kmz`.
///
/// # Errors
///
/// A Japanese two-line string: what happened, then `対処: <next action>`.
pub fn read(source: &str) -> Result<LoadedLog, String> {
    if source == SAMPLE_SOURCE {
        return Ok(sample_log());
    }
    let path = absolute_file(source)?;
    let format = format_of(&path)?;
    let extension = format.label();
    let bytes = std::fs::read(&path).map_err(|error| {
        format!(
            "{} を読めません（{error}）\n対処: ファイルが存在するか、読み取り権限があるかを確認してください。",
            path.display()
        )
    })?;
    let (track, report) = match format {
        DocumentFormat::Bin => parse_dump(&bytes).map_err(|error| {
            format!(
                "{} は M-241 のログとして解析できません（{error}）\n対処: 端末から吸い出した .bin を選ぶか、GPX / KML / KMZ を開いてください。",
                path.display()
            )
        })?,
        DocumentFormat::Gpx | DocumentFormat::Kml => {
            let text = String::from_utf8(bytes).map_err(|_| {
                format!(
                    "{} は UTF-8 のテキストではありません\n対処: 文字コードを確認するか、別の GPX / KML を選んでください。",
                    path.display()
                )
            })?;
            let imported = if format == DocumentFormat::Gpx {
                import_gpx(&text)
            } else {
                import_kml(&text)
            };
            (
                imported.map_err(|error| {
                    format!(
                        "{} を {extension} として解析できません（{error}）\n対処: 対応する GPX / KML か確認するか、.bin を開いてください。",
                        path.display()
                    )
                })?,
                ParseReport::default(),
            )
        }
        DocumentFormat::Kmz => (
            import_kmz(&bytes).map_err(|error| {
                format!(
                    "{} を KMZ として解析できません（{error}）\n対処: 展開されていない KMZ か確認するか、.bin を開いてください。",
                    path.display()
                )
            })?,
            ParseReport::default(),
        ),
        DocumentFormat::Sample => {
            // `format_of` never returns this for a path; guard anyway.
            return Err(internal(
                "sample",
                "見本の読み込み経路が不正です。画面を開き直してください。",
            ));
        }
    };
    if track.points.is_empty() {
        return Err(format!(
            "{} には記録された点がありません\n対処: 記録のあるファイルを選ぶか、端末のログを吸い出してから開いてください。",
            path.display()
        ));
    }
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    Ok(LoadedLog {
        source: path.display().to_string(),
        name,
        format,
        track,
        report,
        photos: Vec::new(),
        media: Vec::new(),
    })
}

/// The summary of a loaded document.
#[must_use]
pub fn summary(loaded: &LoadedLog) -> LogSummary {
    let track = &loaded.track;
    let stats = TrackStats::of(track);
    let span = track.time_span();
    // Distance, duration and average exclude the gaps between sessions: the
    // chart compresses a gap longer than ten minutes to a sliver, so the totals
    // beside it must describe the same thing. Summing the whole span gave the
    // sample "24:44:30 / 0.3 km/h", which read as one continuous 24-hour walk.
    let (distance_m, duration_seconds) = gap_excluded_totals(track);
    let average_speed_kmh = duration_seconds
        .filter(|seconds| *seconds > 0)
        .map(|seconds| distance_m / f64::from(seconds) * 3.6);
    LogSummary {
        source: loaded.source.clone(),
        name: loaded.name.clone(),
        format: loaded.format.label().to_owned(),
        sample: loaded.format == DocumentFormat::Sample,
        points: stats.records,
        fixes: stats.fixes,
        waypoints: track
            .points
            .iter()
            .filter(|point| point.is_waypoint())
            .count(),
        started_at: span.map(|(start, _)| start),
        ended_at: span.map(|(_, end)| end),
        bounds: bounds_of(track),
        distance_m,
        duration_seconds,
        average_speed_kmh,
        max_speed_kmh: stats.max_speed_kmh,
        min_altitude_m: stats.min_altitude_m,
        max_altitude_m: stats.max_altitude_m,
        days: day_summaries(track, span),
        report: ReportDto {
            blocks: loaded.report.blocks,
            empty_blocks: loaded.report.empty_blocks,
            damaged_blocks: loaded.report.damaged_blocks,
            records: loaded.report.records,
            bad_checksums: loaded.report.bad_checksums,
            truncated_records: loaded.report.truncated_records,
            journal_items: loaded.report.journal_items,
            layout_mismatched_blocks: loaded.report.layout_mismatched_blocks,
            unparsed_bytes: loaded.report.unparsed_bytes,
        },
    }
}

/// Convert one track point for the frontend.
#[must_use]
pub fn point_dto(index: usize, point: &TrackPoint) -> PointDto {
    PointDto {
        index,
        utc: point.utc,
        lat: point.latitude,
        lon: point.longitude,
        height_m: point.height_m,
        speed_kmh: point.speed_kmh,
        waypoint: point.is_waypoint(),
    }
}

/// The `[from, to)` slice of a track as DTOs.
///
/// # Errors
///
/// A Japanese two-line string when the range is inverted, past the end, or wider
/// than [`MAX_POINT_SLICE`].
pub fn point_slice(loaded: &LoadedLog, from: usize, to: usize) -> Result<Vec<PointDto>, String> {
    let total = loaded.track.points.len();
    if from > to {
        return Err(format!(
            "点の範囲が逆になっています（from {from} > to {to}）\n対処: 範囲を指定し直してください。"
        ));
    }
    if to > total {
        return Err(format!(
            "点の範囲が記録の外を指しています（to {to} > 全 {total} 点）\n対処: ログを開き直してください。"
        ));
    }
    if to.saturating_sub(from) > MAX_POINT_SLICE {
        return Err(format!(
            "一度に返せる点の上限（{MAX_POINT_SLICE} 点）を超えています\n対処: 範囲を分けて読み込んでください。"
        ));
    }
    Ok(loaded.track.points[from..to]
        .iter()
        .enumerate()
        .map(|(offset, point)| point_dto(from.saturating_add(offset), point))
        .collect())
}

/// Read `directory` and match its photos to the loaded track.
///
/// A photo's capture time comes from its Exif `DateTimeOriginal`; when the file
/// has none, its modification time is used and the row says so. The photo is
/// matched to the nearest point with a position, within
/// [`PHOTO_MAX_GAP_SECONDS`], after [`PhotoOptions`] moves the two clocks onto
/// one timeline. The result replaces the photos already loaded.
///
/// # Errors
///
/// A Japanese two-line string when the path is relative, missing, not a
/// directory, or unreadable.
pub fn load_photos(
    loaded: &mut LoadedLog,
    directory: &str,
    options: &PhotoOptions,
) -> Result<PhotoLoad, String> {
    let path = absolute_directory(directory)?;
    let mut entries: Vec<PathBuf> = std::fs::read_dir(&path)
        .map_err(|error| {
            format!(
                "{} を読めません（{error}）\n対処: フォルダのパスと読み取り権限を確認してください。",
                path.display()
            )
        })?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|entry| entry.is_file())
        .collect();
    entries.sort();

    let mut photos = Vec::new();
    let mut media = Vec::new();
    let mut skipped = Vec::new();
    for file in entries {
        let extension = file
            .extension()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        let name = file
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| file.display().to_string());
        if is_image(&extension) {
            match read_photo(&file) {
                Ok(photo) => photos.push(photo),
                Err(reason) => skipped.push(format!("{name}: {reason}")),
            }
        } else {
            let metadata = std::fs::metadata(&file).ok();
            let bytes = metadata.as_ref().map_or(0, std::fs::Metadata::len);
            let modified_at = metadata
                .and_then(|value| value.modified().ok())
                .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|value| u32::try_from(value.as_secs()).unwrap_or(u32::MAX));
            media.push(MediaItem {
                name,
                bytes,
                modified_at,
                kind: if is_video(&extension) {
                    "video"
                } else {
                    "other"
                },
            });
        }
    }

    match_photos(loaded, photos, options);
    loaded.media = media;
    // Photos in capture order; a photo without a time keeps its name order after
    // the timed ones rather than jumping to the front as `None` would.
    loaded.photos.sort_by_key(|photo| {
        (
            photo.captured_at.is_none(),
            photo.captured_at,
            photo.name.clone(),
        )
    });

    Ok(PhotoLoad {
        directory: path.display().to_string(),
        photos: loaded.photos.iter().map(photo_dto).collect(),
        media: loaded.media.iter().map(media_dto).collect(),
        skipped,
    })
}

/// Match loaded photos to their nearest track point, in place.
///
/// The decisions are the four `07-eztour-spec.ja.md` §6.1 rules: the nearest
/// point, the maximum time difference ([`PhotoOptions::max_gap_seconds`]), the
/// clock conversion ([`PhotoOptions::camera_offset_minutes`] and
/// [`PhotoOptions::log_offset_days`]) and distance grouping
/// ([`PhotoOptions::group_distance_meters`], computed by
/// `m241_core::log::geotag::group_matched_plans`). An unmatched photo keeps its
/// row with `point_index = None`; it is not silently dropped.
///
/// `load_photos` needs the track to match against; keeping this separate makes
/// the matching testable on its own and keeps the file walk free of geometry.
pub fn match_photos(loaded: &mut LoadedLog, photos: Vec<LoadedPhoto>, options: &PhotoOptions) {
    let core_options = GeotagOptions {
        max_gap_seconds: options.max_gap_seconds,
        group_distance_meters: (options.group_distance_meters.is_finite()
            && options.group_distance_meters > 0.0)
            .then_some(options.group_distance_meters),
        ..GeotagOptions::default()
    };
    // Move the track into the photo's clock: `--utc-offset-days` shifts the
    // track by whole days, and `camera_offset_minutes` shifts it by the
    // camera's offset from UTC (e.g. +540 for JST, where `DateTimeOriginal` is
    // 9 hours ahead of UTC). Staying in `i64` avoids wrapping `u32` on a
    // 7168-day rollover offset.
    let track_to_photo_shift =
        i64::from(options.log_offset_days) * 86_400 + i64::from(options.camera_offset_minutes) * 60;
    let timed: Vec<(usize, i64)> = loaded
        .track
        .points
        .iter()
        .enumerate()
        .filter_map(|(index, point)| {
            point
                .utc
                .map(|utc| (index, i64::from(utc) + track_to_photo_shift))
        })
        .collect();

    let mut matched = photos;
    // One plan per photo, in the same order, so grouping can be computed by the
    // core helper and read back by index. A sentinel plan (no point) is kept for
    // an unmatched photo: `group_matched_plans` ignores it, and dropping it
    // would break the index mapping.
    let mut plans = Vec::with_capacity(matched.len());
    for photo in &mut matched {
        let mut plan = GeotagPlan {
            photo: photo.path.clone(),
            capture_time: photo.captured_at.map(|utc| match photo.time_source {
                Some("exif") => CaptureTime::Exif(utc),
                _ => CaptureTime::FileModified(utc),
            }),
            point: None,
            gap_seconds: None,
            skipped: None,
            keep_backup: false,
            group_id: None,
            rewrite_datetime_original: true,
        };
        let Some(captured) = photo.captured_at else {
            plan.skipped = Some(String::from("no capture time"));
            plans.push(plan);
            continue;
        };
        let Some((index, shifted_utc)) = timed
            .iter()
            .min_by_key(|(_, utc)| utc.abs_diff(i64::from(captured)))
            .copied()
        else {
            plan.skipped = Some(String::from("no track point carries a time"));
            plans.push(plan);
            continue;
        };
        let gap = shifted_utc.abs_diff(i64::from(captured));
        plan.gap_seconds = u32::try_from(gap).ok();
        if gap > u64::from(core_options.max_gap_seconds) {
            plan.skipped = Some(format!(
                "the nearest timed point is {gap} s from the capture time"
            ));
            plans.push(plan);
            continue;
        }
        let Some(point) = loaded.track.points.get(index).copied() else {
            plan.skipped = Some(String::from("the matched point disappeared"));
            plans.push(plan);
            continue;
        };
        let Some((lat, lon)) = point.position() else {
            plan.skipped = Some(String::from("the matched point has no position"));
            plans.push(plan);
            continue;
        };
        photo.point_index = Some(index);
        photo.lat = Some(lat);
        photo.lon = Some(lon);
        plan.point = Some(point);
        plans.push(plan);
    }

    if let Some(distance) = core_options.group_distance_meters {
        geotag::group_matched_plans(&mut plans, distance);
        for (photo, plan) in matched.iter_mut().zip(&plans) {
            photo.group_id = plan.group_id;
        }
    }
    loaded.photos = matched;
}

/// The largest gap between a photo's capture time and its matched point.
pub const PHOTO_MAX_GAP_SECONDS: u32 = 1_800;

/// Resolve `path` to the file it names without requiring that file to exist.
///
/// For an existing path this is `canonicalize` (so `..`, `.` and symlinks are
/// resolved). For a new file the parent is resolved and the file name appended,
/// which is enough to compare it with a source that does exist.
fn canonical_destination(path: &Path) -> Option<PathBuf> {
    if path.exists() {
        return std::fs::canonicalize(path).ok();
    }
    let parent = path.parent()?;
    let name = path.file_name()?;
    Some(std::fs::canonicalize(parent).ok()?.join(name))
}

/// Whether two paths name the same file.
///
/// Canonical paths are compared first; on Unix the device and inode are then
/// compared too, so a hard link to the source is the same file even though its
/// path is different.
fn same_file(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    let (Ok(left_meta), Ok(right_meta)) = (std::fs::metadata(left), std::fs::metadata(right))
    else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        left_meta.dev() == right_meta.dev() && left_meta.ino() == right_meta.ino()
    }
    #[cfg(not(unix))]
    {
        let (Ok(left_canonical), Ok(right_canonical)) =
            (std::fs::canonicalize(left), std::fs::canonicalize(right))
        else {
            return false;
        };
        left_canonical == right_canonical
    }
}

/// Export the loaded track in one of the formats the core writers provide.
///
/// # Errors
///
/// A Japanese two-line string when the source does not match the loaded
/// document, the destination is not absolute or cannot be written, or the format
/// is unknown.
pub fn export(
    loaded: &LoadedLog,
    format: &str,
    destination: &str,
) -> Result<ExportOutcome, String> {
    let destination = PathBuf::from(destination);
    if !destination.is_absolute() {
        return Err(String::from(
            "保存先は絶対パスで指定してください\n対処: フォルダを含むパス（例: /home/user/track.gpx）を入力してください。",
        ));
    }
    let source = Path::new(&loaded.source);
    // "Same place" must be decided on the files, not on the strings: `dir/sub/..`
    // names the source, and so does a symlink or a hard link to it. Resolving the
    // destination (and comparing inodes on Unix) is what makes the refusal hold
    // for every spelling of the same file.
    if source.is_absolute()
        && let Some(resolved) = canonical_destination(&destination)
        && same_file(source, &resolved)
    {
        return Err(String::from(
            "元のログと同じファイルには書き出せません\n対処: 別のファイル名を指定してください。",
        ));
    }
    if destination.is_dir() {
        return Err(format!(
            "{} はフォルダです\n対処: ファイル名まで含めた保存先を指定してください。",
            destination.display()
        ));
    }
    let label = format.trim().to_ascii_lowercase();
    let bytes = match label.as_str() {
        "gpx" | "kml" | "csv" | "nmea" | "json" => {
            let export_format = ExportFormat::parse(&label).ok_or_else(|| {
                format!(
                    "{label} は書き出せる形式ではありません\n対処: GPX / KML / CSV / NMEA / JSON / KMZ / HTML / RTF から選んでください。"
                )
            })?;
            m241_core::log::export(&loaded.track, export_format).into_bytes()
        }
        "html" => to_html(&loaded.track).into_bytes(),
        "rtf" => to_rtf(&loaded.track).into_bytes(),
        "kmz" => export_kmz(&loaded.track, &kmz_photos(loaded)).map_err(|error| {
            format!(
                "KMZ を書き出せません（{error}）\n対処: 写真の位置情報を確認するか、別の形式で書き出してください。"
            )
        })?,
        other => {
            return Err(format!(
                "{other} は書き出せる形式ではありません\n対処: GPX / KML / CSV / NMEA / JSON / KMZ / HTML / RTF から選んでください。"
            ));
        }
    };
    if let Some(parent) = destination.parent()
        && !parent.as_os_str().is_empty()
        && !parent.is_dir()
    {
        return Err(format!(
            "{} が見つかりません\n対処: 存在するフォルダを保存先に指定してください。",
            parent.display()
        ));
    }
    std::fs::write(&destination, &bytes).map_err(|error| {
        format!(
            "{} に書き出せません（{error}）\n対処: 保存先の権限と空き容量を確認してください。",
            destination.display()
        )
    })?;
    Ok(ExportOutcome {
        destination: destination.display().to_string(),
        format: label,
        bytes: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
    })
}

/// The sample document: two short sessions over two days.
///
/// The coordinates are a deterministic loop around the public reference point
/// used throughout this repository. The sessions are separate because the track
/// line must not draw a straight jump across a night; the day rows then show two
/// real groupings instead of one.
#[must_use]
pub fn sample_log() -> LoadedLog {
    let mut points: Vec<TrackPoint> = Vec::new();
    for session in sample_sessions() {
        for step in 0..session.points {
            let angle = f64::from(step) * std::f64::consts::TAU / f64::from(session.points);
            let wobble = (angle * 3.0).sin();
            let latitude = session.lat + angle.sin() * session.lat_radius + wobble * 0.0008;
            let longitude = session.lon + angle.cos() * session.lon_radius;
            // One slow swing per session: a sample whose speed oscillates every
            // few seconds would draw a sawtooth in the chart and read as noise.
            let speed = 14.0 + angle.sin() as f32 * 6.0;
            let height = 34.0 + angle.cos() as f32 * 11.0;
            points.push(TrackPoint {
                utc: Some(session.start + step * session.interval),
                latitude: Some(latitude),
                longitude: Some(longitude),
                height_m: Some(height),
                speed_kmh: Some(speed),
                rcr: Some(0),
            });
        }
    }
    // One waypoint per session, so the map's waypoint layer has something to
    // draw and the dock can say how many there are.
    for index in [40_usize, 150] {
        if let Some(point) = points.get_mut(index) {
            point.rcr = Some(0x0008);
        }
    }
    let track = Track {
        points,
        damaged_blocks: Vec::new(),
    };
    LoadedLog {
        source: String::from(SAMPLE_SOURCE),
        name: String::from("見本の記録"),
        format: DocumentFormat::Sample,
        track,
        report: ParseReport::default(),
        photos: Vec::new(),
        media: Vec::new(),
    }
}

/// One synthetic session of the sample.
struct SampleSession {
    start: u32,
    points: u32,
    interval: u32,
    lat: f64,
    lon: f64,
    lat_radius: f64,
    lon_radius: f64,
}

/// 2025-11-16T09:00:00Z, used as the first sample session's start.
const SAMPLE_START: u32 = 1_763_283_600;

fn sample_sessions() -> [SampleSession; 2] {
    [
        SampleSession {
            start: SAMPLE_START,
            points: 120,
            interval: 30,
            lat: 35.681_236,
            lon: 139.767_125,
            lat_radius: 0.004_2,
            lon_radius: 0.005_4,
        },
        SampleSession {
            start: SAMPLE_START + 86_400,
            points: 90,
            interval: 30,
            lat: 35.690_4,
            lon: 139.759_2,
            lat_radius: 0.003_1,
            lon_radius: 0.004_6,
        },
    ]
}

/// The photo list as KMZ placemarks.
fn kmz_photos(loaded: &LoadedLog) -> Vec<KmzPhoto> {
    let mut photos = Vec::new();
    for photo in &loaded.photos {
        let (Some(lat), Some(lon)) = (photo.lat, photo.lon) else {
            continue;
        };
        let Ok(bytes) = std::fs::read(&photo.path) else {
            continue;
        };
        let utc = photo.captured_at.map(|utc| match photo.time_source {
            Some("exif") => CaptureTime::Exif(utc),
            _ => CaptureTime::FileModified(utc),
        });
        photos.push(KmzPhoto {
            name: photo.name.clone(),
            lat,
            lon,
            altitude_m: None,
            utc,
            max_edge_pixels: Some(1_024),
            jpeg_bytes: bytes,
        });
    }
    photos
}

fn photo_dto(photo: &LoadedPhoto) -> PhotoDto {
    PhotoDto {
        path: photo.path.display().to_string(),
        name: photo.name.clone(),
        captured_at: photo.captured_at,
        time_source: photo.time_source.map(str::to_owned),
        point_index: photo.point_index,
        lat: photo.lat,
        lon: photo.lon,
        thumbnail: photo.thumbnail.clone(),
        group_id: photo.group_id,
    }
}

fn media_dto(item: &MediaItem) -> MediaItemDto {
    MediaItemDto {
        name: item.name.clone(),
        bytes: item.bytes,
        modified_at: item.modified_at,
        kind: item.kind.to_owned(),
    }
}

fn read_photo(path: &Path) -> Result<LoadedPhoto, String> {
    let metadata = std::fs::metadata(path).map_err(|error| error.to_string())?;
    let length = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
    if length > MAX_PHOTO_BYTES {
        return Err(String::from("写真の上限（24 MiB）を超えています"));
    }
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    let (captured_at, time_source) = match read_datetime_original(&bytes) {
        Ok(Some(utc)) => (Some(utc), Some("exif")),
        _ => (
            metadata
                .modified()
                .ok()
                .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|value| u32::try_from(value.as_secs()).unwrap_or(u32::MAX)),
            Some("file"),
        ),
    };
    let name = path
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    Ok(LoadedPhoto {
        path: path.to_path_buf(),
        name,
        captured_at,
        time_source,
        point_index: None,
        lat: None,
        lon: None,
        thumbnail: thumbnail(&bytes),
        group_id: None,
    })
}

/// Decode `bytes` and re-encode a small JPEG data URL.
///
/// Returns `None` when the file is not an image the `image` crate can decode; the
/// photo row still appears, with an empty frame, so the operator can see that the
/// file was found but cannot be shown.
fn thumbnail(bytes: &[u8]) -> Option<String> {
    let decoded = image::load_from_memory(bytes).ok()?;
    let longest = decoded.width().max(decoded.height());
    let scaled = if longest > THUMBNAIL_EDGE {
        let ratio = f64::from(THUMBNAIL_EDGE) / f64::from(longest);
        let width = (f64::from(decoded.width()) * ratio).round().max(1.0) as u32;
        let height = (f64::from(decoded.height()) * ratio).round().max(1.0) as u32;
        decoded.resize(width, height, image::imageops::FilterType::Triangle)
    } else {
        decoded
    };
    let rgb = scaled.to_rgb8();
    let mut output = Vec::new();
    let mut cursor = Cursor::new(&mut output);
    let encoder = JpegEncoder::new_with_quality(&mut cursor, 78);
    encoder
        .write_image(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
        )
        .ok()?;
    Some(format!("data:image/jpeg;base64,{}", BASE64.encode(&output)))
}

fn is_image(extension: &str) -> bool {
    matches!(extension, "jpg" | "jpeg" | "png")
}

fn is_video(extension: &str) -> bool {
    matches!(
        extension,
        "mp4" | "mov" | "m4v" | "avi" | "mkv" | "webm" | "mpg" | "mpeg"
    )
}

fn format_of(path: &Path) -> Result<DocumentFormat, String> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    match extension.as_str() {
        "bin" => Ok(DocumentFormat::Bin),
        "gpx" => Ok(DocumentFormat::Gpx),
        "kml" => Ok(DocumentFormat::Kml),
        "kmz" => Ok(DocumentFormat::Kmz),
        other => Err(format!(
            "{} は開ける形式ではありません（.{other}）\n対処: .bin / .gpx / .kml / .kmz のいずれかを選んでください。",
            path.display()
        )),
    }
}

fn absolute_file(source: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(source);
    if !path.is_absolute() {
        return Err(String::from(
            "ログは絶対パスで指定してください\n対処: フォルダを含むパス（例: /home/user/log.bin）を入力するか、ファイルを画面にドロップしてください。",
        ));
    }
    let metadata = std::fs::metadata(&path).map_err(|error| {
        format!(
            "{} が見つかりません（{error}）\n対処: パスを確認するか、ファイルを画面にドロップしてください。",
            path.display()
        )
    })?;
    if !metadata.is_file() {
        return Err(format!(
            "{} はファイルではありません\n対処: ログファイルそのものを指定してください。",
            path.display()
        ));
    }
    if metadata.len() > MAX_LOG_BYTES {
        return Err(format!(
            "{} は 16 MiB を超えています\n対処: 別のログファイルを選んでください。",
            path.display()
        ));
    }
    std::fs::canonicalize(&path).map_err(|error| {
        format!(
            "{} を解決できません（{error}）\n対処: パスを確認してください。",
            path.display()
        )
    })
}

fn absolute_directory(source: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(source);
    if !path.is_absolute() {
        return Err(String::from(
            "写真フォルダは絶対パスで指定してください\n対処: フォルダを含むパス（例: /home/user/photos）を入力してください。",
        ));
    }
    let metadata = std::fs::metadata(&path).map_err(|error| {
        format!(
            "{} が見つかりません（{error}）\n対処: パスを確認してください。",
            path.display()
        )
    })?;
    if !metadata.is_dir() {
        return Err(format!(
            "{} はフォルダではありません\n対処: 写真が入っているフォルダを指定してください。",
            path.display()
        ));
    }
    std::fs::canonicalize(&path).map_err(|error| {
        format!(
            "{} を解決できません（{error}）\n対処: パスを確認してください。",
            path.display()
        )
    })
}

fn bounds_of(track: &Track) -> Option<Bounds> {
    let mut bounds: Option<Bounds> = None;
    for (lat, lon) in track.points.iter().filter_map(TrackPoint::position) {
        bounds = Some(match bounds {
            None => Bounds {
                west: lon,
                south: lat,
                east: lon,
                north: lat,
            },
            Some(current) => Bounds {
                west: current.west.min(lon),
                south: current.south.min(lat),
                east: current.east.max(lon),
                north: current.north.max(lat),
            },
        });
    }
    bounds
}

/// A gap longer than this is not "moving": the chart draws it as a sliver, and
/// the totals and day rows leave it out of distance, duration and average.
const SEGMENT_GAP_SECONDS: u32 = 600;

/// Distance and moving time, summed per session.
///
/// A session ends when two timed points are more than [`SEGMENT_GAP_SECONDS`]
/// apart. `TrackStats::of` decides distance, duration and average on each
/// session with the same rules the rest of the toolkit uses, and the totals are
/// their sum, so the summary cannot drift from the segments the map draws.
fn gap_excluded_totals(track: &Track) -> (f64, Option<u32>) {
    let mut distance_m = 0.0;
    let mut duration_seconds: Option<u32> = None;
    let mut start = 0;
    let mut previous_utc: Option<u32> = None;
    for (index, point) in track.points.iter().enumerate() {
        if let (Some(previous), Some(utc)) = (previous_utc, point.utc)
            && utc.saturating_sub(previous) > SEGMENT_GAP_SECONDS
        {
            add_segment(
                &track.points[start..index],
                &mut distance_m,
                &mut duration_seconds,
            );
            start = index;
        }
        if point.utc.is_some() {
            previous_utc = point.utc;
        }
    }
    add_segment(
        track.points.get(start..).unwrap_or_default(),
        &mut distance_m,
        &mut duration_seconds,
    );
    (distance_m, duration_seconds)
}

fn add_segment(points: &[TrackPoint], distance_m: &mut f64, duration_seconds: &mut Option<u32>) {
    let segment = Track {
        points: points.to_vec(),
        damaged_blocks: Vec::new(),
    };
    let stats = TrackStats::of(&segment);
    *distance_m += stats.distance_m;
    if let Some(seconds) = stats.duration_seconds {
        *duration_seconds = Some(duration_seconds.unwrap_or(0).saturating_add(seconds));
    }
}

fn day_summaries(track: &Track, span: Option<(u32, u32)>) -> Vec<DaySummary> {
    if span.is_none() {
        return Vec::new();
    }
    // Bucket by UTC date in file order; the dump is oldest-first, so the rows
    // come out in the order the operator walked them.
    let mut buckets: Vec<(String, Vec<TrackPoint>)> = Vec::new();
    for point in &track.points {
        let Some(utc) = point.utc else {
            continue;
        };
        let (day, month, year) = utc_date(utc);
        let key = format!("{year:04}-{month:02}-{day:02}");
        match buckets.last_mut() {
            Some((last, points)) if *last == key => points.push(*point),
            _ => buckets.push((key, vec![*point])),
        }
    }
    buckets
        .into_iter()
        .map(|(date, points)| {
            let slice = Track {
                points,
                damaged_blocks: Vec::new(),
            };
            let stats = TrackStats::of(&slice);
            let span = slice.time_span();
            let (distance_m, duration_seconds) = gap_excluded_totals(&slice);
            DaySummary {
                date,
                points: stats.records,
                fixes: stats.fixes,
                distance_m,
                duration_seconds,
                first_utc: span.map(|(start, _)| start),
                last_utc: span.map(|(_, end)| end),
            }
        })
        .collect()
}

fn internal(what: &str, hint: &str) -> String {
    format!("ログ画面の内部状態が不正です（{what}）\n対処: {hint}")
}

fn shift_track_for_geotag(track: &Track, delta_seconds: i64) -> Track {
    let mut shifted = track.clone();
    if delta_seconds != 0 {
        for point in &mut shifted.points {
            point.utc = point
                .utc
                .and_then(|utc| u32::try_from(i64::from(utc) + delta_seconds).ok());
        }
    }
    shifted
}

fn nearest_point_index(track: &Track, utc: u32) -> Option<usize> {
    let mut best: Option<(usize, u32)> = None;
    for (index, point) in track.points.iter().enumerate() {
        let (Some(point_utc), true) = (point.utc, point.has_position()) else {
            continue;
        };
        let gap = point_utc.abs_diff(utc);
        if best.is_none_or(|(_, best_gap)| gap < best_gap) {
            best = Some((index, gap));
        }
    }
    best.map(|(index, _)| index)
}

fn geotag_image_files(directory: &Path) -> Result<Vec<PathBuf>, String> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(directory)
        .map_err(|error| {
            format!(
                "{} を読めません（{error}）\n対処: フォルダのパスと読み取り権限を確認してください。",
                directory.display()
            )
        })?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|entry| entry.is_file())
        .filter(|entry| {
            let ext = entry
                .extension()
                .and_then(|value| value.to_str())
                .map(str::to_ascii_lowercase)
                .unwrap_or_default();
            is_image(&ext)
        })
        .collect();
    entries.sort();
    Ok(entries)
}

fn plan_geotag(
    track: &Track,
    directory: &str,
    options: &GeotagOptionsDto,
) -> Result<(PathBuf, Vec<GeotagPlan>), String> {
    let dir_path = absolute_directory(directory)?;
    let photos = geotag_image_files(&dir_path)?;
    let log_shift_seconds = i64::from(options.log_offset_days) * 86_400;
    let camera_shift_seconds = i64::from(options.camera_offset_minutes) * 60;
    let matching_track = shift_track_for_geotag(track, log_shift_seconds);

    let core_options = GeotagOptions {
        max_gap_seconds: options.max_gap_seconds,
        keep_backup: options.keep_backup,
        photo_shift_seconds: -camera_shift_seconds,
        group_distance_meters: (options.group_distance_meters.is_finite()
            && options.group_distance_meters > 0.0)
            .then_some(options.group_distance_meters),
        max_photo_edge_pixels: None,
        rewrite_datetime_original: options.rewrite_datetime_original,
    };

    let plans = geotag::plan(&photos, &matching_track, &core_options).map_err(|error| {
        format!(
            "ジオタグ計画を作成できません（{error}）\n対処: フォルダ内の写真と読み取り権限を確認してください。"
        )
    })?;

    Ok((dir_path, plans))
}

fn backup_path_for(photo: &Path) -> Option<PathBuf> {
    let file_name = photo.file_name()?;
    let mut backup_name = file_name.to_os_string();
    backup_name.push("_original");
    Some(photo.with_file_name(backup_name))
}

/// Preview EXIF geotagging decisions for every image in `directory`.
///
/// Never modifies any photo file.
///
/// # Errors
///
/// A Japanese two-line string when `directory` is invalid or unreadable.
pub fn geotag_preview(
    loaded: &LoadedLog,
    directory: &str,
    options: &GeotagOptionsDto,
) -> Result<GeotagPreviewDto, String> {
    let (dir_path, plans) = plan_geotag(&loaded.track, directory, options)?;
    let log_shift_seconds = i64::from(options.log_offset_days) * 86_400;
    let matching_track = shift_track_for_geotag(&loaded.track, log_shift_seconds);

    let mut items = Vec::with_capacity(plans.len());
    let mut matched = 0usize;
    let mut skipped = 0usize;

    for plan in plans {
        let name = plan
            .photo
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| plan.photo.display().to_string());
        let (captured_at, time_source) = match plan.capture_time {
            Some(CaptureTime::Exif(utc)) => (Some(utc), Some(String::from("exif"))),
            Some(CaptureTime::FileModified(utc)) => (Some(utc), Some(String::from("file"))),
            None => (None, None),
        };
        let point_index = if plan.point.is_some() {
            captured_at.and_then(|utc| nearest_point_index(&matching_track, utc))
        } else {
            None
        };
        let (lat, lon) = match plan.point.and_then(|point| point.position()) {
            Some((lat, lon)) => (Some(lat), Some(lon)),
            None => (None, None),
        };
        if plan.will_write() {
            matched += 1;
        } else {
            skipped += 1;
        }
        let thumb = std::fs::metadata(&plan.photo)
            .ok()
            .filter(|meta| usize::try_from(meta.len()).unwrap_or(usize::MAX) <= MAX_PHOTO_BYTES)
            .and_then(|_| std::fs::read(&plan.photo).ok())
            .and_then(|bytes| thumbnail(&bytes));
        items.push(GeotagItemDto {
            path: plan.photo.display().to_string(),
            name,
            captured_at,
            time_source,
            point_index,
            lat,
            lon,
            gap_seconds: plan.gap_seconds,
            group_id: plan.group_id,
            thumbnail: thumb,
            skipped_reason: plan.skipped,
        });
    }

    Ok(GeotagPreviewDto {
        directory: dir_path.display().to_string(),
        matched,
        skipped,
        items,
    })
}

/// Apply EXIF GPS geotags to matched photos in `directory` via `m241_core::log::geotag`.
///
/// # Errors
///
/// A Japanese two-line string when `directory` is invalid or planning fails.
pub fn geotag_apply(
    loaded: &LoadedLog,
    directory: &str,
    options: &GeotagOptionsDto,
) -> Result<GeotagApplyOutcome, String> {
    let (_dir_path, plans) = plan_geotag(&loaded.track, directory, options)?;
    let mut written = 0usize;
    let mut skipped = 0usize;
    let mut backups_created = 0usize;
    let mut errors = Vec::new();

    for plan in &plans {
        if !plan.will_write() {
            skipped += 1;
            continue;
        }
        let backup_path = backup_path_for(&plan.photo);
        let had_backup = backup_path.as_deref().is_some_and(Path::exists);
        match geotag::apply(std::slice::from_ref(plan)) {
            Ok(paths) if !paths.is_empty() => {
                written += 1;
                if plan.keep_backup
                    && !had_backup
                    && backup_path.as_deref().is_some_and(Path::exists)
                {
                    backups_created += 1;
                }
            }
            Ok(_) => {
                skipped += 1;
            }
            Err(error) => {
                skipped += 1;
                errors.push(format!("{}: {error}", plan.photo.display()));
            }
        }
    }

    Ok(GeotagApplyOutcome {
        written,
        skipped,
        backups_created,
        errors,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> LoadedLog {
        sample_log()
    }

    #[test]
    fn the_sample_holds_two_days_and_a_waypoint_per_session() {
        let loaded = sample();
        let summary = summary(&loaded);
        assert_eq!(summary.days.len(), 2, "two sample sessions");
        assert_eq!(summary.waypoints, 2);
        assert!(summary.points > 200);
        assert_eq!(summary.points, summary.fixes);
        assert!(summary.sample);
        assert_eq!(summary.format, "sample");
    }

    #[test]
    fn the_summary_excludes_the_gap_between_sessions() {
        // The two sample sessions are a day apart. The span is 89,070 s
        // (24:44:30), but the operator was moving for 6,240 s; the chart draws
        // the overnight gap as a sliver, so the totals must describe the same
        // thing. This is the "24:44:30 / 0.3 km/h" the review found misleading.
        let summary = summary(&sample());
        assert_eq!(summary.duration_seconds, Some(6_240));
        assert!(
            summary.distance_m > 5_000.0 && summary.distance_m < 6_500.0,
            "got {}",
            summary.distance_m
        );
        let average = summary.average_speed_kmh.expect("average speed");
        assert!(average > 2.0 && average < 6.0, "got {average}");
        assert_eq!(summary.days[0].duration_seconds, Some(3_570));
        assert_eq!(summary.days[1].duration_seconds, Some(2_670));
    }

    #[test]
    fn the_sample_bounds_contain_the_reference_coordinate() {
        let summary = summary(&sample());
        let bounds = summary.bounds.expect("sample has a bounding box");
        assert!(bounds.south < 35.681_236 && bounds.north > 35.681_236);
        assert!(bounds.west < 139.767_125 && bounds.east > 139.767_125);
    }

    #[test]
    fn a_point_slice_keeps_the_absolute_indices() {
        let loaded = sample();
        let points = point_slice(&loaded, 10, 15).expect("valid range");
        assert_eq!(points.len(), 5);
        assert_eq!(points[0].index, 10);
        assert_eq!(points[4].index, 14);
    }

    #[test]
    fn an_inverted_or_out_of_range_slice_is_refused_with_two_lines() {
        let loaded = sample();
        for result in [
            point_slice(&loaded, 10, 5),
            point_slice(&loaded, 0, loaded.track.points.len() + 1),
        ] {
            let text = result.expect_err("refused");
            assert_eq!(text.lines().count(), 2, "got {text}");
            assert!(text.contains("\n対処: "), "got {text}");
        }
    }

    #[test]
    fn a_relative_path_is_refused_with_the_absolute_path_hint() {
        let text = read("log.bin").expect_err("refused");
        assert!(text.contains("絶対パス"), "got {text}");
        assert!(text.contains("対処: "), "got {text}");
    }

    #[test]
    fn an_unsupported_extension_names_the_four_formats() {
        let text = format_of(Path::new("/tmp/track.txt")).expect_err("refused");
        assert!(text.contains(".bin"), "got {text}");
        assert!(text.contains(".gpx"), "got {text}");
    }

    /// Build a two-record dump: block header, two records, FF padding.
    fn synthetic_dump() -> Vec<u8> {
        let mut block = vec![0xFF_u8; 0x1_0000];
        // Header: count 2, mask UTC|VALID|LATITUDE|LONGITUDE|HEIGHT|SPEED, policy
        // "logging enabled", one-second period.
        block[0..2].copy_from_slice(&2_u16.to_le_bytes());
        let mask: u32 = 0b11_1111;
        block[2..6].copy_from_slice(&mask.to_le_bytes());
        block[6..8].copy_from_slice(&0x0106_u16.to_le_bytes());
        block[8..12].copy_from_slice(&10_u32.to_le_bytes());
        let mut offset = 0x200;
        for step in 0..2_u32 {
            let mut record = Vec::new();
            record.extend_from_slice(&(1_763_283_600 + step * 30).to_le_bytes());
            record.extend_from_slice(&0_u16.to_le_bytes());
            record.extend_from_slice(&(35.681_236_f32).to_le_bytes());
            record.extend_from_slice(&(139.767_13_f32).to_le_bytes());
            // Height is the top three bytes of a little-endian f32.
            let height = 42.0_f32.to_le_bytes();
            record.extend_from_slice(&height[1..4]);
            record.extend_from_slice(&18.0_f32.to_le_bytes());
            let checksum = record.iter().fold(0_u8, |acc, byte| acc ^ byte);
            record.push(checksum);
            let end = offset + record.len();
            block[offset..end].copy_from_slice(&record);
            offset = end;
        }
        block
    }

    #[test]
    fn a_bin_dump_is_opened_and_summarised() {
        let directory = temp_dir("m241-app-bin");
        let path = directory.join("track.bin");
        std::fs::write(&path, synthetic_dump()).expect("write dump");
        let loaded = read(&path.display().to_string()).expect("open dump");
        assert_eq!(loaded.format, DocumentFormat::Bin);
        let summary = summary(&loaded);
        assert_eq!(summary.points, 2);
        assert_eq!(summary.fixes, 2);
        assert_eq!(summary.waypoints, 0);
        assert_eq!(summary.started_at, Some(1_763_283_600));
        assert_eq!(loaded.report.blocks, 1);
        assert_eq!(loaded.report.records, 2);
        std::fs::remove_dir_all(&directory).ok();
    }

    #[test]
    fn a_kml_and_a_kmz_export_can_be_opened_again() {
        let loaded = sample();
        let directory = temp_dir("m241-app-import");
        for (format, extension) in [("kml", "kml"), ("kmz", "kmz")] {
            let destination = directory.join(format!("track.{extension}"));
            export(&loaded, format, &destination.display().to_string()).expect("export");
            let reopened = read(&destination.display().to_string())
                .unwrap_or_else(|error| panic!("{format}: {error}"));
            // KML writes waypoints as separate Placemarks, which the importer
            // reads back as extra points; the track itself must not lose any.
            assert!(
                reopened.track.points.len() >= loaded.track.points.len(),
                "{format}: {} < {}",
                reopened.track.points.len(),
                loaded.track.points.len()
            );
        }
        std::fs::remove_dir_all(&directory).ok();
    }

    #[test]
    fn a_gpx_export_can_be_opened_again() {
        let loaded = sample();
        let directory = std::env::temp_dir().join(format!(
            "m241-app-logdoc-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|value| value.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&directory).expect("temp dir");
        let destination = directory.join("track.gpx");
        let outcome = export(&loaded, "gpx", &destination.display().to_string()).expect("export");
        assert_eq!(outcome.format, "gpx");
        assert!(outcome.bytes > 0);
        let reopened = read(&destination.display().to_string()).expect("reopen the export");
        assert_eq!(reopened.track.points.len(), loaded.track.points.len());
        assert_eq!(reopened.format, DocumentFormat::Gpx);
        std::fs::remove_dir_all(&directory).ok();
    }

    #[test]
    fn every_export_format_produces_bytes() {
        let loaded = sample();
        let directory = std::env::temp_dir().join(format!(
            "m241-app-export-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|value| value.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&directory).expect("temp dir");
        for format in ["gpx", "kml", "csv", "nmea", "json", "kmz", "html", "rtf"] {
            let destination = directory.join(format!("track.{format}"));
            let outcome = export(&loaded, format, &destination.display().to_string())
                .unwrap_or_else(|error| panic!("{format}: {error}"));
            assert!(outcome.bytes > 0, "{format} produced no bytes");
        }
        std::fs::remove_dir_all(&directory).ok();
    }

    fn temp_dir(prefix: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|value| value.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&directory).expect("temp dir");
        directory
    }

    fn write_jpeg(path: &Path) {
        let image = image::RgbImage::from_fn(16, 12, |x, y| {
            image::Rgb([(x * 12).min(255) as u8, (y * 18).min(255) as u8, 96])
        });
        let mut file = std::fs::File::create(path).expect("create jpeg");
        let encoder = JpegEncoder::new_with_quality(&mut file, 80);
        encoder
            .write_image(image.as_raw(), 16, 12, image::ExtendedColorType::Rgb8)
            .expect("encode jpeg");
    }

    #[test]
    fn photos_are_thumbnailed_and_matched_to_the_nearest_point() {
        let directory = temp_dir("m241-app-photos");
        let photo = directory.join("photo.jpg");
        write_jpeg(&photo);
        // The first sample point's capture time, so the match is the first point.
        let taken = std::time::UNIX_EPOCH + std::time::Duration::from_secs(u64::from(SAMPLE_START));
        let handle = std::fs::File::options()
            .write(true)
            .open(&photo)
            .expect("open photo");
        handle
            .set_times(std::fs::FileTimes::new().set_modified(taken))
            .expect("set mtime");
        drop(handle);
        std::fs::write(directory.join("clip.mp4"), b"not really a video").expect("media");

        let mut loaded = sample();
        let result = load_photos(
            &mut loaded,
            &directory.display().to_string(),
            &PhotoOptions::default(),
        )
        .expect("load");
        assert_eq!(result.photos.len(), 1);
        assert_eq!(result.photos[0].point_index, Some(0));
        assert_eq!(result.photos[0].time_source.as_deref(), Some("file"));
        assert!(
            result.photos[0]
                .thumbnail
                .as_deref()
                .is_some_and(|url| url.starts_with("data:image/jpeg;base64,")),
            "thumbnail was not a JPEG data URL"
        );
        assert_eq!(result.media.len(), 1);
        assert_eq!(result.media[0].kind, "video");
        assert_eq!(loaded.photos.len(), 1);
        std::fs::remove_dir_all(&directory).ok();
    }

    #[test]
    fn exporting_over_the_source_itself_is_refused() {
        let loaded = sample();
        let directory = temp_dir("m241-app-overwrite");
        let destination = directory.join("track.gpx");
        export(&loaded, "gpx", &destination.display().to_string()).expect("first export");
        let reopened = read(&destination.display().to_string()).expect("reopen");
        let text = export(&reopened, "gpx", &destination.display().to_string())
            .expect_err("overwrite refused");
        assert!(text.contains("同じファイル"), "got {text}");
        assert!(text.contains("対処: "), "got {text}");
        std::fs::remove_dir_all(&directory).ok();
    }

    #[test]
    fn exporting_through_a_dot_dot_path_over_the_source_is_refused() {
        // The source and `dir/sub/../track.gpx` are the same file, but the
        // strings differ. `canonicalize` is what closes the hole the review
        // reproduced: without it this overwrote the source (828 bytes written).
        let loaded = sample();
        let directory = temp_dir("m241-app-overwrite-dotdot");
        let destination = directory.join("track.gpx");
        export(&loaded, "gpx", &destination.display().to_string()).expect("first export");
        std::fs::create_dir(directory.join("sub")).expect("sub");
        let reopened = read(&destination.display().to_string()).expect("reopen");
        let disguised = directory.join("sub").join("..").join("track.gpx");
        let before = std::fs::read(&destination).expect("source before");
        let text = export(&reopened, "gpx", &disguised.display().to_string())
            .expect_err("dot-dot spelling must be refused");
        assert!(text.contains("同じファイル"), "got {text}");
        let after = std::fs::read(&destination).expect("source after");
        assert_eq!(after, before, "the source must not be rewritten");
        std::fs::remove_dir_all(&directory).ok();
    }

    #[cfg(unix)]
    #[test]
    fn exporting_through_a_symlink_to_the_source_is_refused() {
        let loaded = sample();
        let directory = temp_dir("m241-app-overwrite-symlink");
        let destination = directory.join("track.gpx");
        export(&loaded, "gpx", &destination.display().to_string()).expect("first export");
        let link = directory.join("innocent.gpx");
        std::os::unix::fs::symlink(&destination, &link).expect("symlink");
        let reopened = read(&destination.display().to_string()).expect("reopen");
        let text = export(&reopened, "gpx", &link.display().to_string())
            .expect_err("a symlink to the source must be refused");
        assert!(text.contains("同じファイル"), "got {text}");
        std::fs::remove_dir_all(&directory).ok();
    }

    #[cfg(unix)]
    #[test]
    fn exporting_over_a_hard_link_to_the_source_is_refused() {
        // A hard link has a different path and canonicalises to itself, so only
        // the inode comparison catches it.
        let loaded = sample();
        let directory = temp_dir("m241-app-overwrite-hardlink");
        let destination = directory.join("track.gpx");
        export(&loaded, "gpx", &destination.display().to_string()).expect("first export");
        let link = directory.join("hardlink.gpx");
        std::fs::hard_link(&destination, &link).expect("hard link");
        let reopened = read(&destination.display().to_string()).expect("reopen");
        let text = export(&reopened, "gpx", &link.display().to_string())
            .expect_err("a hard link to the source must be refused");
        assert!(text.contains("同じファイル"), "got {text}");
        std::fs::remove_dir_all(&directory).ok();
    }

    #[test]
    fn a_relative_destination_is_refused() {
        let loaded = sample();
        let relative = export(&loaded, "gpx", "track.gpx").expect_err("relative destination");
        assert!(relative.contains("絶対パス"), "got {relative}");
        assert!(relative.contains("対処: "), "got {relative}");
    }

    /// Set a JPEG's modification time, which the loader uses when the file has
    /// no Exif capture time.
    fn set_modified(path: &Path, utc: u32) {
        let taken = std::time::UNIX_EPOCH + std::time::Duration::from_secs(u64::from(utc));
        let handle = std::fs::File::options()
            .write(true)
            .open(path)
            .expect("open photo");
        handle
            .set_times(std::fs::FileTimes::new().set_modified(taken))
            .expect("set mtime");
        drop(handle);
    }

    #[test]
    fn a_camera_offset_puts_a_utc_plus_nine_photo_on_the_track() {
        // `DateTimeOriginal` has no timezone; the loader reads it as UTC. A
        // camera set to JST records a wall-clock time nine hours ahead, so with
        // no offset the photo is 32,400 s from every point and must not match.
        let directory = temp_dir("m241-app-photo-offset");
        let photo = directory.join("jst.jpg");
        write_jpeg(&photo);
        set_modified(&photo, SAMPLE_START + 9 * 3_600);

        let mut loaded = sample();
        let matched = load_photos(
            &mut loaded,
            &directory.display().to_string(),
            &PhotoOptions::default(),
        )
        .expect("load without an offset");
        assert_eq!(
            matched.photos[0].point_index, None,
            "a JST capture must not match a UTC track without the offset"
        );

        let inverted = PhotoOptions {
            camera_offset_minutes: -540,
            ..PhotoOptions::default()
        };
        let wrong_sign = load_photos(&mut loaded, &directory.display().to_string(), &inverted)
            .expect("load with the inverted offset");
        assert_eq!(
            wrong_sign.photos[0].point_index, None,
            "a negative offset must move away from a UTC+9 photo, not toward it"
        );

        let options = PhotoOptions {
            camera_offset_minutes: 540,
            ..PhotoOptions::default()
        };
        let matched = load_photos(&mut loaded, &directory.display().to_string(), &options)
            .expect("load with the JST offset");
        assert_eq!(matched.photos[0].point_index, Some(0));
        std::fs::remove_dir_all(&directory).ok();
    }

    #[test]
    fn a_log_offset_moves_the_track_into_the_photo_clock() {
        // `--utc-offset-days` adds whole days to the track. A photo taken one
        // day after the logged instant must match when the track is moved by
        // one day, and must not match when it is not.
        let directory = temp_dir("m241-app-photo-log-offset");
        let photo = directory.join("two-days-later.jpg");
        write_jpeg(&photo);
        set_modified(&photo, SAMPLE_START + 2 * 86_400);

        let mut loaded = sample();
        let unmatched = load_photos(
            &mut loaded,
            &directory.display().to_string(),
            &PhotoOptions::default(),
        )
        .expect("load without a log offset");
        assert_eq!(unmatched.photos[0].point_index, None);

        let options = PhotoOptions {
            log_offset_days: 2,
            ..PhotoOptions::default()
        };
        let matched = load_photos(&mut loaded, &directory.display().to_string(), &options)
            .expect("load with the log offset");
        assert_eq!(matched.photos[0].point_index, Some(0));
        std::fs::remove_dir_all(&directory).ok();
    }

    #[test]
    fn the_time_tolerance_is_the_geotag_max_gap() {
        let directory = temp_dir("m241-app-photo-gap");
        let photo = directory.join("late.jpg");
        write_jpeg(&photo);
        set_modified(&photo, SAMPLE_START + 45);

        let mut loaded = sample();
        let strict = PhotoOptions {
            max_gap_seconds: 10,
            ..PhotoOptions::default()
        };
        let unmatched = load_photos(&mut loaded, &directory.display().to_string(), &strict)
            .expect("load with a 10-second tolerance");
        assert_eq!(unmatched.photos[0].point_index, None);

        let matched = load_photos(
            &mut loaded,
            &directory.display().to_string(),
            &PhotoOptions::default(),
        )
        .expect("load with the default tolerance");
        assert_eq!(matched.photos[0].point_index, Some(1));
        std::fs::remove_dir_all(&directory).ok();
    }

    #[test]
    fn photos_at_the_same_place_share_a_group_when_grouping_is_on() {
        let directory = temp_dir("m241-app-photo-group");
        for name in ["a.jpg", "b.jpg"] {
            let photo = directory.join(name);
            write_jpeg(&photo);
            set_modified(&photo, SAMPLE_START);
        }

        let mut loaded = sample();
        let grouped = load_photos(
            &mut loaded,
            &directory.display().to_string(),
            &PhotoOptions {
                group_distance_meters: 10.0,
                ..PhotoOptions::default()
            },
        )
        .expect("load with grouping");
        assert_eq!(grouped.photos.len(), 2);
        let first = grouped.photos[0].group_id.expect("first group");
        let second = grouped.photos[1].group_id.expect("second group");
        assert_eq!(first, second, "two photos on one point are one place");

        let ungrouped = load_photos(
            &mut loaded,
            &directory.display().to_string(),
            &PhotoOptions::default(),
        )
        .expect("load without grouping");
        assert!(
            ungrouped
                .photos
                .iter()
                .all(|photo| photo.group_id.is_none()),
            "grouping is off by default"
        );
        std::fs::remove_dir_all(&directory).ok();
    }

    #[test]
    fn geotag_preview_and_apply_write_gps_and_backup_once() {
        let directory = temp_dir("m241-app-geotag");
        let photo = directory.join("matched.jpg");
        write_jpeg(&photo);
        set_modified(&photo, SAMPLE_START);
        let png = directory.join("ignore.png");
        std::fs::write(&png, b"not a jpeg").expect("write png");

        let loaded = sample();
        let options = GeotagOptionsDto {
            camera_offset_minutes: 0,
            log_offset_days: 0,
            group_distance_meters: 0.0,
            max_gap_seconds: 300,
            keep_backup: true,
            rewrite_datetime_original: false,
        };

        let preview =
            geotag_preview(&loaded, &directory.display().to_string(), &options).expect("preview");
        assert_eq!(preview.matched, 1);
        assert_eq!(preview.skipped, 1);
        assert_eq!(preview.items.len(), 2);

        let matched_item = preview
            .items
            .iter()
            .find(|item| item.name == "matched.jpg")
            .expect("matched item");
        assert_eq!(matched_item.point_index, Some(0));
        assert!(matched_item.lat.is_some());
        assert!(matched_item.lon.is_some());
        assert_eq!(matched_item.gap_seconds, Some(0));
        assert!(matched_item.skipped_reason.is_none());

        let outcome =
            geotag_apply(&loaded, &directory.display().to_string(), &options).expect("apply");
        assert_eq!(outcome.written, 1);
        assert_eq!(outcome.skipped, 1);
        assert_eq!(outcome.backups_created, 1);
        assert!(outcome.errors.is_empty());

        let backup = directory.join("matched.jpg_original");
        assert!(backup.exists(), "backup must be created");

        let bytes = std::fs::read(&photo).expect("read tagged photo");
        let gps = m241_core::log::read_gps(&bytes)
            .expect("exif gps parses")
            .expect("gps tags written");
        assert!(
            (gps.latitude.expect("gps latitude") - loaded.track.points[0].latitude.unwrap()).abs()
                < 1e-4
        );

        // Second run must skip `_original` backup and not create a second backup.
        set_modified(&photo, SAMPLE_START);
        let second =
            geotag_apply(&loaded, &directory.display().to_string(), &options).expect("apply 2");
        assert_eq!(second.written, 1);
        assert_eq!(second.backups_created, 0);

        std::fs::remove_dir_all(&directory).ok();
    }

    #[test]
    fn geotag_apply_rewrites_datetime_original_with_camera_offset() {
        let directory = temp_dir("m241-app-geotag-tz");
        let photo = directory.join("jst.jpg");
        write_jpeg(&photo);
        let raw = std::fs::read(&photo).expect("read raw jpeg");
        let camera_local = SAMPLE_START + 540 * 60;
        let with_exif = m241_core::log::write_datetime_original(&raw, camera_local)
            .expect("write EXIF DateTimeOriginal");
        std::fs::write(&photo, &with_exif).expect("write jpeg with EXIF time");

        let loaded = sample();
        let options = GeotagOptionsDto {
            camera_offset_minutes: 540,
            log_offset_days: 0,
            group_distance_meters: 0.0,
            max_gap_seconds: 60,
            keep_backup: false,
            rewrite_datetime_original: true,
        };

        let preview =
            geotag_preview(&loaded, &directory.display().to_string(), &options).expect("preview");
        assert_eq!(preview.matched, 1);
        assert_eq!(preview.items[0].captured_at, Some(SAMPLE_START));
        assert_eq!(preview.items[0].point_index, Some(0));
        assert_eq!(preview.items[0].gap_seconds, Some(0));

        let outcome =
            geotag_apply(&loaded, &directory.display().to_string(), &options).expect("apply");
        assert_eq!(outcome.written, 1);
        assert!(outcome.errors.is_empty());

        let updated = std::fs::read(&photo).expect("read updated photo");
        assert_eq!(
            read_datetime_original(&updated).expect("parse DateTimeOriginal"),
            Some(SAMPLE_START)
        );

        std::fs::remove_dir_all(&directory).ok();
    }
}
