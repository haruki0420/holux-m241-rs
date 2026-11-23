//! The Log screen's right-hand drawer and the plates drawn over its map.
//!
//! DESIGN.md §5 makes the map the page. The log file, the playback strip, the
//! zoom and the drawer toggle are plates laid *over* the canvas, and the panels
//! that used to push it down are tabs in a drawer that starts closed. Keeping the
//! geometry here places every plate against one set of sizes, so two of them
//! cannot drift into each other.
//!
//! Two rules keep the canvas readable on its light surface (design review
//! 2026-11-23, M1): every plate is [`theme::RAIL`] at [`PLATE_ALPHA`], which keeps
//! [`theme::INK`] above 11:1 over whatever tile is beneath it, and the map's own
//! line and markers carry a `ground` casing so they read over land and water alike
//! (the casing itself is painted by the map in `log.rs`).

use egui::{Align, Layout, Rect, Sense, Stroke, Ui, Vec2};
use m241_core::log::ParseReport;

use crate::screens::common::{self, Status};
use crate::screens::settings::AppSettings;
use crate::theme::{self, metric, size, space};

use super::{LogScreen, LogTab, sample_track};

/// Opacity of a plate over the map.
///
/// `theme::RAIL` (#111416) at 0.92 over OSM land (#F2EFE9) still composites to
/// about #23262A, so `theme::INK` on it measures above 11:1 — the same plate the
/// attribution uses. Lower alphas start letting a bright tile raise the
/// background behind the text, which is the readability defect the review found
/// in the unplated zoom note.
const PLATE_ALPHA: f32 = 0.92;

/// Height of the selected-point strip under the canvas.
///
/// One table row: the answer to a map click sits directly under the map, not in
/// a panel below the fold (design review 2026-11-23, M3).
pub(super) const STRIP_HEIGHT: f32 = metric::ROW;

/// The canvas height left for the selected-point strip, when there is one.
#[must_use]
pub(super) fn strip_height(track_is_empty: bool) -> f32 {
    if track_is_empty { 0.0 } else { STRIP_HEIGHT }
}

/// What the canvas produced that the overlays describe.
#[derive(Debug, Clone, Copy)]
pub(super) struct Canvas {
    /// The credit's plate, when OSM pixels were painted this frame.
    pub(super) attribution: Option<Rect>,
    /// Why no tiles are on the canvas, when none are.
    pub(super) note: Option<&'static str>,
}

/// Where the plates sit on a canvas.
struct Plates {
    file: Rect,
    playback: Rect,
    zoom: Rect,
}

impl Plates {
    /// Place the plates for a canvas.
    ///
    /// The heights are fixed: the plates are anchored to a corner, so letting one
    /// grow with a translated caption would move it into the plate above it. A
    /// long caption is truncated instead (the full reason stays in the panels
    /// that produced it).
    fn new(map: Rect, file_status: bool, zoom_note: bool) -> Self {
        let file_width = (map.width() * 0.55).clamp(280.0, 440.0);
        let file_height = metric::ROW + if file_status { 22.0 } else { 0.0 } + 2.0 * space::S;
        let file = Rect::from_min_size(
            map.min + Vec2::splat(space::M),
            Vec2::new(file_width, file_height),
        );

        let zoom_width = 300.0_f32.min(map.width() - 2.0 * space::M);
        let zoom_height =
            metric::ROW_COMPACT + if zoom_note { space::XS + 16.0 } else { 0.0 } + 2.0 * space::S;
        let zoom = Rect::from_min_size(
            egui::pos2(
                map.right() - space::M - zoom_width,
                map.bottom() - space::M - zoom_height,
            ),
            Vec2::new(zoom_width, zoom_height),
        );

        let playback_width = (map.width() - 2.0 * space::M).min(720.0);
        // Transport row, timeline row, and the playing point's own row (time,
        // altitude, speed). The three are one plate: the answer to "where am I
        // in the trip" belongs to the timeline, not to a caption elsewhere.
        let playback_height =
            metric::ROW_COMPACT + space::XS + 20.0 + space::XS + 20.0 + 2.0 * space::S;
        let playback = Rect::from_min_size(
            egui::pos2(
                map.center().x - playback_width / 2.0,
                zoom.top() - space::S - playback_height,
            ),
            Vec2::new(playback_width, playback_height),
        );

        Self {
            file,
            playback,
            zoom,
        }
    }

    /// The drawer toggle: the canvas's top-right corner.
    fn toggle(map: Rect) -> Rect {
        let width = 104.0;
        Rect::from_min_size(
            egui::pos2(map.right() - space::M - width, map.top() + space::M),
            Vec2::new(width, metric::ROW_COMPACT + 2.0 * space::S),
        )
    }
}

impl LogScreen {
    /// The thin row above the canvas: the title, the sentence and the sample word.
    pub(super) fn tool_row(&mut self, ui: &mut Ui) -> Rect {
        let (rect, _response) = ui.allocate_exact_size(
            Vec2::new(ui.available_width(), metric::TOOL_ROW),
            Sense::hover(),
        );
        let mut row = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect)
                .layout(Layout::left_to_right(Align::Center)),
        );
        row.label(
            egui::RichText::new("ログ")
                .size(size::TITLE)
                .color(theme::INK)
                .strong(),
        );
        row.add_space(space::M);
        row.label(
            egui::RichText::new(
                "地図が主面です。点を選ぶと下の行に時刻・座標・標高・速度が出ます。",
            )
            .size(size::CAPTION)
            .color(theme::MUTE),
        );
        if self.sample {
            row.add_space(space::M);
            common::sample_marker(&mut row);
        }
        ui.painter().hline(
            rect.x_range(),
            rect.bottom(),
            Stroke::new(metric::HAIRLINE, theme::RULE),
        );
        rect
    }

    /// Draw every plate over the canvas, in z-order.
    pub(super) fn map_overlays(
        &mut self,
        ui: &mut Ui,
        map: Rect,
        canvas: Canvas,
        _settings: &AppSettings,
    ) {
        let plates = Plates::new(map, self.load_status.is_some(), canvas.note.is_some());
        if self.track.points.is_empty() {
            // Nothing to play: the strip and the playback controls are omitted so
            // the empty state owns the canvas it is explaining.
            self.empty_overlay(ui, map);
        } else {
            let rect = overlay(ui, plates.playback, |ui| self.playback_controls(ui));
            super::layout_probe::overlay("playback", rect);
        }
        let rect = overlay(ui, plates.file, |ui| self.file_controls(ui));
        super::layout_probe::overlay("file", rect);
        let rect = overlay(ui, plates.zoom, |ui| self.zoom_controls(ui, canvas));
        super::layout_probe::overlay("zoom", rect);
        if let Some(attribution) = canvas.attribution {
            // The credit is not a plate this module owns: `tiles` paints it and
            // returns the rect so the separation can be measured.
            super::layout_probe::overlay("attribution", attribution);
        }
    }

    /// The drawer toggle, drawn last so the floating drawer cannot bury it.
    pub(super) fn drawer_toggle(&mut self, ui: &mut Ui, map: Rect) -> Rect {
        let plate = Plates::toggle(map);
        let rect = overlay(ui, plate, |ui| {
            if common::tool_button(ui, "パネル", self.drawer_open, true).clicked() {
                self.drawer_open = !self.drawer_open;
            }
        });
        super::layout_probe::overlay("toggle", rect);
        rect
    }

    /// The log file: the path, 開く, and the inline result of the last attempt.
    ///
    /// One or two lines (DESIGN.md §5): the path is the screen's secondary
    /// input, and the map is not allowed to lose more than a row to it.
    fn file_controls(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("ログファイル")
                    .size(size::CAPTION)
                    .color(theme::MUTE),
            );
            let field = (ui.available_width() - metric::ACTION_WIDTH - space::M).max(96.0);
            let _response = ui.add_sized(
                Vec2::new(field, metric::ROW),
                egui::TextEdit::singleline(&mut self.log_path)
                    .hint_text(egui::RichText::new("ログファイルのパス").size(size::CAPTION))
                    .font(egui::TextStyle::Monospace),
            );
            if common::primary_button(ui, "開く", true).clicked() {
                let target = std::path::PathBuf::from(self.log_path.trim());
                let _res = self.load_bin_file(&target);
            }
        });
        if let Some((status, message)) = &self.load_status {
            status_plate_line(ui, *status, message);
        }
    }

    /// The zoom: `−` / `＋`, the level actually drawn, and the map's state note.
    ///
    /// The note shares this plate instead of being painted straight onto the tile:
    /// `mute` on OSM land measured 2.3:1, which is the one caption on the screen a
    /// reader could not see (design review 2026-11-23, M1).
    fn zoom_controls(&mut self, ui: &mut Ui, canvas: Canvas) {
        ui.horizontal(|ui| {
            if common::tool_button(ui, "−", false, true).clicked() {
                self.set_zoom(self.map_zoom - 1.0);
            }
            if common::tool_button(ui, "＋", false, true).clicked() {
                self.set_zoom(self.map_zoom + 1.0);
            }
            ui.add_space(space::S);
            let follow = if self.follow { "追従 " } else { "" };
            ui.label(
                egui::RichText::new(format!("{follow}zoom {:.0}", self.map_zoom))
                    .size(size::CAPTION)
                    .color(theme::INK),
            );
        });
        if let Some(note) = canvas.note {
            ui.label(
                egui::RichText::new(note)
                    .size(size::CAPTION)
                    .color(theme::INK),
            );
        }
    }

    /// The empty log: what is missing and the two ways to fill it.
    ///
    /// The download console moved into the drawer, so this plate names the tab
    /// that holds it rather than drawing a second copy (DESIGN.md §6).
    fn empty_overlay(&mut self, ui: &mut Ui, map: Rect) {
        let width = (map.width() - 2.0 * space::XL).clamp(280.0, 520.0);
        let plate = Rect::from_center_size(map.center(), Vec2::new(width, 136.0));
        let rect = overlay(ui, plate, |ui| {
            common::status_line(ui, Status::Info, "まだログがありません");
            ui.add_space(space::XS);
            ui.label(
                egui::RichText::new(
                    "保存済みの .bin / .gpx ログを開くか、端末をつないでログを吸い出してください。",
                )
                .size(size::UI)
                .color(theme::INK),
            );
            ui.add_space(space::S);
            ui.horizontal(|ui| {
                if common::secondary_button(ui, "ログ操作を開く", true).clicked() {
                    self.open_drawer(LogTab::Ops);
                }
                let mut sample = self.sample;
                if ui
                    .checkbox(
                        &mut sample,
                        egui::RichText::new("見本（実機から読んだ値ではありません）")
                            .size(size::CAPTION),
                    )
                    .changed()
                {
                    self.sample = sample;
                }
            });
            // Only when the toggle is on and nothing has been loaded: re-seeding
            // every frame would throw away the row the user just selected.
            if self.sample && self.track.points.is_empty() {
                self.set_track(sample_track(), ParseReport::default());
            }
        });
        super::layout_probe::overlay("empty", rect);
    }

    /// The selected point, as one line under the canvas (DESIGN.md §5).
    ///
    /// The canvas is a picker, so the answer to a click lives directly under it:
    /// UTC, coordinate in the chosen format, altitude and speed, in one monospace
    /// row that does not wrap into a panel below the fold.
    pub(super) fn selection_strip(&self, ui: &Ui, rect: Rect, settings: &AppSettings) {
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, theme::RADIUS, theme::GROUND);
        painter.hline(
            rect.x_range(),
            rect.top(),
            Stroke::new(metric::HAIRLINE, theme::RULE),
        );
        let text = self
            .selected
            .and_then(|index| self.track.points.get(index))
            .map_or_else(
                || String::from("選択した点  —  地図の点をクリックしてください"),
                |point| {
                    let waypoint = if point.is_waypoint() {
                        "  本体のボタンで付けた目印"
                    } else {
                        ""
                    };
                    format!(
                        "選択した点  {}  {}  標高 {}  速度 {}{waypoint}",
                        common::utc_stamp(point.utc),
                        super::coordinate_text(point, settings),
                        settings.units.format_altitude(point.height_m),
                        settings.units.format_speed(point.speed_kmh),
                    )
                },
            );
        painter.text(
            rect.left_center() + Vec2::new(space::L, 0.0),
            egui::Align2::LEFT_CENTER,
            text,
            egui::FontId::monospace(size::CAPTION),
            theme::INK,
        );
    }

    /// The drawer's contents: five tabs and the panel the selected one holds.
    pub(super) fn drawer_contents(&mut self, ui: &mut Ui, settings: &AppSettings) {
        // Five tabs share the drawer's 332pt inner width. The theme's 12pt
        // button padding leaves the strip a couple of points short, and a
        // `horizontal_wrapped` cannot break between scoped buttons, so the last
        // tab was squeezed into vertical text. The strip uses the smaller step.
        ui.scope(|ui| {
            ui.spacing_mut().button_padding.x = space::S;
            ui.horizontal_wrapped(|ui| {
                for tab in LogTab::ALL {
                    if common::tool_button(ui, tab.label(), self.drawer_tab == tab, true).clicked()
                    {
                        self.drawer_tab = tab;
                    }
                }
            });
        });
        common::hairline(ui);
        ui.add_space(space::S);
        let tab = self.drawer_tab;
        egui::ScrollArea::vertical()
            .id_salt("m241-log-drawer")
            .auto_shrink([false, false])
            .show(ui, |ui| match tab {
                LogTab::Summary => {
                    self.track_days_block(ui);
                    ui.add_space(space::M);
                    self.summary_block(ui, settings);
                    ui.add_space(space::M);
                    self.speed_panel(ui, settings);
                    ui.add_space(space::M);
                    self.point_table(ui, settings);
                }
                LogTab::Photos => self.photo_rail_section(ui),
                LogTab::Edit => self.edit_section(ui),
                LogTab::Export => self.export_bar(ui),
                LogTab::Ops => self.ops_ui(ui),
            });
    }

    /// The drawer as a column beside the canvas (a window at or above `NARROW`).
    pub(super) fn drawer_column(&mut self, ui: &mut Ui, settings: &AppSettings) {
        let panel = egui::Panel::right("m241-log-drawer")
            .exact_size(metric::DRAWER)
            .show_separator_line(false)
            .frame(
                egui::Frame::new()
                    .fill(theme::GROUND)
                    .inner_margin(egui::Margin {
                        left: space::L as i8,
                        right: space::M as i8,
                        top: space::M as i8,
                        bottom: 0,
                    }),
            )
            .show(ui, |ui| self.drawer_contents(ui, settings));
        // The panel only moves the parent's cursor; it leaves the parent's
        // `max_rect` at the full content width, so recording that rect would
        // report the drawer as wide as the page. The panel's own rect is the
        // drawer.
        super::layout_probe::drawer(panel.response.rect);
    }

    /// The drawer laid over the map (a window below `NARROW`).
    ///
    /// 360pt beside an 800pt content area would leave the map a strip, so the
    /// drawer floats instead and the map keeps its size (DESIGN.md §9).
    pub(super) fn drawer_plate(&mut self, ui: &mut Ui, rect: Rect, settings: &AppSettings) {
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect)
                .layout(Layout::top_down(Align::LEFT)),
        );
        egui::Frame::new()
            .fill(theme::RAIL.gamma_multiply(PLATE_ALPHA))
            .stroke(Stroke::new(metric::HAIRLINE, theme::RULE))
            .corner_radius(theme::RADIUS)
            .inner_margin(space::L)
            .show(&mut child, |ui| self.drawer_contents(ui, settings));
        super::layout_probe::drawer(rect);
    }
}

/// A plate over the canvas.
///
/// `theme::RAIL` fill, `rule` hairline, the product's one corner radius, and a
/// fixed size: the caller computes the plate, so two anchored plates cannot grow
/// into each other when a caption is translated. Returns the rect the frame used
/// so a test can hold the no-overlap promise without reading pixels.
fn overlay(ui: &mut Ui, plate: Rect, body: impl FnOnce(&mut Ui)) -> Rect {
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(plate)
            .layout(Layout::top_down(Align::LEFT)),
    );
    let clip = plate.intersect(ui.clip_rect());
    child.set_clip_rect(clip);
    egui::Frame::new()
        .fill(theme::RAIL.gamma_multiply(PLATE_ALPHA))
        .stroke(Stroke::new(metric::HAIRLINE, theme::RULE))
        .corner_radius(theme::RADIUS)
        .inner_margin(space::S)
        .show(&mut child, |ui| {
            ui.set_width((plate.width() - 2.0 * space::S).max(1.0));
            ui.set_height((plate.height() - 2.0 * space::S).max(1.0));
            body(ui);
        })
        .response
        .rect
}

/// One status line inside a plate: colour swatch, the word for the colour, then
/// the message on one truncated line.
///
/// `common::status_line` wraps, and a wrapped refusal made the file plate grow
/// past the row the layout reserved for it. The words stay (DESIGN.md §3: colour
/// is never the only signal); only the message is cut, and the panel that
/// produced it keeps the full sentence.
fn status_plate_line(ui: &mut Ui, status: Status, text: &str) {
    let (colour, word) = status.voice();
    ui.horizontal(|ui| {
        let (rect, _response) = ui.allocate_exact_size(Vec2::splat(space::S), Sense::hover());
        ui.painter().rect_filled(rect, theme::RADIUS, colour);
        ui.label(egui::RichText::new(word).size(size::CAPTION).color(colour));
        ui.add(
            egui::Label::new(egui::RichText::new(text).size(size::UI).color(theme::INK)).truncate(),
        );
    });
}
