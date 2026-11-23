//! The Log screen's right-hand drawer and the plates drawn over its map.
//!
//! DESIGN.md §5 makes the map the page. The log file, the playback strip, the
//! zoom and the drawer toggle are plates laid *over* the canvas, and the panels
//! that used to push it down are tabs in a drawer that starts closed. Keeping the
//! geometry here places every plate against one set of sizes, so two of them
//! cannot drift into each other.
//!
//! The OSM credit is laid out here too: `tiles` paints it, but its width is the
//! canvas left of the zoom plate, so the two are measured against the same
//! numbers instead of each guessing where the other sits (design review
//! 2026-11-23, Must 1).
//!
//! Two rules keep the canvas readable on its light surface (design review
//! 2026-11-23, M1): every plate is [`theme::OVERLAY`] — opaque, so the tile can
//! never raise the background behind [`theme::INK`] — with a 1px [`theme::LINE`]
//! edge and the product's one [`theme::POPUP_SHADOW`], because a plate is a
//! floating layer (DESIGN.md §3); and the map's own line and markers carry a
//! `window` casing so they read over land and water alike (the casing itself is
//! painted by the map in `log.rs`).

use egui::{Align, Layout, Pos2, Rect, Sense, Stroke, Ui, Vec2};
use m241_core::log::ParseReport;

use crate::screens::common::{self, Status};
use crate::screens::settings::AppSettings;
use crate::theme::{self, metric, size, space};

use super::{LogScreen, LogTab, sample_track};

/// The narrowest the credit is wrapped to before it moves above the zoom plate.
///
/// Below this a wrapped URL is all break and no word; a canvas that narrow is
/// outside the documented sizes, so the fallback placement exists only so the
/// two plates still cannot intersect.
const MIN_CREDIT_WIDTH: f32 = 96.0;

/// The shortest a floating drawer is allowed to become.
///
/// Its bottom is lifted above the playback plate so the map's controls stay
/// readable while the drawer is open. A canvas with a track is always tall
/// enough for this at the documented sizes; the floor keeps an unusually short
/// window from collapsing the panel to nothing.
const MIN_FLOAT_HEIGHT: f32 = 160.0;

/// Height of the selected-point strip under the canvas.
///
/// [`metric::SELECTION_STRIP`] (40): the answer to a map click sits directly
/// under the map, not in a panel below the fold (design review 2026-11-23, M3).
/// The strip is one line; the text is truncated with an ellipsis rather than cut
/// at the canvas edge, so a long waypoint suffix still reads as truncated.
const STRIP_HEIGHT: f32 = metric::SELECTION_STRIP;

/// The height the selected-point strip takes from the canvas, when it has one.
#[must_use]
pub(super) fn strip_height(track_is_empty: bool) -> f32 {
    if track_is_empty { 0.0 } else { STRIP_HEIGHT }
}

/// What the canvas produced that the overlays describe.
#[derive(Debug, Clone, Copy)]
pub(super) struct Canvas {
    /// OSM tiles painted this frame.
    ///
    /// The credit is drawn only when this is not zero: crediting a sample or
    /// empty frame is a false statement about where the pixels came from.
    pub(super) tiles: usize,
    /// Why no tiles are on the canvas, when none are.
    pub(super) note: Option<&'static str>,
}

/// Where the plates sit on a canvas.
pub(super) struct Plates {
    pub(super) file: Rect,
    pub(super) playback: Rect,
    pub(super) zoom: Rect,
}

impl Plates {
    /// Place the plates for a canvas.
    ///
    /// The heights are fixed except for the file plate's status line: the plates
    /// are anchored to a corner, so letting one grow with a translated caption
    /// would move it into the plate above it. The status line is measured before
    /// this is called ([`status_height`]) and wraps inside the plate, so a long
    /// refusal is readable in the plate rather than cut at one line.
    ///
    /// Every height is a row of [`metric::DOCK_ROW`] plus the frame's own
    /// [`space::S`] margin and 1px edge, because a plate is a `Frame` and this
    /// function has to predict the rect the frame will paint:
    ///
    /// - file: one `LIST_ROW` action row (the 40pt primary button) + status;
    /// - zoom: one `DOCK_ROW` of tool buttons + the map-state note;
    /// - playback: two `DOCK_ROW`s (transport, then timeline and figures)
    ///   separated by one `space::S`.
    pub(super) fn new(map: Rect, file_status_height: f32, zoom_note_height: f32) -> Self {
        let file_width = (map.width() * 0.55).clamp(280.0, 440.0);
        let file_status = if file_status_height > 0.0 {
            space::S + file_status_height
        } else {
            0.0
        };
        let file_height = metric::LIST_ROW + file_status + 2.0 * space::S + 2.0 * metric::HAIRLINE;
        let file = Rect::from_min_size(
            map.min + Vec2::splat(space::M),
            Vec2::new(file_width, file_height),
        );

        let zoom_width = 300.0_f32.min(map.width() - 2.0 * space::M);
        // The control row is one 36pt tool button high; the map-state note is one
        // measured `CAPTION` line below it. `zoom_note_height` is measured by the
        // caller with the same font the label will use, because the OS face's
        // line height is not `size × 1.2` and guessing it made the plate two
        // points shorter than its frame. The frame's 8pt margin and 1px edge are
        // part of the plate's own height too: `overlay` returns the *frame* rect,
        // not the content rect.
        let zoom_note = if zoom_note_height > 0.0 {
            space::S + zoom_note_height
        } else {
            0.0
        };
        let zoom_height = metric::DOCK_ROW + zoom_note + 2.0 * space::S + 2.0 * metric::HAIRLINE;
        let zoom = Rect::from_min_size(
            egui::pos2(
                map.right() - space::M - zoom_width,
                map.bottom() - space::M - zoom_height,
            ),
            Vec2::new(zoom_width, zoom_height),
        );

        let playback_width = (map.width() - 2.0 * space::M).min(720.0);
        // Two rows, both `DOCK_ROW`: the transport (play / 前 / 次 / 全点表示 /
        // 追従) and the timeline (slider, n/N, the point's time · speed ·
        // altitude), separated by one `space::S`. The plate is deliberately two
        // rows and not three: the selected point's figures used to be the reason
        // for a third row, and they now sit on the second row with the slider so
        // the plate covers less of the map.
        let playback_height =
            2.0 * metric::DOCK_ROW + space::S + 2.0 * space::S + 2.0 * metric::HAIRLINE;
        let playback = Rect::from_min_size(
            egui::pos2(
                map.center().x - playback_width / 2.0,
                // The plates keep at least 12pt between them: a plate is a
                // floating layer, and two floating layers that touch read as one
                // badly divided panel.
                zoom.top() - space::M - playback_height,
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
            Vec2::new(
                width,
                metric::DOCK_ROW + 2.0 * space::S + 2.0 * metric::HAIRLINE,
            ),
        )
    }

    /// The area the OSM credit is laid out in.
    ///
    /// The credit owns the canvas's bottom-left corner and the zoom plate the
    /// bottom-right one. The area stops one `space::M` (12pt) short of the zoom
    /// plate so the plate `tiles` draws from it (the text plus its `space::XS`
    /// margin) keeps at least 12pt of canvas between the credit and the zoom
    /// controls at any canvas width — 8pt read as touching at the width where the
    /// one-line credit just fits (Stage 3, 1264pt). When the canvas is too narrow
    /// to wrap the text into (outside the documented sizes) the credit is placed
    /// above the zoom plate instead, which keeps the two apart vertically.
    pub(super) fn credit(&self, map: Rect) -> Rect {
        let left = map.left() + space::S;
        let right = self.zoom.left() - space::M;
        let width = right - left - space::XS;
        if width >= MIN_CREDIT_WIDTH {
            Rect::from_min_max(
                Pos2::new(left, map.top() + space::M),
                Pos2::new(left + width, map.bottom() - space::S - space::XS),
            )
        } else {
            Rect::from_min_max(
                Pos2::new(left, map.top() + space::M),
                Pos2::new(map.right() - space::S, self.zoom.top() - space::S),
            )
        }
    }

    /// The drawer when it floats over the canvas.
    ///
    /// Right-hand side, starting below the file plate and stopping above the
    /// playback plate: a floating drawer at 0.92 alpha over the zoom and playback
    /// controls made them hard to read (design review 2026-11-23, Should S1). The
    /// toggle is still painted last, so it is never buried.
    pub(super) fn floating(&self, map: Rect, track_is_empty: bool) -> Rect {
        let top = self.file.bottom() + space::M;
        let upper = if track_is_empty {
            self.zoom.top()
        } else {
            self.playback.top()
        } - space::S;
        let bottom = upper
            .max(top + MIN_FLOAT_HEIGHT)
            .min(map.bottom() - space::M);
        Rect::from_min_max(
            Pos2::new(map.right() - space::M - metric::DRAWER, top),
            Pos2::new(map.right() - space::M, bottom),
        )
    }
}

/// The height a status line needs at `plate_width`, so the file plate can grow
/// before the line is drawn.
///
/// The line is a `horizontal_wrapped` row — swatch, the word for the colour, then
/// the message — and the message wraps to what is left. Measuring with the same
/// font and the same width the label will use is what lets the plate hold the
/// whole sentence; `Label::truncate` used to cut it to one line with the full text
/// reachable only by hovering (design review 2026-11-23, Must M5).
pub(super) fn status_height(ui: &Ui, status: Status, text: &str, plate_width: f32) -> f32 {
    let (colour, word) = status.voice();
    let word_width = ui
        .painter()
        .layout_no_wrap(
            word.to_owned(),
            egui::FontId::proportional(size::CAPTION),
            colour,
        )
        .rect
        .width();
    let inner = plate_width - 2.0 * space::S;
    let remaining = (inner - 2.0 * space::S - word_width - space::S).max(64.0);
    ui.painter()
        .layout(
            text.to_owned(),
            egui::FontId::proportional(size::UI),
            theme::INK,
            remaining,
        )
        .rect
        .height()
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
        // The screen title is the design's `title` step (20), one per screen
        // (DESIGN.md §3). It used to be the 24pt display step; 28 is now reserved
        // for an ending screen, so a screen title asks for `TITLE` like the other
        // three screens' headings.
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
            Stroke::new(metric::HAIRLINE, theme::LINE),
        );
        rect
    }

    /// Draw every plate over the canvas, in z-order.
    ///
    /// The credit is painted here rather than by `map_canvas`: it is drawn after
    /// the track and the markers, and its area comes from the same [`Plates`] the
    /// other overlays use, so it cannot be placed into the zoom plate.
    pub(super) fn map_overlays(
        &mut self,
        ui: &mut Ui,
        map: Rect,
        canvas: Canvas,
        plates: &Plates,
        floating: Option<Rect>,
        settings: &AppSettings,
    ) {
        if self.track.points.is_empty() {
            // Nothing to play: the strip and the playback controls are omitted so
            // the empty state owns the canvas it is explaining.
            self.empty_overlay(ui, map, floating);
        } else {
            let rect = overlay(ui, plates.playback, |ui| {
                self.playback_controls(ui, settings);
            });
            super::layout_probe::overlay("playback", rect);
        }
        let rect = overlay(ui, plates.file, |ui| self.file_controls(ui));
        super::layout_probe::overlay("file", rect);
        let rect = overlay(ui, plates.zoom, |ui| self.zoom_controls(ui, canvas));
        super::layout_probe::overlay("zoom", rect);
        if canvas.tiles > 0 {
            let credit = super::tiles::draw_attribution(&ui.painter_at(map), plates.credit(map));
            super::layout_probe::overlay("attribution", credit);
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
    /// input, and the map is not allowed to lose more than a row to it. A refusal
    /// adds the wrapped sentence it needs; the plate grew by the measured height
    /// of that line before it was drawn ([`status_height`]).
    fn file_controls(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("ログファイル")
                    .size(size::CAPTION)
                    .color(theme::MUTE),
            );
            let field = (ui.available_width() - metric::ACTION_WIDTH - space::M).max(96.0);
            let _response = ui.add_sized(
                Vec2::new(field, metric::LIST_ROW),
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
            // The shared status line wraps, and `Plates` grew this plate by the
            // measured height of that wrap, so the whole refusal is drawn.
            common::status_line(ui, *status, message);
        }
    }

    /// The zoom: `−` / `＋`, the level actually drawn, and the map's state note.
    ///
    /// The note shares this plate instead of being painted straight onto the tile:
    /// the caption has to sit on a plate to clear the contrast the rest of the
    /// product holds itself to (design review 2026-11-23, M1).
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
    /// that holds it rather than drawing a second copy (DESIGN.md §6). When the
    /// drawer floats over the canvas it takes the right-hand side, so the plate
    /// is centred in the width that is left; otherwise it would sit under the
    /// panel and the primary action would be unreadable (design review
    /// 2026-11-23, Should S1).
    fn empty_overlay(&mut self, ui: &mut Ui, map: Rect, floating: Option<Rect>) {
        let free = floating.map_or(map, |drawer| {
            Rect::from_min_max(map.min, egui::pos2(drawer.left() - space::M, map.bottom()))
        });
        let width = (free.width() - 2.0 * space::XL).clamp(280.0, 520.0);
        let plate = Rect::from_center_size(free.center(), Vec2::new(width, 136.0));
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
            // `horizontal_wrapped`, not `horizontal`: the action and its checkbox
            // are wider than this plate on a narrow canvas now that the caption is
            // 12pt and the button 14pt. A non-wrapping row grew the frame past the
            // plate and under the floating drawer (the overlap the layout test
            // caught); wrapping keeps the plate inside the width it reserved.
            ui.horizontal_wrapped(|ui| {
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
    /// row that does not wrap into a panel below the fold. A waypoint suffix can
    /// make the row wider than a narrow canvas, so the label truncates with an
    /// ellipsis instead of being cut at the sheet's edge — a cut reads as a
    /// rendering bug, an ellipsis reads as more text.
    pub(super) fn selection_strip(&self, ui: &mut Ui, rect: Rect, settings: &AppSettings) {
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, theme::RADIUS, theme::SURFACE);
        painter.hline(
            rect.x_range(),
            rect.top(),
            Stroke::new(metric::HAIRLINE, theme::LINE),
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
        let mut strip = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_max(
                    egui::pos2(rect.left() + space::L, rect.top()),
                    egui::pos2(rect.right() - space::L, rect.bottom()),
                ))
                .layout(Layout::left_to_right(Align::Center)),
        );
        let _label = strip.add(
            egui::Label::new(
                egui::RichText::new(text)
                    .size(size::CAPTION)
                    .color(theme::INK)
                    .monospace(),
            )
            .truncate(),
        );
    }

    /// The drawer's contents: five tabs and the panel the selected one holds.
    pub(super) fn drawer_contents(&mut self, ui: &mut Ui, settings: &AppSettings) {
        // Five tabs share the drawer's inner width: 332pt beside the canvas
        // (two `space::L` margins) and 328pt when the drawer floats (two
        // `space::S` margins). The theme's 12pt button padding leaves the strip a
        // couple of points short, and a `horizontal_wrapped` cannot break between
        // scoped buttons, so the last tab was squeezed into vertical text. The
        // strip uses the smaller step.
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

    /// The drawer as a column beside the canvas (a content width at or above
    /// `NARROW`).
    pub(super) fn drawer_column(&mut self, ui: &mut Ui, settings: &AppSettings) {
        let panel = egui::Panel::right("m241-log-drawer")
            .exact_size(metric::DRAWER)
            .show_separator_line(false)
            .frame(
                egui::Frame::new()
                    .fill(theme::RAISED)
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

    /// The drawer laid over the map (a content width below `NARROW`).
    ///
    /// 360pt beside a sub-1000pt content area would leave the map a strip, so the
    /// drawer floats instead and the map keeps its size (DESIGN.md §9). The rect
    /// comes from [`Plates::floating`] so the floating drawer is placed against
    /// the same plates as everything else.
    pub(super) fn drawer_plate(&mut self, ui: &mut Ui, rect: Rect, settings: &AppSettings) {
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect)
                .layout(Layout::top_down(Align::LEFT)),
        );
        egui::Frame::new()
            .fill(theme::OVERLAY)
            .stroke(Stroke::new(metric::HAIRLINE, theme::LINE))
            .corner_radius(theme::RADIUS)
            .shadow(theme::POPUP_SHADOW)
            .inner_margin(space::L)
            .show(&mut child, |ui| self.drawer_contents(ui, settings));
        super::layout_probe::drawer(rect);
    }
}

/// A plate over the canvas.
///
/// `theme::OVERLAY` fill, `line` hairline, the product's one corner radius and
/// the one shadow: a plate is a floating layer (DESIGN.md §3), which is the only
/// elevation the design allows besides a fill and a 1px edge. Each plate has a
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
        .fill(theme::OVERLAY)
        .stroke(Stroke::new(metric::HAIRLINE, theme::LINE))
        .corner_radius(theme::RADIUS)
        .shadow(theme::POPUP_SHADOW)
        .inner_margin(space::S)
        .show(&mut child, |ui| {
            ui.set_width((plate.width() - 2.0 * space::S).max(1.0));
            body(ui);
        })
        .response
        .rect
}
