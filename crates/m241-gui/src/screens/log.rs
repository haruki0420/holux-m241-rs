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

use egui::{Align2, FontId, Rect, RichText, Sense, Stroke, Ui, Vec2};
use m241_core::log::{self, ParseReport};
use m241_core::{ExportFormat, LogSummary, Track, TrackPoint};

use crate::screens::common::{self, Status};
use crate::theme::{self, metric, size, space};

/// Rows the point list shows before it scrolls.
///
/// Six is a reference-manual figure: enough to read a stretch of the trip, few
/// enough that the export bar stays above the fold on a 640pt window.
const ROWS_SHOWN: f32 = 6.0;

/// The Log screen's own state.
#[derive(Debug)]
pub struct LogScreen {
    track: Track,
    report: ParseReport,
    /// Which point the figures panel describes.
    selected: Option<usize>,
    /// Days the user has unchecked in the track block.
    hidden_days: BTreeSet<String>,
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
            selected: None,
            hidden_days: BTreeSet::new(),
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
        self.outcome = None;
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

    /// Draw the screen body. The rail is drawn by the shell, not here.
    pub fn ui(&mut self, ui: &mut Ui) {
        common::heading(
            ui,
            "ログ",
            "吸い出したログを地図に描き、写真の時刻と突き合わせて GPX に書き出します。",
        );
        ui.add_space(space::L);

        if self.track.points.is_empty() {
            self.empty_state(ui);
            return;
        }

        // The three-zone layout collapses below 1100pt (DESIGN.md §9). Content
        // width, not window width: the rail already took its share, so measuring
        // the window would keep the columns on a screen that has no room for them.
        let wide = ui.available_width() >= metric::NARROW;
        if wide {
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(
                    Vec2::new(metric::RAIL_BLOCK, ui.available_height()),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| self.rail_block(ui),
                );
                ui.add_space(space::L);
                ui.vertical(|ui| {
                    self.map_panel(ui);
                    self.speed_panel(ui);
                    self.selected_panel(ui);
                });
            });
        } else {
            self.map_panel(ui);
            ui.add_space(space::M);
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| self.summary_block(ui));
                ui.add_space(space::L);
                ui.vertical(|ui| self.selected_panel(ui));
            });
            ui.add_space(space::M);
            self.speed_panel(ui);
        }

        ui.add_space(space::L);
        self.point_table(ui);
        ui.add_space(space::L);
        self.export_bar(ui);
    }

    /// Empty log: the action that fills it, not a shrug (DESIGN.md §6).
    fn empty_state(&mut self, ui: &mut Ui) {
        common::panel(ui, "ログ", |ui| {
            common::status_line(ui, Status::Info, "まだログがありません");
            ui.add_space(space::S);
            ui.label(
                RichText::new("端末をつないでから、下のボタンでログを吸い出してください。")
                    .size(size::UI)
                    .color(theme::MUTE),
            );
            ui.add_space(space::M);
            ui.horizontal(|ui| {
                let _ignored = ui.add_enabled(
                    false,
                    egui::Button::new(RichText::new("ログを吸い出す").size(size::UI))
                        .corner_radius(theme::RADIUS),
                );
                ui.label(
                    RichText::new("未検証 — 端末の読み出しは接続画面の実装後")
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
    fn summary_block(&self, ui: &mut Ui) {
        let summary = LogSummary::of(&self.track);
        ui.label(RichText::new("集計").size(size::CAPTION).color(theme::MUTE));
        common::hairline(ui);
        ui.add_space(space::XS);
        for (label, value) in [
            ("距離", self.total_distance()),
            ("時間", self.elapsed()),
            ("平均", self.average_speed()),
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
    fn rail_block(&mut self, ui: &mut Ui) {
        ui.label(
            RichText::new("トラック")
                .size(size::CAPTION)
                .color(theme::MUTE),
        );
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
        ui.label(RichText::new("写真").size(size::CAPTION).color(theme::MUTE));
        common::hairline(ui);
        ui.add_space(space::XS);
        ui.label(
            RichText::new("写真の読み込みは未実装です")
                .size(size::CAPTION)
                .color(theme::MUTE),
        );

        ui.add_space(space::M);
        self.summary_block(ui);
    }

    /// 地図: the track painted from latitude and longitude.
    fn map_panel(&mut self, ui: &mut Ui) {
        common::panel(ui, "地図", |ui| {
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
                        line.clone(),
                        egui::epaint::PathStroke::new(metric::TRACK_LINE, theme::DATA),
                    ));
                }
                for (index, pos) in &plotted {
                    let is_selected = self.selected == Some(*index);
                    let colour = if is_selected { theme::INK } else { theme::DATA };
                    let radius = if is_selected {
                        metric::POINT_DOT * 2.0
                    } else {
                        metric::POINT_DOT
                    };
                    painter.circle_filled(*pos, radius, colour);
                }
            }

            painter.text(
                rect.right_bottom() + Vec2::new(-space::S, -space::S),
                Align2::RIGHT_BOTTOM,
                "緯度経度から描画（地図タイルなし）",
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
                self.selected = Some(index);
            }
        });
    }

    /// 速度: one series, baseline at zero, no smoothing.
    fn speed_panel(&mut self, ui: &mut Ui) {
        common::panel(ui, "速度", |ui| {
            let (rect, _ignored) = ui.allocate_exact_size(
                Vec2::new(ui.available_width(), metric::CHART),
                Sense::hover(),
            );
            let painter = ui.painter_at(rect);
            let speeds: Vec<f32> = self
                .track
                .points
                .iter()
                .filter_map(|point| point.speed_kmh)
                .collect();
            let Some(maximum) = speeds.iter().copied().reduce(f32::max) else {
                painter.text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    "このログには速度が記録されていません",
                    FontId::proportional(size::UI),
                    theme::MUTE,
                );
                return;
            };
            // A flat series is a legal log (parked with a fix); a zero span would
            // divide by zero and paint nothing, so the chart flattens it instead.
            let span = if maximum > 0.0 { maximum } else { 1.0 };
            let baseline = rect.bottom() - space::S;
            painter.line_segment(
                [
                    egui::pos2(rect.left(), baseline),
                    egui::pos2(rect.right(), baseline),
                ],
                Stroke::new(metric::HAIRLINE, theme::RULE),
            );
            let step = rect.width() / (speeds.len().max(2) - 1) as f32;
            let path: Vec<egui::Pos2> = speeds
                .iter()
                .enumerate()
                .map(|(index, speed)| {
                    egui::pos2(
                        rect.left() + step * index as f32,
                        baseline - (speed / span) * (rect.height() - space::L),
                    )
                })
                .collect();
            if path.len() > 1 {
                painter.add(egui::Shape::line(
                    path,
                    egui::epaint::PathStroke::new(metric::TRACK_LINE, theme::DATA),
                ));
            }
            painter.text(
                rect.left_top() + Vec2::new(space::XS, space::XS),
                Align2::LEFT_TOP,
                format!("{maximum:.0} km/h"),
                FontId::monospace(size::CAPTION),
                theme::MUTE,
            );
            painter.text(
                rect.right_bottom() + Vec2::new(-space::XS, -space::XS),
                Align2::RIGHT_BOTTOM,
                "0",
                FontId::monospace(size::CAPTION),
                theme::MUTE,
            );
        });
    }

    /// 選択した点: the figures for one record, right-aligned in a ruled block.
    fn selected_panel(&mut self, ui: &mut Ui) {
        common::panel(ui, "選択した点", |ui| {
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
                    common::metres(point.height_m),
                    common::speed(point.speed_kmh)
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
    fn point_table(&mut self, ui: &mut Ui) {
        let shown = self.visible_indices();
        ui.label(
            RichText::new("点の一覧")
                .size(size::CAPTION)
                .color(theme::MUTE),
        );
        common::hairline(ui);
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
                    self.point_row(ui, index, &point);
                }
            });
    }

    fn point_row(&mut self, ui: &mut Ui, index: usize, point: &TrackPoint) {
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
            common::speed(point.speed_kmh),
            font,
            colour,
        );
        ui.painter().hline(
            rect.x_range(),
            rect.bottom(),
            Stroke::new(metric::HAIRLINE, theme::RULE),
        );
        if response.clicked() {
            self.selected = Some(index);
        }
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
                    self.format = format;
                    self.destination = format!("m241-track.{}", format.extension());
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
            let _ignored = ui.add_enabled(
                false,
                egui::Button::new(RichText::new("写真に位置").size(size::UI))
                    .corner_radius(theme::RADIUS),
            );
            ui.label(
                RichText::new("未検証")
                    .size(size::CAPTION)
                    .color(theme::MUTE),
            );
        });

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
                self.export();
            }
        });

        if let Some((status, message)) = &self.outcome {
            common::status_line(ui, *status, message);
        }
    }

    /// Render the chosen format and write it. The conversion is
    /// `m241_core::log::export`, the same function the CLI calls, so the two
    /// cannot drift into different files from the same log.
    fn export(&mut self) {
        let track = &self.track;
        if track.points.is_empty() {
            self.outcome = Some((Status::Err, String::from("書き出す点がありません")));
            return;
        }
        if self.destination.trim().is_empty() {
            self.outcome = Some((Status::Err, String::from("保存先を入力してください")));
            return;
        }
        let document = log::export(track, self.format);
        let outcome = std::fs::write(self.destination.trim(), document);
        self.outcome = Some(match outcome {
            Ok(()) => (
                Status::Ok,
                format!(
                    "{} に {} 点を書き出しました",
                    self.destination.trim(),
                    track.fix_count()
                ),
            ),
            Err(err) => (Status::Err, format!("書き出せませんでした: {err}")),
        });
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

    fn total_distance(&self) -> String {
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
        format!("{total:.1} km")
    }

    fn elapsed(&self) -> String {
        let span = self.track.time_span();
        common::duration(span.map(|(start, end)| end.saturating_sub(start)))
    }

    fn average_speed(&self) -> String {
        let Some((start, end)) = self.track.time_span() else {
            return String::from("—");
        };
        let seconds = end.saturating_sub(start);
        if seconds == 0 {
            return String::from("—");
        }
        let kilometres = self
            .total_distance()
            .trim_end_matches(" km")
            .parse::<f64>()
            .unwrap_or(0.0);
        format!("{:.1} km/h", kilometres / (f64::from(seconds) / 3_600.0))
    }
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
}
