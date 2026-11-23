//! ログ — the Log screen (DESIGN.md §5).
//!
//! The map is the page. One 40pt tool row (the title and the sample marker), then
//! the canvas: the log file, the playback strip, the zoom and the drawer toggle
//! are plates drawn *over* it, and the panels that used to push the map down live
//! in a right-hand drawer that starts closed. The page does not scroll; the canvas
//! follows the window, because a map that has to be scrolled to is not the main
//! surface.
//!
//! Background tiles are real OpenStreetMap fetches (`tiles`), so they only happen
//! from a real window. A sample or headless frame therefore says why it shows no
//! tiles instead of pretending to load them, and the track is still painted from
//! the points themselves — the projection is what makes the shape of the trip
//! visible, which is the job on this screen (DESIGN.md §5, §10).
//!
//! # What this module and its submodules do
//!
//! The page (the tool row, the canvas, the selected-point strip), the map, the
//! drawer's five tabs and the point table are here. The submodules own the work
//! behind the controls — `drawer` owns the plates drawn over the canvas and the
//! drawer's column/float geometry:
//!
//! * `ops` is the ログ操作 console — the CLI's `log` and `nmea` commands as
//!   panels. It **does talk to a device**: download and the erase backup call
//!   `device`'s PMTK182 worker on a thread (`Port::open` +
//!   `PmtkClient::download_log`), and the NMEA relay opens the port too. Those
//!   paths are unverified on hardware from this screen, and the panels say so.
//!   The log erase cannot run from this panel: the GUI links `m241-flash` for
//!   the Firmware screen's write path, but the log console has no route to the
//!   erase client, and erasing the user's track needs the user's own
//!   instruction — so the confirmed action stops at a refusal that says so.
//! * `project` is the CLI-compatible JSON project document and the `log read`
//!   conversion; `nmea` is the date repair and day arithmetic; `proxy` is the
//!   relay thread; `device` is the transfer worker.
//!
//! A track arrived at from disk can only be written back into the photos and the
//! export file, never the logger, unless the person uses the download panel —
//! the screen never writes a track to the unit.
//!
//! # Controls that exist, and the ones that do not
//!
//! The CLI's `log kmz --utc-offset-days` and `--tz-offset-minutes` have a control
//! here for KMZ (`kmz_tz_offset_minutes`); `--utc-offset-days` is the 「読み書き」
//! panel's 日付補正 or a project's stored correction, applied before matching, so
//! an uncorrected dump must go through one of those before a photo match means
//! anything. The `icon/` thumbnails are an **opt-in** checkbox
//! (`kmz_icons`, default off): the core records the `icon/` semantics as
//! unconfirmed (`crates/m241-core/src/log/kmz.rs`), so the default output stays
//! the shape the CLI's `log kmz` writes and the thumbnails are a decision the
//! person makes, not one this screen makes for them. Enabling it marks the KMZ
//! row 未検証, as the writer already does for Google Earth.
//!
//! The map selects a point by clicking it, with no drag range selection.

// The write token lives one level down, where this module cannot construct it:
// see `geotag_gate` for why the privacy below `log.rs` is what closes M2.
mod geotag_gate;
// The right-hand drawer and the plates drawn on the map. They own the layout so
// the overlays are placed against one set of sizes and cannot drift into each
// other (DESIGN.md §5).
mod drawer;
// The three wirings added after the design pass. Each owns its own state and
// returns the result to this module, so `log.rs` keeps drawing the page and the
// submodules keep their rules (photo scope, KMZ limits, edit history) in one
// place each.
mod editing;
mod kmz_export;
mod photo_shift;
// Background map tiles (DESIGN.md §5, §10). Crate-visible rather than public:
// the Settings screen needs the cache path, its usage and its clear action, and
// nothing outside this crate needs the tile machinery. Making the module public
// turned 34 helpers into a crate-external API that no consumer used.
pub(crate) mod tiles;
// The CLI-equivalent operations console: downloading, backing up and erasing,
// JSON projects, NMEA repair/relay, and `log read`. The panels live in `ops`;
// the transfer, the project document and the NMEA arithmetic have their own
// modules below it.
mod device;
mod nmea;
mod ops;
mod project;
mod proxy;
#[cfg(test)]
mod test_support;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::{Align2, FontId, Rect, RichText, Sense, Stroke, StrokeKind, Ui, Vec2};
use m241_core::log::geotag::{self, CaptureTime, GeotagOptions, GeotagPlan, PhotoTimeShift};
use m241_core::log::{self, ParseReport};
use m241_core::{ExportFormat, Track, TrackPoint};

use crate::screens::common::{self, Status};
use crate::screens::settings::AppSettings;
use crate::theme::{self, metric, size, space};

use self::editing::TrackEditState;
use self::geotag_gate::GeotagConfirmation;
use self::ops::LogOps;
use self::photo_shift::{PhotoShiftSettings, ScopeKind};
use self::project::ReadFacts;
use self::tiles::{MapOptions, MapView};

/// Records the rects the screen allocated, for the layout tests.
///
/// "The map fills the content", "the plates do not overlap" and "the drawer opens
/// on every tab" are layout facts, not paint facts: a shape list cannot say how
/// tall the canvas was. The screen writes them down as it draws, keyed by thread
/// like `widgets::rail`'s transition record, because the render tests run in
/// parallel. Nothing is kept outside a test build.
mod layout_probe {
    use egui::Rect;

    /// One frame's geometry, as the screen drew it.
    #[cfg(test)]
    #[derive(Debug, Clone)]
    pub(super) struct Frame {
        /// The rect the Log screen was handed.
        pub(super) content: Rect,
        /// The tool row above the canvas.
        pub(super) tool: Rect,
        /// The canvas.
        pub(super) map: Rect,
        /// Every plate drawn over the canvas, by name.
        pub(super) overlays: Vec<(&'static str, Rect)>,
        /// The drawer, when it is open.
        pub(super) drawer: Option<Rect>,
    }

    #[cfg(test)]
    impl Default for Frame {
        fn default() -> Self {
            Self {
                content: Rect::NOTHING,
                tool: Rect::NOTHING,
                map: Rect::NOTHING,
                overlays: Vec::new(),
                drawer: None,
            }
        }
    }

    #[cfg(test)]
    static FRAMES: std::sync::Mutex<std::collections::BTreeMap<String, Frame>> =
        std::sync::Mutex::new(std::collections::BTreeMap::new());

    #[cfg(test)]
    fn key() -> String {
        format!("{:?}", std::thread::current().id())
    }

    /// The frame this thread drew last, if the Log screen was among the screens.
    #[cfg(test)]
    pub(super) fn last() -> Option<Frame> {
        FRAMES
            .lock()
            .ok()
            .and_then(|frames| frames.get(&key()).cloned())
    }

    #[cfg(test)]
    fn with_frame(update: impl FnOnce(&mut Frame)) {
        if let Ok(mut frames) = FRAMES.lock() {
            update(frames.entry(key()).or_default());
        }
    }

    pub(super) fn begin(_content: Rect) {
        #[cfg(test)]
        with_frame(|frame| {
            frame.content = _content;
            frame.overlays.clear();
            frame.drawer = None;
        });
    }

    pub(super) fn tool(_rect: Rect) {
        #[cfg(test)]
        with_frame(|frame| frame.tool = _rect);
    }

    pub(super) fn map(_rect: Rect) {
        #[cfg(test)]
        with_frame(|frame| frame.map = _rect);
    }

    pub(super) fn overlay(_name: &'static str, _rect: Rect) {
        #[cfg(test)]
        with_frame(|frame| frame.overlays.push((_name, _rect)));
    }

    pub(super) fn drawer(_rect: Rect) {
        #[cfg(test)]
        with_frame(|frame| frame.drawer = Some(_rect));
    }
}

/// Rows the point list shows before it scrolls.
///
/// Six is a reference-manual figure: enough to read a stretch of the trip, few
/// enough that the export bar stays above the fold on a 640pt window.
const ROWS_SHOWN: f32 = 6.0;

/// Maximum `.bin` log size accepted from disk (16 MiB — well above any M-241 flash).
const MAX_LOG_BYTES: usize = 16 * 1024 * 1024;

/// Zoom the map raises itself to when follow mode turns on or a point is picked
/// while following.
///
/// Why a floor at all: following means the selected point stays under the cursor,
/// and at a zoom below 12 the whole trip still fits on the canvas, so a gap of a
/// few hundred metres moves the marker by a couple of points — the map looks
/// frozen while playback runs. At 12 one canvas holds a city, which is the grain
/// at which "where am I now" is readable.
const FOLLOW_MIN_ZOOM: f32 = 12.0;

/// Lowest zoom the user can ask for while not following.
const MIN_ZOOM: f32 = tiles::MIN_TILE_ZOOM as f32;

/// Highest zoom the map allows. OSM serves tiles to 19 and the projection is
/// defined there; the track is drawn over the same view, so the limit is the
/// tiles' rather than a separate one.
const MAX_ZOOM: f32 = tiles::MAX_TILE_ZOOM as f32;

/// Playback step while motion is allowed (DESIGN.md §3: 150 ms is the authored
/// transition; a track that steps four points a second reads as playback).
const PLAY_STEP: Duration = Duration::from_millis(250);

/// Playback step with reduced motion on: half the rate, no rapid motion.
const PLAY_STEP_REDUCED: Duration = Duration::from_millis(500);

/// A planned photo entry paired with the matched index in [`Track::points`].
#[derive(Debug, Clone, PartialEq)]
pub struct PhotoEntry {
    /// Read-only geotag plan produced by `m241_core::log::geotag::plan`.
    pub plan: GeotagPlan,
    /// Index into the *current* track's [`Track::points`] of the point this photo
    /// was paired with, when it was paired at all.
    ///
    /// The index is not "the nearest point in time": it is the nearest point that
    /// **has a position** ([`find_matching_point_index`]), so it is always a point
    /// the map can draw and a frame can write. `None` means the track has no fix
    /// to pair with — a photo in that state is never written (the map has no
    /// pixel for it, and [`LogScreen::sync_photo_to_selected`] skips it).
    ///
    /// It is an index, not a copy, so it is re-derived whenever the track is
    /// replaced ([`LogScreen::relink_photos`]). Reordering [`Track::points`]
    /// without re-linking would make it point at a different record.
    pub point_index: Option<usize>,
}

/// The photos a confirmed geotag write will touch, as file names in plan order.
///
/// The confirmation prompt and [`LogScreen::apply_geotags`] both render this same
/// list, so the number of names a person reads is the number of files that change.
fn geotag_targets(entries: &[PhotoEntry]) -> Vec<&Path> {
    entries
        .iter()
        .filter(|entry| entry.plan.will_write())
        .map(|entry| entry.plan.photo.as_path())
        .collect()
}

/// One plan paired with the track point it will be written from.
///
/// The four places that build a [`PhotoEntry`] go through here so the
/// `find_matching_point_index` rule cannot be spelled differently in one of
/// them; the sample block and the three file paths all take the same route.
/// One point's coordinate in the format the Settings screen selected.
///
/// A UTM refusal (a latitude outside its bands) falls back to decimal degrees
/// rather than blanking the field: the point still has a coordinate, and the
/// selected format is a display choice, not a measurement.
fn coordinate_text(point: &TrackPoint, settings: &AppSettings) -> String {
    match (point.latitude, point.longitude) {
        (Some(latitude), Some(longitude)) => {
            log::format_coordinate(latitude, longitude, settings.coord_format).map_or_else(
                |_refused| common::coordinate(Some(latitude), Some(longitude)),
                |(latitude, longitude)| format!("{latitude} {longitude}"),
            )
        }
        _ => common::coordinate(None, None),
    }
}

/// What each export format writes, in the product's voice.
///
/// NMEA is the one that needs the sentence: the core writes `$GPRMC` today and
/// the GGA/GSA expansion is a separate core change, so a chip that just said
/// "NMEA" would claim more than the file contains.
fn export_format_note(format: ExportFormat) -> &'static str {
    match format {
        ExportFormat::Gpx => "GPX 1.1。地図アプリ向けの標準形式です",
        ExportFormat::Kml => "KML 2.2。Google Earth などで開く形式です",
        ExportFormat::Csv => "CSV。1 行 1 点で、表計算向けです",
        ExportFormat::Nmea => {
            "NMEA 0183。現在は $GPRMC を 1 点 1 文で出力します。$GPGGA / $GPGSA の追加はコア側で予定"
        }
        ExportFormat::Json => "JSON。点の配列と集計を 1 オブジェクトにします",
    }
}

fn photo_entry(track: &Track, plan: GeotagPlan) -> PhotoEntry {
    let point_index = find_matching_point_index(track, &plan);
    PhotoEntry { plan, point_index }
}

/// A tab in the Log screen's right-hand drawer (DESIGN.md §5).
///
/// The drawer holds every panel the map used to push down: 集計 keeps the figures,
/// the day filter, the speed chart and the point list; 写真, 編集, 書き出し and
/// ログ操作 keep the blocks they had. These are tabs of one screen, not rail
/// entries: the rail navigates the product's four screens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LogTab {
    /// 集計: the day filter, the figures, the speed chart and the point list.
    #[default]
    Summary,
    /// 写真: the folder, the match plan and the clock shift.
    Photos,
    /// 編集: split, merge, delete.
    Edit,
    /// 書き出し: the format, the destination, the geotag and KMZ options.
    Export,
    /// ログ操作: download, backup, erase gates, projects, NMEA repair/relay, read.
    Ops,
}

impl LogTab {
    /// Every tab, in the order the drawer draws them.
    pub const ALL: [Self; 5] = [
        Self::Summary,
        Self::Photos,
        Self::Edit,
        Self::Export,
        Self::Ops,
    ];

    /// The tab's label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Summary => "集計",
            Self::Photos => "写真",
            Self::Edit => "編集",
            Self::Export => "書き出し",
            Self::Ops => "ログ操作",
        }
    }
}

/// The Log screen's own state.
#[derive(Debug)]
pub struct LogScreen {
    track: Track,
    report: ParseReport,
    /// The facts the current track was read with, for a `--format json` export:
    /// the applied day shift and, for an NMEA capture, the line counts.
    ///
    /// Reset by [`Self::set_track`]: a track from another source has other
    /// facts, and carrying the previous read's `nmea` counts onto a dump export
    /// would describe the wrong input. The read panel and a project load set it
    /// right after they replace the track.
    read_facts: Option<ReadFacts>,
    /// Path input for opening an offline `.bin` log dump.
    log_path: String,
    /// Inline result of the most recent `.bin` load attempt.
    load_status: Option<(Status, String)>,
    /// Which point the figures panel describes.
    selected: Option<usize>,
    /// Days the user has unchecked in the track block.
    hidden_days: BTreeSet<String>,
    /// Whether playback is currently advancing along the track.
    playing: bool,
    /// Timestamp of the previous playback step.
    last_step_at: Option<Instant>,
    /// Whether the map follows the current point with zoom >= 12.
    follow: bool,
    /// When true, the map projects the whole track bounds to fit the canvas.
    fit_mode: bool,
    /// Logical slippy-map zoom level (`1.0..=19.0`), clamped >= 12 while following.
    map_zoom: f32,
    /// Background tiles, their textures and the fetch workers.
    tiles: tiles::MapTiles,
    /// Path input for loading photos (directory or single JPEG).
    photo_path: String,
    /// Read-only photo plans loaded via `geotag::plan`.
    photo_entries: Vec<PhotoEntry>,
    /// Index of the currently highlighted/synced photo in [`Self::photo_entries`].
    selected_photo: Option<usize>,
    /// Inline status of the photo loader / geotag action.
    photo_status: Option<(Status, String)>,
    /// Explicit inline confirmation gate before writing GPS EXIF tags.
    confirm_geotag: bool,
    format: ExportFormat,
    /// Whether the KMZ writer, rather than [`Self::format`], is selected.
    kmz: bool,
    /// 最大長辺 for photos embedded in a KMZ (`--max-photo-edge`); empty means
    /// the originals are embedded untouched.
    kmz_max_edge: String,
    /// Bundle an `icon/` thumbnail with each photo in the KMZ.
    ///
    /// **Default off.** The core records the `icon/` semantics (thumbnail size,
    /// what `kmzMaxPhotoEdgeSize` bounds, how the KML would refer to them) as
    /// unconfirmed, and its plain `export_kmz` — the function the CLI calls —
    /// writes no `icon/` entry. A default-on checkbox would make the GUI's KMZ
    /// differ from the CLI's with no decision by the person, so the thumbnails are
    /// opt-in and the KMZ row carries 未検証 while they are on.
    kmz_icons: bool,
    /// Camera clock offset from UTC in minutes for KMZ photo matching
    /// (`log kmz --tz-offset-minutes`); empty or zero means UTC.
    kmz_tz_offset_minutes: String,
    /// Clock shift and the photos it applies to.
    photo_shift: PhotoShiftSettings,
    /// 「同じ場所の写真をまとめる」 threshold in metres; empty means no grouping.
    photo_group_distance: String,
    /// Largest accepted photo/point time gap in seconds (`--max-gap`).
    photo_max_gap: String,
    /// Keep the original JPEG as `<name>_original` before EXIF is written.
    photo_keep_backup: bool,
    /// Split / merge / delete and the core editor's undo history.
    edit: TrackEditState,
    destination: String,
    outcome: Option<(Status, String)>,
    sample: bool,
    /// Whether the right-hand drawer is open.
    ///
    /// Closed by default: the map is the page, and the drawer is where the
    /// controls wait (DESIGN.md §5). It opens on the tab that holds what the
    /// person asked for — the empty state's button opens ログ操作, so download is
    /// one press away without a panel taking the first screen.
    drawer_open: bool,
    /// The tab the open drawer shows.
    drawer_tab: LogTab,
    /// The CLI-equivalent operations console (download, project, NMEA, read).
    ops: LogOps,
}

impl Default for LogScreen {
    fn default() -> Self {
        Self {
            track: Track::default(),
            report: ParseReport::default(),
            read_facts: None,
            log_path: String::new(),
            load_status: None,
            selected: None,
            hidden_days: BTreeSet::new(),
            playing: false,
            last_step_at: None,
            follow: false,
            fit_mode: true,
            map_zoom: FOLLOW_MIN_ZOOM,
            tiles: tiles::MapTiles::default(),
            photo_path: String::new(),
            photo_entries: Vec::new(),
            selected_photo: None,
            photo_status: None,
            confirm_geotag: false,
            format: ExportFormat::Gpx,
            kmz: false,
            kmz_max_edge: String::new(),
            kmz_icons: false,
            kmz_tz_offset_minutes: String::from("0"),
            photo_shift: PhotoShiftSettings::default(),
            photo_group_distance: String::new(),
            photo_max_gap: String::from("300"),
            photo_keep_backup: true,
            edit: TrackEditState::default(),
            destination: String::from("m241-track.gpx"),
            outcome: None,
            sample: false,
            drawer_open: false,
            drawer_tab: LogTab::Summary,
            ops: LogOps::default(),
        }
    }
}

impl LogScreen {
    /// Replace the log this screen shows.
    ///
    /// The download path calls this; the sample toggle calls it too. Taking a
    /// `Track` and its [`ParseReport`] together is deliberate: a summary without the
    /// damage report would let the screen print "10,399 points" for a dump where
    /// four blocks failed their checksum.
    pub fn set_track(&mut self, track: Track, report: ParseReport) {
        self.selected = track.points.first().map(|_ignored| 0);
        // The editor's history belongs to the track it was seeded from; a new
        // track that could still undo into the old one would be a lie.
        self.edit.reset(&track);
        self.photo_shift.status = None;
        self.read_facts = None;
        self.track = track;
        self.report = report;
        self.playing = false;
        self.last_step_at = None;
        self.fit_mode = true;
        self.follow = false;
        self.map_zoom = self.compute_fit_zoom();
        // A different log means a different part of the world: the previous
        // textures, pending requests and failures belong to the old one.
        self.tiles.reset();
        self.hidden_days.clear();
        self.close_geotag_confirmation();
        self.outcome = None;
        self.relink_photos();
    }

    /// Record the facts the new track was read with, for a JSON export.
    ///
    /// Called right after [`Self::set_track`] by the two paths that read a file
    /// rather than receive a dump (`log read` and `log project load`). It is a
    /// separate call because those paths know the counts the parser returned,
    /// and a `set_track` that guessed them would be inventing facts for the
    /// download path.
    pub fn set_read_facts(&mut self, facts: ReadFacts) {
        self.read_facts = Some(facts);
    }

    /// Fill the screen with the sample track.
    ///
    /// Public for the headless render tests and for a future `--demo` flag. The
    /// screen labels the values 「見本」, so code calling this does not turn invented
    /// coordinates into a presentation of somebody's trip.
    pub fn show_sample(&mut self) {
        self.sample = true;
        self.set_track(sample_track(), ParseReport::default());
    }

    /// Install a fake tile fetcher with no cache (tests only).
    ///
    /// The render and App tests draw the real screen; without this seam the
    /// "headless never fetches" test could only inspect the caption, which is
    /// exactly the hole the Stage 1 review found: a broken window gate could
    /// have opened a real socket and left the test green.
    #[cfg(test)]
    pub(crate) fn set_tile_fetcher_for_test(
        &mut self,
        fetcher: std::sync::Arc<dyn tiles::TileFetcher>,
    ) {
        self.tiles = tiles::MapTiles::with_fetcher_for_test(fetcher);
    }

    /// Fill the photo block with two sample entries, for the screenshot renderer.
    ///
    /// No file is read or written: the screenshot only draws the block, and a
    /// path that does not exist cannot be exported by an unattended render. The
    /// entries ride on the sample track's first two points, so the rows say
    /// 「一致」 for the reason a real pair would.
    pub fn show_sample_photos(&mut self) {
        let mut entries = Vec::new();
        for (index, name) in [(0_usize, "0042.jpg"), (1_usize, "0043.jpg")] {
            let Some(point) = self.track.points.get(index).copied() else {
                continue;
            };
            entries.push(photo_entry(
                &self.track,
                GeotagPlan {
                    photo: PathBuf::from("photos").join(name),
                    capture_time: point.utc.map(CaptureTime::Exif),
                    point: Some(point),
                    gap_seconds: Some(0),
                    skipped: None,
                    keep_backup: self.photo_keep_backup,
                    group_id: None,
                },
            ));
        }
        self.photo_path = String::from("photos");
        self.photo_entries = entries;
        // The block is captioned 「見本」 and the status says so too: the
        // screenshot's photo rows must not read as "two JPEGs were loaded". One
        // line, so the rail keeps its §5 height at 1280×800.
        self.photo_status = Some((
            Status::Ok,
            format!("見本の写真 {} 枚（未読込）", self.photo_entries.len()),
        ));
        self.selected_photo = None;
        self.sync_photo_to_selected();
        if self.selected_photo.is_none() && !self.photo_entries.is_empty() {
            self.selected_photo = Some(0);
        }
    }

    /// Fill the shift controls with sample values for the screenshot renderer.
    ///
    /// No file is read: the camera is invented and the entries are the sample
    /// photo block, so the frame shows the controls the wiring added without
    /// pretending a real camera was resolved.
    pub fn show_sample_photo_shift(&mut self) {
        self.photo_shift.show_sample(&self.photo_entries);
    }

    /// Set the KMZ 最大長辺 field; empty means the original bytes are embedded.
    pub fn set_kmz_max_edge(&mut self, value: impl Into<String>) {
        self.kmz_max_edge = value.into();
    }

    /// The log currently shown.
    #[must_use]
    pub fn track(&self) -> &Track {
        &self.track
    }

    /// Index of the currently selected track point.
    #[must_use]
    pub fn selected_index(&self) -> Option<usize> {
        self.selected
    }

    /// Whether track playback is currently running.
    #[must_use]
    pub fn is_playing(&self) -> bool {
        self.playing
    }

    /// Whether follow mode is enabled on the map.
    #[must_use]
    pub fn is_following(&self) -> bool {
        self.follow
    }

    /// Current logical map zoom level.
    #[must_use]
    pub fn map_zoom(&self) -> f32 {
        self.map_zoom
    }

    /// Loaded photo entries (read-only geotag plans).
    #[must_use]
    pub fn photo_entries(&self) -> &[PhotoEntry] {
        &self.photo_entries
    }

    /// Currently highlighted/synced photo index.
    #[must_use]
    pub fn selected_photo(&self) -> Option<usize> {
        self.selected_photo
    }

    /// Number of loaded photos that matched a track point and are eligible for tagging.
    #[must_use]
    pub fn matched_photo_count(&self) -> usize {
        self.photo_entries
            .iter()
            .filter(|entry| entry.plan.will_write())
            .count()
    }

    /// Most recent load or export outcome, for callers and tests.
    #[must_use]
    pub fn outcome(&self) -> Option<&(Status, String)> {
        self.outcome.as_ref()
    }

    /// Whether the geotag confirmation prompt is open.
    ///
    /// Read-only on purpose: the prompt is opened by 「写真に位置」 and closed by
    /// answering it or by any change that invalidates it, so there is no public
    /// setter a caller could use to fake a confirmation.
    #[must_use]
    pub fn geotag_confirmation_open(&self) -> bool {
        self.confirm_geotag
    }

    /// The photos a confirmed geotag write would change, in plan order.
    ///
    /// The same list the confirmation prompt prints, so a caller (or a test) can
    /// check that the count on screen is the count of files that would change.
    #[must_use]
    pub fn geotag_targets(&self) -> Vec<&Path> {
        geotag_targets(&self.photo_entries)
    }

    /// Most recent `.bin` load status.
    #[must_use]
    pub fn load_status(&self) -> Option<&(Status, String)> {
        self.load_status.as_ref()
    }

    /// Open the right-hand drawer on `tab`.
    ///
    /// Public because the render tests and the `--screenshot` drawer frames reach
    /// a panel without synthesising a click; the toggle on the map is the route a
    /// person takes. The drawer starts closed (DESIGN.md §5), and the only code
    /// that closes it is the toggle itself, so there is no `close_drawer` to go
    /// stale.
    pub fn open_drawer(&mut self, tab: LogTab) {
        self.drawer_open = true;
        self.drawer_tab = tab;
    }

    /// Load a log from disk (offline, read-only): `.bin` via [`log::parse_dump`],
    /// `.gpx` via [`log::import_gpx`].
    ///
    /// The extension is the dispatch, because the two formats have no common
    /// sniffing byte and the CLI splits them the same way (`log read` vs
    /// `import_gpx`).
    ///
    /// # Errors
    ///
    /// Returns a Japanese error string if the path is empty, too large, unreadable,
    /// or not a valid dump / GPX document.
    pub fn load_bin_file(&mut self, path: &Path) -> Result<(), String> {
        let path_text = path.to_string_lossy();
        if path_text.trim().is_empty() {
            let msg = String::from("ログファイルのパスを入力してください");
            self.load_status = Some((Status::Err, msg.clone()));
            return Err(msg);
        }
        let metadata = std::fs::metadata(path)
            .map_err(|err| format!("{} を読めません: {err}", path.display()));
        let metadata = match metadata {
            Ok(m) => m,
            Err(msg) => {
                self.load_status = Some((Status::Err, msg.clone()));
                return Err(msg);
            }
        };
        let length = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
        if length > MAX_LOG_BYTES {
            let msg = format!(
                "{} は {} あります。ログファイルの上限を超えています",
                path.display(),
                common::bytes(length)
            );
            self.load_status = Some((Status::Err, msg.clone()));
            return Err(msg);
        }
        let is_gpx = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("gpx"));
        if is_gpx {
            return self.load_gpx_file(path);
        }
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(err) => {
                let msg = format!("{} を読めません: {err}", path.display());
                self.load_status = Some((Status::Err, msg.clone()));
                return Err(msg);
            }
        };
        let (track, report) = match log::parse_dump(&bytes) {
            Ok(parsed) => parsed,
            Err(err) => {
                let msg = format!("ログを解析できません: {err}");
                self.load_status = Some((Status::Err, msg.clone()));
                return Err(msg);
            }
        };
        let count = track.points.len();
        self.sample = false;
        self.log_path = path.display().to_string();
        self.set_track(track, report);
        self.destination = path
            .with_extension(self.format.extension())
            .display()
            .to_string();
        self.load_status = Some((
            Status::Ok,
            format!("{} から {count} 点を読み込みました", path.display()),
        ));
        Ok(())
    }

    /// Load a GPX 1.0/1.1 document through the core's importer.
    fn load_gpx_file(&mut self, path: &Path) -> Result<(), String> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) => {
                let msg = format!("{} を GPX として読めません: {err}", path.display());
                self.load_status = Some((Status::Err, msg.clone()));
                return Err(msg);
            }
        };
        let track = match log::import_gpx(&text) {
            Ok(track) => track,
            Err(err) => {
                let msg = format!("{} を GPX として解析できません: {err}", path.display());
                self.load_status = Some((Status::Err, msg.clone()));
                return Err(msg);
            }
        };
        let count = track.points.len();
        self.sample = false;
        self.log_path = path.display().to_string();
        self.set_track(track, ParseReport::default());
        // A GPX document is the GUI's `log read --gpx`: the JSON export says so
        // with `"input": "gpx"`, as the CLI does, and `auto` was not applied to
        // its dates (there is no shift here to report).
        self.set_read_facts(ReadFacts {
            offset_days: 0,
            nmea: None,
            gpx: true,
        });
        self.destination = path
            .with_extension(self.format.extension())
            .display()
            .to_string();
        self.load_status = Some((
            Status::Ok,
            format!(
                "{} から {count} 点を GPX として読み込みました",
                path.display()
            ),
        ));
        Ok(())
    }

    /// Select a track point by index, syncing the closest photo and enforcing follow zoom.
    ///
    /// Closes an open geotag confirmation: the prompt lists the files a write
    /// would change, and selecting another point re-syncs the highlighted photo,
    /// so the prompt's premise has moved.
    pub fn select_point(&mut self, index: Option<usize>) {
        let previous_photo = self.selected_photo;
        self.selected = index.filter(|i| *i < self.track.points.len());
        self.close_geotag_confirmation();
        if self.follow && self.map_zoom < FOLLOW_MIN_ZOOM {
            self.map_zoom = FOLLOW_MIN_ZOOM;
        }
        self.sync_photo_to_selected();
        self.replan_by_camera_if_base_changed(previous_photo);
    }

    /// Toggle playback on or off.
    pub fn toggle_play(&mut self) {
        if self.track.points.is_empty() {
            self.playing = false;
            return;
        }
        self.playing = !self.playing;
        if self.playing {
            let at_end = self
                .selected
                .is_some_and(|idx| idx + 1 >= self.track.points.len());
            if self.selected.is_none() || at_end {
                self.select_point(Some(0));
            }
            self.last_step_at = Some(Instant::now());
        } else {
            self.last_step_at = None;
        }
    }

    /// Advance the selected point by one step. Stops playback at the final point.
    pub fn step_next(&mut self) -> bool {
        let len = self.track.points.len();
        if len == 0 {
            self.playing = false;
            return false;
        }
        let next = match self.selected {
            None => 0,
            Some(idx) if idx + 1 < len => idx + 1,
            Some(_) => {
                self.playing = false;
                return false;
            }
        };
        self.select_point(Some(next));
        true
    }

    /// Move the selected point backward by one step.
    pub fn step_prev(&mut self) -> bool {
        let len = self.track.points.len();
        if len == 0 {
            return false;
        }
        let prev = match self.selected {
            None => 0,
            Some(0) => return false,
            Some(idx) => idx - 1,
        };
        self.select_point(Some(prev));
        true
    }

    /// Fit the entire track inside the map view and disable follow mode.
    pub fn fit_all(&mut self) {
        self.follow = false;
        self.fit_mode = true;
        self.map_zoom = self.compute_fit_zoom();
    }

    /// Enable or disable follow mode. Enabling follow mode enforces `zoom >= 12`.
    ///
    /// Turning follow on with nothing selected picks the first point: the
    /// projection centres on the selected point, and with no selection it falls
    /// back to the track's bounding-box centre, so the map would be "following"
    /// something that is not the current position until playback started.
    pub fn set_follow(&mut self, follow: bool) {
        self.follow = follow;
        if follow {
            self.fit_mode = false;
            let first_with_position = self.track.points.iter().position(TrackPoint::has_position);
            let fallback = first_with_position.or(if self.track.points.is_empty() {
                None
            } else {
                Some(0)
            });
            let selection_is_unplottable = self
                .selected
                .and_then(|idx| self.track.points.get(idx))
                .is_none_or(|point| !point.has_position());
            if selection_is_unplottable && let Some(index) = fallback {
                self.select_point(Some(index));
            }
            if self.map_zoom < FOLLOW_MIN_ZOOM {
                self.map_zoom = FOLLOW_MIN_ZOOM;
            }
        }
    }

    /// Set the map zoom level explicitly, respecting the follow-mode floor (`>= 12`).
    pub fn set_zoom(&mut self, zoom: f32) {
        self.fit_mode = false;
        let min_zoom = if self.follow {
            FOLLOW_MIN_ZOOM
        } else {
            MIN_ZOOM
        };
        self.map_zoom = zoom.clamp(min_zoom, MAX_ZOOM);
    }

    /// The geotag options the photo block currently asks for.
    ///
    /// Grouping and the backup are carried into the plan (the core copies them
    /// onto each [`GeotagPlan`]), so the plan a person reviews is the plan
    /// `apply` writes.
    ///
    /// # Errors
    ///
    /// A non-numeric grouping distance. Zero (or an empty field) means "do not
    /// group", which is the core's `None`.
    fn geotag_options(&self) -> Result<GeotagOptions, String> {
        let trimmed = self.photo_group_distance.trim();
        let group_distance_meters = if trimmed.is_empty() {
            None
        } else {
            let metres = trimmed.parse::<f64>().map_err(|_| {
                format!("{trimmed:?} は距離（m）ではありません。数字か空欄にしてください")
            })?;
            (metres > 0.0).then_some(metres)
        };
        let max_gap_seconds = match self.photo_max_gap.trim() {
            "" => GeotagOptions::default().max_gap_seconds,
            text => text.parse::<u32>().map_err(|_| {
                format!("{text:?} は秒数ではありません。一致させる最大の差を秒で入力してください")
            })?,
        };
        Ok(GeotagOptions {
            keep_backup: self.photo_keep_backup,
            group_distance_meters,
            max_gap_seconds,
            ..GeotagOptions::default()
        })
    }

    /// Scan a file or directory for photos and build a read-only geotag plan.
    ///
    /// # Errors
    ///
    /// Returns a Japanese error string if the path does not exist, cannot be read,
    /// contains no files, or `geotag::plan` fails.
    pub fn load_photos_from_path(&mut self, path: &Path) -> Result<usize, String> {
        let paths = match kmz_export::collect_photos(path) {
            Ok(paths) => paths,
            Err(message) => {
                self.photo_status = Some((Status::Err, message.clone()));
                return Err(message);
            }
        };
        // The first plan is the unshifted one, so a photo list exists before a
        // `byCamera` scope can name the highlighted photo it compares against.
        let options = match self.geotag_options() {
            Ok(options) => options,
            Err(message) => {
                self.photo_status = Some((Status::Err, message.clone()));
                return Err(message);
            }
        };
        let plans = match geotag::plan(&paths, &self.track, &options) {
            Ok(plans) => plans,
            Err(err) => {
                let message = format!("写真を読み込めません: {err}");
                self.photo_status = Some((Status::Err, message.clone()));
                return Err(message);
            }
        };

        self.photo_path = path.display().to_string();
        self.photo_entries = plans
            .into_iter()
            .map(|plan| photo_entry(&self.track, plan))
            .collect();
        self.photo_shift.selected.clear();
        self.photo_shift.pending = false;
        self.close_geotag_confirmation();
        self.sync_photo_to_selected();
        if self.selected_photo.is_none() && !self.photo_entries.is_empty() {
            self.selected_photo = Some(0);
        }
        // A non-default shift or scope is now applicable, because the highlighted
        // photo exists; the unshifted plan above stays on screen if it fails, and
        // `replan_photos` marks it stale so it is not written.
        if self.photo_shift.kind != ScopeKind::All
            || self
                .photo_shift
                .shift
                .total_seconds()
                .is_some_and(|seconds| seconds != 0)
        {
            self.replan_photos()?;
        }

        let total = self.photo_entries.len();
        let matched = self.matched_photo_count();
        self.photo_status = Some((
            Status::Ok,
            format!("{total} 枚を読み込みました（一致 {matched} 枚・5 分以内）"),
        ));
        Ok(matched)
    }

    /// Re-plan the loaded photos with the current shift and scope.
    ///
    /// On success the entries are replaced by the new decision. On failure the
    /// previous entries stay on screen so the list and controls remain
    /// operable, and [`PhotoShiftSettings::plan_stale`] is set: the plan a
    /// person reviews must be the plan `apply` writes, so an older plan must not
    /// be written while the controls ask for something else.
    ///
    /// # Errors
    ///
    /// The scope's own refusal, re-worded for the rail. The caller decides
    /// whether the older plan can still be shown (an explicit control change) or
    /// must be dropped (a track edit moved the points it names).
    fn replan_photos(&mut self) -> Result<(), String> {
        let paths: Vec<PathBuf> = self
            .photo_entries
            .iter()
            .map(|entry| entry.plan.photo.clone())
            .collect();
        self.photo_shift.pending = false;
        if paths.is_empty() {
            return Ok(());
        }
        let options = match self.geotag_options() {
            Ok(options) => options,
            Err(message) => {
                self.photo_shift.status = Some((Status::Err, message.clone()));
                return Err(message);
            }
        };
        match self
            .photo_shift
            .plan(&paths, &self.track, &options, self.selected_photo)
        {
            Ok(plans) => {
                self.photo_shift.note_plans(&plans);
                self.photo_entries = plans
                    .into_iter()
                    .map(|plan| photo_entry(&self.track, plan))
                    .collect();
                self.close_geotag_confirmation();
                self.photo_shift.plan_stale = false;
                self.photo_status = None;
                self.photo_shift.status = Some((
                    Status::Ok,
                    format!("{} 枚を再計画しました", self.photo_entries.len()),
                ));
                Ok(())
            }
            Err(message) => {
                self.photo_shift.plan_stale = true;
                self.close_geotag_confirmation();
                self.photo_shift.status = Some((
                    Status::Err,
                    format!("{message}。前の計画のままなので書き込みは止めています"),
                ));
                Err(message)
            }
        }
    }

    /// Select a photo entry by index and jump `selected_point` to its matched track point.
    ///
    /// Closes an open geotag confirmation for the same reason as [`Self::select_point`].
    pub fn select_photo(&mut self, photo_index: usize) {
        let Some(entry) = self.photo_entries.get(photo_index) else {
            return;
        };
        let point_index = entry.point_index;
        let previous_photo = self.selected_photo;
        self.selected_photo = Some(photo_index);
        if let Some(point_index) = point_index
            && point_index < self.track.points.len()
        {
            // `select_point` re-syncs the highlighted photo and may pick a
            // different one than the row that was clicked. The photo the person
            // clicked is the one they asked to look at, so it is set again after;
            // the map still moves to the clicked photo's point.
            self.select_point(Some(point_index));
            self.selected_photo = Some(photo_index);
        } else {
            self.close_geotag_confirmation();
        }
        self.replan_by_camera_if_base_changed(previous_photo);
    }

    /// Re-plan when the highlighted photo's camera changes under `byCamera`.
    ///
    /// The highlighted row is both "the photo the map points at" and the base
    /// photo `byCamera` reads Make/Model from. Selecting another photo of the
    /// same camera leaves the scope identical, so only a *camera* change
    /// re-plans; that keeps playback from re-reading the folder every step while
    /// still making the list follow the highlighted camera.
    fn replan_by_camera_if_base_changed(&mut self, previous_photo: Option<usize>) {
        if self.photo_shift.kind != ScopeKind::ByCamera || self.selected_photo == previous_photo {
            return;
        }
        let Some(photo) = self
            .selected_photo
            .and_then(|index| self.photo_entries.get(index))
            .map(|entry| entry.plan.photo.clone())
        else {
            return;
        };
        if self.photo_shift.by_camera_unchanged(&photo) {
            return;
        }
        let _replan_failed = self.replan_photos();
    }

    /// Apply the planned geotags to disk.
    ///
    /// Takes the [`GeotagConfirmation`] the prompt produced, so this cannot be
    /// reached without it: a second caller (a future 「保存して次へ」, a bulk path,
    /// a test) has to ask [`Self::confirm_geotag_write`], which is the button the
    /// person pressed. The type lives in the private child module
    /// [`geotag_gate`], so this module cannot mint one either — the argument is a
    /// compile-time proof, not a convention. On success the confirmation is
    /// closed and the plans are re-planned, because those photos now carry a
    /// position.
    ///
    /// **Private on purpose.** The only caller is the 「書き込む」 button in
    /// [`Self::export_bar`]. A public writer would hand every future call site a
    /// function it can only call from inside this module — the compiler would say
    /// "unreachable type", and the temptation is to make the token public again.
    /// A screen-private writer with a private token is the pair that holds.
    ///
    /// # Errors
    ///
    /// Returns a Japanese error string if no matched photos exist or writing EXIF fails.
    fn apply_geotags(&mut self, _confirmation: GeotagConfirmation) -> Result<usize, String> {
        if self.photo_shift.plan_stale {
            let msg = String::from(
                "写真の計画が最新ではないため書き込みません。範囲かシフトを直すか、写真を読み込み直してください",
            );
            self.outcome = Some((Status::Err, msg.clone()));
            self.close_geotag_confirmation();
            return Err(msg);
        }
        let plans: Vec<GeotagPlan> = self
            .photo_entries
            .iter()
            .map(|entry| entry.plan.clone())
            .collect();
        let matched = plans.iter().filter(|p| p.will_write()).count();
        if matched == 0 {
            let msg = String::from("位置を書き込める写真がありません");
            self.outcome = Some((Status::Err, msg.clone()));
            self.close_geotag_confirmation();
            return Err(msg);
        }
        // Counted before the write and before the shift is cleared: the message
        // has to say how many clocks moved, and `shifted_count` reads the value
        // that is about to be zeroed.
        let moved = self.photo_shift.shifted_count(&plans);
        let shift_seconds = self.photo_shift.shift.total_seconds();
        let result = geotag::apply(&plans);
        // Closed before the branch: success and failure both end the confirmation,
        // because a failed write leaves nothing the person can approve again.
        self.close_geotag_confirmation();
        match result {
            Ok(written) => {
                let count = written.len();
                // The shift is written into `DateTimeOriginal` by `apply`, so it
                // must not stay armed: the next plan would move the same clocks a
                // second time. Zeroing it keeps the controls and the files in step.
                self.photo_shift.shift = PhotoTimeShift::default();
                self.photo_shift.status = None;
                let msg = if shift_seconds.is_some_and(|seconds| seconds != 0) {
                    format!(
                        "{count} 枚の写真に位置情報を書き込みました（シフト: 範囲内 {} 枚 / 範囲外 {} 枚。シフト値は 0 に戻しました）",
                        moved.min(count),
                        count.saturating_sub(moved)
                    )
                } else {
                    format!("{count} 枚の写真に位置情報を書き込みました")
                };
                // Re-plan before recording the message: a byCamera re-plan that
                // fails has to keep its own error visible instead of being
                // overwritten by the success sentence.
                if self.relink_photos() {
                    self.photo_shift.status = None;
                }
                self.outcome = Some((Status::Ok, msg.clone()));
                self.photo_status = Some((Status::Ok, msg));
                Ok(count)
            }
            Err(err) => {
                let msg = format!("写真へ書き込めませんでした: {err}");
                self.outcome = Some((Status::Err, msg.clone()));
                Err(msg)
            }
        }
    }

    /// Open the geotag confirmation prompt.
    ///
    /// Only meaningful when a photo would be written; with none, the prompt would
    /// offer to change zero files. A pending control value is planned first, and
    /// a failed re-plan refuses with its own sentence rather than offering to
    /// write the older plan the person is no longer looking at.
    fn open_geotag_confirmation(&mut self) {
        if self.photo_shift.pending {
            let _replan_failed = self.replan_photos();
        }
        if self.photo_shift.plan_stale {
            let msg = String::from(
                "写真の計画が最新ではないため確認を開けません。範囲かシフトを直すか、写真を読み込み直してください",
            );
            self.outcome = Some((Status::Err, msg));
            self.confirm_geotag = false;
            return;
        }
        self.confirm_geotag = self.matched_photo_count() > 0;
    }

    /// Close the geotag confirmation prompt.
    ///
    /// Called by 「キャンセル」, after a write, and by every change that would make
    /// the prompt's premise false (another photo loaded, another point selected,
    /// a new track): the prompt names a set of files, and it must not survive a
    /// change to that set.
    fn close_geotag_confirmation(&mut self) {
        self.confirm_geotag = false;
    }

    /// Answer the open confirmation prompt with 「書き込む」.
    ///
    /// The one producer of [`GeotagConfirmation`], and the caller is the one
    /// 「書き込む」 button. Returns `None` when no prompt is open, so a click that
    /// arrives after the prompt closed writes nothing.
    fn confirm_geotag_write(&mut self) -> Option<GeotagConfirmation> {
        if !self.confirm_geotag {
            return None;
        }
        Some(GeotagConfirmation::answered_confirmation())
    }

    /// Draw the screen body using default settings.
    pub fn ui(&mut self, ui: &mut Ui) {
        self.ui_with_settings(ui, &AppSettings::default());
    }

    /// Draw the screen body with active application settings (units & reduced motion).
    ///
    /// The map is the page (DESIGN.md §5): one tool row, then the canvas, with the
    /// log file, the playback strip and the zoom drawn over it and the rest of the
    /// controls in a drawer that starts closed. Nothing scrolls — the canvas
    /// follows the window, because a map that has to be scrolled to is not the
    /// main surface.
    pub fn ui_with_settings(&mut self, ui: &mut Ui, settings: &AppSettings) {
        self.advance_playback_if_due(ui, settings.reduced_motion);
        self.handle_keyboard(ui);

        let content = ui.max_rect();
        layout_probe::begin(content);
        let tool = self.tool_row(ui);
        layout_probe::tool(tool);

        // A wide *content* area gives the drawer its own column; a narrow one lays
        // it over the map. The threshold is the Log body's width, not the window's:
        // DESIGN.md §5/§9 name the content width, and the rail and the page margin
        // are part of the window, not of this screen.
        let wide = content.width() >= metric::NARROW;
        if self.drawer_open && wide {
            self.drawer_column(ui, settings);
        }

        let available = ui.available_rect_before_wrap();
        let strip = drawer::strip_height(self.track.points.is_empty());
        let map_rect = Rect::from_min_size(
            available.min,
            Vec2::new(available.width(), (available.height() - strip).max(0.0)),
        );
        let strip_rect = Rect::from_min_max(
            egui::pos2(available.left(), map_rect.bottom()),
            available.max,
        );
        layout_probe::map(map_rect);
        // `Sense::CLICK` rather than `Sense::click()`: the canvas picks the
        // nearest point from the pointer's position, so it has nothing to say to
        // a keyboard and must not become an invisible tab stop (design review
        // 2026-11-23, M4).
        let response = ui.allocate_rect(map_rect, Sense::CLICK);
        let canvas = self.map_canvas(ui, map_rect, &response, settings);
        ui.allocate_rect(strip_rect, Sense::hover());
        if strip > 0.0 {
            self.selection_strip(ui, strip_rect, settings);
        }

        // The plates depend on two measurements the canvas produced: the zoom
        // note adds a row, and a file-load refusal adds the wrapped lines the
        // plate has to hold. Both are measured before anything is laid out.
        let file_status_height = self.load_status.as_ref().map_or(0.0, |(status, message)| {
            let plate_width = (map_rect.width() * 0.55).clamp(280.0, 440.0);
            drawer::status_height(ui, *status, message, plate_width)
        });
        let plates = drawer::Plates::new(map_rect, file_status_height, canvas.note.is_some());
        let floating = (self.drawer_open && !wide)
            .then(|| plates.floating(map_rect, self.track.points.is_empty()));
        self.map_overlays(ui, map_rect, canvas, &plates, floating);
        if let Some(rect) = floating {
            self.drawer_plate(ui, rect, settings);
        }
        // Last, so the floating drawer cannot bury the only way out of it.
        self.drawer_toggle(ui, map_rect);
    }

    /// Draw the operations console without holding two `&mut` to this screen.
    ///
    /// The console takes the screen because a finished download or a loaded
    /// project replaces the track and the export settings; taking the console
    /// out first is what lets the call hold `&mut self` instead of a pile of
    /// split field borrows.
    fn ops_ui(&mut self, ui: &mut Ui) {
        let mut ops = std::mem::take(&mut self.ops);
        ops.ui(ui, self);
        self.ops = ops;
    }

    fn advance_playback_if_due(&mut self, ui: &Ui, reduced_motion: bool) {
        let interval = if reduced_motion {
            PLAY_STEP_REDUCED
        } else {
            PLAY_STEP
        };
        if self.advance_playback(Instant::now(), interval) {
            ui.ctx().request_repaint_after(interval);
        }
    }

    /// Step playback forward when `interval` has passed; returns `true` while it is
    /// still running, so the caller can ask for another frame.
    ///
    /// The clock is a parameter rather than read here so the rule (elapsed time
    /// decides, the end stops it) can be tested without sleeping. At the last
    /// point the step fails, and playback stops: a run that kept `playing` true
    /// would ask egui to repaint forever with nothing left to advance.
    fn advance_playback(&mut self, now: Instant, interval: Duration) -> bool {
        if !self.playing {
            return false;
        }
        let due = self
            .last_step_at
            .is_none_or(|prev| now.saturating_duration_since(prev) >= interval);
        if due {
            if self.step_next() {
                self.last_step_at = Some(now);
            } else {
                self.playing = false;
                self.last_step_at = None;
            }
        }
        self.playing
    }

    fn handle_keyboard(&mut self, ui: &Ui) {
        if self.track.points.is_empty() || ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        let (space_pressed, right_pressed, left_pressed) = ui.input(|input| {
            (
                input.key_pressed(egui::Key::Space),
                input.key_pressed(egui::Key::ArrowRight),
                input.key_pressed(egui::Key::ArrowLeft),
            )
        });
        if space_pressed {
            self.toggle_play();
        } else if right_pressed {
            let _moved = self.step_next();
        } else if left_pressed {
            let _moved = self.step_prev();
        }
    }

    /// The 集計 figures: distance, elapsed and moving time, average and top
    /// speed, and the altitude range.
    ///
    /// The record count lives on the Connect screen's log panel; repeating it
    /// here made this subordinate block compete with the point list beside it.
    fn summary_block(&self, ui: &mut Ui, settings: &AppSettings) {
        common::block_title(ui, "集計", self.sample);
        common::hairline(ui);
        ui.add_space(space::XS);
        let moving = self.moving_seconds();
        let altitude = self.altitude_range().map_or_else(
            || String::from("—"),
            |(low, high)| {
                format!(
                    "{} – {}",
                    settings.units.format_altitude(Some(low)),
                    settings.units.format_altitude(Some(high))
                )
            },
        );
        for (label, value) in [
            ("距離", self.total_distance_with_settings(settings)),
            ("時間", self.elapsed()),
            (
                "移動時間",
                if moving == 0 {
                    String::from("—")
                } else {
                    common::duration(Some(moving))
                },
            ),
            ("平均", self.average_speed_with_settings(settings)),
            (
                "最高速度",
                settings.units.format_speed(self.max_speed_kmh()),
            ),
            ("高度", altitude),
        ] {
            common::figure_row_compact(ui, label, &value);
        }
        if self.report.damaged_blocks > 0 {
            common::status_line(
                ui,
                Status::Err,
                &format!("{} ブロックが壊れています", self.report.damaged_blocks),
            );
        }
    }

    /// The day filter, at the top of the 集計 tab.
    ///
    /// A checked day is drawn; an unchecked one leaves the canvas, the fit and the
    /// point list together (ezTour §5: the track list selects the track). That is
    /// what lets a trip covering ten degrees be read one day — and one zoom level
    /// — at a time, rather than only as a continent-wide outline.
    fn track_days_block(&mut self, ui: &mut Ui) {
        common::block_title(ui, "トラック", self.sample);
        common::hairline(ui);
        ui.add_space(space::XS);
        for (day, count) in self.days() {
            let mut shown = !self.hidden_days.contains(&day);
            let label = format!("{day}  {count} 点");
            if ui
                .checkbox(&mut shown, RichText::new(label).size(size::UI))
                .changed()
            {
                if shown {
                    let _removed = self.hidden_days.remove(&day);
                } else {
                    let _inserted = self.hidden_days.insert(day);
                }
                // Hiding a day changes which records are on screen, so it also
                // changes the answer to "which photos will be written".
                self.close_geotag_confirmation();
            }
        }
    }

    /// 写真 rail block: load a photo folder/file with `geotag::plan` and sync with the track.
    fn photo_rail_section(&mut self, ui: &mut Ui) {
        // The sample marker is the block title here too: the screenshot's photo
        // rows are invented, and every other invented figure on the screen
        // already wears 「見本」 (DESIGN.md §10).
        common::block_title(ui, "写真", self.sample);
        common::hairline(ui);
        ui.add_space(space::XS);

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("フォルダ")
                    .size(size::CAPTION)
                    .color(theme::MUTE),
            );
            // The load action shares the folder's row: the photo rail is one of
            // the densest blocks and a second button row pushed the shift/scope
            // controls under the footer at 1280×800.
            let width = (ui.available_width() - 64.0 - space::S).max(64.0);
            let _response = ui.add_sized(
                Vec2::new(width, metric::ROW),
                egui::TextEdit::singleline(&mut self.photo_path)
                    .hint_text(RichText::new("写真フォルダ").size(size::CAPTION))
                    .font(egui::TextStyle::Monospace),
            );
            if common::tool_button(ui, "読込", false, true).clicked() {
                let target = PathBuf::from(self.photo_path.trim());
                let _res = self.load_photos_from_path(&target);
            }
        });

        if let Some((status, message)) = &self.photo_status {
            ui.add_space(space::XS);
            common::status_line(ui, *status, message);
        }

        // The two options the CLI exposes as flags: grouping by distance and
        // keeping a sibling `_original` before the rewrite. They are drawn before
        // the empty check so the backup promise is visible before any photo is
        // loaded, not only once a write is possible.
        ui.add_space(space::XS);
        let has_entries = !self.photo_entries.is_empty();
        let mut options_changed = false;
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("グループ")
                    .size(size::CAPTION)
                    .color(theme::MUTE),
            );
            let reserve = if has_entries { 64.0 + space::S } else { 0.0 };
            let width = (ui.available_width() - reserve).max(48.0);
            // Re-plan when the field is done, not on every keystroke: planning
            // reads every JPEG, and typing "500" would read the folder three
            // times. The 再計画 button is the immediate path.
            if ui
                .add_sized(
                    Vec2::new(width, metric::ROW),
                    egui::TextEdit::singleline(&mut self.photo_group_distance)
                        .hint_text(RichText::new("m").size(size::CAPTION))
                        .font(egui::TextStyle::Monospace),
                )
                .lost_focus()
            {
                options_changed = true;
            }
            if has_entries && common::tool_button(ui, "再計画", false, true).clicked() {
                options_changed = true;
            }
        });
        ui.label(
            RichText::new("同じ場所の写真をまとめる（m）。空欄なら 1 枚ずつ扱います")
                .size(size::CAPTION)
                .color(theme::MUTE),
        );
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("一致の許容")
                    .size(size::CAPTION)
                    .color(theme::MUTE),
            );
            let width = (ui.available_width() - 40.0).max(48.0);
            let response = ui.add_sized(
                Vec2::new(width, metric::ROW),
                egui::TextEdit::singleline(&mut self.photo_max_gap)
                    .hint_text(RichText::new("秒（既定 300）").size(size::CAPTION))
                    .font(egui::TextStyle::Monospace),
            );
            if response.lost_focus() {
                options_changed = true;
            }
            ui.label(RichText::new("秒").size(size::CAPTION).color(theme::MUTE));
        });
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("カメラ時差")
                    .size(size::CAPTION)
                    .color(theme::MUTE),
            );
            let width = (ui.available_width() - 40.0).max(48.0);
            let _response = ui.add_sized(
                Vec2::new(width, metric::ROW),
                egui::TextEdit::singleline(&mut self.kmz_tz_offset_minutes)
                    .hint_text(RichText::new("分（UTC との差）").size(size::CAPTION))
                    .font(egui::TextStyle::Monospace),
            );
            ui.label(RichText::new("分").size(size::CAPTION).color(theme::MUTE));
        });
        ui.label(
            RichText::new("カメラ時差は KMZ の写真照合だけに使います（例: UTC+9 は 540）")
                .size(size::CAPTION)
                .color(theme::MUTE),
        );
        if ui
            .checkbox(
                &mut self.photo_keep_backup,
                RichText::new("元の写真を `_original` として残す（既定 ON）").size(size::CAPTION),
            )
            .changed()
        {
            options_changed = true;
        }
        ui.label(
            RichText::new("オフにすると元の JPEG を直接書き換えます（元に戻せません）")
                .size(size::CAPTION)
                .color(theme::MUTE),
        );
        if options_changed && has_entries {
            let _replanned = self.replan_photos();
        }

        ui.add_space(space::XS);
        if self.photo_entries.is_empty() {
            ui.label(
                RichText::new("写真を読み込むと時刻から軌跡と突き合わせます")
                    .size(size::CAPTION)
                    .color(theme::MUTE),
            );
            // A refused load or a track edit that invalidated the plan drops the
            // entries; its reason still has to be drawn where the photo controls
            // are, not only where the list used to be.
            if let Some((status, message)) = &self.photo_shift.status {
                common::status_line(ui, *status, message);
            }
            return;
        }

        let mut clicked_photo: Option<usize> = None;
        // A checkbox in the list feeds the `選択中` shift scope; the flag lets
        // the re-plan run once after the list rather than once per frame.
        let mut shift_selection_changed = false;
        let entries_len = self.photo_entries.len();
        egui::ScrollArea::vertical()
            .id_salt("m241-log-photos")
            .max_height(metric::ROW * 5.0)
            .auto_shrink([false, true])
            .show_rows(ui, metric::ROW_COMPACT, entries_len, |ui, range| {
                for index in range {
                    // Copied out of the entry before the checkbox mutates the
                    // selection set: the row's strings are all the row needs, and
                    // holding the borrow across a `&mut self.photo_shift` would
                    // not compile.
                    let Some((name, suffix, skipped)) =
                        self.photo_entries.get(index).map(|entry| {
                            let name = entry
                                .plan
                                .photo
                                .file_name()
                                .and_then(|name| name.to_str())
                                .unwrap_or("photo")
                                .to_owned();
                            let suffix = if entry.plan.will_write() {
                                entry
                                    .plan
                                    .gap_seconds
                                    .map_or_else(|| String::from("一致"), |gap| format!("{gap}s"))
                            } else {
                                String::from("対象外")
                            };
                            (name, suffix, entry.plan.skipped.clone())
                        })
                    else {
                        continue;
                    };
                    ui.horizontal(|ui| {
                        let mut checked = self.photo_shift.selected.contains(&index);
                        if ui.add(egui::Checkbox::without_text(&mut checked)).changed() {
                            if checked {
                                let _inserted = self.photo_shift.selected.insert(index);
                            } else {
                                let _removed = self.photo_shift.selected.remove(&index);
                            }
                            shift_selection_changed = true;
                        }
                        let selected = self.selected_photo == Some(index);
                        let response = ui.selectable_label(
                            selected,
                            RichText::new(format!("{name}  {suffix}"))
                                .size(size::CAPTION)
                                .color(if selected { theme::INK } else { theme::MUTE })
                                .monospace(),
                        );
                        if let Some(reason) = &skipped {
                            let _ = response.clone().on_hover_text(reason);
                        }
                        if response.clicked() {
                            clicked_photo = Some(index);
                        }
                    });
                }
            });
        if let Some(index) = clicked_photo {
            self.select_photo(index);
        }

        // `photo_shift.ui` opens with its own `space::S`; a second one here
        // pushed 集計's heading past the rail's viewport at 1280×800.
        let controls_settled = self.photo_shift.ui(ui, &self.photo_entries);
        if shift_selection_changed || controls_settled {
            let _replanned = self.replan_photos();
        }
    }

    /// 編集: split / merge / delete with the editor's undo history.
    ///
    /// The editor returns its concatenated tracks whenever it changed; the
    /// screen adopts them here so the map, tables and photo matching all read
    /// the same points. The block lives in the rail, not beside the point list,
    /// because the point list is a full-width table whose selection is already
    /// the screen's densest control.
    fn edit_section(&mut self, ui: &mut Ui) {
        common::block_title(ui, "編集", self.sample);
        common::hairline(ui);
        // Name and colour are display-only: `Track` carries neither yet (the
        // model extension is a separate core change). Showing where they will
        // come from is honest about the gap; inventing a per-track colour would
        // not be.
        let name = if self.sample {
            String::from("見本")
        } else {
            Path::new(self.log_path.trim()).file_stem().map_or_else(
                || String::from("—"),
                |stem| stem.to_string_lossy().into_owned(),
            )
        };
        common::figure_row_compact(ui, "トラック名", &name);
        common::figure_row_compact(ui, "色", "データ色（軌跡）");
        ui.label(
            RichText::new("名前と色は表示のみです（コアのモデル拡張待ち）")
                .size(size::CAPTION)
                .color(theme::MUTE),
        );
        ui.add_space(space::XS);
        if let Some(track) = self.edit.ui(ui) {
            self.adopt_edited_track(track);
        }
    }

    /// Replace the shown track with the editor's result and re-derive what
    /// depends on point indices: the selection, the map fit and the photo links.
    fn adopt_edited_track(&mut self, track: Track) {
        let length = track.points.len();
        self.track = track;
        self.selected = match self.selected {
            Some(index) if index < length => Some(index),
            _ => (length > 0).then_some(0),
        };
        self.playing = false;
        self.last_step_at = None;
        self.fit_mode = true;
        self.map_zoom = self.compute_fit_zoom();
        // Split / merge / delete can move the track out of the area whose tiles
        // are held, so the map layer is rebuilt from the new bounds.
        self.tiles.reset();
        self.hidden_days.clear();
        self.close_geotag_confirmation();
        self.relink_photos();
    }

    /// 地図: the canvas. Tiles, the track over them, and the picker.
    ///
    /// The map is the product's one light surface (DESIGN.md §5), so everything
    /// on it carries a `window` casing: the polyline is 5pt of `data` over a 7pt
    /// `window` edge, and each record is a palette hue over a `window` disc. The
    /// draw order is the contract — grid, tiles, casing, line, records, start/end,
    /// photos, selection, and the credit last in `map_overlays` — because a
    /// marker drawn before its background disappears under it. The OSM credit is
    /// not drawn here: it is an overlay, so `map_overlays` paints it after the
    /// plates, and its width comes from the plate layout (design review
    /// 2026-11-23, M1 and Should 1).
    fn map_canvas(
        &mut self,
        ui: &Ui,
        rect: Rect,
        response: &egui::Response,
        settings: &AppSettings,
    ) -> drawer::Canvas {
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, theme::RADIUS, theme::SUNKEN);

        let view = self.sync_fit_zoom(rect);
        let plotted: Vec<(usize, egui::Pos2)> = self.plotted_points(rect);
        if plotted.is_empty() {
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                "位置のある点がありません",
                FontId::proportional(size::UI),
                theme::MUTE,
            );
            return drawer::Canvas {
                tiles: 0,
                note: None,
            };
        }
        let first_index = plotted.first().map(|(index, _pos)| *index);
        let last_index = plotted.last().map(|(index, _pos)| *index);
        // The grid stays underneath the tiles: it is the offline canvas, and it
        // also fills a tile that has not arrived yet.
        draw_grid(&painter, rect);
        let drawn = tiles::draw_map(
            ui,
            &painter,
            view,
            &mut self.tiles,
            MapOptions {
                enabled: settings.map_tiles,
                sample: self.sample,
            },
        );
        let line: Vec<egui::Pos2> = plotted.iter().map(|(_index, pos)| *pos).collect();
        if line.len() > 1 {
            // 07-eztour §5's 5pt line, in the product's data colour, over the
            // casing the light surface needs. The numbers live in `metric::track`
            // so this screen names tokens rather than widths.
            painter.add(egui::Shape::line(
                line.clone(),
                metric::track::casing_stroke(),
            ));
            painter.add(egui::Shape::line(line, metric::track::line_stroke()));
        }
        for (index, pos) in &plotted {
            if self.selected == Some(*index) {
                continue;
            }
            // A record: a `window` casing under the cycle's colour. On the offline
            // grid the palette hue carries the sequence (07-eztour §5); on an OSM
            // tile those hues measure under 2:1 against land, so the casing is what
            // keeps the marker visible there (design review 2026-11-23, M1).
            painter.circle_filled(*pos, metric::POINT_DOT + metric::HAIRLINE, theme::WINDOW);
            painter.circle_filled(*pos, metric::POINT_DOT, metric::track::record_fill(*index));
            // Waypoints are the device's own POI button: a second, wider ring so
            // they read as a place rather than as another sample.
            if self
                .track
                .points
                .get(*index)
                .is_some_and(TrackPoint::is_waypoint)
            {
                painter.circle_stroke(
                    *pos,
                    metric::POINT_DOT * 2.0,
                    Stroke::new(metric::TRACK_LINE, theme::INK),
                );
            }
        }

        // 始点 / 終点 (DESIGN.md §5 through ezTour §5): the start is a small ring,
        // the end a large one. The radius separates them, so keep both off the
        // waypoint ring's 2× radius (`a_waypoint_is_drawn_as_a_ring`); the `ground`
        // disc under each ring is what makes the marker visible on a bright tile.
        for (index, radius) in [
            (first_index, metric::POINT_DOT * 1.5),
            (last_index, metric::POINT_DOT * 2.5),
        ] {
            if let Some(index) = index
                && self.selected != Some(index)
                && let Some((_, pos)) = plotted.iter().find(|(idx, _)| *idx == index)
            {
                painter.circle_filled(*pos, radius, theme::WINDOW);
                painter.circle_stroke(*pos, radius, Stroke::new(metric::HAIRLINE, theme::INK));
            }
        }

        // Matched photo markers along the track.
        for (photo_idx, entry) in self.photo_entries.iter().enumerate() {
            let Some(pt_idx) = entry.point_index else {
                continue;
            };
            if let Some((_, pos)) = plotted.iter().find(|(idx, _)| *idx == pt_idx) {
                let active_photo = self.selected_photo == Some(photo_idx);
                let marker_size = if active_photo { space::S } else { space::XS };
                let marker_rect = Rect::from_center_size(*pos, Vec2::splat(marker_size));
                painter.rect_filled(marker_rect, theme::RADIUS, theme::WINDOW);
                painter.rect_stroke(
                    marker_rect,
                    theme::RADIUS,
                    Stroke::new(metric::HAIRLINE, theme::INK),
                    StrokeKind::Outside,
                );
            }
        }

        // Selected / active playback marker drawn last so it stays on top.
        //
        // A `ground` halo, then a solid `ink` disc, then a thin `ink` ring one
        // radius out. The disc is filled rather than stroked so it cannot be
        // confused with a waypoint's ring — that ring is the only translucent,
        // stroked circle at twice a record's radius, which is what the render
        // test counts.
        if let Some(selected_idx) = self.selected
            && let Some((_, pos)) = plotted.iter().find(|(idx, _)| *idx == selected_idx)
        {
            painter.circle_filled(*pos, metric::POINT_DOT * 3.5, theme::WINDOW);
            painter.circle_filled(*pos, metric::POINT_DOT * 2.0, theme::INK);
            painter.circle_stroke(
                *pos,
                metric::POINT_DOT * 3.0,
                Stroke::new(metric::HAIRLINE, theme::INK),
            );
        }

        // OSM requires the credit where its tiles are shown; a sample, offline or
        // still-loading frame shows none, so it carries none. The credit itself is
        // painted by `map_overlays`, after the plates, so the area it may use comes
        // from the same plate layout as the zoom.
        let tiles = drawn.tiles;

        // Clicking the map selects the nearest plotted point: the map is a
        // picker, not a picture, and a track you cannot ask about is a diagram.
        // The answer is the strip under the canvas, so it is never below a fold.
        if response.clicked()
            && let Some(pointer) = response.interact_pointer_pos()
            && let Some((index, _distance)) = plotted
                .iter()
                .map(|(index, pos)| (*index, pos.distance(pointer)))
                .min_by(|left, right| left.1.total_cmp(&right.1))
        {
            self.select_point(Some(index));
        }

        drawer::Canvas {
            tiles,
            note: drawn.background.note(),
        }
    }

    /// The playback strip drawn over the canvas: the transport, then the timeline.
    ///
    /// The zoom owns its own plate; the selected point's figures live in the strip
    /// under the canvas, so this plate stays two rows tall (DESIGN.md §5).
    fn playback_controls(&mut self, ui: &mut Ui) {
        let len = self.track.points.len();
        if len == 0 {
            return;
        }
        ui.horizontal_wrapped(|ui| {
            let play_label = if self.playing {
                "一時停止"
            } else {
                "再生"
            };
            // The toolbar is the `tool_button` class, not egui's default filled
            // face: on this screen the default made the playback strip the
            // brightest thing on the page, and a filled box beside 中止 (outline)
            // read as a second primary action (design review, Should 4).
            if common::tool_button(ui, play_label, self.playing, true).clicked() {
                self.toggle_play();
            }
            if common::tool_button(ui, "前の点", false, true).clicked() {
                let _moved = self.step_prev();
            }
            if common::tool_button(ui, "次の点", false, true).clicked() {
                let _moved = self.step_next();
            }
            if common::tool_button(ui, "全点表示", false, true).clicked() {
                self.fit_all();
            }

            let follow = self.follow;
            if common::tool_button(ui, "追従", follow, true).clicked() {
                self.set_follow(!follow);
            }
        });

        ui.add_space(space::XS);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("再生位置")
                    .size(size::CAPTION)
                    .color(theme::MUTE),
            );
            let max_idx = len.saturating_sub(1);
            let pos = self.selected.unwrap_or(0).min(max_idx);
            let track_width = (ui.available_width() - 64.0).max(48.0);
            // `Sense::CLICK | Sense::DRAG`, not `Sense::click_and_drag()`: the
            // timeline reads the pointer's position, so it is not keyboard
            // operable and must not be a focusable stop that shows nothing
            // (design review 2026-11-23, M4).
            let (rect, response) = ui.allocate_exact_size(
                Vec2::new(track_width, metric::ROW * 0.5),
                Sense::CLICK | Sense::DRAG,
            );
            let painter = ui.painter_at(rect);
            let y = rect.center().y;
            // The empty track is `line` at the line width, not a hairline: a 1pt
            // line on `window` measured about 1:1, which is not a track at all
            // (design review 2026-11-23, the progress-bar finding).
            painter.hline(
                rect.x_range(),
                y,
                Stroke::new(metric::TRACK_LINE, theme::LINE),
            );
            if max_idx > 0 {
                let thumb_x = rect.left() + rect.width() * (pos as f32 / max_idx as f32);
                let filled =
                    Rect::from_min_max(rect.left_top(), egui::pos2(thumb_x, rect.bottom()));
                // Full-opacity `data`: the old 35% tint measured 1.6:1 against
                // the ground, so the travelled part of the track was invisible.
                painter.rect_filled(filled, theme::RADIUS, theme::DATA);
                painter.circle_filled(egui::pos2(thumb_x, y), metric::POINT_DOT, theme::INK);
            }
            // Picking a point is what a click or a drag *means* here: the thumb is
            // rebuilt every frame from `self.selected`, so a drag that only moved a
            // local value would spring back and select nothing. The selection is
            // made when the gesture ends — on release, and on each drag frame so the
            // map follows the thumb while it moves.
            if (response.clicked() || response.dragged() || response.drag_stopped())
                && let Some(pointer) = response.interact_pointer_pos()
                && max_idx > 0
            {
                let fraction = ((pointer.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
                let target = (fraction * max_idx as f32).round() as usize;
                self.select_point(Some(target.min(max_idx)));
            }
            ui.label(
                RichText::new(format!("{}/{}", pos + 1, len))
                    .size(size::CAPTION)
                    .color(theme::MUTE)
                    .monospace(),
            );
        });
    }

    /// 速度: one series, baseline at zero, no smoothing, with active point marker.
    fn speed_panel(&mut self, ui: &mut Ui, settings: &AppSettings) {
        let indexed_speeds: Vec<(usize, f32)> = self
            .track
            .points
            .iter()
            .enumerate()
            .filter_map(|(idx, point)| {
                point
                    .speed_kmh
                    .map(|s| (idx, settings.units.speed_value(s)))
            })
            .collect();
        let maximum = indexed_speeds
            .iter()
            .map(|(_idx, speed)| *speed)
            .reduce(f32::max);
        let note =
            maximum.map(|max_val| format!("最大 {max_val:.0} {}", settings.units.speed_unit()));
        let title = if self.sample {
            format!("速度／{}", common::SAMPLE_WORD)
        } else {
            String::from("速度")
        };
        common::panel_captioned(ui, &title, note.as_deref(), |ui| {
            // `Sense::CLICK`, not `Sense::click()`: picking on the chart needs the
            // pointer's x, so it has no keyboard mode and must not be a tab stop
            // that shows no focus ring (design review 2026-11-23, M4).
            let (rect, response) = ui
                .allocate_exact_size(Vec2::new(ui.available_width(), metric::CHART), Sense::CLICK);
            let painter = ui.painter_at(rect);
            let Some(maximum) = maximum else {
                painter.text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    "このログには速度が記録されていません",
                    FontId::proportional(size::UI),
                    theme::MUTE,
                );
                return;
            };
            let span = if maximum > 0.0 { maximum } else { 1.0 };
            let baseline = rect.bottom() - space::S;
            painter.line_segment(
                [
                    egui::pos2(rect.left(), baseline),
                    egui::pos2(rect.right(), baseline),
                ],
                Stroke::new(metric::HAIRLINE, theme::LINE),
            );
            let step = rect.width() / (indexed_speeds.len().max(2) - 1) as f32;
            let path: Vec<(usize, egui::Pos2)> = indexed_speeds
                .iter()
                .enumerate()
                .map(|(plot_idx, (track_idx, speed))| {
                    (
                        *track_idx,
                        egui::pos2(
                            rect.left() + step * plot_idx as f32,
                            baseline - (speed / span) * (rect.height() - space::L),
                        ),
                    )
                })
                .collect();
            let line: Vec<egui::Pos2> = path.iter().map(|(_idx, pos)| *pos).collect();
            if line.len() > 1 {
                painter.add(egui::Shape::line(
                    line,
                    egui::epaint::PathStroke::new(metric::TRACK_LINE, theme::DATA),
                ));
            }

            if let Some(selected_idx) = self.selected
                && let Some((_, active_pos)) = path
                    .iter()
                    .find(|(track_idx, _)| *track_idx == selected_idx)
            {
                painter.vline(
                    active_pos.x,
                    rect.top()..=baseline,
                    Stroke::new(metric::HAIRLINE, theme::INK),
                );
                painter.circle_filled(*active_pos, metric::POINT_DOT * 1.5, theme::INK);
            }

            painter.text(
                rect.right_bottom() + Vec2::new(-space::XS, -space::XS),
                Align2::RIGHT_BOTTOM,
                "0",
                FontId::monospace(size::CAPTION),
                theme::MUTE,
            );

            if response.clicked()
                && let Some(pointer) = response.interact_pointer_pos()
                && let Some((track_idx, _)) = path
                    .iter()
                    .map(|(track_idx, pos)| (*track_idx, (pos.x - pointer.x).abs()))
                    .min_by(|a, b| a.1.total_cmp(&b.1))
            {
                self.select_point(Some(track_idx));
            }
        });
    }

    /// The point list, scrolled and virtualised: a real log is 10k rows and
    /// `show_rows` keeps the frame cost independent of the length.
    fn point_table(&mut self, ui: &mut Ui, settings: &AppSettings) {
        let shown = self.visible_indices();
        common::block_title(ui, "点の一覧", self.sample);
        common::hairline(ui);
        let mut clicked_index: Option<usize> = None;
        egui::ScrollArea::vertical()
            .id_salt("m241-log-points")
            .max_height(metric::ROW * ROWS_SHOWN)
            .auto_shrink([false, true])
            .show_rows(ui, metric::ROW, shown.len(), |ui, range| {
                for row in range {
                    let Some(index) = shown.get(row).copied() else {
                        continue;
                    };
                    // Copied out of the track before the row is drawn: the row can
                    // change the selection, and a `&TrackPoint` borrowed from
                    // `self.track` would forbid that. `TrackPoint` is `Copy`.
                    let Some(point) = self.track.points.get(index).copied() else {
                        continue;
                    };
                    if self.point_row(ui, index, &point, settings).clicked() {
                        clicked_index = Some(index);
                    }
                }
            });
        if let Some(index) = clicked_index {
            self.select_point(Some(index));
        }
    }

    fn point_row(
        &self,
        ui: &mut Ui,
        index: usize,
        point: &TrackPoint,
        settings: &AppSettings,
    ) -> egui::Response {
        let selected = self.selected == Some(index);
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), metric::ROW), Sense::click());
        if selected {
            ui.painter()
                .rect_filled(rect, theme::RADIUS, theme::OVERLAY);
        }
        let colour = if selected { theme::INK } else { theme::MUTE };
        let font = FontId::monospace(size::CAPTION);
        let painter = ui.painter_at(rect);
        painter.text(
            rect.left_center() + Vec2::new(space::S, 0.0),
            Align2::LEFT_CENTER,
            common::utc_stamp(point.utc),
            font.clone(),
            colour,
        );
        painter.text(
            rect.center() + Vec2::new(0.0, 0.0),
            Align2::CENTER_CENTER,
            coordinate_text(point, settings),
            font.clone(),
            colour,
        );
        painter.text(
            rect.right_center() + Vec2::new(-space::S, 0.0),
            Align2::RIGHT_CENTER,
            settings.units.format_speed(point.speed_kmh),
            font,
            colour,
        );
        ui.painter().hline(
            rect.x_range(),
            rect.bottom(),
            Stroke::new(metric::HAIRLINE, theme::LINE),
        );
        // Rows are focusable (Sense::click); without this the keyboard's position
        // in a 10,000-row list was invisible (design review, Must 1).
        common::focus_ring(ui, &response);
        response
    }

    /// The export panel: a format, a destination, one button that says what it does.
    ///
    /// The panel is drawn in the 360pt drawer (DESIGN.md §5), not in the
    /// full-width footer it used to be, so every row is sized for the drawer.
    /// The six writer chips take two explicit rows: `horizontal_wrapped` cannot
    /// break between two [`common::tool_button`]s, because each is a `ui.scope`
    /// and a scope is placed without a wrap check. Left to wrap, the sixth chip
    /// and the geotag button were pushed past the drawer's clip edge, where egui
    /// stops painting them — the feature was present in the code and absent from
    /// the screen.
    fn export_bar(&mut self, ui: &mut Ui) {
        ui.label(
            RichText::new("書き出し")
                .size(size::CAPTION)
                .color(theme::MUTE),
        );
        ui.add_space(space::XS);
        ui.horizontal(|ui| self.format_chips(ui, &ExportFormat::ALL[..3]));
        ui.horizontal(|ui| {
            self.format_chips(ui, &ExportFormat::ALL[3..]);
            // A sixth writer rather than a sixth `ExportFormat`: KMZ bundles the
            // photos too, so it is not a rendering of the track alone.
            if common::tool_button(ui, "KMZ", self.kmz, true).clicked() {
                self.set_kmz_export();
            }
        });

        ui.add_space(space::S);
        let matched = self.matched_photo_count();
        let photo_button_label = if matched > 0 {
            format!("写真に位置 ({matched} 枚)")
        } else {
            String::from("写真に位置")
        };
        if common::secondary_button(ui, &photo_button_label, matched > 0).clicked() {
            self.open_geotag_confirmation();
        }
        if matched == 0 {
            // Why the button above it is disabled, under the button. The photo
            // block is in another tab, so a reason that lives only there is a
            // reason the reader here cannot see (DESIGN.md §6: a refusal says
            // what the next step is, where the control is).
            ui.label(
                RichText::new("写真を読み込むと位置を書き込めます")
                    .size(size::CAPTION)
                    .color(theme::MUTE),
            );
        }

        // The destination and the one export action stack: at the drawer's 332pt
        // an input field, a caption and a 128pt button do not share a line
        // without the button being clipped at the panel edge.
        ui.add_space(space::M);
        ui.label(
            RichText::new("保存先")
                .size(size::CAPTION)
                .color(theme::MUTE),
        );
        let _response = ui.add_sized(
            Vec2::new(ui.available_width(), metric::ROW),
            egui::TextEdit::singleline(&mut self.destination).font(egui::TextStyle::Monospace),
        );
        ui.add_space(space::S);
        if common::primary_button(ui, "書き出す", true).clicked() {
            let _res = self.export();
        }

        // KMZ is the one writer that needs a second input, so its folder and its
        // long-edge limit sit directly under the action that reads them rather
        // than in the photo tab. The rows stack: four controls on one 332pt line
        // put the last of them past the drawer's clip edge.
        if self.kmz {
            ui.add_space(space::M);
            ui.label(RichText::new("写真").size(size::CAPTION).color(theme::MUTE));
            let width = ui.available_width();
            let _response = ui.add_sized(
                Vec2::new(width, metric::ROW),
                egui::TextEdit::singleline(&mut self.photo_path)
                    .hint_text(RichText::new("写真フォルダ").size(size::CAPTION))
                    .font(egui::TextStyle::Monospace),
            );
            ui.add_space(space::XS);
            ui.horizontal(|ui| {
                kmz_export::max_edge_field(ui, &mut self.kmz_max_edge);
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.kmz_icons, RichText::new("icon").size(size::CAPTION))
                    .on_hover_text(
                        "写真ごとに長辺 320 px のサムネイルを icon/ に同梱します（既定 OFF。icon/ の仕様は core で未確定のため、CLI と同じ出力を既定にしています）",
                    );
                // Google Earth / `unzip` are not part of this build's checks, so
                // the writer wears the word DESIGN.md §6 requires rather than
                // looking like a format that has been opened and judged. The
                // explanation is on hover.
                ui.label(
                    RichText::new("未検証")
                        .size(size::CAPTION)
                        .color(theme::MUTE),
                )
                .on_hover_text("表示中の写真計画（5 分以内）をそのまま同梱します。CLI と同じ export_kmz ですが、Google Earth や unzip での表示は未検証です");
            });
        }

        if self.confirm_geotag {
            ui.add_space(space::S);
            // The prompt names every file it will change, not just a count: the
            // write cannot be undone from this screen, and "一致した 3 枚" does not
            // let a person check which three.
            let targets = geotag_targets(&self.photo_entries);
            let names: Vec<String> = targets
                .iter()
                .map(|path| {
                    path.file_name().map_or_else(
                        || path.display().to_string(),
                        |name| name.to_string_lossy().into_owned(),
                    )
                })
                .collect();
            common::status_line(
                ui,
                Status::Waiting,
                &format!(
                    "次の {} 枚の JPEG に位置情報を書き込みますか？（{}）",
                    names.len(),
                    names.join("、")
                ),
            );
            if self.photo_keep_backup {
                common::status_line(
                    ui,
                    Status::Info,
                    "元の写真は同じフォルダに `<名前>_original` として残します",
                );
            } else {
                common::status_line(
                    ui,
                    Status::Err,
                    "元の写真を残しません（`_original` は作りません）。書き込むと元に戻せません",
                );
            }
            if let Some(note) = self.photo_shift.shift_note(&self.photo_entries) {
                common::status_line(ui, Status::Waiting, &note);
            }
            ui.horizontal_wrapped(|ui| {
                if common::primary_button(ui, "書き込む", true).clicked()
                    && let Some(confirmation) = self.confirm_geotag_write()
                {
                    let _res = self.apply_geotags(confirmation);
                }
                if common::secondary_button(ui, "キャンセル", true).clicked() {
                    self.close_geotag_confirmation();
                }
            });
        }

        if let Some((status, message)) = &self.outcome {
            common::status_line(ui, *status, message);
        }
    }

    /// One row of writer chips, the selected one marked.
    ///
    /// Split out so [`Self::export_bar`] can break the six writers across two
    /// rows explicitly instead of relying on a wrapping layout that cannot see
    /// inside a scoped button.
    fn format_chips(&mut self, ui: &mut Ui, formats: &[ExportFormat]) {
        for &format in formats {
            let label = format.extension().to_uppercase();
            let selected = !self.kmz && self.format == format;
            if common::tool_button(ui, &label, selected, true)
                .on_hover_text(export_format_note(format))
                .clicked()
            {
                self.set_export_format(format);
            }
        }
    }

    /// Change the active export format and update the destination extension while preserving the stem.
    pub fn set_export_format(&mut self, format: ExportFormat) {
        self.kmz = false;
        self.format = format;
        let trimmed = self.destination.trim();
        if trimmed.is_empty() {
            self.destination = format!("m241-track.{}", format.extension());
        } else {
            self.destination = Path::new(trimmed)
                .with_extension(format.extension())
                .display()
                .to_string();
        }
    }

    /// Select the KMZ writer and give the destination its `.kmz` extension.
    pub fn set_kmz_export(&mut self) {
        self.kmz = true;
        let trimmed = self.destination.trim();
        if trimmed.is_empty() {
            self.destination = String::from("m241-track.kmz");
        } else {
            self.destination = Path::new(trimmed)
                .with_extension("kmz")
                .display()
                .to_string();
        }
    }

    /// Set the export destination path.
    pub fn set_destination(&mut self, path: impl Into<String>) {
        self.destination = path.into();
    }

    /// Render the chosen writer and write it, refusing to overwrite an existing
    /// file silently.
    ///
    /// The conversion is `m241_core::log::export`, the same function the CLI calls,
    /// so the two cannot drift into different files from the same log. When the
    /// KMZ chip is selected the archive writer takes over; see
    /// [`Self::export_kmz`].
    ///
    /// # Errors
    ///
    /// Returns a Japanese error string if the track is empty, destination is empty,
    /// the destination already exists, a KMZ input is missing or invalid, or
    /// writing fails.
    pub fn export(&mut self) -> Result<(), String> {
        if self.kmz {
            return self.export_kmz();
        }
        if self.track.points.is_empty() {
            return self.fail(String::from("書き出す点がありません"));
        }
        let dest_path = self.checked_destination()?;
        let dest_trimmed = self.destination.trim().to_owned();
        let document = log::export(&self.track, self.format);
        // Same fields as `log read --format json`: the NMEA counts and the day
        // shift are facts about how this track was read, so they ride along
        // when that read's output is JSON, and the `position` object is derived
        // from the track the CLI derives it from. `None` (a download, the
        // sample) writes the exporter's own object plus the position.
        let document = project::attach_read_report(
            document,
            self.format,
            self.read_facts.unwrap_or_default(),
            &self.track,
        );
        match std::fs::write(&dest_path, document) {
            Ok(()) => {
                self.outcome = Some((
                    Status::Ok,
                    format!(
                        "{dest_trimmed} に {} 点を書き出しました",
                        self.track.fix_count()
                    ),
                ));
                Ok(())
            }
            Err(err) => self.fail(format!("書き出せませんでした: {err}")),
        }
    }

    /// Match the photos in 写真 to the track and write a KMZ archive.
    ///
    /// The plans are the ones on screen — the same list `apply_geotags` writes
    /// and the rail shows — not a second planning pass from the folder. The one
    /// exception is an empty photo block: with nothing displayed there is no
    /// review to honour, so the archive is planned the way the CLI does (no
    /// shift, every photo), and no controls are on screen to disagree with it.
    ///
    /// The order of the checks follows the fields the person can fix: the
    /// destination, the long-edge limit, then the folder. Each refusal is stored
    /// in [`Self::outcome`], which the export bar prints under the action.
    ///
    /// # Errors
    ///
    /// The first input that cannot be used, as a Japanese sentence.
    fn export_kmz(&mut self) -> Result<(), String> {
        if self.track.points.is_empty() {
            return self.fail(String::from("書き出す点がありません"));
        }
        if self.photo_shift.pending && !self.photo_entries.is_empty() {
            let _replan_failed = self.replan_photos();
        }
        if self.photo_shift.plan_stale {
            return self.fail(String::from(
                "写真の計画が最新ではないため KMZ を書き出しません。範囲かシフトを直すか、写真を読み込み直してください",
            ));
        }
        // The KML `<TimeStamp>` is the plan's capture time, and an armed shift
        // moves that time while the photo's Exif clock only changes when the
        // tags are written. Refusing here keeps the archive from disagreeing
        // with the photos it bundles; the next step is the geotag write, which
        // zeroes the shift.
        if self
            .photo_shift
            .shift
            .total_seconds()
            .is_some_and(|seconds| seconds != 0)
        {
            return self.fail(String::from(
                "シフトを設定したままでは KMZ の時刻と写真 Exif が食い違います。先に「写真に位置」で書き込むとシフトは 0 に戻ります",
            ));
        }
        let dest_path = self.checked_destination()?;
        let dest_trimmed = self.destination.trim().to_owned();
        // The chip and the destination can disagree (the field is editable), and
        // a ZIP named `.gpx` would lie about its contents: refuse here rather than
        // write an archive under the wrong extension.
        let extension_matches = dest_path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("kmz"));
        if !extension_matches {
            return self.fail(String::from(
                "保存先の拡張子が .kmz ではありません。KMZ チップを選び直すか、保存先を .kmz にしてください",
            ));
        }
        let max_edge = match kmz_export::parse_max_edge(&self.kmz_max_edge) {
            Ok(max_edge) => max_edge,
            Err(message) => return self.fail(message),
        };
        let photo_path = PathBuf::from(self.photo_path.trim());
        let paths = match kmz_export::collect_photos(&photo_path) {
            Ok(paths) => paths,
            Err(message) => return self.fail(message),
        };
        let options = match self.geotag_options() {
            Ok(options) => options,
            Err(message) => return self.fail(message),
        };
        let tz_minutes = match self.kmz_tz_offset_minutes.trim() {
            "" => 0,
            text => match text.parse::<i32>() {
                Ok(minutes) => minutes,
                Err(_) => {
                    return self.fail(format!(
                        "{text:?} はカメラの時差（分）ではありません。例: UTC+9 は 540"
                    ));
                }
            },
        };
        let plans: Vec<GeotagPlan> = if self.photo_entries.is_empty() {
            if tz_minutes == 0 {
                match geotag::plan(&paths, &self.track, &options) {
                    Ok(plans) => plans,
                    Err(err) => return self.fail(format!("写真を計画できません: {err}")),
                }
            } else {
                match kmz_export::plan_with_tz(&paths, &self.track, &options, tz_minutes) {
                    Ok(plans) => plans,
                    Err(message) => return self.fail(message),
                }
            }
        } else {
            let shown: Vec<PathBuf> = self
                .photo_entries
                .iter()
                .map(|entry| entry.plan.photo.clone())
                .collect();
            if shown != paths {
                return self.fail(String::from(
                    "一覧の写真と写真フォルダが一致しません。「読込」でこのフォルダを読み込んでから書き出してください",
                ));
            }
            if tz_minutes == 0 {
                self.photo_entries
                    .iter()
                    .map(|entry| entry.plan.clone())
                    .collect()
            } else {
                // A camera-clock offset moves which point each photo matches,
                // so the on-screen plan cannot be reused; the archive is
                // re-planned in the camera's clock exactly as `log kmz` does.
                match kmz_export::plan_with_tz(&paths, &self.track, &options, tz_minutes) {
                    Ok(plans) => plans,
                    Err(message) => return self.fail(message),
                }
            }
        };
        let embedded = plans.iter().filter(|plan| plan.will_write()).count();
        let reasons = kmz_export::skipped_reasons(&plans);
        let archive = match kmz_export::build_archive(&self.track, &plans, max_edge, self.kmz_icons)
        {
            Ok(archive) => archive,
            Err(message) => return self.fail(message),
        };
        match std::fs::write(&dest_path, archive) {
            Ok(()) => {
                let mut message = format!(
                    "{dest_trimmed} に {} 点・{embedded} 枚を書き出しました",
                    self.track.points.len()
                );
                if !reasons.is_empty() {
                    message.push_str(&format!(
                        "（対象外 {} 枚: {}）",
                        plans.len().saturating_sub(embedded),
                        reasons.join("、")
                    ));
                }
                self.outcome = Some((Status::Ok, message));
                Ok(())
            }
            Err(err) => self.fail(format!("KMZ を書き出せませんでした: {err}")),
        }
    }

    /// The destination path, or the sentence that says why it cannot be used.
    ///
    /// # Errors
    ///
    /// Empty input, or a path that already exists: an export that quietly
    /// replaced the previous one would destroy the only copy of it.
    fn checked_destination(&mut self) -> Result<PathBuf, String> {
        let trimmed = self.destination.trim().to_owned();
        if trimmed.is_empty() {
            return self.fail(String::from("保存先を入力してください"));
        }
        let path = PathBuf::from(&trimmed);
        if path.exists() {
            return self.fail(format!(
                "{trimmed} はすでに存在するため上書きしません。別のファイル名を指定してください"
            ));
        }
        Ok(path)
    }

    /// Store an operation's error beside the control and hand it back.
    fn fail<T>(&mut self, message: String) -> Result<T, String> {
        self.outcome = Some((Status::Err, message.clone()));
        Err(message)
    }

    /// The view for this frame, publishing the fitted zoom to the `+`/`−` buttons.
    ///
    /// Fit is computed from the canvas size and the Mercator bbox; the buttons
    /// work from `map_zoom`, which [`Self::compute_fit_zoom`] estimates from the
    /// degree span. On an east-west track the two differ by about one and a half
    /// levels, so `+` straight after 全点表示 could shrink the map. Publishing
    /// the zoom that was actually drawn keeps a press monotonic, and the label
    /// already shows `view.zoom`.
    fn sync_fit_zoom(&mut self, rect: Rect) -> MapView {
        let view = self.map_view(rect);
        if self.fit_mode {
            self.map_zoom = view.zoom as f32;
        }
        view
    }

    /// The view the map and the track are both drawn through.
    ///
    /// `fit_mode` fits the whole track; otherwise the canvas is centred on the
    /// selected point (or on the track's middle when nothing is selected) at
    /// `map_zoom`. Both branches use the Mercator projection the tiles are laid
    /// out in: a track drawn through a different projection drifts off the
    /// roads the tiles show, which is the defect the map exists to remove.
    fn map_view(&self, rect: Rect) -> MapView {
        let points: Vec<(f64, f64)> = self
            .track
            .points
            .iter()
            .filter(|point| !self.point_hidden(point))
            .filter_map(|point| Some((point.latitude?, point.longitude?)))
            .collect();
        if points.is_empty() || self.fit_mode {
            return MapView::fit(rect, space::M, &points);
        }
        let (center_latitude, center_longitude) = self
            .selected
            .and_then(|index| self.track.points.get(index))
            .and_then(|point| Some((point.latitude?, point.longitude?)))
            .unwrap_or_else(|| {
                let Some(&(first_latitude, first_longitude)) = points.first() else {
                    return (0.0, 0.0);
                };
                let (mut min_latitude, mut max_latitude) = (first_latitude, first_latitude);
                let (mut min_longitude, mut max_longitude) = (first_longitude, first_longitude);
                for &(latitude, longitude) in &points {
                    min_latitude = min_latitude.min(latitude);
                    max_latitude = max_latitude.max(latitude);
                    min_longitude = min_longitude.min(longitude);
                    max_longitude = max_longitude.max(longitude);
                }
                (
                    (min_latitude + max_latitude) / 2.0,
                    (min_longitude + max_longitude) / 2.0,
                )
            });
        MapView::new(
            center_latitude,
            center_longitude,
            f64::from(self.map_zoom),
            rect,
        )
    }

    /// Every point with a position that the day filter leaves visible, as
    /// (index, pixel).
    ///
    /// Kept as indices rather than as a filtered copy so a click on the map can
    /// select the *original* row in the table. Hidden days are not projected at
    /// all: the track list is what chooses the track on the canvas (ezTour §5).
    fn plotted_points(&self, rect: Rect) -> Vec<(usize, egui::Pos2)> {
        let view = self.map_view(rect);
        self.track
            .points
            .iter()
            .enumerate()
            .filter(|(_index, point)| !self.point_hidden(point))
            .filter_map(|(index, point)| {
                let latitude = point.latitude?;
                let longitude = point.longitude?;
                Some((index, view.project(latitude, longitude)))
            })
            .collect()
    }

    fn compute_fit_zoom(&self) -> f32 {
        let mut iter = self
            .track
            .points
            .iter()
            .filter(|point| !self.point_hidden(point))
            .filter_map(|p| Some((p.latitude?, p.longitude?)));
        let Some((first_lat, first_lon)) = iter.next() else {
            return FOLLOW_MIN_ZOOM;
        };
        let (mut min_lat, mut max_lat) = (first_lat, first_lat);
        let (mut min_lon, mut max_lon) = (first_lon, first_lon);
        for (lat, lon) in iter {
            min_lat = min_lat.min(lat);
            max_lat = max_lat.max(lat);
            min_lon = min_lon.min(lon);
            max_lon = max_lon.max(lon);
        }
        let span = (max_lat - min_lat).max(max_lon - min_lon);
        if span <= 1e-6 {
            return 15.0;
        }
        let z = (360.0 / span).log2() - 1.0;
        (z as f32).clamp(MIN_ZOOM, MAX_ZOOM)
    }

    fn sync_photo_to_selected(&mut self) {
        // Why the rule: a photo belongs where it was taken, so the track point with
        // the closest capture time wins; a photo without a capture time falls back
        // to the index distance, which is the only "near" left.
        //
        // Why the early returns: with no selected point there is no "here" to sync
        // to, and clearing the highlight then would erase the photo the person
        // chose in the rail every time they pressed 前の点 past the start.
        if self.photo_entries.is_empty() {
            self.selected_photo = None;
            return;
        }
        let Some(selected_idx) = self.selected else {
            return;
        };
        let selected_utc = self
            .track
            .points
            .get(selected_idx)
            .and_then(|point| point.utc);

        // An entry's `point_index` exists only for a positioned point (see
        // [`find_matching_point_index`]), so a skipped entry never becomes "the
        // highlighted photo" here either.
        let best = self
            .photo_entries
            .iter()
            .enumerate()
            .filter_map(|(photo_idx, entry)| {
                let pt_idx = entry.point_index?;
                let distance = match (selected_utc, entry.plan.capture_time) {
                    (Some(u1), Some(cap)) => u64::from(u1.abs_diff(cap.utc())),
                    _ => pt_idx.abs_diff(selected_idx) as u64,
                };
                Some((photo_idx, distance))
            })
            .min_by_key(|(_idx, dist)| *dist)
            .map(|(photo_idx, _dist)| photo_idx);

        if best.is_some() {
            self.selected_photo = best;
        }
    }

    /// Re-plan after a track edit, returning whether the entries still describe
    /// the track.
    ///
    /// The edit moved the points the old plan names, so a re-plan failure here
    /// cannot keep the entries: a row whose `point_index` points at a deleted
    /// record would write a coordinate the edit removed. The entries are dropped
    /// with the reason and the person is asked to load the photos again. An
    /// explicit control change can keep its older list instead — that plan still
    /// matched the track, and [`Self::replan_photos`] marks it stale so it is not
    /// written.
    fn relink_photos(&mut self) -> bool {
        if self.photo_entries.is_empty() {
            return true;
        }
        // Re-pick the highlighted photo against the edited track first: a
        // `byCamera` re-plan reads its Make/Model from that row, so it has to be
        // the row the new points point at.
        self.sync_photo_to_selected();
        let Err(message) = self.replan_photos() else {
            return true;
        };
        self.photo_entries.clear();
        self.selected_photo = None;
        self.photo_shift.selected.clear();
        self.photo_shift.pending = false;
        self.photo_shift.plan_stale = false;
        self.close_geotag_confirmation();
        self.photo_shift.status = Some((
            Status::Err,
            format!(
                "トラック編集で写真の対応を更新できませんでした: {message}。写真を読み込み直してください"
            ),
        ));
        false
    }

    /// `11/14` → count, in the order the device recorded them.
    fn days(&self) -> Vec<(String, usize)> {
        let mut days: Vec<(String, usize)> = Vec::new();
        for point in &self.track.points {
            let day = common::utc_date(point.utc);
            match days.last_mut() {
                Some((last, count)) if *last == day => *count += 1,
                _ => days.push((day, 1)),
            }
        }
        days
    }

    /// Row indices the day filter leaves visible.
    fn visible_indices(&self) -> Vec<usize> {
        self.track
            .points
            .iter()
            .enumerate()
            .filter(|(_index, point)| !self.point_hidden(point))
            .map(|(index, _point)| index)
            .collect()
    }

    /// Whether the day filter hides this point.
    ///
    /// The map asks this for every point three times a frame, so the empty set —
    /// the usual case — takes a fast path instead of formatting a date per point.
    fn point_hidden(&self, point: &TrackPoint) -> bool {
        !self.hidden_days.is_empty() && self.hidden_days.contains(&common::utc_date(point.utc))
    }

    fn total_distance_km(&self) -> f64 {
        let mut total = 0.0;
        let mut previous: Option<&TrackPoint> = None;
        for point in &self.track.points {
            if let Some(before) = previous
                && let Some(step) = common::distance_km(before, point)
            {
                total += step;
            }
            previous = Some(point);
        }
        total
    }

    fn total_distance_with_settings(&self, settings: &AppSettings) -> String {
        settings.units.format_distance_km(self.total_distance_km())
    }

    fn elapsed(&self) -> String {
        let span = self.track.time_span();
        common::duration(span.map(|(start, end)| end.saturating_sub(start)))
    }

    fn average_speed_with_settings(&self, settings: &AppSettings) -> String {
        let Some((start, end)) = self.track.time_span() else {
            return String::from("—");
        };
        let seconds = end.saturating_sub(start);
        if seconds == 0 {
            return String::from("—");
        }
        let avg_kmh = (self.total_distance_km() / (f64::from(seconds) / 3_600.0)) as f32;
        settings.units.format_speed(Some(avg_kmh))
    }

    /// The fastest record the log carries, when any record has a speed.
    fn max_speed_kmh(&self) -> Option<f32> {
        self.track
            .points
            .iter()
            .filter_map(|point| point.speed_kmh)
            .max_by(f32::total_cmp)
    }

    /// Lowest and highest altitude the log carries, when any record has one.
    fn altitude_range(&self) -> Option<(f32, f32)> {
        let (mut low, mut high): (Option<f32>, Option<f32>) = (None, None);
        for height in self.track.points.iter().filter_map(|point| point.height_m) {
            low = Some(low.map_or(height, |current| current.min(height)));
            high = Some(high.map_or(height, |current| current.max(height)));
        }
        low.zip(high)
    }

    /// Seconds spent moving: consecutive records with times, a short gap, and a
    /// speed above walking pace at either end.
    ///
    /// `m241_core` has no `TrackStats` yet (a separate change), so this is the
    /// existing data's honest answer: a gap longer than an hour is treated as a
    /// pause rather than as moving time, and a record without a speed cannot
    /// claim movement on its own.
    fn moving_seconds(&self) -> u32 {
        const MAX_GAP_SECONDS: u32 = 3_600;
        let mut total = 0u32;
        for pair in self.track.points.windows(2) {
            let (Some(before), Some(after)) = (pair.first(), pair.get(1)) else {
                continue;
            };
            let (Some(start), Some(end)) = (before.utc, after.utc) else {
                continue;
            };
            let gap = end.saturating_sub(start);
            let moving = before.speed_kmh.is_some_and(|speed| speed > 0.5)
                || after.speed_kmh.is_some_and(|speed| speed > 0.5);
            if moving && gap > 0 && gap <= MAX_GAP_SECONDS {
                total = total.saturating_add(gap);
            }
        }
        total
    }
}

/// The point a photo's position should be written from.
///
/// The contract `PhotoEntry::point_index` promises: the returned index is the
/// point **nearest in time** among those that have both a time and a position.
/// Positioned-only keeps the invariants the map and the writer rely on — every
/// returned index is drawable, and the confirmation list (which counts
/// `will_write()` plans) cannot contain a photo paired with an unplottable point.
/// `min_by_key` over the time difference is the nearest-neighbour rule of
/// `m241_core::log::geotag` itself; this function only re-finds the index that
/// answer came from. A track with no positioned point yields `None`.
fn find_matching_point_index(track: &Track, plan: &GeotagPlan) -> Option<usize> {
    let target_utc = plan.point?.utc?;
    track
        .points
        .iter()
        .enumerate()
        .filter(|(_, p)| p.has_position() && p.utc.is_some())
        .min_by_key(|(_, p)| p.utc.map_or(u32::MAX, |u| u.abs_diff(target_utc)))
        .map(|(idx, _)| idx)
}

/// Faint grid, so a track with a straight edge can be read as geography rather
/// than as a rectangle that got painted.
fn draw_grid(painter: &egui::Painter, rect: Rect) {
    let stroke = Stroke::new(metric::HAIRLINE, theme::LINE.gamma_multiply(0.6));
    for step in 1..4 {
        let fraction = step as f32 / 4.0;
        let x = rect.left() + rect.width() * fraction;
        let y = rect.top() + rect.height() * fraction;
        painter.vline(x, rect.y_range(), stroke);
        painter.hline(rect.x_range(), y, stroke);
    }
}

/// A plausible trip, for looking at the layout without a device.
///
/// The coordinates are the generic city-centre pair the repository uses as its
/// example (`AGENTS.md` §9): a real track is somebody's location history and does
/// not belong in a source file.
fn sample_track() -> Track {
    const START: u32 = 1_763_000_000;
    let mut points = Vec::new();
    for step in 0..240_u32 {
        let angle = f64::from(step) * 0.05;
        // A loop that returns near where it started, so the map shows an outline
        // rather than a straight line that any canvas would draw.
        let latitude = 35.681_236 + angle.sin() * 0.012;
        let longitude = 139.767_125 + angle.cos() * 0.015;
        points.push(TrackPoint {
            utc: Some(START + step * 30),
            latitude: Some(latitude),
            longitude: Some(longitude),
            height_m: Some(40.0 + f32::from(step as u16 % 20)),
            speed_kmh: Some(18.0 + (angle * 2.0).sin() as f32 * 9.0),
            rcr: Some(0),
        });
    }
    // One waypoint: the product's unit tests and the Log screen both need a
    // record that carries the button bit, or the "目印" line never renders.
    if let Some(point) = points.get_mut(60) {
        point.rcr = Some(0x0008);
    }
    Track {
        points,
        damaged_blocks: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> LogScreen {
        let mut screen = LogScreen::default();
        screen.set_track(sample_track(), ParseReport::default());
        screen
    }

    /// Flatten nested shape trees into leaf shapes.
    fn flatten<'a>(shapes: &'a [egui::Shape], out: &mut Vec<&'a egui::Shape>) {
        for shape in shapes {
            match shape {
                egui::Shape::Vec(children) => flatten(children, out),
                other => out.push(other),
            }
        }
    }

    /// The leaf shapes one full shell frame paints at `width`×`height`.
    ///
    /// Two passes so the immediate-mode layout settles; the second frame's
    /// shapes are the ones a person sees. `prepare` fills the screen state the
    /// test is about.
    fn shell_shapes(
        width: f32,
        height: f32,
        prepare: impl FnOnce(&mut crate::App),
    ) -> Vec<egui::Shape> {
        let context = egui::Context::default();
        crate::theme::install(&context);
        let mut app = crate::App::default();
        app.select(crate::screens::Screen::Log);
        prepare(&mut app);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(width, height),
            )),
            ..egui::RawInput::default()
        };
        let mut shapes = Vec::new();
        for _pass in 0..2 {
            let output = context.run_ui(input.clone(), |ui| app.draw(ui));
            shapes = output
                .shapes
                .iter()
                .map(|clipped| clipped.shape.clone())
                .collect();
            output.drop_without_applying_deltas();
        }
        shapes
    }

    #[test]
    fn a_point_row_draws_the_focus_ring() {
        // The design review's Must 1 covers the point list as well as the buttons:
        // a 10,000-row list whose focused row looks like every other row is not
        // keyboard-operable. The row paints the shared 2pt ink ring from its
        // `Response`; this focuses the real row and measures the frame.
        let screen = sample();
        let point = screen
            .track()
            .points
            .first()
            .copied()
            .expect("the sample track has points");
        let settings = AppSettings::default();
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(640.0, 400.0),
            )),
            ..egui::RawInput::default()
        };
        let mut id = None;
        let mut found = false;
        for pass in 0..3 {
            let output = ctx.run_ui(input.clone(), |ui| {
                let response = screen.point_row(ui, 0, &point, &settings);
                if pass == 0 {
                    id = Some(response.id);
                }
            });
            let shapes: Vec<egui::Shape> = output
                .shapes
                .iter()
                .map(|clipped| clipped.shape.clone())
                .collect();
            output.drop_without_applying_deltas();
            if pass > 0 {
                let mut flat = Vec::new();
                flatten(&shapes, &mut flat);
                found = flat.iter().any(|shape| {
                    matches!(
                        shape,
                        egui::Shape::Rect(rect)
                            if (rect.stroke.width - metric::FOCUS_RING).abs() < f32::EPSILON
                                && rect.stroke.color == theme::INK
                    )
                });
            }
            if pass == 0
                && let Some(id) = id
            {
                ctx.memory_mut(|memory| memory.request_focus(id));
            }
        }
        assert!(found, "the focused point row drew no focus ring");
    }

    #[test]
    fn a_new_screen_has_an_empty_log_and_a_gpx_default() {
        let screen = LogScreen::default();
        assert!(screen.track().is_empty());
        // GPX is the format every other tool reads; defaulting to it means the
        // first export works without a decision.
        assert_eq!(screen.format, ExportFormat::Gpx);
        assert!(screen.destination.ends_with(".gpx"));
    }

    #[test]
    fn a_track_is_projected_into_the_canvas() {
        let screen = sample();
        let rect = Rect::from_min_size(egui::pos2(0.0, 0.0), Vec2::new(400.0, 240.0));
        let plotted = screen.plotted_points(rect);
        assert_eq!(plotted.len(), screen.track().points.len());
        for (_index, pos) in &plotted {
            assert!(
                rect.contains(*pos),
                "a point was projected outside the canvas at {pos:?}"
            );
        }
    }

    #[test]
    fn north_is_at_the_top_of_the_canvas() {
        // A y that grows with latitude would draw the trip upside down, which is
        // the kind of mistake that reads as "the device is broken".
        let screen = sample();
        let rect = Rect::from_min_size(egui::pos2(0.0, 0.0), Vec2::new(400.0, 240.0));
        let northern = screen
            .track()
            .points
            .iter()
            .filter_map(|point| Some((point.latitude?, 0)))
            .enumerate()
            .max_by(|left, right| left.1.0.total_cmp(&right.1.0))
            .map(|(index, _)| index);
        let plotted = screen.plotted_points(rect);
        let Some(northern_index) = northern else {
            return;
        };
        let north_y = plotted
            .iter()
            .find(|(index, _)| *index == northern_index)
            .map(|(_, pos)| pos.y);
        let mean_y: f32 = plotted.iter().map(|(_, pos)| pos.y).sum::<f32>() / plotted.len() as f32;
        if let Some(north_y) = north_y {
            assert!(
                north_y < mean_y,
                "the northernmost point is below the mean; the projection is flipped"
            );
        }
    }

    #[test]
    fn an_empty_track_projects_to_nothing() {
        let screen = LogScreen::default();
        let rect = Rect::from_min_size(egui::pos2(0.0, 0.0), Vec2::new(100.0, 100.0));
        assert!(screen.plotted_points(rect).is_empty());
    }

    #[test]
    fn a_single_fix_does_not_divide_by_zero() {
        // One point means both spans are zero; a naive `max - min` divisor gives
        // NaN and the canvas paints nothing at all.
        let mut screen = LogScreen::default();
        screen.set_track(
            Track {
                points: vec![TrackPoint {
                    utc: Some(1_700_000_000),
                    latitude: Some(35.0),
                    longitude: Some(139.0),
                    ..TrackPoint::default()
                }],
                damaged_blocks: Vec::new(),
            },
            ParseReport::default(),
        );
        let rect = Rect::from_min_size(egui::pos2(0.0, 0.0), Vec2::new(100.0, 100.0));
        let plotted = screen.plotted_points(rect);
        assert_eq!(plotted.len(), 1);
        let pos = plotted.first().map(|(_, pos)| *pos);
        if let Some(pos) = pos {
            assert!(pos.x.is_finite() && pos.y.is_finite(), "got {pos:?}");
            assert!(rect.contains(pos), "the single point left the canvas");
        }
    }

    #[test]
    fn the_day_filter_removes_whole_days() {
        let mut screen = sample();
        let days = screen.days();
        assert_eq!(days.len(), 1, "the sample is one day: {days:?}");
        let Some((day, _count)) = days.first().cloned() else {
            return;
        };
        assert_eq!(screen.visible_indices().len(), screen.track().points.len());
        let _inserted = screen.hidden_days.insert(day);
        assert!(screen.visible_indices().is_empty());
    }

    #[test]
    fn the_days_are_counted_in_recorded_order() {
        let screen = sample();
        let days = screen.days();
        let total: usize = days.iter().map(|(_day, count)| count).sum();
        assert_eq!(total, screen.track().points.len());
    }

    #[test]
    fn a_sample_summary_states_the_distance_the_points_imply() {
        let screen = sample();
        let distance = screen.total_distance_with_settings(&AppSettings::default());
        assert!(
            distance.ends_with(" km"),
            "the figure is missing its unit: {distance}"
        );
        let value: f64 = distance.trim_end_matches(" km").parse().unwrap_or(0.0);
        assert!(value > 0.0, "a loop of 240 points measured 0 km");
    }

    struct ScratchDir(PathBuf);

    impl LogScreen {
        /// Hide the first day of the sample, the way the rail's checkbox does.
        fn hide_day_for_test(&mut self) {
            let Some((day, _count)) = self.days().first().cloned() else {
                return;
            };
            let _inserted = self.hidden_days.insert(day);
            self.close_geotag_confirmation();
        }
    }

    impl ScratchDir {
        fn new(label: &str) -> Self {
            use std::sync::atomic::{AtomicU32, Ordering};
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let serial = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "m241-gui-log-{}-{label}-{serial}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).expect("create scratch dir");
            Self(path)
        }
    }

    impl Drop for ScratchDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn synthetic_bin_dump(points: &[(u32, f32, f32, f32)]) -> Vec<u8> {
        use m241_core::log::block::{
            BLOCK_HEADER_SIZE, BLOCK_SIZE, HOLUX_EXTRA_BIT, POLICY_LOG_ENABLED, bit,
        };
        let mask = HOLUX_EXTRA_BIT
            | (1u32 << u32::from(bit::UTC))
            | (1u32 << u32::from(bit::LATITUDE))
            | (1u32 << u32::from(bit::LONGITUDE))
            | (1u32 << u32::from(bit::HEIGHT))
            | (1u32 << u32::from(bit::SPEED));
        let mut block = vec![0xFFu8; BLOCK_SIZE];
        let count = points.len() as u16;
        block[0..2].copy_from_slice(&count.to_le_bytes());
        block[2..6].copy_from_slice(&mask.to_le_bytes());
        block[6..8].copy_from_slice(&POLICY_LOG_ENABLED.to_le_bytes());

        let mut offset = BLOCK_HEADER_SIZE;
        for (utc, lat, lon, speed) in points {
            let mut rec = Vec::with_capacity(20);
            rec.extend_from_slice(&utc.to_le_bytes());
            rec.extend_from_slice(&lat.to_le_bytes());
            rec.extend_from_slice(&lon.to_le_bytes());
            // Height = 42.0m (little-endian f32 0x4228_0000 -> upper 3 bytes 0x00, 0x28, 0x42)
            rec.extend_from_slice(&[0x00, 0x28, 0x42]);
            rec.extend_from_slice(&speed.to_le_bytes());
            let checksum = rec.iter().fold(0u8, |acc, b| acc ^ b);
            rec.push(checksum);
            block[offset..offset + rec.len()].copy_from_slice(&rec);
            offset += rec.len();
        }
        block
    }

    fn minimal_jpeg_with_mtime(dir: &Path, name: &str, utc: u32) -> PathBuf {
        let mut bytes = vec![0xFF, 0xD8];
        // JFIF APP0
        bytes.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x10]);
        bytes.extend_from_slice(b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0");
        // EOI
        bytes.extend_from_slice(&[0xFF, 0xD9]);
        write_jpeg_with_mtime(dir, name, &bytes, utc)
    }

    /// A JPEG whose Exif IFD0 names `make` / `model`, written for the camera
    /// scope tests.
    ///
    /// The core's fixture builders are `#[cfg(test)]` inside `m241-core`, so a
    /// GUI test cannot reuse them; this is the smallest block `CameraModel`
    /// reads (big-endian TIFF with two ASCII IFD0 entries). The names are longer
    /// than four bytes on purpose, so both values live in the value area instead
    /// of the inline field.
    fn jpeg_with_camera(dir: &Path, name: &str, make: &str, model: &str, utc: u32) -> PathBuf {
        let mut tiff = Vec::new();
        tiff.extend_from_slice(b"MM");
        tiff.extend_from_slice(&42_u16.to_be_bytes());
        tiff.extend_from_slice(&8_u32.to_be_bytes());
        let make_end = 8 + 2 + 12 * 2 + 4; // IFD0 count, two entries, next-IFD link
        let model_end = make_end + make.len() + 1;
        tiff.extend_from_slice(&2_u16.to_be_bytes());
        push_ascii_entry(&mut tiff, 0x010F, make, make_end);
        push_ascii_entry(&mut tiff, 0x0110, model, model_end);
        tiff.extend_from_slice(&0_u32.to_be_bytes());
        tiff.extend_from_slice(make.as_bytes());
        tiff.push(0);
        tiff.extend_from_slice(model.as_bytes());
        tiff.push(0);

        let mut payload = b"Exif\0\0".to_vec();
        payload.extend_from_slice(&tiff);
        let mut bytes = vec![0xFF, 0xD8];
        push_jpeg_segment(&mut bytes, 0xE0, b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0");
        push_jpeg_segment(&mut bytes, 0xE1, &payload);
        bytes.extend_from_slice(&[0xFF, 0xD9]);
        write_jpeg_with_mtime(dir, name, &bytes, utc)
    }

    fn push_ascii_entry(out: &mut Vec<u8>, tag: u16, text: &str, value_offset: usize) {
        out.extend_from_slice(&tag.to_be_bytes());
        out.extend_from_slice(&2_u16.to_be_bytes());
        out.extend_from_slice(
            &u32::try_from(text.len() + 1)
                .expect("a test name fits")
                .to_be_bytes(),
        );
        out.extend_from_slice(&u32::try_from(value_offset).expect("fits").to_be_bytes());
    }

    fn push_jpeg_segment(out: &mut Vec<u8>, marker: u8, payload: &[u8]) {
        out.extend_from_slice(&[0xFF, marker]);
        let length = u16::try_from(payload.len() + 2).expect("a test segment fits");
        out.extend_from_slice(&length.to_be_bytes());
        out.extend_from_slice(payload);
    }

    fn write_jpeg_with_mtime(dir: &Path, name: &str, bytes: &[u8], utc: u32) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, bytes).expect("write jpeg");
        let file = std::fs::File::options()
            .write(true)
            .open(&path)
            .expect("open jpeg for mtime");
        let times = std::fs::FileTimes::new()
            .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(u64::from(utc)));
        file.set_times(times).expect("set jpeg mtime");
        path
    }

    /// Texts with the drawer open on `tab`, where the panels live.
    fn screen_texts_on(screen: &mut LogScreen, tab: LogTab) -> Vec<String> {
        test_support::screen_texts_on(screen, tab)
    }

    /// The same, with positions.
    fn screen_text_runs_on(screen: &mut LogScreen, tab: LogTab) -> Vec<(String, egui::Pos2)> {
        test_support::screen_text_runs_on(screen, tab)
    }

    #[test]
    fn playback_controls_step_toggle_stop_at_end_and_enforce_follow_zoom() {
        let mut screen = sample();
        assert_eq!(screen.selected_index(), Some(0));
        assert!(!screen.is_playing());
        assert!(!screen.is_following());

        assert!(screen.step_next());
        assert_eq!(screen.selected_index(), Some(1));
        assert!(screen.step_prev());
        assert_eq!(screen.selected_index(), Some(0));
        assert!(!screen.step_prev());
        assert_eq!(screen.selected_index(), Some(0));

        // Follow mode must enforce zoom >= 12 even if previously zoomed out.
        screen.set_zoom(6.0);
        assert!((screen.map_zoom() - 6.0).abs() < f32::EPSILON);
        screen.set_follow(true);
        assert!(screen.is_following());
        assert!(screen.map_zoom() >= FOLLOW_MIN_ZOOM);

        // Trying to zoom below 12 while following clamps at 12.
        screen.set_zoom(4.0);
        assert!(screen.map_zoom() >= FOLLOW_MIN_ZOOM);

        // fit_all disables follow mode.
        screen.fit_all();
        assert!(!screen.is_following());

        // Advancing to the end stops playback cleanly.
        let last = screen.track().points.len() - 1;
        screen.select_point(Some(last));
        screen.toggle_play();
        assert!(screen.is_playing());
        // toggle_play from the end restarts from 0.
        assert_eq!(screen.selected_index(), Some(0));

        screen.select_point(Some(last));
        assert!(!screen.step_next());
        assert!(!screen.is_playing());
    }

    #[test]
    fn loading_bin_dump_and_exporting_gpx_csv_kml_refuses_silent_overwrite() {
        let dir = ScratchDir::new("bin-export");
        let bin_path = dir.0.join("trip.bin");
        let dump = synthetic_bin_dump(&[
            (1_763_000_000, 35.681_236, 139.767_12, 12.0),
            (1_763_000_030, 35.682_236, 139.768_12, 18.0),
            (1_763_000_060, 35.683_236, 139.769_12, 24.0),
        ]);
        std::fs::write(&bin_path, dump).expect("write bin");

        let mut screen = LogScreen::default();
        screen.show_sample();
        assert!(screen.sample);

        screen.load_bin_file(&bin_path).expect("load synthetic bin");
        assert!(!screen.sample, "loading real file exits sample mode");
        assert_eq!(screen.track().points.len(), 3);
        assert!(screen.destination.ends_with("trip.gpx"));

        // Export GPX, KML, CSV.
        for (format, needle) in [
            (ExportFormat::Gpx, "<gpx"),
            (ExportFormat::Kml, "<kml"),
            (ExportFormat::Csv, "INDEX,TRACK NUMBER,UTC DATE"),
        ] {
            screen.set_export_format(format);
            let out_path = PathBuf::from(&screen.destination);
            screen.export().expect("export new file");
            let contents = std::fs::read_to_string(&out_path).expect("read exported");
            assert!(contents.contains(needle), "missing {needle} in {format:?}");
        }

        // Refusing silent overwrite on existing GPX file.
        screen.set_export_format(ExportFormat::Gpx);
        let gpx_path = PathBuf::from(&screen.destination);
        std::fs::write(&gpx_path, "SENTINEL").expect("write sentinel");
        let err = screen.export().expect_err("must refuse overwrite");
        assert!(err.contains("上書きしません"), "unexpected error: {err}");
        assert_eq!(
            std::fs::read_to_string(&gpx_path).expect("read sentinel"),
            "SENTINEL"
        );
    }

    /// A scratch directory of two photos, whose bytes say nothing about a location.
    ///
    /// `a.jpg` is captured at the sample track's first point and `b.jpg` 900 s
    /// later, so both match a positioned point within `geotag`'s five-minute limit.
    fn photo_pair(dir: &ScratchDir) -> (PathBuf, PathBuf) {
        let photo_a = minimal_jpeg_with_mtime(&dir.0, "a.jpg", 1_763_000_000);
        let photo_b = minimal_jpeg_with_mtime(&dir.0, "b.jpg", 1_763_000_900);
        (photo_a, photo_b)
    }

    #[test]
    fn loading_photos_plans_read_only_and_syncs_selection_both_ways() {
        let dir = ScratchDir::new("photos");
        let (photo_a, photo_b) = photo_pair(&dir);
        let orig_a = std::fs::read(&photo_a).expect("orig a");
        let orig_b = std::fs::read(&photo_b).expect("orig b");

        let mut screen = sample();
        // sample track starts at 1_763_000_000 with 30s steps -> index 0 and index 30.
        let matched = screen
            .load_photos_from_path(&dir.0)
            .expect("plan photos in dir");
        assert_eq!(matched, 2);
        assert_eq!(screen.photo_entries().len(), 2);

        // Planning must be strictly read-only.
        assert_eq!(std::fs::read(&photo_a).expect("after plan a"), orig_a);
        assert_eq!(std::fs::read(&photo_b).expect("after plan b"), orig_b);

        // Selecting photo B jumps selected track point to index 30.
        screen.select_photo(1);
        assert_eq!(screen.selected_photo(), Some(1));
        assert_eq!(screen.selected_index(), Some(30));

        // Stepping track back to index 0 highlights photo A.
        screen.select_point(Some(0));
        assert_eq!(screen.selected_photo(), Some(0));
    }

    #[test]
    fn asking_to_geotag_changes_no_bytes_until_the_write_is_confirmed() {
        // The write cannot be undone from this screen, so the two halves are
        // asserted separately: opening the prompt (「写真に位置」) must leave every
        // byte alone, and only answering it with 「書き込む」 may change a file.
        // Opening the prompt is the only way to obtain the token `apply_geotags`
        // needs, so there is no third path to check.
        let dir = ScratchDir::new("geotag-gate");
        let (photo_a, photo_b) = photo_pair(&dir);
        let orig_a = std::fs::read(&photo_a).expect("orig a");
        let orig_b = std::fs::read(&photo_b).expect("orig b");

        let mut screen = sample();
        assert_eq!(screen.load_photos_from_path(&dir.0).expect("plan"), 2);
        screen.open_geotag_confirmation();
        assert!(screen.geotag_confirmation_open());
        assert_eq!(
            std::fs::read(&photo_a).expect("after opening a"),
            orig_a,
            "opening the confirmation wrote to a.jpg"
        );
        assert_eq!(
            std::fs::read(&photo_b).expect("after opening b"),
            orig_b,
            "opening the confirmation wrote to b.jpg"
        );

        // The prompt names exactly the files the token will write.
        let mut targets: Vec<PathBuf> = screen
            .geotag_targets()
            .into_iter()
            .map(Path::to_path_buf)
            .collect();
        targets.sort();
        assert_eq!(targets, vec![photo_a.clone(), photo_b.clone()]);

        let confirmation = screen
            .confirm_geotag_write()
            .expect("an open prompt yields the token");
        let written = screen
            .apply_geotags(confirmation)
            .expect("confirmed write succeeds");
        assert_eq!(written, 2);
        assert!(
            !screen.geotag_confirmation_open(),
            "the prompt stayed open after the write"
        );
        for photo in [&photo_a, &photo_b] {
            assert_ne!(
                std::fs::read(photo).expect("after write"),
                Vec::<u8>::new(),
                "a written photo became empty"
            );
        }
        assert_ne!(
            std::fs::read(&photo_a).expect("after write a"),
            orig_a,
            "a.jpg has no GPS block after a confirmed write"
        );
        assert_ne!(
            std::fs::read(&photo_b).expect("after write b"),
            orig_b,
            "b.jpg has no GPS block after a confirmed write"
        );
    }

    #[test]
    fn confirming_needs_an_open_prompt() {
        let dir = ScratchDir::new("geotag-no-prompt");
        let (photo_a, _photo_b) = photo_pair(&dir);
        let orig = std::fs::read(&photo_a).expect("orig");
        let mut screen = sample();
        assert_eq!(screen.load_photos_from_path(&dir.0).expect("plan"), 2);
        assert!(!screen.geotag_confirmation_open());
        assert!(
            screen.confirm_geotag_write().is_none(),
            "a write token was produced with no prompt on screen"
        );
        assert_eq!(std::fs::read(&photo_a).expect("after"), orig);
    }

    #[test]
    fn a_change_that_moves_what_would_be_written_closes_the_prompt() {
        // The prompt names a set of files. Selecting another point, selecting
        // another photo, loading another folder, applying a day filter or
        // replacing the track all change the answer, so each must close it rather
        // than leave "書き込む" for a set nobody reviewed.
        let dir = ScratchDir::new("geotag-invalidate");
        let (_photo_a, _photo_b) = photo_pair(&dir);
        let mut screen = sample();
        assert_eq!(screen.load_photos_from_path(&dir.0).expect("plan"), 2);

        type Action = fn(&mut LogScreen, &Path);
        let actions: [(&str, Action); 5] = [
            ("select another point", |screen, _dir| {
                screen.select_point(Some(3));
            }),
            ("select another photo", |screen, _dir| {
                screen.select_photo(1);
            }),
            ("reload the folder", |screen, dir| {
                let _ = screen.load_photos_from_path(dir);
            }),
            ("hide a day", |screen, _dir| {
                screen.hide_day_for_test();
            }),
            ("replace the track", |screen, _dir| {
                screen.set_track(sample_track(), ParseReport::default());
            }),
        ];
        for (label, action) in actions {
            screen.open_geotag_confirmation();
            assert!(
                screen.geotag_confirmation_open(),
                "{label}: prompt did not open"
            );
            action(&mut screen, &dir.0);
            assert!(
                !screen.geotag_confirmation_open(),
                "{label} left the geotag prompt open"
            );
        }
    }

    #[test]
    fn a_photo_at_a_point_without_a_position_is_not_a_geotag_target() {
        // `find_matching_point_index` only returns positioned points: a photo whose
        // nearest record has a time but no fix cannot be drawn, so it must not be
        // one of the files the confirmation lists.
        let dir = ScratchDir::new("photos-unpositioned");
        let _jpeg = minimal_jpeg_with_mtime(&dir.0, "locked.jpg", 1_763_000_000);
        let mut track = sample_track();
        for point in &mut track.points {
            point.latitude = None;
            point.longitude = None;
        }
        let mut screen = LogScreen::default();
        screen.set_track(track, ParseReport::default());
        assert_eq!(
            screen.load_photos_from_path(&dir.0).expect("plan"),
            0,
            "a track with no fix must match no photo"
        );
        assert!(screen.geotag_targets().is_empty());
        for entry in screen.photo_entries() {
            assert_eq!(entry.point_index, None);
        }
        screen.open_geotag_confirmation();
        assert!(
            !screen.geotag_confirmation_open(),
            "a prompt opened with nothing to write"
        );
    }

    #[test]
    fn a_photo_at_a_half_positioned_point_is_not_a_geotag_target() {
        // The intermediate state the invariant exists for: a record with a
        // latitude but no longitude (or the other way round) is not a position.
        // `has_position()` is `latitude.is_some() && longitude.is_some()`, and
        // matching on one of the two would pair a photo with a point the map
        // cannot draw and a frame cannot write. Both halves are exercised,
        // because a mutation that drops only one conjunct is the likely one.
        for missing_longitude in [false, true] {
            let dir = ScratchDir::new("photos-half-position");
            let _jpeg = minimal_jpeg_with_mtime(&dir.0, "half.jpg", 1_763_000_000);
            let mut track = sample_track();
            // Every record loses one coordinate; the half that remains still
            // looks like a fix to a single-field check.
            for point in &mut track.points {
                if missing_longitude {
                    point.longitude = None;
                } else {
                    point.latitude = None;
                }
            }
            let mut screen = LogScreen::default();
            screen.set_track(track, ParseReport::default());
            assert_eq!(
                screen.load_photos_from_path(&dir.0).expect("plan"),
                0,
                "missing longitude = {missing_longitude}: a photo matched a point \
                 that has only one coordinate"
            );
            assert!(screen.geotag_targets().is_empty());
            for entry in screen.photo_entries() {
                assert_eq!(
                    entry.point_index, None,
                    "missing longitude = {missing_longitude}: the photo was paired \
                     with an unplottable point"
                );
            }
        }
    }

    #[test]
    fn the_matched_index_is_always_a_point_with_both_coordinates() {
        // `find_matching_point_index` decides which index the map draws a photo
        // at, so it has to apply the same rule `geotag` applies: a record with one
        // coordinate is not a position. The photo-level tests cannot see this —
        // `geotag::plan` already refuses a half-positioned record, so the photo
        // is skipped before the index is asked for; the plan is therefore built
        // here, from point 0, and the track is damaged after it exists the way a
        // truncated record would be.
        let mut track = sample_track();
        let Some(point) = track.points.first().copied() else {
            panic!("the sample has points");
        };
        let plan = GeotagPlan {
            photo: PathBuf::from("a.jpg"),
            capture_time: None,
            point: Some(point),
            gap_seconds: Some(0),
            skipped: None,
            keep_backup: false,
            group_id: None,
        };
        assert_eq!(
            find_matching_point_index(&track, &plan),
            Some(0),
            "the nearest positioned record is point 0 to begin with"
        );
        if let Some(point) = track.points.first_mut() {
            point.longitude = None;
        }
        assert_eq!(
            find_matching_point_index(&track, &plan),
            Some(1),
            "a half-positioned record was returned as a photo's point: the map \
             cannot draw it and a frame cannot write it"
        );
        // The other half of the same rule, for a mutation that keeps only one.
        if let Some(point) = track.points.first_mut() {
            point.latitude = None;
            point.longitude = Some(139.767_125);
        }
        assert_eq!(
            find_matching_point_index(&track, &plan),
            Some(1),
            "a record with only a longitude was returned as a photo's point"
        );
    }

    #[test]
    fn a_new_screen_writes_nothing_without_a_confirmation() {
        // The token is the only way into `apply_geotags`; a screen that never
        // opened the prompt has none, which is the state a second call site must
        // reproduce through the UI rather than by calling the writer.
        let screen = sample();
        assert_eq!(screen.matched_photo_count(), 0);
        assert!(screen.geotag_targets().is_empty());
        assert!(!screen.geotag_confirmation_open());
    }

    #[test]
    fn playback_only_steps_once_the_interval_has_passed() {
        let mut screen = sample();
        // The clock starts where playback was pressed, so the arithmetic below is
        // about the interval rather than about how long the test took to run.
        let start = Instant::now();
        screen.toggle_play();
        screen.last_step_at = Some(start);
        assert_eq!(screen.selected_index(), Some(0));
        let interval = PLAY_STEP;

        // Half the interval is not due: the marker must not move.
        assert!(screen.advance_playback(start + interval / 2, interval));
        assert_eq!(screen.selected_index(), Some(0), "the marker moved early");

        // At the interval it steps exactly once, and the clock restarts.
        assert!(screen.advance_playback(start + interval, interval));
        assert_eq!(screen.selected_index(), Some(1));
        assert!(screen.advance_playback(start + interval + interval / 2, interval));
        assert_eq!(screen.selected_index(), Some(1), "a step repeated early");
    }

    #[test]
    fn playback_stops_at_the_last_point_and_does_not_run_on() {
        let mut screen = sample();
        let last = screen.track().points.len() - 1;
        screen.select_point(Some(0));
        screen.toggle_play();
        assert!(screen.is_playing());
        // Stand at the second-to-last point with the step due: the next tick is the
        // one that reaches the end.
        screen.select_point(Some(last - 1));
        let start = Instant::now();
        screen.last_step_at = Some(start);
        let interval = PLAY_STEP;

        // The step that lands on the last point is still a step; playback only
        // stops when the next tick finds nothing to advance to.
        assert!(screen.advance_playback(start + interval, interval));
        assert_eq!(
            screen.selected_index(),
            Some(last),
            "playback stopped short of the end"
        );
        assert!(
            screen.is_playing(),
            "playback stopped one tick early, before showing the last point"
        );
        assert!(
            !screen.advance_playback(start + interval * 2, interval),
            "playback is still running at the last point; it would repaint forever"
        );
        assert!(!screen.is_playing());
        // A further tick changes nothing and asks for no repaint.
        assert!(!screen.advance_playback(start + interval * 3, interval));
        assert_eq!(screen.selected_index(), Some(last));
    }

    #[test]
    fn a_finished_playback_asks_for_no_further_frame() {
        // M3 from the Stage 3 review, whose symptom was "the repaint never
        // stops": `advance_playback`'s return value is the contract (`true` means
        // "still running, ask for another frame"), so an idle screen must answer
        // `false` on every tick once the last point is reached — not just stop
        // the marker. (Deleting the `self.playing = false` at the end changes no
        // state, because `step_next` already clears it; this pins the answer that
        // actually decides whether egui is asked to draw again.)
        let mut screen = sample();
        let last = screen.track().points.len() - 1;
        screen.toggle_play();
        assert!(screen.is_playing());
        screen.select_point(Some(last - 1));
        let start = Instant::now();
        screen.last_step_at = Some(start);
        let interval = PLAY_STEP;

        assert!(screen.advance_playback(start + interval, interval));
        assert_eq!(screen.selected_index(), Some(last));
        // The next due tick is the one where "nothing to advance" happens.
        assert!(!screen.advance_playback(start + interval * 2, interval));
        // …and it must stay false: a run that kept the flag would answer `true`
        // here, which is the bug.
        for tick in 3..6 {
            assert!(
                !screen.advance_playback(start + interval * tick, interval),
                "tick {tick} after the end still asked for a repaint"
            );
        }
        assert!(!screen.is_playing());
    }

    #[test]
    fn a_finished_playback_asks_egui_for_no_further_frame() {
        // The same rule through the draw pass rather than through the return
        // value: egui's observer sees every `request_repaint_after` the context
        // makes, including the one `advance_playback_if_due` asks for while
        // playback runs. Only the two playback intervals are counted, so egui's
        // own scheduling does not answer the question.
        use std::sync::{Arc, Mutex};

        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut screen = sample();
        let last = screen.track().points.len() - 1;
        screen.select_point(Some(0));
        let frames = Arc::new(Mutex::new(Vec::<Duration>::new()));
        let seen = Arc::clone(&frames);
        ctx.set_repaint_observer(move |info| {
            // Playback is the only thing in this crate that asks for a delayed
            // frame, and its two intervals are the two constants below. egui's
            // own requests are for 0 ms (a pass) or its tooltip delays, so a
            // count restricted to these two durations is a count of ours.
            let ours = info.delay == PLAY_STEP || info.delay == PLAY_STEP_REDUCED;
            if ours {
                seen.lock().expect("repaint log").push(info.delay);
            }
        });
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(1280.0, 2400.0),
            )),
            ..egui::RawInput::default()
        };
        let draw = |screen: &mut LogScreen| {
            let output = ctx.run_ui(input.clone(), |ui| screen.ui(ui));
            output.drop_without_applying_deltas();
        };

        // Idle: no playback, no request.
        draw(&mut screen);
        assert!(
            frames.lock().expect("repaint log").is_empty(),
            "an idle screen asked for a playback frame"
        );

        // Playing somewhere in the middle: a frame is asked for.
        screen.select_point(Some(last - 1));
        screen.toggle_play();
        draw(&mut screen);
        assert!(
            !frames.lock().expect("repaint log").is_empty(),
            "a running playback did not ask for its next frame"
        );

        // Park it on the last point with nothing to advance. The interval decides
        // when the next tick is due, so wait for it rather than assuming a fast
        // frame: from the first due frame on, playback must be over and no
        // further frame may be asked for.
        screen.select_point(Some(last));
        std::thread::sleep(PLAY_STEP_REDUCED + Duration::from_millis(50));
        frames.lock().expect("repaint log").clear();
        let mut finished_at: Option<usize> = None;
        for frame in 0..6 {
            draw(&mut screen);
            let asked = frames.lock().expect("repaint log").len();
            if !screen.is_playing() {
                if finished_at.is_none() {
                    finished_at = Some(frame);
                }
                assert!(
                    asked == 0,
                    "frame {frame} drew a finished playback and still asked egui to \
                     repaint ({asked} requests); it would repaint forever"
                );
            }
        }
        assert!(
            finished_at.is_some(),
            "playback never stopped after reaching the last point"
        );
        assert!(!screen.is_playing());
    }

    #[test]
    fn reduced_motion_slows_playback_to_the_longer_interval() {
        // The setting's doc promises playback steps at half the rate; this pins
        // the two intervals together so neither drifts alone.
        assert!(
            PLAY_STEP_REDUCED >= PLAY_STEP * 2,
            "reduced motion must at least halve the playback rate"
        );
        let mut screen = sample();
        let start = Instant::now();
        screen.toggle_play();
        screen.last_step_at = Some(start);
        // Due for the normal rate, not for the reduced one.
        assert!(screen.advance_playback(start + PLAY_STEP, PLAY_STEP_REDUCED));
        assert_eq!(
            screen.selected_index(),
            Some(0),
            "reduced motion stepped at the normal rate"
        );
        assert!(screen.advance_playback(start + PLAY_STEP_REDUCED, PLAY_STEP_REDUCED));
        assert_eq!(screen.selected_index(), Some(1));
    }

    #[test]
    fn following_picks_the_first_point_that_can_be_plotted() {
        // With nothing selected the projection centres on the track's bounding-box
        // centre, so follow mode would be "following" a fixed middle until playback
        // started. Turning it on must choose a point, and the first record here has
        // no fix, so it must not be that one.
        let mut track = sample_track();
        if let Some(head) = track.points.first_mut() {
            head.latitude = None;
            head.longitude = None;
        }
        let mut screen = LogScreen::default();
        screen.set_track(track, ParseReport::default());
        assert_eq!(screen.selected_index(), Some(0));

        screen.fit_all();
        assert_eq!(
            screen.selected_index(),
            Some(0),
            "fit_all itself must not change the selection"
        );
        screen.set_follow(true);
        assert_eq!(
            screen.selected_index(),
            Some(1),
            "follow mode did not choose the first plottable point"
        );
    }

    #[test]
    fn following_with_a_plotted_selection_keeps_it() {
        // The point a person was reading must not be replaced just because follow
        // mode was switched on.
        let mut screen = sample();
        screen.select_point(Some(7));
        screen.set_follow(true);
        assert_eq!(screen.selected_index(), Some(7));
    }

    #[test]
    fn following_centres_the_map_on_the_selected_point() {
        // The projection puts the centre coordinate at the canvas centre when
        // following, so the selected point's pixel *is* the centre. `fit_all`
        // then `set_follow(true)` is the order a person makes with the two
        // buttons, and it used to leave the map centred on the bounding box.
        let mut screen = sample();
        let rect = Rect::from_min_size(egui::pos2(0.0, 0.0), Vec2::new(400.0, 240.0));
        screen.fit_all();
        screen.set_follow(true);
        let middle = screen.selected_index().expect("follow selected a point");
        screen.select_point(Some(middle));
        let plotted = screen.plotted_points(rect);
        let centre = plotted
            .iter()
            .find(|(index, _pos)| *index == middle)
            .map(|(_index, pos)| *pos);
        let Some(centre) = centre else {
            panic!("the selected point was not plotted");
        };
        assert!(
            (centre - rect.center()).length() < 0.5,
            "the selected point is at {centre:?}, not at the canvas centre {:?}",
            rect.center()
        );

        // Stepping follows: the next point becomes the new centre.
        assert!(screen.step_next());
        let next = screen.selected_index().expect("stepped");
        let plotted = screen.plotted_points(rect);
        let centre = plotted
            .iter()
            .find(|(index, _pos)| *index == next)
            .map(|(_index, pos)| *pos);
        let Some(centre) = centre else {
            panic!("the stepped point was not plotted");
        };
        assert!(
            (centre - rect.center()).length() < 0.5,
            "after a step the map is centred on {centre:?}, not on the new point"
        );
    }

    #[test]
    fn a_waypoint_is_drawn_as_a_ring() {
        // The POI bit (`rcr == 0x0008`) is what the device's own button sets, and
        // `is_waypoint` is the predicate `map_panel` asks before it draws the ring.
        // The sample carries one, so the rule is exercised on real record data
        // rather than on a hand-made point.
        let screen = sample();
        let waypoints: Vec<usize> = screen
            .track()
            .points
            .iter()
            .enumerate()
            .filter(|(_index, point)| point.is_waypoint())
            .map(|(index, _point)| index)
            .collect();
        assert_eq!(waypoints, vec![60], "the sample must carry exactly one POI");
        let point = screen.track().points.get(60).copied().expect("waypoint");
        assert_eq!(point.rcr, Some(0x0008));
        // The ring is drawn at twice an ordinary point's radius, which is what
        // makes it read as a place rather than as another sample.
        assert!((metric::POINT_DOT * 2.0 - 6.0).abs() < f32::EPSILON);
    }

    #[test]
    fn the_track_is_five_points_of_data_over_a_seven_point_ground_casing() {
        // 07-eztour-spec §5's 「幅 5」 is read from the shapes that were painted,
        // not from the constants that produced them: the previous version
        // compared `line_stroke().color` with `theme::DATA`, which is the same
        // expression twice, and it would have kept passing if the canvas stopped
        // drawing the line altogether. The casing and full-opacity rule come from
        // the light OSM surface (design review 2026-11-23, M1).
        let shapes = shell_shapes(1280.0, 800.0, |app| app.log_mut().show_sample());
        let mut flat = Vec::new();
        flatten(&shapes, &mut flat);

        let tracks: Vec<&egui::epaint::PathShape> = flat
            .iter()
            .filter_map(|shape| match shape {
                egui::Shape::Path(path) if path.points.len() > 100 => Some(path),
                _ => None,
            })
            .collect();
        let casing = tracks
            .iter()
            .find(|path| (path.stroke.width - 7.0).abs() < f32::EPSILON)
            .expect("the track casing did not render");
        assert_eq!(
            casing.stroke.color,
            egui::epaint::ColorMode::Solid(theme::WINDOW),
            "the casing under the track is not the window colour"
        );
        let line = tracks
            .iter()
            .find(|path| (path.stroke.width - 5.0).abs() < f32::EPSILON)
            .expect("the track polyline did not render");
        assert_eq!(
            line.stroke.color,
            egui::epaint::ColorMode::Solid(theme::DATA),
            "the track is not the data colour at full opacity"
        );
        assert!(
            casing.stroke.width > line.stroke.width,
            "the casing does not show past the polyline"
        );
    }

    #[test]
    fn the_record_fill_cycles_through_the_ten_colours() {
        // The render test can only see that at least three hues reached the
        // canvas: a modulo that stopped cycling, or a palette that lost a
        // colour, would still leave three. `TRACK_PALETTE`'s own doc says the
        // sequence is what the records carry, so the cycle is pinned directly.
        assert_eq!(
            theme::TRACK_PALETTE.len(),
            10,
            "the cycle the reference spec names has ten colours"
        );
        assert_eq!(
            metric::track::record_fill(10),
            metric::track::record_fill(0),
            "the eleventh record does not return to the first colour"
        );
        assert_eq!(
            metric::track::record_fill(0),
            theme::DATA.gamma_multiply(metric::track::LINE_OPACITY),
            "the first colour is not the product's data colour at the record opacity"
        );
        for index in 1..theme::TRACK_PALETTE.len() {
            assert_ne!(
                metric::track::record_fill(index),
                metric::track::record_fill(index - 1),
                "records {index} and {} share a hue, so the sequence is invisible there",
                index - 1,
            );
        }
    }

    #[test]
    fn summary_formats_imperial_units_when_selected() {
        let screen = sample();
        let metric_settings = AppSettings::default();
        let imperial_settings = AppSettings {
            units: crate::screens::settings::UnitSystem::Imperial,
            ..AppSettings::default()
        };
        assert!(
            screen
                .total_distance_with_settings(&metric_settings)
                .ends_with(" km")
        );
        assert!(
            screen
                .total_distance_with_settings(&imperial_settings)
                .ends_with(" mi")
        );
        assert!(
            screen
                .average_speed_with_settings(&imperial_settings)
                .ends_with(" mph")
        );
    }

    #[test]
    fn the_kmz_writer_replaces_the_unsupported_label_and_retargets_the_extension() {
        // DESIGN.md §5 draws a KMZ chip beside GPX/KML/CSV. It is the archive
        // writer rather than an `ExportFormat`, so selecting it must also move
        // the destination extension; leaving `.gpx` would write a ZIP named as a
        // GPX file.
        let mut screen = sample();
        assert!(!screen.kmz);
        assert!(screen.destination.ends_with(".gpx"));
        screen.set_kmz_export();
        assert!(screen.kmz);
        assert!(screen.destination.ends_with(".kmz"));
        screen.set_export_format(ExportFormat::Csv);
        assert!(!screen.kmz);
        assert!(screen.destination.ends_with(".csv"));
    }

    #[test]
    fn the_kmz_long_edge_is_optional_but_zero_is_refused() {
        assert_eq!(
            super::kmz_export::parse_max_edge("").expect("blank means no limit"),
            None
        );
        assert_eq!(
            super::kmz_export::parse_max_edge("1600").expect("a pixel count"),
            Some(1600)
        );
        let error = super::kmz_export::parse_max_edge("0").expect_err("zero is refused");
        assert!(error.contains("1 以上"), "got {error}");
        assert!(super::kmz_export::parse_max_edge("wide").is_err());
    }

    #[test]
    fn kmz_export_bundles_the_track_and_the_photos_inside_the_gap() {
        let dir = ScratchDir::new("kmz-export");
        let (_photo_a, _photo_b) = photo_pair(&dir);
        let mut screen = sample();
        screen.load_photos_from_path(&dir.0).expect("load photos");
        screen.set_kmz_export();
        let destination = dir.0.join("track.kmz");
        screen.set_destination(destination.display().to_string());
        screen.export().expect("write the archive");
        let archive = std::fs::read(&destination).expect("read the archive");
        assert!(
            archive.starts_with(b"PK\x03\x04"),
            "the archive has no ZIP local-file header"
        );
        let (status, message) = screen.outcome().expect("an outcome was recorded");
        assert_eq!(*status, Status::Ok);
        assert!(message.contains("2 枚"), "got {message}");
    }

    #[test]
    fn a_refused_kmz_export_names_the_input_beside_the_action() {
        // DESIGN.md §6: the refusal says what the next step is, where the control
        // is. Both messages are stored on the screen's outcome, which the export
        // bar prints under the button.
        let dir = ScratchDir::new("kmz-errors");
        let mut screen = sample();
        screen.set_kmz_export();
        screen.set_destination(dir.0.join("out.kmz").display().to_string());
        screen.photo_path.clear();
        let error = screen.export().expect_err("no photo folder was given");
        assert!(error.contains("写真フォルダ"), "got {error}");
        assert!(matches!(screen.outcome(), Some((Status::Err, _))));

        screen.set_kmz_max_edge("0");
        screen.photo_path = dir.0.display().to_string();
        let error = screen.export().expect_err("a zero long edge is refused");
        assert!(error.contains("1 以上"), "got {error}");
        assert!(
            !dir.0.join("out.kmz").exists(),
            "a refused export wrote a file"
        );
    }

    #[test]
    fn a_loaded_folder_plans_every_photo_with_the_armed_shift() {
        let dir = ScratchDir::new("shift-load");
        let (_photo_a, _photo_b) = photo_pair(&dir);
        let mut screen = sample();
        screen.photo_shift.kind = ScopeKind::All;
        screen.photo_shift.shift.seconds = 900;
        screen
            .load_photos_from_path(&dir.0)
            .expect("load shifted plans");
        assert_eq!(
            screen.photo_entries.len(),
            2,
            "a shifted photo is not dropped"
        );
        let first = screen
            .photo_entries
            .first()
            .and_then(|entry| entry.plan.capture_time)
            .map(CaptureTime::utc);
        assert_eq!(
            first,
            Some(1_763_000_900),
            "the all-photos scope moved the first clock by 900s"
        );
    }

    #[test]
    fn the_selected_scope_moves_only_the_checked_photos() {
        let dir = ScratchDir::new("shift-selected");
        let (_photo_a, _photo_b) = photo_pair(&dir);
        let mut screen = sample();
        screen.load_photos_from_path(&dir.0).expect("load photos");
        screen.photo_shift.kind = ScopeKind::Selected;
        let _inserted = screen.photo_shift.selected.insert(1);
        screen.photo_shift.shift.seconds = 900;
        screen.replan_photos().expect("replan the two photos");
        assert_eq!(
            screen.photo_entries.len(),
            2,
            "a photo outside the scope is not dropped"
        );
        let first = screen
            .photo_entries
            .first()
            .and_then(|entry| entry.plan.capture_time)
            .map(CaptureTime::utc);
        let second = screen
            .photo_entries
            .get(1)
            .and_then(|entry| entry.plan.capture_time)
            .map(CaptureTime::utc);
        assert_eq!(
            first,
            Some(1_763_000_000),
            "the unchecked photo kept its clock"
        );
        assert_eq!(
            second,
            Some(1_763_001_800),
            "the checked photo moved by 900s"
        );
    }

    #[test]
    fn a_failed_replan_keeps_the_list_and_draws_the_reason_beside_the_controls() {
        // The failure this covers: the old code cleared `photo_entries` before
        // the list was drawn, so the refusal was never rendered, the list and
        // the shift controls vanished, and a stale "OK 2 枚" stayed behind.
        let dir = ScratchDir::new("shift-camera");
        let (_photo_a, _photo_b) = photo_pair(&dir);
        let mut screen = sample();
        screen.load_photos_from_path(&dir.0).expect("load photos");
        screen.select_photo(0);
        screen.photo_shift.kind = ScopeKind::ByCamera;
        let error = screen
            .replan_photos()
            .expect_err("the synthetic JPEG names no camera");
        assert!(error.contains("カメラ名"), "got {error}");
        assert_eq!(
            screen.photo_entries.len(),
            2,
            "a failed re-plan kept neither the list nor the controls it belongs to"
        );
        let (status, message) = screen
            .photo_shift
            .status
            .as_ref()
            .expect("the reason is stored beside the controls");
        assert_eq!(*status, Status::Err);
        assert!(message.contains("カメラ名"), "got {message}");
        // The older plan must not be written while the controls ask for another.
        screen.open_geotag_confirmation();
        assert!(!screen.geotag_confirmation_open());
        assert!(matches!(screen.outcome(), Some((Status::Err, _))));

        // Rendered: the list rows and the refusal are both in the frame, so the
        // error is shown where the person can act on it.
        let texts = screen_texts_on(&mut screen, LogTab::Photos);
        assert!(
            texts.iter().any(|text| text.contains("a.jpg")),
            "the photo list vanished from the frame: {texts:?}"
        );
        assert!(
            texts.iter().any(|text| text.contains("カメラ名")),
            "the refusal was not drawn next to the photo controls: {texts:?}"
        );
        assert!(
            texts.iter().any(|text| text.contains("全写真")),
            "the scope controls were not drawn: {texts:?}"
        );
    }

    #[test]
    fn the_camera_scope_shows_the_base_camera_and_replans_when_it_changes() {
        let dir = ScratchDir::new("shift-camera-pick");
        // Two cameras: the first two photos share one, the third is another.
        let _first = jpeg_with_camera(&dir.0, "a.jpg", "HOLUX", "M-241X", 1_763_000_000);
        let _same = jpeg_with_camera(&dir.0, "b.jpg", "HOLUX", "M-241X", 1_763_000_030);
        let _other = jpeg_with_camera(&dir.0, "c.jpg", "Other", "X-1000", 1_763_000_060);
        let mut screen = sample();
        screen.load_photos_from_path(&dir.0).expect("load photos");
        // The list is sorted by path: a, b, c.
        screen.select_photo(0);
        screen.photo_shift.kind = ScopeKind::ByCamera;
        screen.photo_shift.shift.seconds = 900;
        screen.replan_photos().expect("the first camera resolves");

        assert_eq!(
            screen.photo_shift.camera_label().as_deref(),
            Some("HOLUX M-241X"),
            "the resolved camera is not shown to the person"
        );
        let capture_time = |screen: &LogScreen, index: usize| {
            screen
                .photo_entries
                .get(index)
                .and_then(|entry| entry.plan.capture_time)
                .map(CaptureTime::utc)
        };
        assert_eq!(capture_time(&screen, 0), Some(1_763_000_900));
        assert_eq!(capture_time(&screen, 1), Some(1_763_000_930));
        assert_eq!(
            capture_time(&screen, 2),
            Some(1_763_000_060),
            "a different camera's clock moved"
        );

        // Highlighting a photo of the other camera re-plans without a control
        // change: the list now follows the new base camera.
        screen.select_photo(2);
        assert_eq!(
            screen.photo_shift.camera_label().as_deref(),
            Some("Other X-1000")
        );
        assert_eq!(
            capture_time(&screen, 0),
            Some(1_763_000_000),
            "the first camera's clocks stayed shifted after the base changed"
        );
        assert_eq!(capture_time(&screen, 1), Some(1_763_000_030));
        assert_eq!(capture_time(&screen, 2), Some(1_763_000_960));

        let texts = screen_texts_on(&mut screen, LogTab::Photos);
        assert!(
            texts
                .iter()
                .any(|text| text.contains("基準カメラ: Other X-1000")),
            "the resolved camera is not drawn: {texts:?}"
        );
        assert!(
            texts.iter().any(|text| text.contains("範囲内 1 枚")),
            "the moved-photo count is not drawn: {texts:?}"
        );
    }

    #[test]
    fn a_shift_write_reports_the_in_scope_and_out_of_scope_counts() {
        let dir = ScratchDir::new("shift-report");
        let (_photo_a, _photo_b) = photo_pair(&dir);
        let mut screen = sample();
        screen.load_photos_from_path(&dir.0).expect("load photos");
        screen.photo_shift.kind = ScopeKind::Selected;
        let _checked = screen.photo_shift.selected.insert(0);
        screen.photo_shift.shift.seconds = 900;
        screen.replan_photos().expect("the selected scope resolves");
        screen.open_geotag_confirmation();
        assert!(screen.geotag_confirmation_open());
        let confirmation = screen
            .confirm_geotag_write()
            .expect("the prompt yields the token");
        let written = screen
            .apply_geotags(confirmation)
            .expect("the confirmed write succeeds");
        assert_eq!(written, 2);
        let (_status, message) = screen.outcome().expect("the write left an outcome");
        assert!(
            message.contains("範囲内 1 枚") && message.contains("範囲外 1 枚"),
            "the success sentence does not say how many clocks moved: {message}"
        );
        assert!(
            message.contains("2 枚の写真に位置情報"),
            "the sentence lost the written count: {message}"
        );
    }

    #[test]
    fn a_track_edit_that_breaks_the_photo_plan_drops_the_stale_list() {
        // relink runs after an edit, so its failure cannot keep entries whose
        // point indices name records the edit removed. The list is dropped and
        // the person is asked to load the folder again.
        let dir = ScratchDir::new("relink-failure");
        let (_photo_a, _photo_b) = photo_pair(&dir);
        let mut screen = sample();
        screen.load_photos_from_path(&dir.0).expect("load photos");
        screen.select_photo(0);
        screen.photo_shift.kind = ScopeKind::ByCamera;
        screen.set_track(sample_track(), ParseReport::default());
        assert!(
            screen.photo_entries.is_empty(),
            "a stale plan survived the edit that invalidated it"
        );
        let (_status, message) = screen
            .photo_shift
            .status
            .as_ref()
            .expect("the relink failure is reported");
        assert!(message.contains("読み込み直して"), "got {message}");
    }

    #[test]
    fn a_kmz_export_while_a_shift_is_armed_asks_for_the_geotag_write_first() {
        // The KML `<TimeStamp>` is the plan's capture time; an armed shift moves
        // it while the photo's Exif clock only changes on the geotag write, so
        // exporting a KMZ then would put a time in the archive the photo does not
        // carry.
        let dir = ScratchDir::new("kmz-shift");
        let (_photo_a, _photo_b) = photo_pair(&dir);
        let mut screen = sample();
        screen.load_photos_from_path(&dir.0).expect("load photos");
        screen.photo_shift.kind = ScopeKind::All;
        screen.photo_shift.shift.seconds = 900;
        screen.replan_photos().expect("replan with the shift");
        screen.set_kmz_export();
        screen.set_destination(dir.0.join("out.kmz").display().to_string());
        let error = screen.export().expect_err("an armed shift refuses the KMZ");
        assert!(error.contains("写真に位置"), "got {error}");
        assert!(
            !dir.0.join("out.kmz").exists(),
            "a refused KMZ export wrote a file"
        );
    }

    #[test]
    fn a_kmz_export_names_the_first_photos_it_could_not_bundle() {
        let dir = ScratchDir::new("kmz-skipped");
        let _matched = minimal_jpeg_with_mtime(&dir.0, "a.jpg", 1_763_000_000);
        let _far = minimal_jpeg_with_mtime(&dir.0, "b.jpg", 1_763_020_000);
        let mut screen = sample();
        screen.load_photos_from_path(&dir.0).expect("load photos");
        screen.set_kmz_export();
        let destination = dir.0.join("track.kmz");
        screen.set_destination(destination.display().to_string());
        screen.export().expect("write the archive");
        let (_status, message) = screen.outcome().expect("an outcome was recorded");
        assert!(
            message.contains("対象外 1 枚") && message.contains("b.jpg"),
            "the archive did not name the photo it left out: {message}"
        );
    }

    #[test]
    fn a_kmz_export_refuses_a_folder_the_rail_was_not_loaded_from() {
        // The KMZ now bundles the displayed plan, so a footer path that points
        // somewhere else would produce an archive the person never reviewed.
        let dir = ScratchDir::new("kmz-other-folder");
        let (_photo_a, _photo_b) = photo_pair(&dir);
        let other = ScratchDir::new("kmz-other-target");
        let _other_photo = minimal_jpeg_with_mtime(&other.0, "c.jpg", 1_763_000_000);
        let mut screen = sample();
        screen.load_photos_from_path(&dir.0).expect("load photos");
        screen.set_kmz_export();
        screen.set_destination(dir.0.join("out.kmz").display().to_string());
        screen.photo_path = other.0.display().to_string();
        let error = screen.export().expect_err("the folders differ");
        assert!(error.contains("一致しません"), "got {error}");
    }

    #[test]
    fn the_photo_block_is_captioned_as_sample() {
        // The screenshot is taken with `show_sample_photos`; the block must wear
        // 「見本」 like every other invented figure, or the image reads as proof
        // that two JPEGs were loaded.
        let mut screen = LogScreen::default();
        screen.show_sample();
        screen.show_sample_photos();
        let runs = screen_text_runs_on(&mut screen, LogTab::Photos);
        // The block title 「写真」 carries a 「見本」 on the same caption row, to
        // its right. Comparing y positions is what ties the marker to this block
        // rather than to the five other sample captions on the screen.
        let marked = runs.iter().any(|(text, pos)| {
            *text == "写真"
                && runs.iter().any(|(marker, marker_pos)| {
                    marker == "見本" && (marker_pos.y - pos.y).abs() < 2.0 && marker_pos.x > pos.x
                })
        });
        assert!(marked, "the photo block has no 見本 marker: {runs:?}");
        assert!(
            runs.iter().any(|(text, _pos)| text.contains("見本の写真")),
            "the sample photo status does not say it is a sample"
        );
    }

    #[test]
    fn a_delete_is_confirmed_before_it_changes_the_track_and_undo_restores_it() {
        let mut screen = sample();
        let before = screen.track().points.len();
        screen.edit.delete_list = String::from("1,2");
        screen.edit.request_delete();
        assert_eq!(
            screen.track().points.len(),
            before,
            "nothing changes before 実行"
        );
        let edited = screen.edit.confirm().expect("the armed delete runs");
        assert_eq!(edited.points.len(), before - 2);
        screen.adopt_edited_track(edited);
        assert_eq!(screen.track().points.len(), before - 2);
        let restored = screen.edit.undo().expect("undo restores the points");
        assert_eq!(restored.points.len(), before);
    }

    #[test]
    fn a_range_split_keeps_every_point_and_the_merge_joins_the_tracks_back() {
        let mut screen = sample();
        let before = screen.track().points.len();
        screen.edit.split_from = String::from("10");
        screen.edit.split_to = String::from("20");
        screen.edit.request_split();
        let split = screen.edit.confirm().expect("the armed split runs");
        assert_eq!(split.points.len(), before, "a split loses no point");
        assert_eq!(
            screen.edit.track_count(),
            3,
            "a range split makes three tracks"
        );
        screen.edit.request_merge_all();
        let merged = screen.edit.confirm().expect("the armed merge runs");
        assert_eq!(merged.points.len(), before);
        assert_eq!(screen.edit.track_count(), 1, "the merge produced one track");
    }

    #[test]
    fn an_edge_split_that_would_leave_an_empty_half_is_refused() {
        let mut screen = sample();
        // Point numbers are 1-based: 1 with an empty end is the first-point cut
        // that would leave an empty left half.
        screen.edit.split_from = String::from("1");
        screen.edit.split_to.clear();
        screen.edit.request_split();
        assert!(
            screen.edit.confirm().is_none(),
            "the refusal left nothing to run"
        );
        assert_eq!(
            screen.track().points.len(),
            sample_track().points.len(),
            "a refused split changed the track"
        );
    }

    #[test]
    fn a_gpx_file_loads_through_the_core_importer() {
        // `log read` accepts a dump; the open action also accepts the GPX this
        // tool writes, because a round trip through GPX is how a person moves a
        // track between machines without the raw dump.
        let dir = ScratchDir::new("gpx-load");
        let path = dir.0.join("track.gpx");
        let expected = sample_track();
        std::fs::write(&path, log::export::to_gpx(&expected)).expect("write the gpx");
        let mut screen = LogScreen::default();
        screen
            .load_bin_file(&path)
            .expect("the GPX loads through import_gpx");
        assert_eq!(screen.track().points.len(), expected.points.len());
        assert!(
            screen
                .load_status()
                .is_some_and(|(status, message)| *status == Status::Ok && message.contains("GPX")),
            "got {:?}",
            screen.load_status()
        );
    }

    #[test]
    fn the_summary_reports_top_speed_altitude_range_and_moving_time() {
        let mut screen = sample();
        let top = screen.max_speed_kmh().expect("the sample carries speeds");
        assert!(top > 20.0, "the top speed looked wrong: {top}");
        let (low, high) = screen.altitude_range().expect("the sample carries heights");
        assert!(low <= high, "the altitude range is inverted: {low} {high}");
        assert!(
            screen.moving_seconds() > 0,
            "a track with speeds reported no moving time"
        );

        let texts = screen_texts_on(&mut screen, LogTab::Summary);
        for label in ["最高速度", "高度", "移動時間"] {
            assert!(
                texts.iter().any(|text| text.contains(label)),
                "the summary has no {label} row: {texts:?}"
            );
        }
    }

    #[test]
    fn the_selected_point_uses_the_chosen_coordinate_format() {
        let screen = sample();
        let point = screen.track().points.first().copied().expect("a point");
        let mut settings = AppSettings::default();
        let decimal = coordinate_text(&point, &settings);
        assert!(
            decimal.contains('.'),
            "decimal degrees looked wrong: {decimal}"
        );
        settings.coord_format = m241_core::log::CoordFormat::Dms;
        let dms = coordinate_text(&point, &settings);
        assert!(dms.contains('°') && dms.contains('\''), "got {dms}");
        settings.coord_format = m241_core::log::CoordFormat::Utm;
        let utm = coordinate_text(&point, &settings);
        assert!(utm.contains('E') && utm.contains('N'), "got {utm}");
    }

    #[test]
    fn the_photo_options_group_by_distance_and_default_the_backup_on() {
        let mut screen = sample();
        assert!(screen.photo_keep_backup, "the backup must default on");
        assert!(screen.geotag_options().expect("valid options").keep_backup);
        assert_eq!(
            screen
                .geotag_options()
                .expect("valid options")
                .group_distance_meters,
            None
        );
        assert_eq!(
            screen
                .geotag_options()
                .expect("valid options")
                .max_gap_seconds,
            m241_core::log::geotag::DEFAULT_MAX_GAP_SECONDS
        );

        let dir = ScratchDir::new("photo-options");
        let _pair = photo_pair(&dir);
        screen.photo_group_distance = String::from("1000000");
        screen
            .load_photos_from_path(&dir.0)
            .expect("load the photos");
        let groups: Vec<Option<usize>> = screen
            .photo_entries()
            .iter()
            .map(|entry| entry.plan.group_id)
            .collect();
        assert_eq!(groups.len(), 2);
        assert!(
            groups.first().copied().flatten().is_some(),
            "grouping set no group id: {groups:?}"
        );
        assert_eq!(
            groups.first().copied().flatten(),
            groups.get(1).copied().flatten(),
            "both photos should share a group: {groups:?}"
        );

        screen.photo_keep_backup = false;
        screen.load_photos_from_path(&dir.0).expect("load again");
        assert!(
            screen
                .photo_entries()
                .iter()
                .all(|entry| !entry.plan.keep_backup),
            "turning the backup off did not reach the plans"
        );

        screen.photo_group_distance = String::from("wide");
        assert!(
            screen.load_photos_from_path(&dir.0).is_err(),
            "a non-numeric distance was accepted"
        );
        screen.photo_group_distance.clear();
        screen.photo_max_gap = String::from("soon");
        assert!(
            screen.load_photos_from_path(&dir.0).is_err(),
            "a non-numeric gap was accepted"
        );
    }

    #[test]
    fn a_kmz_camera_offset_matches_in_the_camera_clock_and_keeps_utc_times() {
        // The camera is set to UTC+9: its `DateTimeOriginal` is the UTC capture
        // plus 32,400 s. Without the offset the nearest point is nine hours away;
        // with it the photo matches, and the plan's capture time is put back on
        // UTC so the archive's `<TimeStamp>` agrees with the track.
        let dir = ScratchDir::new("kmz-tz");
        let utc = 1_763_000_000;
        let _photo = minimal_jpeg_with_mtime(&dir.0, "a.jpg", utc + 32_400);
        let track = Track {
            points: vec![TrackPoint {
                utc: Some(utc),
                latitude: Some(35.681_236),
                longitude: Some(139.767_125),
                ..TrackPoint::default()
            }],
            damaged_blocks: Vec::new(),
        };
        let options = GeotagOptions {
            max_gap_seconds: 300,
            ..GeotagOptions::default()
        };
        let paths = vec![dir.0.join("a.jpg")];
        let without = geotag::plan(&paths, &track, &options).expect("plan without tz");
        assert!(
            !without.first().is_some_and(GeotagPlan::will_write),
            "the unshifted plan unexpectedly matched"
        );
        let with = super::kmz_export::plan_with_tz(&paths, &track, &options, 540)
            .expect("plan in the camera clock");
        let plan = with.first().expect("one plan");
        assert!(plan.will_write(), "the shifted plan did not match");
        assert_eq!(
            plan.capture_time.map(CaptureTime::utc),
            Some(utc),
            "the capture time was not put back on UTC"
        );
    }

    #[test]
    fn a_kmz_with_the_icon_option_bundles_an_icon_entry() {
        let dir = ScratchDir::new("kmz-icons");
        let _pair = photo_pair(&dir);
        let mut screen = sample();
        screen.load_photos_from_path(&dir.0).expect("load photos");
        screen.set_kmz_export();
        // The core records the icon/ semantics as unconfirmed and the CLI writes
        // no icon/ entry, so thumbnails are a decision the person makes.
        assert!(
            !screen.kmz_icons,
            "the icon/ writer must be opt-in, not the default"
        );
        let output_dir = dir.0.join("out");
        std::fs::create_dir_all(&output_dir).expect("create the output directory");
        screen.kmz_icons = true;
        let with_icons = output_dir.join("with-icons.kmz");
        screen.set_destination(with_icons.display().to_string());
        screen.export().expect("export with icons");
        let archive = std::fs::read(&with_icons).expect("read the archive");
        assert!(
            archive.windows(5).any(|window| window == b"icon/"),
            "the icon/ entries are missing"
        );

        let without_icons = output_dir.join("without-icons.kmz");
        screen.kmz_icons = false;
        screen.set_destination(without_icons.display().to_string());
        screen.export().expect("export without icons");
        let archive = std::fs::read(&without_icons).expect("read the archive");
        assert!(
            !archive.windows(5).any(|window| window == b"icon/"),
            "the icon/ entries were written with the option off"
        );
    }

    #[test]
    fn the_edit_block_shows_the_track_name_and_colour_as_display_only() {
        let mut screen = sample();
        screen.log_path = String::from("holiday.gpx");
        let texts = screen_texts_on(&mut screen, LogTab::Edit);
        assert!(
            texts.iter().any(|text| text.contains("holiday")),
            "the track name is missing: {texts:?}"
        );
        assert!(
            texts.iter().any(|text| text.contains("データ色")),
            "the colour row is missing: {texts:?}"
        );
    }

    /// A fake tile fetcher for the full-screen tests: a 1×1 PNG.
    #[derive(Debug, Default)]
    struct CountingTiles;

    impl tiles::TileFetcher for CountingTiles {
        fn fetch(&self, _tile: tiles::TileId) -> Result<Vec<u8>, tiles::TileError> {
            Ok(tiny_png())
        }
    }

    /// A 1×1 opaque PNG.
    fn tiny_png() -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("PNG header");
            writer
                .write_image_data(&[255_u8, 0, 0, 255])
                .expect("PNG data");
        }
        bytes
    }

    /// A raw input that looks like an X11 window to the map gate.
    fn windowed_input() -> egui::RawInput {
        windowed_input_at(1280.0, 800.0)
    }

    /// The same, at a chosen size: the geometry tests drive more than the two
    /// documented windows, and the tile gate does not care which one it is.
    fn windowed_input_at(width: f32, height: f32) -> egui::RawInput {
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(width, height),
            )),
            ..egui::RawInput::default()
        };
        input.viewports.insert(
            egui::ViewportId::ROOT,
            egui::ViewportInfo {
                native_pixels_per_point: Some(1.0),
                inner_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(width, height),
                )),
                ..egui::ViewportInfo::default()
            },
        );
        input
    }

    /// Draw one full screen frame and return every text primitive it produced.
    fn frame_texts(
        context: &egui::Context,
        input: &egui::RawInput,
        screen: &mut LogScreen,
    ) -> Vec<String> {
        let output = context.run_ui(input.clone(), |ui| screen.ui(ui));
        let shapes: Vec<egui::Shape> = output
            .shapes
            .iter()
            .map(|clipped| clipped.shape.clone())
            .collect();
        let mut flat = Vec::new();
        flatten(&shapes, &mut flat);
        let texts = flat
            .iter()
            .filter_map(|shape| match shape {
                egui::Shape::Text(text) => Some(text.galley.job.text.clone()),
                _ => None,
            })
            .collect();
        output.drop_without_applying_deltas();
        texts
    }

    #[test]
    fn the_attribution_is_drawn_only_when_a_tile_is_on_screen() {
        // The credit names where the pixels came from. A sample or loading
        // frame has no OSM pixels, so it must not carry the credit; the two
        // tests this replaces asserted the credit and "no tiles" on the same
        // frame, which made the contradiction a passing contract.
        let context = egui::Context::default();
        crate::theme::install(&context);
        let input = windowed_input();
        let mut screen = LogScreen::default();
        screen.set_tile_fetcher_for_test(std::sync::Arc::new(CountingTiles));
        screen.set_track(sample_track(), ParseReport::default());
        for _ in 0..200 {
            let _texts = frame_texts(&context, &input, &mut screen);
            if screen.tiles.texture_count() > 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            screen.tiles.texture_count() > 0,
            "the fake fetch produced no texture"
        );
        let texts = frame_texts(&context, &input, &mut screen);
        assert!(
            texts
                .iter()
                .any(|text| text.contains("© OpenStreetMap contributors")),
            "a frame with OSM tiles carried no attribution: {texts:?}"
        );

        // The same screen switched to the sample: no OSM pixels, no credit.
        screen.sample = true;
        screen.tiles.reset();
        let texts = frame_texts(&context, &input, &mut screen);
        assert!(
            !texts
                .iter()
                .any(|text| text.contains("© OpenStreetMap contributors")),
            "a sample frame credited OSM tiles it did not draw: {texts:?}"
        );
        assert!(
            texts
                .iter()
                .any(|text| text.contains("見本：地図タイルは表示しません")),
            "the sample frame did not say it shows no tiles: {texts:?}"
        );
    }

    #[test]
    fn a_new_track_forgets_the_previous_areas_tiles() {
        // The stage 1 review found `set_track` leaving the old textures, pending
        // requests and failures in place: after loading another log the map kept
        // drawing the previous area under the new track.
        let context = egui::Context::default();
        crate::theme::install(&context);
        let input = windowed_input();
        let mut screen = LogScreen::default();
        screen.set_tile_fetcher_for_test(std::sync::Arc::new(CountingTiles));
        screen.set_track(sample_track(), ParseReport::default());
        for _ in 0..200 {
            let _texts = frame_texts(&context, &input, &mut screen);
            if screen.tiles.texture_count() > 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            screen.tiles.texture_count() > 0,
            "the fake fetch produced no texture"
        );
        screen.set_track(sample_track(), ParseReport::default());
        assert_eq!(
            screen.tiles.texture_count(),
            0,
            "the previous track's tiles survived set_track"
        );
    }

    #[test]
    fn a_fit_view_publishes_its_zoom_to_the_zoom_buttons() {
        // Fit is computed by `MapView::fit` (canvas + Mercator); `+`/`−` work
        // from `map_zoom`. Without publishing the fitted zoom, `+` right after
        // 全点表示 could lower the displayed zoom.
        let mut screen = sample();
        let rect = Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(770.0, 320.0));
        let view = screen.sync_fit_zoom(rect);
        assert!(screen.fit_mode, "a fresh track must start in fit mode");
        assert!(
            (screen.map_zoom - view.zoom as f32).abs() < 1e-6,
            "the buttons' zoom {} differs from the drawn zoom {}",
            screen.map_zoom,
            view.zoom
        );
        let before = screen.map_zoom;
        screen.set_zoom(screen.map_zoom + 1.0);
        assert!(
            screen.map_zoom > before,
            "`+` after 全点表示 lowered the zoom: {before} -> {}",
            screen.map_zoom
        );
        assert!(!screen.fit_mode, "a zoom press must leave fit mode");
    }

    /// Drive the real shell at `size` and return the Log screen's geometry.
    ///
    /// The title bar, the rail and the content margins are part of the rect the
    /// screen is handed, so the layout promises are measured through
    /// `App::draw` — handing the screen a rectangle would make them true by
    /// construction.
    fn log_frame_at(width: f32, height: f32, tab: Option<LogTab>) -> layout_probe::Frame {
        log_frame(width, height, tab, true, false)
    }

    /// The same, with the sample state and the tile fetcher chosen by the caller.
    ///
    /// `tiles` gives the frame a window gate and a fake fetcher that answers, so
    /// the OSM credit is actually painted; without it the canvas is headless and
    /// the credit is (correctly) absent.
    fn log_frame(
        width: f32,
        height: f32,
        tab: Option<LogTab>,
        sample: bool,
        tiles: bool,
    ) -> layout_probe::Frame {
        let context = egui::Context::default();
        crate::theme::install(&context);
        let mut app = crate::App::default();
        app.select(crate::screens::Screen::Log);
        if sample {
            // `set_track` rather than `show_sample`: the sample flag suppresses
            // OSM tiles (a sample frame has no window), and the credit tests need
            // tiles on the canvas with a track under them.
            app.log_mut()
                .set_track(sample_track(), ParseReport::default());
        }
        if tiles {
            app.log_mut()
                .set_tile_fetcher_for_test(std::sync::Arc::new(CountingTiles));
        }
        if let Some(tab) = tab {
            app.log_mut().open_drawer(tab);
        }
        let input = if tiles {
            windowed_input_at(width, height)
        } else {
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(width, height),
                )),
                ..egui::RawInput::default()
            }
        };
        if tiles {
            // The fake fetch runs on a worker; the credit only exists on a frame
            // that painted a tile, so wait for the texture before recording.
            for _attempt in 0..200 {
                let output = context.run_ui(input.clone(), |ui| app.draw(ui));
                output.drop_without_applying_deltas();
                if app.log_mut().tiles.texture_count() > 0 {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(
                app.log_mut().tiles.texture_count() > 0,
                "the fake fetch produced no texture"
            );
        }
        for _pass in 0..2 {
            let output = context.run_ui(input.clone(), |ui| app.draw(ui));
            output.drop_without_applying_deltas();
        }
        layout_probe::last().expect("the Log screen recorded its layout")
    }

    /// The rect recorded for the overlay named `name`.
    fn overlay_rect(frame: &layout_probe::Frame, name: &'static str) -> Rect {
        frame
            .overlays
            .iter()
            .find(|(overlay, _rect)| *overlay == name)
            .map(|(_name, rect)| *rect)
            .unwrap_or_else(|| panic!("no {name} plate was drawn: {:?}", frame.overlays))
    }

    /// Assert that every recorded overlay is on the canvas and none intersect.
    fn assert_overlays_apart(frame: &layout_probe::Frame, label: &str) {
        for (index, (name, rect)) in frame.overlays.iter().enumerate() {
            assert!(
                frame.map.contains_rect(*rect),
                "{name} sits outside the canvas at {label}: {rect:?} not in {:?}",
                frame.map,
            );
            for (other_name, other) in &frame.overlays[index + 1..] {
                assert!(
                    !rect.intersects(*other),
                    "{name} {rect:?} overlaps {other_name} {other:?} at {label}",
                );
            }
        }
    }

    #[test]
    fn the_map_is_the_page_at_both_documented_sizes() {
        // DESIGN.md §5: the map is the page, and DESIGN.md §8 names the two sizes
        // the review judges. The floor is the layout promise the review set: 80%
        // of the content at 1280×800 and 75% at 1024×640.
        for (width, height, floor) in [(1280.0, 800.0, 0.80), (1024.0, 640.0, 0.75)] {
            let frame = log_frame_at(width, height, None);
            let covered = frame.map.height() / frame.content.height();
            assert!(
                covered >= floor,
                "at {width}×{height} the map covers {:.1}% of the content, below the {:.0}% floor",
                covered * 100.0,
                floor * 100.0,
            );
            assert!(
                frame.tool.height() <= 48.0,
                "the tool row above the map is {}pt tall, over the 48pt limit",
                frame.tool.height(),
            );
            assert!(
                (frame.map.width() - frame.content.width()).abs() < 1.0,
                "the map does not use the content width at {width}×{height}",
            );
        }
    }

    #[test]
    fn a_long_load_error_wraps_inside_the_file_plate() {
        // Must 7: the file plate used to cut the refusal to one line and the full
        // sentence was reachable only by hovering the label — not by keyboard.
        // The plate now reserves the measured wrapped height and draws every row,
        // so the whole sentence is on screen.
        let message = String::from(
            "/very/long/path/to/a/log/file/that/does/not/exist/on/this/machine.bin \
             を読めません: そのようなファイルやディレクトリはありません。パスを確認してください。",
        );
        let context = egui::Context::default();
        crate::theme::install(&context);
        let mut app = crate::App::default();
        app.select(crate::screens::Screen::Log);
        app.log_mut()
            .set_track(sample_track(), ParseReport::default());
        app.log_mut().load_status = Some((Status::Err, message.clone()));
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(1280.0, 800.0),
            )),
            ..egui::RawInput::default()
        };
        let mut laid_out = String::new();
        let mut rows = 0;
        for _pass in 0..2 {
            let output = context.run_ui(input.clone(), |ui| app.draw(ui));
            let shapes: Vec<egui::Shape> = output
                .shapes
                .iter()
                .map(|clipped| clipped.shape.clone())
                .collect();
            let mut flat = Vec::new();
            flatten(&shapes, &mut flat);
            laid_out.clear();
            rows = 0;
            for shape in &flat {
                if let egui::Shape::Text(text) = shape
                    && text.galley.job.text.contains("を読めません")
                {
                    laid_out = text.galley.job.text.clone();
                    rows = text.galley.rows.len();
                }
            }
            output.drop_without_applying_deltas();
        }
        assert_eq!(
            laid_out, message,
            "the plate laid out only part of the refusal"
        );
        assert!(
            rows >= 2,
            "the refusal was laid out on {rows} row(s); it is still being cut"
        );
        let frame = layout_probe::last().expect("the Log screen recorded its layout");
        let file = overlay_rect(&frame, "file");
        assert!(
            file.height() > metric::ROW + 3.0 * space::S + 16.0,
            "the file plate did not grow for the wrapped line: {}pt",
            file.height(),
        );
    }

    #[test]
    fn the_map_plates_do_not_overlap_each_other_or_the_credit() {
        // The overlays are fixed plates anchored to the canvas corners: a caption
        // that grew with its text would move one plate into the next. The geometry
        // is recorded as it is drawn and checked, rather than judged from a
        // screenshot (DESIGN.md §5, design review 2026-11-23, M1).
        let context = egui::Context::default();
        crate::theme::install(&context);
        // One empty frame so the fonts exist before a bare `Painter` lays out the
        // credit; the credit's geometry is what has to clear the zoom plate.
        let font_output = context.run_ui(egui::RawInput::default(), |ui| {
            let _ = ui.allocate_exact_size(Vec2::splat(1.0), Sense::hover());
        });
        font_output.drop_without_applying_deltas();

        for (width, height) in [(1280.0, 800.0), (1024.0, 640.0)] {
            let frame = log_frame_at(width, height, None);
            let label = format!("{width}×{height}");
            assert_overlays_apart(&frame, &label);
            assert!(
                frame.overlays.iter().any(|(name, _)| *name == "zoom"),
                "no zoom plate was drawn at {label}",
            );
            let zoom = overlay_rect(&frame, "zoom");
            // A closed-drawer sample frame draws no OSM tiles, so the credit is
            // not in the recorded overlays; it is laid out from the plate geometry
            // that the screen would use on a tiled frame.
            let plates = super::drawer::Plates::new(frame.map, 0.0, false);
            let painter =
                egui::Painter::new(context.clone(), egui::LayerId::background(), frame.map);
            let credit = tiles::draw_attribution(&painter, plates.credit(frame.map));
            assert!(
                !credit.intersects(zoom),
                "the credit {credit:?} overlaps the zoom plate {zoom:?} at {label}",
            );
            assert!(
                frame.map.contains_rect(credit),
                "the credit sits outside the canvas at {label}",
            );
        }
    }

    #[test]
    fn the_credit_and_the_zoom_plate_stay_apart_with_the_drawer_open() {
        // Stage 1, Must 1: with the drawer open the canvas shrinks, and at a
        // 1200pt window the credit's 356pt plate ran under the zoom plate (map
        // width 616, credit right edge 360 from the left, zoom left edge 304).
        // The credit is now laid out in the canvas left of the zoom plate and
        // wraps to fit, so the two cannot meet at any width; the drawer's
        // threshold is the content width, so 1200 floats the drawer and the
        // narrow column starts at 1000pt of content. 1264 / 1269 bracket the
        // width where the one-line credit just fits; the gap there is the
        // smallest on the page, so it is sampled rather than assumed.
        for width in [1100.0, 1200.0, 1240.0, 1264.0, 1269.0, 1280.0] {
            let label = format!("{width}×800 with the drawer open");
            let frame = log_frame(width, 800.0, Some(LogTab::Summary), true, true);
            let credit = overlay_rect(&frame, "attribution");
            let zoom = overlay_rect(&frame, "zoom");
            assert!(
                !credit.intersects(zoom),
                "the credit {credit:?} overlaps the zoom plate {zoom:?} at {label}",
            );
            assert!(
                credit.right() + space::M <= zoom.left() + 0.5,
                "the credit {credit:?} is closer than {}pt to the zoom plate's left edge {:?} at {label}",
                space::M,
                zoom.left(),
            );
            assert_overlays_apart(&frame, &label);

            // The floating drawer is lifted above the playback and zoom plates,
            // so opening it does not make the map's own controls unreadable.
            let drawer = frame.drawer.expect("the drawer was not recorded");
            if drawer.left() < frame.map.right() - 1.0 {
                assert!(
                    !drawer.intersects(zoom),
                    "the floating drawer {drawer:?} covers the zoom plate {zoom:?} at {label}",
                );
                if let Some(playback) = frame
                    .overlays
                    .iter()
                    .find(|(name, _rect)| *name == "playback")
                    .map(|(_name, rect)| *rect)
                {
                    assert!(
                        !drawer.intersects(playback),
                        "the floating drawer {drawer:?} covers the playback plate {playback:?} at {label}",
                    );
                }
            }
        }
    }

    #[test]
    fn an_empty_log_keeps_its_plate_clear_of_the_drawer() {
        // The empty state's plate carries the screen's primary action (ログ操作を
        // 開く) and the 見本 checkbox. A floating drawer over the canvas used to
        // cover it; the plate is now centred in the canvas width the drawer
        // leaves, and the drawer is not drawn over it at any of the widths the
        // geometry tests visit.
        for width in [1000.0, 1024.0, 1200.0, 1280.0] {
            let label = format!("{width}×640 empty log");
            let frame = log_frame(width, 640.0, Some(LogTab::Ops), false, false);
            let empty = overlay_rect(&frame, "empty");
            let drawer = frame.drawer.expect("the drawer was not recorded");
            assert!(
                !empty.intersects(drawer),
                "the empty-state plate {empty:?} sits under the drawer {drawer:?} at {label}",
            );
            assert!(
                frame.map.contains_rect(empty),
                "the empty-state plate sits outside the canvas at {label}",
            );
        }
    }

    #[test]
    fn every_drawer_tab_opens_and_draws_its_panel() {
        // The drawer is where the five panels live (DESIGN.md §5), so "the
        // feature is still there" means "its tab still renders it".
        let signatures = [
            (LogTab::Summary, "距離"),
            (LogTab::Photos, "フォルダ"),
            (LogTab::Edit, "トラック名"),
            (LogTab::Export, "保存先"),
            (LogTab::Ops, "吸い出し"),
        ];
        for (tab, signature) in signatures {
            let mut screen = sample();
            let texts = screen_texts_on(&mut screen, tab);
            assert!(
                texts.iter().any(|text| text == tab.label()),
                "the {} tab is not drawn: {texts:?}",
                tab.label(),
            );
            assert!(
                texts.iter().any(|text| text.contains(signature)),
                "the {} tab drew no panel ({} missing): {texts:?}",
                tab.label(),
                signature,
            );
        }
    }

    #[test]
    fn the_drawer_is_beside_the_map_when_wide_and_over_it_when_narrow() {
        // 1024×640 is a supported window (DESIGN.md §8). A 360pt column beside an
        // 800pt content area would leave the map a strip, so the drawer floats
        // over it below `NARROW` (DESIGN.md §9).
        let wide = log_frame_at(1280.0, 800.0, Some(LogTab::Summary));
        let wide_drawer = wide.drawer.expect("the wide window drew no drawer");
        assert!(
            (wide_drawer.width() - metric::DRAWER).abs() < 1.0,
            "the drawer is {:.1}pt wide, expected {}",
            wide_drawer.width(),
            metric::DRAWER,
        );
        assert!(
            wide.map.right() <= wide_drawer.left() + 1.0,
            "the wide drawer {:?} overlaps the map {:?}",
            wide_drawer,
            wide.map,
        );

        let narrow = log_frame_at(1024.0, 640.0, Some(LogTab::Summary));
        let narrow_drawer = narrow.drawer.expect("the narrow window drew no drawer");
        assert!(
            narrow_drawer.left() < narrow.map.right() - 1.0,
            "the narrow drawer {:?} does not float over the map {:?}",
            narrow_drawer,
            narrow.map,
        );
        assert!(
            (narrow.map.width() - narrow.content.width()).abs() < 1.0,
            "the floating drawer shrank the map instead of covering it",
        );
        assert!(
            narrow.map.contains_rect(narrow_drawer),
            "the floating drawer {:?} is not inside the canvas {:?}",
            narrow_drawer,
            narrow.map,
        );
    }
}
