//! ログ — the Log screen (DESIGN.md §5).
//!
//! Three zones, right-aligned figures, ruled tables: the manual's reference page.
//! The track is painted from the points themselves rather than through a slippy
//! map, because a tile fetcher needs the network and cannot be exercised headlessly;
//! the projection is forty lines and is what makes the shape of the trip visible,
//! which is the job on this screen.
//!
//! State is held here and nothing is read from the device yet: the download that
//! fills [`LogScreen::points`] is the firmware screen's twin problem (both need the
//! session layer), and this screen is written so that filling it is a `set_track`
//! call rather than a rewrite.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::{Align2, FontId, Rect, RichText, Sense, Stroke, StrokeKind, Ui, Vec2};
use m241_core::log::geotag::{self, GeotagOptions, GeotagPlan};
use m241_core::log::{self, ParseReport};
use m241_core::{ExportFormat, LogSummary, Track, TrackPoint};

use crate::screens::common::{self, Status};
use crate::screens::settings::AppSettings;
use crate::theme::{self, metric, size, space};

/// Rows the point list shows before it scrolls.
///
/// Six is a reference-manual figure: enough to read a stretch of the trip, few
/// enough that the export bar stays above the fold on a 640pt window.
const ROWS_SHOWN: f32 = 6.0;

/// Maximum `.bin` log size accepted from disk (16 MiB — well above any M-241 flash).
const MAX_LOG_BYTES: usize = 16 * 1024 * 1024;

/// Minimum map zoom when follow mode is active (per ezTour spec §5).
pub const FOLLOW_MIN_ZOOM: f32 = 12.0;

/// A planned photo entry paired with the matched index in [`Track::points`].
#[derive(Debug, Clone, PartialEq)]
pub struct PhotoEntry {
    /// Read-only geotag plan produced by `m241_core::log::geotag::plan`.
    pub plan: GeotagPlan,
    /// Index of the matched point in the current track, when matched.
    pub point_index: Option<usize>,
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
        self.confirm_geotag = false;
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
    pub fn select_point(&mut self, index: Option<usize>) {
        self.selected = index.filter(|i| *i < self.track.points.len());
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
    pub fn set_follow(&mut self, follow: bool) {
        self.follow = follow;
        if follow {
            self.fit_mode = false;
            if self.map_zoom < FOLLOW_MIN_ZOOM {
                self.map_zoom = FOLLOW_MIN_ZOOM;
            }
        }
    }

    /// Set the map zoom level explicitly, respecting the follow-mode floor (`>= 12`).
    pub fn set_zoom(&mut self, zoom: f32) {
        self.fit_mode = false;
        let min_zoom = if self.follow { FOLLOW_MIN_ZOOM } else { 1.0 };
        self.map_zoom = zoom.clamp(min_zoom, 18.0);
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
        self.confirm_geotag = false;
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
    pub fn select_photo(&mut self, photo_index: usize) {
        let Some(entry) = self.photo_entries.get(photo_index) else {
            return;
        };
        self.selected_photo = Some(photo_index);
        if let Some(point_index) = entry.point_index
            && point_index < self.track.points.len()
        {
            self.selected = Some(point_index);
            if self.follow && self.map_zoom < FOLLOW_MIN_ZOOM {
                self.map_zoom = FOLLOW_MIN_ZOOM;
            }
        }
    }

    /// Apply the previously planned geotags to disk. Must only be called after explicit user confirmation.
    ///
    /// # Errors
    ///
    /// Returns a Japanese error string if no matched photos exist or writing EXIF fails.
    pub fn apply_geotags(&mut self) -> Result<usize, String> {
        let plans: Vec<GeotagPlan> = self
            .photo_entries
            .iter()
            .map(|entry| entry.plan.clone())
            .collect();
        let matched = plans.iter().filter(|p| p.will_write()).count();
        if matched == 0 {
            let msg = String::from("位置を書き込める写真がありません");
            self.outcome = Some((Status::Err, msg.clone()));
            self.confirm_geotag = false;
            return Err(msg);
        }
        match geotag::apply(&plans) {
            Ok(written) => {
                let count = written.len();
                self.confirm_geotag = false;
                let msg = format!("{count} 枚の写真に位置情報を書き込みました");
                self.outcome = Some((Status::Ok, msg.clone()));
                self.photo_status = Some((Status::Ok, msg));
                Ok(count)
            }
            Err(err) => {
                self.confirm_geotag = false;
                let msg = format!("写真へ書き込めませんでした: {err}");
                self.outcome = Some((Status::Err, msg.clone()));
                Err(msg)
            }
        }
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
            // The review's fourth finding: this screen showed sample coordinates and
            // sample speeds with no marking at all. The band is under the heading and
            // above every figure, so it cannot be scrolled or cropped away from them.
            if self.sample {
                ui.add_space(space::S);
                common::sample_marker(ui);
            }
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
        if !self.playing {
            return;
        }
        let interval = if reduced_motion {
            Duration::from_millis(500)
        } else {
            Duration::from_millis(250)
        };
        let now = Instant::now();
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
        if self.playing {
            ui.ctx().request_repaint_after(interval);
        }
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
        common::panel(ui, "ログファイル (.bin)", |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("ファイル")
                        .size(size::CAPTION)
                        .color(theme::MUTE),
                );
                let _response = ui.add(
                    egui::TextEdit::singleline(&mut self.log_path)
                        .hint_text(RichText::new("path/to/log.bin").size(size::CAPTION))
                        .font(egui::TextStyle::Monospace)
                        .desired_width(
                            (ui.available_width() - metric::ACTION_WIDTH - space::XL).max(120.0),
                        ),
                );
                if ui
                    .add(
                        egui::Button::new(RichText::new("ログを開く").size(size::UI))
                            .corner_radius(theme::RADIUS)
                            .min_size(Vec2::new(metric::ACTION_WIDTH, 0.0)),
                    )
                    .clicked()
                {
                    let target = PathBuf::from(self.log_path.trim());
                    let _res = self.load_bin_file(&target);
                }
            });
            if let Some((status, message)) = &self.load_status {
                ui.add_space(space::XS);
                common::status_line(ui, *status, message);
            }
        });
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
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("ログファイル")
                        .size(size::CAPTION)
                        .color(theme::MUTE),
                );
                let _response = ui.add(
                    egui::TextEdit::singleline(&mut self.log_path)
                        .hint_text(RichText::new("path/to/log.bin").size(size::CAPTION))
                        .font(egui::TextStyle::Monospace)
                        .desired_width(
                            (ui.available_width() - metric::ACTION_WIDTH - space::XXL).max(120.0),
                        ),
                );
                if ui
                    .add(
                        egui::Button::new(RichText::new("ログを開く").size(size::UI))
                            .corner_radius(theme::RADIUS)
                            .min_size(Vec2::new(metric::ACTION_WIDTH, 0.0)),
                    )
                    .clicked()
                {
                    let target = PathBuf::from(self.log_path.trim());
                    let _res = self.load_bin_file(&target);
                }
            });
            if let Some((status, message)) = &self.load_status {
                ui.add_space(space::S);
                common::status_line(ui, *status, message);
            }
            ui.add_space(space::M);
            ui.horizontal(|ui| {
                let _ignored = ui.add_enabled(
                    false,
                    egui::Button::new(RichText::new("ログを吸い出す").size(size::UI))
                        .corner_radius(theme::RADIUS),
                );
                ui.label(
                    RichText::new("未検証 — 端末からの直接吸い出しは CLI の `m241 log dump` を利用してください")
                        .size(size::CAPTION)
                        .color(theme::MUTE),
                );
            });
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
    fn summary_block(&self, ui: &mut Ui, settings: &AppSettings) {
        let summary = LogSummary::of(&self.track);
        common::block_title(ui, "集計", self.sample);
        common::hairline(ui);
        ui.add_space(space::XS);
        for (label, value) in [
            ("距離", self.total_distance_with_settings(settings)),
            ("時間", self.elapsed()),
            ("平均", self.average_speed_with_settings(settings)),
            ("点数", common::grouped(summary.records)),
            ("位置", common::grouped(summary.fixes)),
        ] {
            ui.horizontal(|ui| {
                ui.label(RichText::new(label).size(size::CAPTION).color(theme::MUTE));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(value)
                            .size(size::UI)
                            .color(theme::INK)
                            .monospace(),
                    );
                });
            });
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
            .show_rows(ui, metric::ROW, entries_len, |ui, range| {
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
                draw_grid(&painter, rect);
                let line: Vec<egui::Pos2> = plotted.iter().map(|(_index, pos)| *pos).collect();
                if line.len() > 1 {
                    painter.add(egui::Shape::line(
                        line,
                        egui::epaint::PathStroke::new(metric::TRACK_LINE, theme::DATA),
                    ));
                }
                for (index, pos) in &plotted {
                    if self.selected == Some(*index) {
                        continue;
                    }
                    painter.circle_filled(*pos, metric::POINT_DOT, theme::DATA);
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
                format!("追従 zoom {:.0}（地図タイルなし）", self.map_zoom)
            } else {
                format!("zoom {:.0}（地図タイルなし）", self.map_zoom)
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
            let mut pos = self.selected.unwrap_or(0).min(max_idx);
            if ui
                .add(
                    egui::Slider::new(&mut pos, 0..=max_idx)
                        .show_value(false)
                        .trailing_fill(true),
                )
                .changed()
            {
                self.select_point(Some(pos));
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
        ui.label(
            RichText::new("書き出し")
                .size(size::CAPTION)
                .color(theme::MUTE),
        );
        common::hairline(ui);
        ui.add_space(space::S);

        ui.horizontal_wrapped(|ui| {
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
            if ui
                .add_enabled(
                    matched > 0,
                    egui::Button::new(RichText::new(photo_button_label).size(size::UI))
                        .corner_radius(theme::RADIUS),
                )
                .clicked()
            {
                self.confirm_geotag = true;
            }
            if matched == 0 {
                ui.label(
                    RichText::new("写真読込後にプレビュー可能")
                        .size(size::CAPTION)
                        .color(theme::MUTE),
                );
            }
        });

        if self.confirm_geotag {
            let matched = self.matched_photo_count();
            ui.add_space(space::S);
            ui.horizontal_wrapped(|ui| {
                common::status_line(
                    ui,
                    Status::Waiting,
                    &format!("一致した {matched} 枚の JPEG に位置情報を書き込みますか？"),
                );
                if ui
                    .add(
                        egui::Button::new(RichText::new("書き込む").size(size::UI))
                            .corner_radius(theme::RADIUS),
                    )
                    .clicked()
                {
                    let _res = self.apply_geotags();
                }
                if ui
                    .add(
                        egui::Button::new(RichText::new("キャンセル").size(size::UI))
                            .corner_radius(theme::RADIUS),
                    )
                    .clicked()
                {
                    self.confirm_geotag = false;
                }
            });
        }

        ui.add_space(space::S);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("保存先")
                    .size(size::CAPTION)
                    .color(theme::MUTE),
            );
            let _response = ui.add(
                egui::TextEdit::singleline(&mut self.destination)
                    .font(egui::TextStyle::Monospace)
                    .desired_width(ui.available_width() - metric::ACTION_WIDTH - space::L),
            );
            if ui
                .add(
                    egui::Button::new(RichText::new("書き出す").size(size::UI))
                        .corner_radius(theme::RADIUS)
                        .min_size(Vec2::new(metric::ACTION_WIDTH, 0.0)),
                )
                .clicked()
            {
                let _res = self.export();
            }
        });

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
        (z as f32).clamp(1.0, 18.0)
    }

    fn sync_photo_to_selected(&mut self) {
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

    #[cfg(test)]
    fn total_distance(&self) -> String {
        self.total_distance_with_settings(&AppSettings::default())
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

fn find_matching_point_index(track: &Track, plan: &GeotagPlan) -> Option<usize> {
    let target = plan.point?;
    let target_utc = target.utc?;
    track
        .points
        .iter()
        .enumerate()
        .filter(|(_, p)| p.has_position() && p.utc.is_some())
        .min_by_key(|(_, p)| p.utc.map_or(u32::MAX, |u| u.abs_diff(target_utc)))
        .map(|(idx, _)| idx)
}

/// A faint grid, so a track with a straight edge can be read as geography rather
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
        let distance = screen.total_distance();
        assert!(
            distance.ends_with(" km"),
            "the figure is missing its unit: {distance}"
        );
        let value: f64 = distance.trim_end_matches(" km").parse().unwrap_or(0.0);
        assert!(value > 0.0, "a loop of 240 points measured 0 km");
    }

    struct ScratchDir(PathBuf);

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

    #[test]
    fn loading_photos_plans_read_only_and_syncs_selection_both_ways() {
        let dir = ScratchDir::new("photos");
        let photo_a = minimal_jpeg_with_mtime(&dir.0, "a.jpg", 1_763_000_000);
        let photo_b = minimal_jpeg_with_mtime(&dir.0, "b.jpg", 1_763_000_900);
        let orig_a = std::fs::read(&photo_a).expect("orig a");
        let orig_b = std::fs::read(&photo_b).expect("orig b");

        let mut screen = sample();
        // sample track starts at 1_763_000_000 with 30s steps -> index 0 and index 30.
        let matched = screen
            .load_photos_from_path(&dir.0)
            .expect("plan photos in dir");
        assert_eq!(matched, 2);
        assert_eq!(screen.photo_entries().len(), 2);

        // Must be strictly read-only until explicit confirmation.
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
