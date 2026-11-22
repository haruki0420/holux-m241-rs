//! ログ — the Log screen (DESIGN.md §5).
//!
//! Three zones, right-aligned figures, ruled tables: the manual's reference page.
//! The track is painted from the points themselves rather than through a slippy
//! map, because a tile fetcher needs the network and cannot be exercised headlessly;
//! the projection is forty lines and is what makes the shape of the trip visible,
//! which is the job on this screen.
//!
//! This module owns the offline half of the screen: reading a `.bin` dump from
//! disk, scanning a photo folder, writing EXIF tags into the photos a person
//! confirmed, and exporting GPX/KML/CSV. Nothing here talks to a device — the
//! download that fills [`LogScreen::track`] is the Firmware screen's twin
//! unsolved problem (both need the session layer) — and a track arrived at from
//! disk can only be written back into the photos and the export file, never the
//! logger.

// The write token lives one level down, where this module cannot construct it:
// see `geotag_gate` for why the privacy below `log.rs` is what closes M2.
mod geotag_gate;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::{Align2, FontId, Rect, RichText, Sense, Stroke, StrokeKind, Ui, Vec2};
use m241_core::log::geotag::{self, GeotagOptions, GeotagPlan};
use m241_core::log::{self, ParseReport};
use m241_core::{ExportFormat, Track, TrackPoint};

use crate::screens::common::{self, Status};
use crate::screens::settings::AppSettings;
use crate::theme::{self, metric, size, space};

use self::geotag_gate::GeotagConfirmation;

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
const MIN_ZOOM: f32 = 1.0;

/// Highest zoom the projection allows; above this the whole loop fits in a few
/// pixels and the shape stops being readable.
const MAX_ZOOM: f32 = 18.0;

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

/// The Log screen's own state.
#[derive(Debug)]
pub struct LogScreen {
    track: Track,
    report: ParseReport,
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
    /// Logical slippy-map zoom level (`1.0..=18.0`), clamped >= 12 while following.
    map_zoom: f32,
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
    destination: String,
    outcome: Option<(Status, String)>,
    sample: bool,
}

impl Default for LogScreen {
    fn default() -> Self {
        Self {
            track: Track::default(),
            report: ParseReport::default(),
            log_path: String::new(),
            load_status: None,
            selected: None,
            hidden_days: BTreeSet::new(),
            playing: false,
            last_step_at: None,
            follow: false,
            fit_mode: true,
            map_zoom: FOLLOW_MIN_ZOOM,
            photo_path: String::new(),
            photo_entries: Vec::new(),
            selected_photo: None,
            photo_status: None,
            confirm_geotag: false,
            format: ExportFormat::Gpx,
            destination: String::from("m241-track.gpx"),
            outcome: None,
            sample: false,
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
        self.track = track;
        self.report = report;
        self.playing = false;
        self.last_step_at = None;
        self.fit_mode = true;
        self.follow = false;
        self.map_zoom = self.compute_fit_zoom();
        self.hidden_days.clear();
        self.close_geotag_confirmation();
        self.outcome = None;
        self.relink_photos();
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

    /// Load a `.bin` binary log dump from disk (offline, read-only) via [`log::parse_dump`].
    ///
    /// # Errors
    ///
    /// Returns a Japanese error string if the path is empty, too large, unreadable,
    /// or not a valid M-241 binary log dump.
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

    /// Select a track point by index, syncing the closest photo and enforcing follow zoom.
    ///
    /// Closes an open geotag confirmation: the prompt lists the files a write
    /// would change, and selecting another point re-syncs the highlighted photo,
    /// so the prompt's premise has moved.
    pub fn select_point(&mut self, index: Option<usize>) {
        self.selected = index.filter(|i| *i < self.track.points.len());
        self.close_geotag_confirmation();
        if self.follow && self.map_zoom < FOLLOW_MIN_ZOOM {
            self.map_zoom = FOLLOW_MIN_ZOOM;
        }
        self.sync_photo_to_selected();
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

    /// Scan a file or directory for photos and build a read-only geotag plan.
    ///
    /// # Errors
    ///
    /// Returns a Japanese error string if the path does not exist, cannot be read,
    /// contains no files, or `geotag::plan` fails.
    pub fn load_photos_from_path(&mut self, path: &Path) -> Result<usize, String> {
        let path_text = path.to_string_lossy();
        if path_text.trim().is_empty() {
            let msg = String::from("写真フォルダまたはファイルのパスを入力してください");
            self.photo_status = Some((Status::Err, msg.clone()));
            return Err(msg);
        }
        let mut paths: Vec<PathBuf> = Vec::new();
        if path.is_file() {
            paths.push(path.to_path_buf());
        } else if path.is_dir() {
            let entries = match std::fs::read_dir(path) {
                Ok(e) => e,
                Err(err) => {
                    let msg = format!("{} を開けません: {err}", path.display());
                    self.photo_status = Some((Status::Err, msg.clone()));
                    return Err(msg);
                }
            };
            for entry in entries.flatten() {
                let candidate = entry.path();
                let is_hidden = candidate
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with('.'));
                if candidate.is_file() && !is_hidden {
                    paths.push(candidate);
                }
            }
            paths.sort();
        } else {
            let msg = format!("{} が見つかりません", path.display());
            self.photo_status = Some((Status::Err, msg.clone()));
            return Err(msg);
        }

        if paths.is_empty() {
            let msg = format!("{} に写真ファイルがありません", path.display());
            self.photo_status = Some((Status::Err, msg.clone()));
            return Err(msg);
        }

        let plans = match geotag::plan(&paths, &self.track, &GeotagOptions::default()) {
            Ok(p) => p,
            Err(err) => {
                let msg = format!("写真を読み込めません: {err}");
                self.photo_status = Some((Status::Err, msg.clone()));
                return Err(msg);
            }
        };

        self.photo_path = path.display().to_string();
        self.photo_entries = plans
            .into_iter()
            .map(|plan| {
                let point_index = find_matching_point_index(&self.track, &plan);
                PhotoEntry { plan, point_index }
            })
            .collect();
        self.close_geotag_confirmation();
        self.sync_photo_to_selected();
        if self.selected_photo.is_none() && !self.photo_entries.is_empty() {
            self.selected_photo = Some(0);
        }

        let total = self.photo_entries.len();
        let matched = self.matched_photo_count();
        self.photo_status = Some((
            Status::Ok,
            format!("{total} 枚を読み込みました（一致 {matched} 枚）"),
        ));
        Ok(matched)
    }

    /// Select a photo entry by index and jump `selected_point` to its matched track point.
    ///
    /// Closes an open geotag confirmation for the same reason as [`Self::select_point`].
    pub fn select_photo(&mut self, photo_index: usize) {
        let Some(entry) = self.photo_entries.get(photo_index) else {
            return;
        };
        let point_index = entry.point_index;
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
        let result = geotag::apply(&plans);
        // Closed before the branch: success and failure both end the confirmation,
        // because a failed write leaves nothing the person can approve again.
        self.close_geotag_confirmation();
        match result {
            Ok(written) => {
                let count = written.len();
                let msg = format!("{count} 枚の写真に位置情報を書き込みました");
                self.outcome = Some((Status::Ok, msg.clone()));
                self.photo_status = Some((Status::Ok, msg));
                self.relink_photos();
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
    /// offer to change zero files.
    fn open_geotag_confirmation(&mut self) {
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
    pub fn ui_with_settings(&mut self, ui: &mut Ui, settings: &AppSettings) {
        self.advance_playback_if_due(ui, settings.reduced_motion);
        self.handle_keyboard(ui);

        if !self.track.points.is_empty() {
            // The export bar is pinned: it is the reason the screen exists, and at
            // 1024×640 it used to be the part that fell off the bottom. Omitted
            // when there is no track yet so the empty state does not offer to
            // export nothing.
            common::footer(ui, "m241-log-export", |ui| self.export_bar(ui));
        }

        common::scrolling(ui, "m241-log-body", |ui| {
            common::heading(
                ui,
                "ログ",
                "吸い出したログや .bin ファイルを地図に描き、写真の時刻と突き合わせて書き出します。",
            );
            // Every panel and block on this screen carries 「見本」 in its own
            // caption when the figures are sample data, so a separate band would
            // spend height the 1280×800 page needs for the speed chart.
            ui.add_space(space::L);

            if self.track.points.is_empty() {
                self.empty_state(ui);
                return;
            }

            self.file_loader_bar(ui);
            ui.add_space(space::M);

            // The three-zone layout collapses below 1100pt (DESIGN.md §9). The figure is
            // a *window* size in the design, and it has to be read as one: the rail and
            // the content margins have already taken ~216pt by the time this runs, so
            // measuring the content would collapse the layout at the 1280-wide window
            // the design calls wide.
            let window_width = ui.ctx().content_rect().width();
            let wide = window_width >= metric::NARROW;
            if wide {
                ui.horizontal_top(|ui| {
                    ui.allocate_ui_with_layout(
                        Vec2::new(metric::RAIL_BLOCK, ui.available_height()),
                        egui::Layout::top_down(egui::Align::LEFT),
                        |ui| self.rail_block(ui, settings),
                    );
                    ui.add_space(space::L);
                    ui.vertical(|ui| {
                        self.map_panel(ui);
                        self.speed_panel(ui, settings);
                        self.selected_panel(ui, settings);
                    });
                });
            } else {
                self.map_panel(ui);
                ui.add_space(space::M);
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| self.summary_block(ui, settings));
                    ui.add_space(space::L);
                    ui.vertical(|ui| self.selected_panel(ui, settings));
                });
                ui.add_space(space::M);
                self.photo_rail_section(ui);
                ui.add_space(space::M);
                self.speed_panel(ui, settings);
            }

            ui.add_space(space::L);
            self.point_table(ui, settings);
        });
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

    fn file_loader_bar(&mut self, ui: &mut Ui) {
        // One compact row rather than a panel: opening another dump is the
        // screen's secondary action, and the reference-manual page has to keep
        // the map and the speed chart above the fold at 1280×800.
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("ログファイル")
                    .size(size::CAPTION)
                    .color(theme::MUTE),
            );
            let _response = ui.add_sized(
                Vec2::new(
                    (ui.available_width() - metric::ACTION_WIDTH - space::L).max(120.0),
                    metric::ROW,
                ),
                egui::TextEdit::singleline(&mut self.log_path)
                    .hint_text(RichText::new("ログファイルのパス").size(size::CAPTION))
                    .font(egui::TextStyle::Monospace),
            );
            if common::primary_button(ui, "ログを開く", true).clicked() {
                let target = PathBuf::from(self.log_path.trim());
                let _res = self.load_bin_file(&target);
            }
        });
        if let Some((status, message)) = &self.load_status {
            common::status_line(ui, *status, message);
        }
    }

    /// Empty log: the action that fills it, not a shrug (DESIGN.md §6).
    fn empty_state(&mut self, ui: &mut Ui) {
        common::panel(ui, "ログ", |ui| {
            common::status_line(ui, Status::Info, "まだログがありません");
            ui.add_space(space::S);
            ui.label(
                RichText::new(
                    "保存済みの .bin ログを開くか、端末をつないでからログを吸い出してください。",
                )
                .size(size::UI)
                .color(theme::MUTE),
            );
            ui.add_space(space::M);
            // The action that works today is the screen's one primary control.
            // The path belongs to the action, so it sits on the same line.
            ui.horizontal(|ui| {
                let _response = ui.add_sized(
                    Vec2::new(
                        (ui.available_width() - metric::ACTION_WIDTH - space::L).max(120.0),
                        metric::ROW,
                    ),
                    egui::TextEdit::singleline(&mut self.log_path)
                        .hint_text(RichText::new("ログファイルのパス").size(size::CAPTION))
                        .font(egui::TextStyle::Monospace),
                );
                if common::primary_button(ui, "ログを開く", true).clicked() {
                    let target = PathBuf::from(self.log_path.trim());
                    let _res = self.load_bin_file(&target);
                }
            });
            if let Some((status, message)) = &self.load_status {
                ui.add_space(space::XS);
                common::status_line(ui, *status, message);
            }
            ui.add_space(space::S);
            // The action that needs the session layer: disabled, and it says
            // 未検証 in words rather than looking like a button that works.
            let _download = common::secondary_button(ui, "ログを吸い出す", false);
            ui.label(
                RichText::new(
                    "未検証 — 端末からの直接吸い出しは CLI の `m241 log dump` を利用してください",
                )
                .size(size::CAPTION)
                .color(theme::MUTE),
            );
        });

        ui.add_space(space::M);
        ui.checkbox(
            &mut self.sample,
            RichText::new("見本のログを表示（実機から読んだ値ではありません）")
                .size(size::CAPTION)
                .color(theme::MUTE),
        );
        // Only when the toggle is turned on and nothing has been loaded: re-seeding
        // every frame would throw away the row the user just selected.
        if self.sample && self.track.points.is_empty() {
            self.set_track(sample_track(), ParseReport::default());
        }
    }

    /// The 集計 figures: distance, duration, average speed.
    ///
    /// Three rows, the ones DESIGN.md §5's rail block names. 点数 and 位置 used to
    /// be repeated here; the point table below is the place a count belongs, and
    /// the rail block has to stay subordinate to it.
    fn summary_block(&self, ui: &mut Ui, settings: &AppSettings) {
        common::block_title(ui, "集計", self.sample);
        common::hairline(ui);
        ui.add_space(space::XS);
        for (label, value) in [
            ("距離", self.total_distance_with_settings(settings)),
            ("時間", self.elapsed()),
            ("平均", self.average_speed_with_settings(settings)),
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

    /// The left rail block: days, photos, totals. Subordinate to the tab rail: the
    /// same staircase rhythm, no fill (DESIGN.md §5).
    fn rail_block(&mut self, ui: &mut Ui, settings: &AppSettings) {
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

        ui.add_space(space::M);
        self.photo_rail_section(ui);

        ui.add_space(space::M);
        self.summary_block(ui, settings);
    }

    /// 写真 rail block: load a photo folder/file with `geotag::plan` and sync with the track.
    fn photo_rail_section(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("写真").size(size::CAPTION).color(theme::MUTE));
        common::hairline(ui);
        ui.add_space(space::XS);

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("フォルダ")
                    .size(size::CAPTION)
                    .color(theme::MUTE),
            );
            let _response = ui.add(
                egui::TextEdit::singleline(&mut self.photo_path)
                    .hint_text(RichText::new("写真フォルダ").size(size::CAPTION))
                    .font(egui::TextStyle::Monospace)
                    .desired_width((ui.available_width() - space::XXL * 2.0).max(64.0)),
            );
        });
        ui.add_space(space::XS);
        if ui
            .add(
                egui::Button::new(RichText::new("写真を読込").size(size::UI))
                    .corner_radius(theme::RADIUS),
            )
            .clicked()
        {
            let target = PathBuf::from(self.photo_path.trim());
            let _res = self.load_photos_from_path(&target);
        }

        if let Some((status, message)) = &self.photo_status {
            ui.add_space(space::XS);
            common::status_line(ui, *status, message);
        }

        ui.add_space(space::XS);
        if self.photo_entries.is_empty() {
            ui.label(
                RichText::new("写真を読み込むと時刻から軌跡と突き合わせます")
                    .size(size::CAPTION)
                    .color(theme::MUTE),
            );
            return;
        }

        let mut clicked_photo: Option<usize> = None;
        let entries_len = self.photo_entries.len();
        egui::ScrollArea::vertical()
            .id_salt("m241-log-photos")
            .max_height(metric::ROW * 5.0)
            .auto_shrink([false, true])
            .show_rows(ui, metric::TABLE_HEADER, entries_len, |ui, range| {
                for index in range {
                    let Some(entry) = self.photo_entries.get(index) else {
                        continue;
                    };
                    let name = entry
                        .plan
                        .photo
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("photo");
                    let selected = self.selected_photo == Some(index);
                    let suffix = if entry.plan.will_write() {
                        entry
                            .plan
                            .gap_seconds
                            .map_or_else(|| String::from("一致"), |g| format!("{g}s"))
                    } else {
                        String::from("対象外")
                    };
                    let row_text = format!("{name}  {suffix}");
                    let response = ui.selectable_label(
                        selected,
                        RichText::new(row_text)
                            .size(size::CAPTION)
                            .color(if selected { theme::INK } else { theme::MUTE })
                            .monospace(),
                    );
                    if let Some(reason) = &entry.plan.skipped {
                        let _ = response.clone().on_hover_text(reason);
                    }
                    if response.clicked() {
                        clicked_photo = Some(index);
                    }
                }
            });
        if let Some(index) = clicked_photo {
            self.select_photo(index);
        }
    }

    /// 地図: the track painted from latitude and longitude, with playback and follow controls.
    fn map_panel(&mut self, ui: &mut Ui) {
        let sample = self.sample;
        common::sample_panel(ui, "地図", sample, |ui| {
            let height = metric::PLOT;
            let (rect, response) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::click());
            let painter = ui.painter_at(rect);
            painter.rect_filled(rect, theme::RADIUS, theme::RAIL);

            let plotted: Vec<(usize, egui::Pos2)> = self.plotted_points(rect);
            if plotted.is_empty() {
                painter.text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    "位置のある点がありません",
                    FontId::proportional(size::UI),
                    theme::MUTE,
                );
            } else {
                let first_index = plotted.first().map(|(index, _pos)| *index);
                let last_index = plotted.last().map(|(index, _pos)| *index);
                draw_grid(&painter, rect);
                let line: Vec<egui::Pos2> = plotted.iter().map(|(_index, pos)| *pos).collect();
                if line.len() > 1 {
                    // The spec's track numbers (07-eztour §5 「幅 5・不透明度 0.7」)
                    // live in `metric::track`, so this screen names a token rather
                    // than a number. The single hue is the product's data colour;
                    // where the ten-colour cycle went is explained with
                    // `track_palette_colour` and in docs/dev/01-design-decisions.ja.md.
                    painter.add(egui::Shape::line(line, metric::track::line_stroke()));
                }
                for (index, pos) in &plotted {
                    if self.selected == Some(*index) {
                        continue;
                    }
                    // Every point here has a position by construction, and each
                    // takes the next colour in the cycle: a long track then reads as
                    // a sequence rather than as one flat shape.
                    painter.circle_filled(*pos, metric::POINT_DOT, track_palette_colour(*index));
                    // Waypoints are the device's own POI button: drawn as a ring
                    // so they read as a place rather than as another sample.
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

                // 始点 / 終点 (DESIGN.md §5 through ezTour §5): the start is a small
                // ring, the end a large one, and both are ink. Size and ink are a
                // deliberate reading of ezTour's `line_start` / `line_end` icons:
                // the ten-colour palette from `track_palette_colour` is reserved
                // for the records themselves, so a ring in a palette hue would
                // read as an eleventh record, and the selected marker (3×, ink,
                // filled) already says "what you are looking at". The ring radius
                // is what separates start from end, so keep both off the waypoint
                // ring's 2× radius (`a_waypoint_is_drawn_as_a_ring`).
                for (index, radius) in [
                    (first_index, metric::POINT_DOT * 1.5),
                    (last_index, metric::POINT_DOT * 2.5),
                ] {
                    if let Some(index) = index
                        && self.selected != Some(index)
                        && let Some((_, pos)) = plotted.iter().find(|(idx, _)| *idx == index)
                    {
                        painter.circle_stroke(
                            *pos,
                            radius,
                            Stroke::new(metric::HAIRLINE, theme::INK),
                        );
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
                        painter.rect_stroke(
                            marker_rect,
                            theme::RADIUS,
                            Stroke::new(metric::HAIRLINE, theme::INK),
                            StrokeKind::Outside,
                        );
                    }
                }

                // Selected / active playback marker drawn last so it stays on top.
                if let Some(selected_idx) = self.selected
                    && let Some((_, pos)) = plotted.iter().find(|(idx, _)| *idx == selected_idx)
                {
                    painter.circle_filled(*pos, metric::POINT_DOT * 2.0, theme::INK);
                    painter.circle_stroke(
                        *pos,
                        metric::POINT_DOT * 3.0,
                        Stroke::new(metric::HAIRLINE, theme::INK),
                    );
                }
            }

            let mode_note = if self.follow {
                format!("追従 zoom {:.0}（地図タイルなし・軌跡のみ）", self.map_zoom)
            } else {
                format!("zoom {:.0}（地図タイルなし・軌跡のみ）", self.map_zoom)
            };
            painter.text(
                rect.right_bottom() + Vec2::new(-space::S, -space::S),
                Align2::RIGHT_BOTTOM,
                mode_note,
                FontId::proportional(size::CAPTION),
                theme::MUTE,
            );

            // Clicking the map selects the nearest plotted point: the map is a
            // picker, not a picture, and a track you cannot ask about is a diagram.
            if response.clicked()
                && let Some(pointer) = response.interact_pointer_pos()
                && let Some((index, _distance)) = plotted
                    .iter()
                    .map(|(index, pos)| (*index, pos.distance(pointer)))
                    .min_by(|left, right| left.1.total_cmp(&right.1))
            {
                self.select_point(Some(index));
            }

            ui.add_space(space::S);
            self.playback_controls(ui);
        });
    }

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
            if ui
                .add(
                    egui::Button::new(RichText::new(play_label).size(size::UI))
                        .corner_radius(theme::RADIUS),
                )
                .clicked()
            {
                self.toggle_play();
            }
            if ui
                .add(
                    egui::Button::new(RichText::new("前の点").size(size::UI))
                        .corner_radius(theme::RADIUS),
                )
                .clicked()
            {
                let _moved = self.step_prev();
            }
            if ui
                .add(
                    egui::Button::new(RichText::new("次の点").size(size::UI))
                        .corner_radius(theme::RADIUS),
                )
                .clicked()
            {
                let _moved = self.step_next();
            }
            if ui
                .add(
                    egui::Button::new(RichText::new("全点表示").size(size::UI))
                        .corner_radius(theme::RADIUS),
                )
                .clicked()
            {
                self.fit_all();
            }

            let mut follow = self.follow;
            if ui
                .add(egui::Button::selectable(
                    follow,
                    RichText::new("追従").size(size::UI),
                ))
                .clicked()
            {
                follow = !follow;
                self.set_follow(follow);
            }

            if ui
                .add(
                    egui::Button::new(RichText::new("−").size(size::UI))
                        .corner_radius(theme::RADIUS),
                )
                .clicked()
            {
                self.set_zoom(self.map_zoom - 1.0);
            }
            if ui
                .add(
                    egui::Button::new(RichText::new("＋").size(size::UI))
                        .corner_radius(theme::RADIUS),
                )
                .clicked()
            {
                self.set_zoom(self.map_zoom + 1.0);
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
            let (rect, response) = ui.allocate_exact_size(
                Vec2::new(track_width, metric::ROW * 0.5),
                Sense::click_and_drag(),
            );
            let painter = ui.painter_at(rect);
            let y = rect.center().y;
            painter.hline(
                rect.x_range(),
                y,
                Stroke::new(metric::HAIRLINE, theme::RULE),
            );
            if max_idx > 0 {
                let thumb_x = rect.left() + rect.width() * (pos as f32 / max_idx as f32);
                let filled =
                    Rect::from_min_max(rect.left_top(), egui::pos2(thumb_x, rect.bottom()));
                painter.rect_filled(filled, theme::RADIUS, theme::DATA.gamma_multiply(0.35));
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
            let (rect, response) = ui.allocate_exact_size(
                Vec2::new(ui.available_width(), metric::CHART),
                Sense::click(),
            );
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
                Stroke::new(metric::HAIRLINE, theme::RULE),
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

    /// 選択した点: the figures for one record, right-aligned in a ruled block.
    fn selected_panel(&mut self, ui: &mut Ui, settings: &AppSettings) {
        let sample = self.sample;
        common::sample_panel(ui, "選択した点", sample, |ui| {
            let Some(point) = self.selected.and_then(|index| self.track.points.get(index)) else {
                common::status_line(ui, Status::Info, "点を選んでください");
                return;
            };
            ui.label(
                RichText::new(common::utc_stamp(point.utc))
                    .size(size::UI)
                    .color(theme::INK)
                    .monospace(),
            );
            ui.add_space(space::XS);
            ui.label(
                RichText::new(common::coordinate(point.latitude, point.longitude))
                    .size(size::UI)
                    .color(theme::INK)
                    .monospace(),
            );
            ui.add_space(space::XS);
            ui.label(
                RichText::new(format!(
                    "標高 {}   速度 {}",
                    settings.units.format_altitude(point.height_m),
                    settings.units.format_speed(point.speed_kmh)
                ))
                .size(size::CAPTION)
                .color(theme::MUTE),
            );
            if point.is_waypoint() {
                // The device records a button press as a bit in the record; the
                // screen says so in words rather than painting a second colour.
                ui.label(
                    RichText::new("この点は本体のボタンで付けた目印です")
                        .size(size::CAPTION)
                        .color(theme::MUTE),
                );
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
                    if self.point_row(ui, index, &point, settings) {
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
    ) -> bool {
        let selected = self.selected == Some(index);
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), metric::ROW), Sense::click());
        if selected {
            ui.painter().rect_filled(rect, theme::RADIUS, theme::PANEL);
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
            common::coordinate(point.latitude, point.longitude),
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
            Stroke::new(metric::HAIRLINE, theme::RULE),
        );
        response.clicked()
    }

    /// The export row: a format, a destination, one button that says what it does.
    fn export_bar(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new("書き出し")
                    .size(size::CAPTION)
                    .color(theme::MUTE),
            );
            for format in ExportFormat::ALL {
                let label = format.extension().to_uppercase();
                let selected = self.format == format;
                if ui
                    .add(egui::Button::selectable(
                        selected,
                        RichText::new(label).size(size::UI),
                    ))
                    .clicked()
                {
                    self.set_export_format(format);
                }
            }
            // Named in the design, absent from the exporter: saying so is better
            // than a button that quietly writes something else (§7, no icons for
            // words — and no control that lies about what it produces).
            let _ignored = ui.add_enabled(
                false,
                egui::Button::new(RichText::new("KMZ").size(size::UI)).corner_radius(theme::RADIUS),
            );
            ui.label(
                RichText::new("未対応")
                    .size(size::CAPTION)
                    .color(theme::MUTE),
            );

            let matched = self.matched_photo_count();
            let photo_button_label = if matched > 0 {
                format!("写真に位置 ({matched} 枚)")
            } else {
                String::from("写真に位置")
            };
            if common::secondary_button(ui, &photo_button_label, matched > 0).clicked() {
                self.open_geotag_confirmation();
            }
        });

        // The destination and the one export action sit on their own line: a
        // fixed-width field cannot share a wrapping row with a min-size button
        // without the button being clipped at the panel edge.
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("保存先")
                    .size(size::CAPTION)
                    .color(theme::MUTE),
            );
            let field_width = (ui.available_width() - metric::ACTION_WIDTH - space::S).max(120.0);
            let _response = ui.add_sized(
                Vec2::new(field_width, metric::ROW),
                egui::TextEdit::singleline(&mut self.destination).font(egui::TextStyle::Monospace),
            );
            if common::primary_button(ui, "書き出す", true).clicked() {
                let _res = self.export();
            }
        });

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
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add(
                        egui::Button::new(RichText::new("書き込む").size(size::UI))
                            .corner_radius(theme::RADIUS),
                    )
                    .clicked()
                    && let Some(confirmation) = self.confirm_geotag_write()
                {
                    let _res = self.apply_geotags(confirmation);
                }
                if ui
                    .add(
                        egui::Button::new(RichText::new("キャンセル").size(size::UI))
                            .corner_radius(theme::RADIUS),
                    )
                    .clicked()
                {
                    self.close_geotag_confirmation();
                }
            });
        }

        if let Some((status, message)) = &self.outcome {
            common::status_line(ui, *status, message);
        }
    }

    /// Change the active export format and update the destination extension while preserving the stem.
    pub fn set_export_format(&mut self, format: ExportFormat) {
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

    /// Set the export destination path.
    pub fn set_destination(&mut self, path: impl Into<String>) {
        self.destination = path.into();
    }

    /// Render the chosen format and write it, refusing to overwrite an existing file silently.
    ///
    /// The conversion is `m241_core::log::export`, the same function the CLI calls,
    /// so the two cannot drift into different files from the same log.
    ///
    /// # Errors
    ///
    /// Returns a Japanese error string if the track is empty, destination is empty,
    /// the destination already exists, or writing fails.
    pub fn export(&mut self) -> Result<(), String> {
        let track = &self.track;
        if track.points.is_empty() {
            let msg = String::from("書き出す点がありません");
            self.outcome = Some((Status::Err, msg.clone()));
            return Err(msg);
        }
        let dest_trimmed = self.destination.trim().to_owned();
        if dest_trimmed.is_empty() {
            let msg = String::from("保存先を入力してください");
            self.outcome = Some((Status::Err, msg.clone()));
            return Err(msg);
        }
        let dest_path = Path::new(&dest_trimmed);
        if dest_path.exists() {
            let msg = format!(
                "{dest_trimmed} はすでに存在するため上書きしません。別のファイル名を指定してください"
            );
            self.outcome = Some((Status::Err, msg.clone()));
            return Err(msg);
        }
        let document = log::export(track, self.format);
        match std::fs::write(dest_path, document) {
            Ok(()) => {
                self.outcome = Some((
                    Status::Ok,
                    format!("{dest_trimmed} に {} 点を書き出しました", track.fix_count()),
                ));
                Ok(())
            }
            Err(err) => {
                let msg = format!("書き出せませんでした: {err}");
                self.outcome = Some((Status::Err, msg.clone()));
                Err(msg)
            }
        }
    }

    /// Every point with a position, as (index, pixel).
    ///
    /// Kept as indices rather than as a filtered copy so a click on the map can
    /// select the *original* row in the table.
    fn plotted_points(&self, rect: Rect) -> Vec<(usize, egui::Pos2)> {
        let fixes: Vec<(usize, f64, f64)> = self
            .track
            .points
            .iter()
            .enumerate()
            .filter_map(|(index, point)| match (point.latitude, point.longitude) {
                (Some(latitude), Some(longitude)) => Some((index, latitude, longitude)),
                _ => None,
            })
            .collect();
        let Some(&(_, first_latitude, first_longitude)) = fixes.first() else {
            return Vec::new();
        };

        if !self.fit_mode {
            let (center_lat, center_lon) = self
                .selected
                .and_then(|idx| self.track.points.get(idx))
                .and_then(|p| Some((p.latitude?, p.longitude?)))
                .unwrap_or_else(|| {
                    let (mut min_lat, mut max_lat) = (first_latitude, first_latitude);
                    let (mut min_lon, mut max_lon) = (first_longitude, first_longitude);
                    for (_, lat, lon) in &fixes {
                        min_lat = min_lat.min(*lat);
                        max_lat = max_lat.max(*lat);
                        min_lon = min_lon.min(*lon);
                        max_lon = max_lon.max(*lon);
                    }
                    ((min_lat + max_lat) / 2.0, (min_lon + max_lon) / 2.0)
                });
            let deg_per_px = 360.0 / (256.0 * 2.0_f64.powf(f64::from(self.map_zoom)));
            let lon_scale = center_lat.to_radians().cos().abs().max(0.1);
            let center = rect.center();
            return fixes
                .iter()
                .map(|(index, lat, lon)| {
                    let dx = ((*lon - center_lon) * lon_scale / deg_per_px) as f32;
                    let dy = ((center_lat - *lat) / deg_per_px) as f32;
                    (*index, egui::pos2(center.x + dx, center.y + dy))
                })
                .collect();
        }

        let (mut min_latitude, mut max_latitude) = (first_latitude, first_latitude);
        let (mut min_longitude, mut max_longitude) = (first_longitude, first_longitude);
        for (_, latitude, longitude) in &fixes {
            min_latitude = min_latitude.min(*latitude);
            max_latitude = max_latitude.max(*latitude);
            min_longitude = min_longitude.min(*longitude);
            max_longitude = max_longitude.max(*longitude);
        }

        let padding = space::M;
        let width = (rect.width() - padding * 2.0).max(1.0);
        let height = (rect.height() - padding * 2.0).max(1.0);
        let latitude_span = (max_latitude - min_latitude).max(f64::EPSILON);
        let longitude_span = (max_longitude - min_longitude).max(f64::EPSILON);
        // One scale for both axes, so the drawn shape is the shape of the trip
        // rather than a stretched one that fits the rectangle.
        let scale = (width as f64 / longitude_span).min(height as f64 / latitude_span);
        let drawn_width = longitude_span * scale;
        let drawn_height = latitude_span * scale;
        let left = rect.left() + padding + ((width as f64 - drawn_width) / 2.0) as f32;
        let top = rect.top() + padding + ((height as f64 - drawn_height) / 2.0) as f32;

        fixes
            .iter()
            .map(|(index, latitude, longitude)| {
                let x = left + ((longitude - min_longitude) * scale) as f32;
                // Latitude grows northwards and y grows downwards, so the
                // subtraction is what puts north at the top.
                let y = top + ((max_latitude - latitude) * scale) as f32;
                (*index, egui::pos2(x, y))
            })
            .collect()
    }

    fn compute_fit_zoom(&self) -> f32 {
        let mut iter = self
            .track
            .points
            .iter()
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

    fn relink_photos(&mut self) {
        if self.photo_entries.is_empty() {
            return;
        }
        let paths: Vec<PathBuf> = self
            .photo_entries
            .iter()
            .map(|entry| entry.plan.photo.clone())
            .collect();
        if let Ok(plans) = geotag::plan(&paths, &self.track, &GeotagOptions::default()) {
            self.photo_entries = plans
                .into_iter()
                .map(|plan| {
                    let point_index = find_matching_point_index(&self.track, &plan);
                    PhotoEntry { plan, point_index }
                })
                .collect();
            self.sync_photo_to_selected();
        }
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
            .filter(|(_index, point)| !self.hidden_days.contains(&common::utc_date(point.utc)))
            .map(|(index, _point)| index)
            .collect()
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
    let stroke = Stroke::new(metric::HAIRLINE, theme::RULE.gamma_multiply(0.6));
    for step in 1..4 {
        let fraction = step as f32 / 4.0;
        let x = rect.left() + rect.width() * fraction;
        let y = rect.top() + rect.height() * fraction;
        painter.vline(x, rect.y_range(), stroke);
        painter.hline(rect.x_range(), y, stroke);
    }
}

/// Where the logger had been: the §5 ten-colour cycle over plotted records.
///
/// Kept out of `map_panel` so the test can ask "was this index painted in the
/// palette?" without reaching into a painter. The colour itself is the theme's
/// record fill, which applies the specified opacity.
fn track_palette_colour(index: usize) -> egui::Color32 {
    metric::track::record_fill(index)
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
        let path = dir.join(name);
        std::fs::write(&path, &bytes).expect("write jpeg");
        let file = std::fs::File::options()
            .write(true)
            .open(&path)
            .expect("open jpeg for mtime");
        let times = std::fs::FileTimes::new()
            .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(u64::from(utc)));
        file.set_times(times).expect("set jpeg mtime");
        path
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
            (ExportFormat::Csv, "index,utc,lat,lon"),
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
    fn the_track_uses_the_specified_width_and_opacity() {
        // 07-eztour-spec §5 「幅 5・不透明度 0.7」: the two numbers are carried over
        // from the reference implementation, so they are asserted as literal
        // values rather than by re-reading the constant.
        assert!((metric::track::LINE_WIDTH - 5.0).abs() < f32::EPSILON);
        assert!((metric::track::LINE_OPACITY - 0.7).abs() < 0.000_1);
        // The ten-colour cycle: every record takes one of ten hues, and the
        // product's data colour is the first of them.
        assert_eq!(theme::TRACK_PALETTE.len(), 10);
        assert_eq!(track_palette_colour(0), theme::DATA.gamma_multiply(0.7));
        assert_eq!(
            track_palette_colour(10),
            track_palette_colour(0),
            "the palette does not wrap after ten records"
        );
        assert_ne!(
            track_palette_colour(1),
            track_palette_colour(0),
            "the second record repeats the first colour"
        );
    }

    #[test]
    fn summary_formats_imperial_units_when_selected() {
        let screen = sample();
        let metric_settings = AppSettings::default();
        let imperial_settings = AppSettings {
            units: crate::screens::settings::UnitSystem::Imperial,
            reduced_motion: false,
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
}
