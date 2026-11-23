//! Furniture the four screens share: the status voice, the card and the table,
//! the figure rows, the button hierarchy, the input grammar, and the formatting
//! of times and figures.
//!
//! The grammar is DESIGN.md §2's `one-seam-instrument`: hierarchy is made with a
//! fill, 20pt of inner space and a 1px edge. A rule is for a *data* boundary (a
//! table row, a figure row) and nothing else, which is why the card and the
//! button classes own a surface and the old "ruled panel with no fill" is gone.
//!
//! Why the status voice is a type rather than a habit: DESIGN.md §3 says a status
//! colour is never the only signal. If every screen chose a colour at its own call
//! site, one call site would eventually paint a red dot with no word beside it and
//! nothing would notice. [`Status::voice`] is the only place a status colour is
//! chosen, and it returns the word with it, so a caller cannot take one without
//! the other.

use egui::style::WidgetVisuals;
use egui::{Color32, RichText, Sense, Stroke, StrokeKind, Ui, Vec2};
use m241_core::TrackPoint;

use crate::theme::{self, metric, size, space};

/// A state worth saying out loud, in the product's voice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Done, verified, safe.
    Ok,
    /// Failed. The caller supplies the reason.
    Err,
    /// Neither good news nor bad: a plain statement of state.
    Info,
    /// Claimed by the design but never measured on hardware (DESIGN.md §6).
    ///
    /// Mute, never amber: an unverified feature is a fact to disclose, not a
    /// danger to announce.
    Unverified,
    /// The next move is physical — press the button, pull the battery.
    Waiting,
    /// Not done yet. Used for the items of a checklist whose other items are [`Ok`],
    /// where "状態" would read as a statement about the device rather than a to-do.
    ///
    /// [`Ok`]: Self::Ok
    Todo,
}

impl Status {
    /// The colour and the word, together.
    ///
    /// One tuple, not two functions: the pair is what makes "colour alone never
    /// carries meaning" structural instead of aspirational.
    #[must_use]
    pub fn voice(self) -> (Color32, &'static str) {
        match self {
            Self::Ok => (theme::OK, "OK"),
            Self::Err => (theme::ERR, "エラー"),
            Self::Info => (theme::MUTE, "状態"),
            Self::Unverified => (theme::MUTE, "未検証"),
            Self::Waiting => (theme::MUTE, "操作待ち"),
            Self::Todo => (theme::MUTE, "未"),
        }
    }
}

/// A screen's title and the one sentence that says why the screen exists.
///
/// The title is [`size::TITLE`] (20) — one per screen (DESIGN.md §3). It used to
/// be 24, the same 4pt band as the shell's own title, which put two headings of
/// equal weight inside 40pt of each other (DESIGN.md §10). 28 is reserved for an
/// ending screen's single line.
pub fn heading(ui: &mut Ui, title: &str, sentence: &str) {
    ui.label(
        RichText::new(title)
            .size(size::TITLE)
            .color(theme::INK)
            .strong(),
    );
    ui.add_space(space::XS);
    ui.label(RichText::new(sentence).size(size::BODY).color(theme::MUTE));
}

/// A card: a `SURFACE` fill, a 1px `LINE` edge, the one radius, 20pt inside.
///
/// This replaces the old "hairline border, no fill": a fill plus an edge plus
/// space is how a surface reads as a surface (DESIGN.md §2), and the previous
/// build had none of the three, so every block was a rectangle ruled onto the
/// page.
pub fn card(ui: &mut Ui, title: &str, body: impl FnOnce(&mut Ui)) {
    card_with_border(ui, title, theme::LINE, body);
}

/// The same card, with a caller-chosen edge colour.
///
/// The edge is a parameter rather than a second card function because exactly one
/// caller needs to emphasise it — the firmware screen's write step and its
/// undefined-device state, where amber is allowed. Passing the colour keeps the
/// amber *token* referenced only in the destructive module, which is what the
/// design gate enforces, while the frame itself stays in one place.
pub fn card_with_border(ui: &mut Ui, title: &str, border: Color32, body: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(theme::SURFACE)
        .stroke(Stroke::new(metric::HAIRLINE, border))
        .corner_radius(theme::RADIUS)
        .inner_margin(space::CARD)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(title).size(size::UI).color(theme::MUTE));
            ui.add_space(space::S);
            body(ui);
        });
}

/// A card. Kept under the old name for the screens this branch has not migrated.
///
/// New code calls [`card`]; the two are the same frame, so a screen cannot end up
/// with two card shapes by being migrated late.
pub fn panel(ui: &mut Ui, title: &str, body: impl FnOnce(&mut Ui)) {
    card(ui, title, body);
}

/// [`card`] with a caller-chosen edge colour; see [`card_with_border`].
pub fn panel_with_border(ui: &mut Ui, title: &str, border: Color32, body: impl FnOnce(&mut Ui)) {
    card_with_border(ui, title, border, body);
}

/// A card whose caption row carries a right-aligned note as well as the title.
///
/// The note is where a figure about the panel as a whole belongs — the speed
/// chart's maximum, for instance. Drawing it inside the plot put it on top of
/// the series the review had to read (DESIGN.md §3: a label never overlaps the
/// data it labels).
pub fn panel_captioned(ui: &mut Ui, title: &str, note: Option<&str>, body: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(theme::SURFACE)
        .stroke(Stroke::new(metric::HAIRLINE, theme::LINE))
        .corner_radius(theme::RADIUS)
        .inner_margin(space::CARD)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(title).size(size::UI).color(theme::MUTE));
                if let Some(note) = note {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(note).size(size::CAPTION).color(theme::FAINT));
                    });
                }
            });
            ui.add_space(space::S);
            body(ui);
        });
}

/// A card whose caption carries the word [`SAMPLE_WORD`] when the figures in it
/// are sample values.
///
/// Every card that can show sample figures uses this instead of [`card`], so the
/// word travels with the numbers: a screenshot cropped to one card still says the
/// figures were not read from a device (DESIGN.md §6, PRODUCT.md).
pub fn sample_panel(ui: &mut Ui, title: &str, sample: bool, body: impl FnOnce(&mut Ui)) {
    if sample {
        panel_captioned(ui, &format!("{title}／{SAMPLE_WORD}"), None, body);
    } else {
        card(ui, title, body);
    }
}

/// A block caption, with the sample word on the right when the block is sample.
///
/// The counterpart of [`sample_panel`] for the blocks that are not cards (the Log
/// screen's 集計 and 点の一覧): same word, same place, so the marker is one
/// convention rather than a per-screen habit.
pub fn block_title(ui: &mut Ui, title: &str, sample: bool) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).size(size::UI).color(theme::MUTE));
        if sample {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(SAMPLE_WORD)
                        .size(size::CAPTION)
                        .color(theme::FAINT),
                );
            });
        }
    });
}

/// The body of a screen: one vertical scroll area.
///
/// 1024×640 is a supported window size (DESIGN.md §8), and at that height the
/// screens' content is taller than the content area. Anything not in a scroll
/// area is *unreachable*, not merely cropped, which is what the review found.
pub fn scrolling(ui: &mut Ui, id: &str, body: impl FnOnce(&mut Ui)) {
    egui::ScrollArea::vertical()
        .id_salt(id)
        .auto_shrink([false, false])
        .show(ui, body);
}

/// A footer pinned below the scroll area: the actions a person can take without
/// scrolling.
///
/// The export bar and the wizard's 中止 / 続行 / 書き込む buttons moved here. They
/// are the reason the screen exists, so they must not be the part that falls off
/// the bottom of a small window. A 1px `LINE` separates them from the scrolling
/// body: a data boundary is the one thing a rule is for (§2), so the footer does
/// not get a fill of its own.
pub fn footer(ui: &mut Ui, id: &str, body: impl FnOnce(&mut Ui)) {
    egui::Panel::bottom(id)
        .show_separator_line(false)
        .frame(
            egui::Frame::new()
                .fill(theme::WINDOW)
                .inner_margin(egui::Margin {
                    left: 0,
                    right: 0,
                    top: space::M as i8,
                    bottom: 0,
                }),
        )
        .show(ui, |ui| {
            hairline(ui);
            ui.add_space(space::S);
            body(ui);
        });
}

/// The one word that marks a figure as not read from a device.
///
/// A constant rather than a literal per call site: the review found the word on
/// two screens' toggles and on none of the figures those toggles produced, which
/// is what happens when the marker is a sentence each screen writes for itself.
pub const SAMPLE_WORD: &str = "見本";

/// One status line: coloured swatch, the word for that colour, then the text.
///
/// The swatch exists because the colour is a scan aid; the word is the signal.
/// Remove the swatch and nothing is lost but speed; remove the word and the line
/// becomes unreadable in monochrome, which is what §3 forbids.
pub fn status_line(ui: &mut Ui, status: Status, text: &str) {
    let (colour, word) = status.voice();
    // `horizontal_wrapped`, not `horizontal`: a refusal names the path it failed
    // on, and a non-wrapping row let a long one widen the rail instead of
    // wrapping inside it (the map then lost half its width to the overflow).
    ui.horizontal_wrapped(|ui| {
        let (rect, _ignored) = ui.allocate_exact_size(Vec2::splat(space::S), Sense::hover());
        ui.painter().rect_filled(rect, theme::RADIUS, colour);
        ui.label(RichText::new(word).size(size::CAPTION).color(colour));
        ui.label(RichText::new(text).size(size::UI).color(theme::INK));
    });
}

/// A figure row: caption label on the left, tabular figure on the right, a
/// 1px `LINE` underneath.
///
/// Row height 32, the table rhythm (DESIGN.md §3). The value is always monospace
/// so `0x402F6` and `0x402F7` line up whether or not the surrounding face has
/// tabular digits. The rule is one of the two places a rule is still correct: the
/// rows are a data table, and their boundary is data, not decoration.
pub fn figure_row(ui: &mut Ui, label: &str, value: &str) {
    let (rect, _response) = figure_row_sized(ui, label, value, metric::ROW);
    ui.painter().hline(
        rect.x_range(),
        rect.bottom(),
        Stroke::new(metric::HAIRLINE, theme::LINE),
    );
}

/// A compact figure row (28), for a subordinate block.
///
/// DESIGN.md §5 makes a rail block a tighter table than the content tables.
pub fn figure_row_compact(ui: &mut Ui, label: &str, value: &str) {
    let (rect, _response) = figure_row_sized(ui, label, value, metric::ROW_COMPACT);
    ui.painter().hline(
        rect.x_range(),
        rect.bottom(),
        Stroke::new(metric::HAIRLINE, theme::LINE),
    );
}

/// The two ends of a figure row, without the rule.
///
/// For a row that carries an explanation underneath: the caller hangs the note
/// below the pair and draws one rule under both, so the note belongs to the
/// figure instead of sitting across a rule from it. Returns nothing: both call
/// sites place the note in the layout, so a `Rect` nobody reads would be an
/// invitation to guess at positions instead of using the layout.
pub fn figure_row_open(ui: &mut Ui, label: &str, value: &str) {
    let _ = figure_row_sized(ui, label, value, metric::ROW);
}

/// The label/value pair at a chosen row height.
fn figure_row_sized(
    ui: &mut Ui,
    label: &str,
    value: &str,
    height: f32,
) -> (egui::Rect, egui::Response) {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.text(
        rect.left_center(),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(size::UI),
        theme::MUTE,
    );
    painter.text(
        rect.right_center(),
        egui::Align2::RIGHT_CENTER,
        value,
        egui::FontId::monospace(size::UI),
        theme::INK,
    );
    (rect, response)
}

/// The header of a table: caption columns at the given fractions of the
/// width, one table-header high (28), with the same 1px `LINE` as a body row.
pub fn table_header(ui: &mut Ui, columns: &[(&str, f32, egui::Align)]) {
    let width = ui.available_width();
    let (rect, _response) =
        ui.allocate_exact_size(Vec2::new(width, metric::TABLE_HEADER), Sense::hover());
    let painter = ui.painter_at(rect);
    let mut left = rect.left();
    for (name, fraction, align) in columns {
        let cell = egui::Rect::from_min_size(
            egui::pos2(left, rect.top()),
            Vec2::new(width * fraction, rect.height()),
        );
        let (anchor, text_anchor) = match align {
            egui::Align::Min => (cell.left_center(), egui::Align2::LEFT_CENTER),
            egui::Align::Center => (cell.center(), egui::Align2::CENTER_CENTER),
            egui::Align::Max => (cell.right_center(), egui::Align2::RIGHT_CENTER),
        };
        painter.text(
            anchor,
            text_anchor,
            *name,
            egui::FontId::monospace(size::CAPTION),
            theme::MUTE,
        );
        left += cell.width();
    }
    painter.hline(
        rect.x_range(),
        rect.bottom(),
        Stroke::new(metric::HAIRLINE, theme::LINE),
    );
}

/// One table body row: monospace cells on the same fractions as
/// [`table_header`], a 1px `LINE` underneath.
///
/// Every cell is monospace, which is the point of the table: a column of offsets
/// is a column because the glyphs have one width (DESIGN.md §3).
pub fn table_row(ui: &mut Ui, cells: &[(&str, f32)]) {
    let width = ui.available_width();
    let (rect, _response) = ui.allocate_exact_size(Vec2::new(width, metric::ROW), Sense::hover());
    let painter = ui.painter_at(rect);
    let mut left = rect.left();
    for (text, fraction) in cells {
        let cell = egui::Rect::from_min_size(
            egui::pos2(left, rect.top()),
            Vec2::new(width * fraction, rect.height()),
        );
        painter.text(
            cell.left_center(),
            egui::Align2::LEFT_CENTER,
            *text,
            egui::FontId::monospace(size::UI),
            theme::INK,
        );
        left += cell.width();
    }
    painter.hline(
        rect.x_range(),
        rect.bottom(),
        Stroke::new(metric::HAIRLINE, theme::LINE),
    );
}

/// The design's focus ring: 2pt `ink`, inset by the hairline (DESIGN.md §6).
///
/// egui's `Button` draws its own focus outline only when it has no explicit
/// stroke of its own (the button replaces the frame style with the argument), and
/// every button in this product states its role. So the ring is drawn here, from
/// the same `Response` the widget returned, and every custom control calls it.
pub fn focus_ring(ui: &Ui, response: &egui::Response) {
    if response.has_focus() {
        ui.painter().rect_stroke(
            response.rect.shrink(metric::HAIRLINE),
            theme::RADIUS,
            Stroke::new(metric::FOCUS_RING, theme::INK),
            StrokeKind::Inside,
        );
    }
}

// ---------------------------------------------------------------------------
// Buttons: four classes, six states each.
//
// A class is a face (idle / hover / pressed fill), an ink and an edge. The face
// is written into the *scoped* widget visuals rather than into
// `Button::fill()`, because an explicit fill freezes the widget at one state and
// the previous build's buttons therefore had no hover or pressed feedback at all.
// ---------------------------------------------------------------------------

/// One button class: its three faces, its ink, its edges and its box.
#[derive(Clone, Copy)]
struct ButtonFace {
    /// The face at rest.
    idle: Color32,
    /// The face under the pointer: one step brighter than `idle`.
    hover: Color32,
    /// The face while pressed: one step darker than `idle`.
    pressed: Color32,
    /// The label colour on every face.
    ink: Color32,
    /// The edge at rest; `LINE` for a quiet control, `LINE_STRONG` for a bordered
    /// one.
    edge: Color32,
    /// The edge under the pointer.
    hover_edge: Color32,
    /// The box's height: 40 for a screen action, 36 for a tool or a tab.
    height: f32,
    /// The box's minimum width, so two actions line up.
    min_width: f32,
}

impl ButtonFace {
    /// The disabled face: a dropped fill, a `FAINT` word that still reads.
    fn disabled(self) -> Self {
        Self {
            idle: theme::WINDOW,
            hover: theme::WINDOW,
            pressed: theme::WINDOW,
            ink: theme::FAINT,
            edge: theme::LINE,
            hover_edge: theme::LINE,
            ..self
        }
    }
}

/// Write one class's state faces into the current `Ui`'s widget visuals.
///
/// Scoped: `ui.scope` in [`painted_button`] owns the mutation, so one button's
/// face cannot leak into the next widget.
fn set_button_faces(ui: &mut Ui, face: ButtonFace) {
    let visuals = &mut ui.style_mut().visuals;
    let radius = theme::RADIUS;
    visuals.widgets.inactive = WidgetVisuals {
        weak_bg_fill: face.idle,
        bg_fill: face.idle,
        bg_stroke: Stroke::new(metric::HAIRLINE, face.edge),
        fg_stroke: Stroke::new(metric::HAIRLINE, face.ink),
        corner_radius: radius,
        expansion: 0.0,
    };
    visuals.widgets.hovered = WidgetVisuals {
        weak_bg_fill: face.hover,
        bg_fill: face.hover,
        bg_stroke: Stroke::new(metric::HAIRLINE, face.hover_edge),
        fg_stroke: Stroke::new(metric::HAIRLINE, face.ink),
        corner_radius: radius,
        expansion: 0.0,
    };
    visuals.widgets.active = WidgetVisuals {
        weak_bg_fill: face.pressed,
        bg_fill: face.pressed,
        bg_stroke: Stroke::new(metric::HAIRLINE, theme::LINE_STRONG),
        fg_stroke: Stroke::new(metric::HAIRLINE, face.ink),
        corner_radius: radius,
        expansion: 0.0,
    };
    visuals.widgets.open = visuals.widgets.hovered;
}

/// Add one button of a class and paint its focus ring.
fn painted_button(ui: &mut Ui, label: &str, enabled: bool, face: ButtonFace) -> egui::Response {
    let face = if enabled { face } else { face.disabled() };
    ui.scope(|ui| {
        // A disabled control keeps its own contrast pair rather than egui's
        // `disabled_alpha`, which would dim a 7:1 pair under 4.5:1.
        ui.style_mut().visuals.disabled_alpha = 1.0;
        set_button_faces(ui, face);
        let response = ui.add_enabled(
            enabled,
            egui::Button::new(RichText::new(label).size(size::UI).color(face.ink))
                .corner_radius(theme::RADIUS)
                .min_size(Vec2::new(face.min_width, face.height)),
        );
        focus_ring(ui, &response);
        response
    })
    .inner
}

/// The primary action: an `ink` face with a `window` word, 40pt high.
///
/// This is the safe action the screen recommends. When it cannot be taken it
/// becomes an outline with a `FAINT` word rather than a dimmed bone face: dimming
/// a 16:1 pair with egui's `disabled_alpha` lands under 4.5:1, which is exactly
/// the defect the review found on the destructive button.
pub fn primary_button(ui: &mut Ui, label: &str, enabled: bool) -> egui::Response {
    painted_button(
        ui,
        label,
        enabled,
        ButtonFace {
            idle: theme::INK,
            hover: theme::INK.gamma_multiply(1.04),
            pressed: theme::INK.gamma_multiply(0.92),
            ink: theme::WINDOW,
            edge: theme::INK,
            hover_edge: theme::INK,
            height: metric::LIST_ROW,
            min_width: metric::ACTION_WIDTH,
        },
    )
}

/// A secondary action: a transparent face with a `LINE_STRONG` edge, 40pt high.
///
/// The same gesture as the primary one at a quieter weight, so 中止 / 参照 / 再読込
/// read as a class below the one action a screen recommends.
pub fn secondary_button(ui: &mut Ui, label: &str, enabled: bool) -> egui::Response {
    painted_button(
        ui,
        label,
        enabled,
        ButtonFace {
            idle: Color32::TRANSPARENT,
            hover: theme::RAISED,
            pressed: theme::SURFACE,
            ink: theme::INK,
            edge: theme::LINE_STRONG,
            hover_edge: theme::INK,
            height: metric::LIST_ROW,
            min_width: metric::ACTION_WIDTH * 0.7,
        },
    )
}

/// A compact control: a `RAISED` face with an edge, 36pt high.
///
/// The map's playback controls and the export-format chips are neither the
/// screen's primary action nor its secondary one. A tool used to be a `rule`
/// outline with no fill, which made the Log toolbar read as a row of TUI keys;
/// a small face is what makes it a control. Selected is an `OVERLAY` face with an
/// `ink` word — the same selection language as [`choice_button`], and amber is
/// never used.
pub fn tool_button(ui: &mut Ui, label: &str, selected: bool, enabled: bool) -> egui::Response {
    let (idle, ink, edge) = if selected {
        (theme::OVERLAY, theme::INK, theme::LINE_STRONG)
    } else {
        (theme::RAISED, theme::MUTE, theme::LINE)
    };
    painted_button(
        ui,
        label,
        enabled,
        ButtonFace {
            idle,
            hover: theme::OVERLAY,
            pressed: theme::SURFACE,
            ink,
            edge,
            hover_edge: theme::LINE_STRONG,
            height: metric::DOCK_ROW,
            min_width: 0.0,
        },
    )
}

/// One option of a segmented control: an `OVERLAY` face when selected, clear when
/// not, 36pt high.
///
/// Selected is a surface and an `ink` word, not a status colour: selection in this
/// product is never amber (DESIGN.md §3, §7).
///
/// This control has no disabled state on purpose: the settings choices are live
/// whenever the screen shows them, so there is nothing to gate.
pub fn choice_button(ui: &mut Ui, label: &str, selected: bool) -> egui::Response {
    let (idle, ink, edge) = if selected {
        (theme::OVERLAY, theme::INK, theme::LINE_STRONG)
    } else {
        (Color32::TRANSPARENT, theme::MUTE, theme::LINE)
    };
    painted_button(
        ui,
        label,
        true,
        ButtonFace {
            idle,
            hover: theme::RAISED,
            pressed: theme::SURFACE,
            ink,
            edge,
            hover_edge: theme::LINE_STRONG,
            height: metric::DOCK_ROW,
            min_width: metric::ACTION_WIDTH * 0.75,
        },
    )
}

// ---------------------------------------------------------------------------
// Input: one grammar.
//
// An editable field is a `SUNKEN` well with a `LINE_STRONG` edge, the one radius
// and 12×8 of inner space — everywhere, on every screen. A read-only row is a
// rule. That is the whole distinction between "you can type here" and "this is a
// label", and it is the design review's Must 5.
// ---------------------------------------------------------------------------

/// The frame an editable field wears: a `SUNKEN` well with a strong edge.
pub fn field_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(theme::SUNKEN)
        .stroke(Stroke::new(metric::HAIRLINE, theme::LINE_STRONG))
        .corner_radius(theme::RADIUS)
        .inner_margin(egui::Margin::symmetric(space::M as i8, space::S as i8))
}

/// A one-line text field in the product's one input grammar.
///
/// Returned as a builder rather than added directly so a call site can still set
/// a hint, a width or a monospace face without restating the frame.
pub fn text_field(text: &mut String) -> egui::TextEdit<'_> {
    egui::TextEdit::singleline(text)
        .font(egui::FontId::proportional(size::UI))
        .text_color(theme::INK)
        .frame(field_frame())
}

/// A labelled field: caption above, field below, full width.
///
/// The label position and the field's width are the same on every screen, which
/// is what "one grammar" means; a screen that needs a narrower field uses
/// [`text_field`] and `.desired_width()`.
pub fn labelled_field(ui: &mut Ui, label: &str, value: &mut String) -> egui::Response {
    ui.vertical(|ui| {
        ui.label(RichText::new(label).size(size::CAPTION).color(theme::MUTE));
        ui.add_space(space::XS);
        ui.add_sized(
            Vec2::new(ui.available_width(), metric::LIST_ROW),
            text_field(value),
        )
    })
    .inner
}

/// A `SUNKEN` well around read-only content, with the same edge as a field.
///
/// For the one case that is neither a field nor a table: a block of output that
/// belongs *inside* the page rather than on top of it.
pub fn well(ui: &mut Ui, body: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(theme::SUNKEN)
        .stroke(Stroke::new(metric::HAIRLINE, theme::LINE_STRONG))
        .corner_radius(theme::RADIUS)
        .inner_margin(egui::Margin::symmetric(space::M as i8, space::S as i8))
        .show(ui, body);
}

/// The 「見本」 marker, for any value that did not come from a device.
///
/// PRODUCT.md makes "say what the device is doing" a rule; sample data is the one
/// case where the honest sentence is "no device is doing anything, this is a
/// drawing of one". The firmware screen prints this under its heading; cards
/// that carry sample figures use [`sample_panel`] and [`block_title`], which put
/// the same word in their caption instead of adding a band above them.
pub fn sample_marker(ui: &mut Ui) {
    ui.label(
        RichText::new("見本（実機から読んだ値ではありません）")
            .size(size::CAPTION)
            .color(theme::MUTE),
    );
}

/// A horizontal rule that takes its space in the layout, in `LINE`.
///
/// Allocated, not painted at the cursor: a rule that reserves no height is one the
/// next element starts on top of, which looks like a border rather than a rule.
/// Use it for a data boundary; a block boundary is a move to a new surface.
pub fn hairline(ui: &mut Ui) {
    let width = ui.available_width();
    let (rect, _ignored) =
        ui.allocate_exact_size(Vec2::new(width, metric::HAIRLINE), Sense::hover());
    ui.painter().rect_filled(rect, theme::RADIUS, theme::LINE);
}

/// `2026-11-15 08:14:22 UTC`, or an em dash when the point carries no time.
///
/// Always UTC: the device records UTC, and a local-time rendering would hide the
/// very offset this product exists to repair.
#[must_use]
pub fn utc_stamp(stamp: Option<u32>) -> String {
    match stamp {
        Some(stamp) => {
            let (year, month, day, hour, minute, second) = split_utc(stamp);
            format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02} UTC")
        }
        None => String::from("—"),
    }
}

/// `11/15`, for the day filter rows.
#[must_use]
pub fn utc_date(stamp: Option<u32>) -> String {
    match stamp {
        Some(stamp) => {
            let (_year, month, day, _hour, _minute, _second) = split_utc(stamp);
            format!("{month:02}/{day:02}")
        }
        None => String::from("—"),
    }
}

/// `212,860 B`.
#[must_use]
pub fn bytes(value: usize) -> String {
    format!("{} B", grouped(value))
}

/// `42 m`, or `—` when the record had no height.
#[must_use]
pub fn metres(value: Option<f32>) -> String {
    value.map_or_else(|| String::from("—"), |metres| format!("{metres:.0} m"))
}

/// `18.4 km/h`, or `—` when the record had no speed.
#[must_use]
pub fn speed(value: Option<f32>) -> String {
    value.map_or_else(|| String::from("—"), |speed| format!("{speed:.1} km/h"))
}

/// `35.681236 139.767125`, or a phrase that says the record has no fix.
#[must_use]
pub fn coordinate(latitude: Option<f64>, longitude: Option<f64>) -> String {
    match (latitude, longitude) {
        (Some(latitude), Some(longitude)) => format!("{latitude:.6} {longitude:.6}"),
        _ => String::from("位置なし"),
    }
}

/// `2時間18分`, or `—` when the log has no timestamps at all.
#[must_use]
pub fn duration(seconds: Option<u32>) -> String {
    match seconds {
        Some(seconds) => {
            let hours = seconds / 3_600;
            let minutes = (seconds % 3_600) / 60;
            if hours > 0 {
                format!("{hours}時間{minutes:02}分")
            } else {
                format!("{minutes}分")
            }
        }
        None => String::from("—"),
    }
}

/// Great-circle distance between two records, when both carry a fix.
///
/// Haversine on a spherical Earth. The figure is shown to 0.1 km, where the
/// spherical assumption is far below the display precision, and a ellipsoidal
/// model would be claims of accuracy this device cannot support anyway.
#[must_use]
pub fn distance_km(from: &TrackPoint, to: &TrackPoint) -> Option<f64> {
    const EARTH_RADIUS_KM: f64 = 6_371.008_8;
    let (Some(from_lat), Some(from_lon)) = (from.latitude, from.longitude) else {
        return None;
    };
    let (Some(to_lat), Some(to_lon)) = (to.latitude, to.longitude) else {
        return None;
    };
    let phi_from = from_lat.to_radians();
    let phi_to = to_lat.to_radians();
    let delta_phi = (to_lat - from_lat).to_radians();
    let delta_lambda = (to_lon - from_lon).to_radians();
    let h = (delta_phi / 2.0).sin().powi(2)
        + phi_from.cos() * phi_to.cos() * (delta_lambda / 2.0).sin().powi(2);
    Some(2.0 * EARTH_RADIUS_KM * h.sqrt().asin())
}

/// Split a Unix timestamp into UTC calendar fields.
fn split_utc(stamp: u32) -> (i64, u32, u32, u32, u32, u32) {
    let seconds = i64::from(stamp);
    let days = seconds.div_euclid(86_400);
    let second_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = second_of_day / 3_600;
    let minute = (second_of_day % 3_600) / 60;
    let second = second_of_day % 60;
    (year, month, day, hour as u32, minute as u32, second as u32)
}

/// Days since 1970-01-01 to a proleptic Gregorian date.
///
/// Howard Hinnant's `civil_from_days`, integer only. A calendar crate would be a
/// dependency and a locale; this is twenty lines and deterministic, which is what
/// a timestamp needs.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
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
    let month_position = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_position + 2) / 5 + 1;
    let month = if month_position < 10 {
        month_position + 3
    } else {
        month_position - 9
    };
    let year = if month <= 2 { year + 1 } else { year };
    (year, month as u32, day as u32)
}

/// Thousands separators, because `212860` is read as `two hundred thousand` only
/// after a second look.
#[must_use]
pub fn grouped(value: usize) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (position, character) in digits.chars().enumerate() {
        let remaining = digits.len() - position;
        if position > 0 && remaining.is_multiple_of(3) {
            out.push(',');
        }
        out.push(character);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Flatten nested `Shape::Vec` trees into a list of leaf shapes.
    fn flatten<'a>(shapes: &'a [egui::Shape], out: &mut Vec<&'a egui::Shape>) {
        for shape in shapes {
            match shape {
                egui::Shape::Vec(children) => flatten(children, out),
                other => out.push(other),
            }
        }
    }

    /// Focus `draw`'s widget and report whether the 2pt ink ring reached the frame.
    fn focus_ring_is_painted(mut draw: impl FnMut(&mut Ui) -> egui::Response) -> bool {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(640.0, 400.0),
            )),
            ..egui::RawInput::default()
        };
        let mut id = None;
        for pass in 0..3 {
            let output = ctx.run_ui(input.clone(), |ui| {
                let response = draw(ui);
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
                if flat.iter().any(|shape| {
                    matches!(
                        shape,
                        egui::Shape::Rect(rect)
                            if (rect.stroke.width - metric::FOCUS_RING).abs() < f32::EPSILON
                                && rect.stroke.color == theme::INK
                    )
                }) {
                    return true;
                }
            }
            if pass == 0
                && let Some(id) = id
            {
                ctx.memory_mut(|memory| memory.request_focus(id));
            }
        }
        false
    }

    /// The rects a `draw` call painted with `fill`, in draw order.
    fn rects_with_fill(mut draw: impl FnMut(&mut Ui), fill: Color32) -> usize {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(640.0, 400.0),
            )),
            ..egui::RawInput::default()
        };
        let output = ctx.run_ui(input, |ui| draw(ui));
        let shapes: Vec<egui::Shape> = output
            .shapes
            .iter()
            .map(|clipped| clipped.shape.clone())
            .collect();
        output.drop_without_applying_deltas();
        let mut flat = Vec::new();
        flatten(&shapes, &mut flat);
        flat.iter()
            .filter(|shape| {
                matches!(
                    shape,
                    egui::Shape::Rect(rect) if rect.fill == fill && rect.rect.width() > 20.0
                )
            })
            .count()
    }

    #[test]
    fn every_custom_button_draws_the_focus_ring() {
        // The design review's Must: an explicit frame made egui's Button skip its
        // own focus outline, so every custom control painted the same whether
        // focused or not. The ring is drawn from the response; this drives each
        // button class through the real widget, gives it focus, and looks for the
        // 2pt ink rectangle in the frame.
        assert!(
            focus_ring_is_painted(|ui| primary_button(ui, "接続して確認", true)),
            "the primary button has no focus ring"
        );
        assert!(
            focus_ring_is_painted(|ui| secondary_button(ui, "中止", true)),
            "the secondary button has no focus ring"
        );
        assert!(
            focus_ring_is_painted(|ui| choice_button(ui, "オン", true)),
            "the choice button has no focus ring"
        );
        assert!(
            focus_ring_is_painted(|ui| tool_button(ui, "再生", false, true)),
            "the tool button has no focus ring"
        );
    }

    #[test]
    fn a_button_carries_a_face_not_only_a_word() {
        // The button hierarchy is a fill: the primary action is an ink face, a
        // secondary action is a transparent one with a strong edge, and a tool is
        // a raised face. A row of words with no faces was the "TUI toolbar" the
        // design review found.
        assert!(
            rects_with_fill(
                |ui| {
                    primary_button(ui, "書き込む", true);
                },
                theme::INK
            ) > 0,
            "the primary button has no ink face"
        );
        assert!(
            rects_with_fill(
                |ui| {
                    tool_button(ui, "再生", false, true);
                },
                theme::RAISED
            ) > 0,
            "an idle tool button has no raised face"
        );
        assert!(
            rects_with_fill(
                |ui| {
                    tool_button(ui, "再生", true, true);
                },
                theme::OVERLAY
            ) > 0,
            "a selected tool button has no overlay face"
        );
        assert!(
            rects_with_fill(
                |ui| {
                    choice_button(ui, "オン", true);
                },
                theme::OVERLAY
            ) > 0,
            "a selected choice has no overlay face"
        );
        assert_eq!(
            rects_with_fill(
                |ui| {
                    secondary_button(ui, "中止", true);
                },
                theme::INK
            ),
            0,
            "the secondary action grew an ink face; it is the quiet class"
        );
    }

    #[test]
    fn a_disabled_action_keeps_a_readable_word() {
        // DESIGN.md §6: disabled drops the face and keeps the word readable. The
        // previous build replaced the bone face with a dimmed one, which took the
        // label under 4.5:1. A disabled class must not paint an ink face and must
        // still draw a word (the widget is present).
        assert_eq!(
            rects_with_fill(
                |ui| {
                    primary_button(ui, "書き込む", false);
                },
                theme::INK
            ),
            0,
            "the disabled primary action kept its ink face"
        );
    }

    #[test]
    fn a_card_is_a_surface_with_an_edge_not_a_bare_rule() {
        // DESIGN.md §2: a card is a fill + a 1px edge + 20pt of inner space.
        // Counting the surface rect is the cheap way to prove the fill exists;
        // the previous `panel` painted a border on the page's own ground.
        assert!(
            rects_with_fill(
                |ui| {
                    card(ui, "ポート", |ui| {
                        ui.label("body");
                    });
                },
                theme::SURFACE
            ) > 0,
            "a card has no SURFACE fill"
        );
    }

    #[test]
    fn every_status_colour_carries_a_word() {
        // The invariant DESIGN.md §3 states, as a test: there is no status whose
        // colour arrives without a label.
        for status in [
            Status::Ok,
            Status::Err,
            Status::Info,
            Status::Unverified,
            Status::Waiting,
            Status::Todo,
        ] {
            let (_colour, word) = status.voice();
            assert!(!word.is_empty(), "{status:?} has a colour but no word");
        }
    }

    #[test]
    fn the_unverified_word_is_the_one_design_specifies() {
        // DESIGN.md §6 names the string, so a reworded status is a design change.
        let (_colour, word) = Status::Unverified.voice();
        assert_eq!(word, "未検証");
    }

    #[test]
    fn the_input_grammar_is_a_sunken_well_with_a_strong_edge() {
        // DESIGN.md §3: an editable field is SUNKEN with a LINE_STRONG edge. Both
        // halves are needed: the fill alone is 1.13:1 against the page, and the
        // edge alone would leave the field looking like a button.
        let frame = field_frame();
        assert_eq!(frame.fill, theme::SUNKEN);
        assert_eq!(frame.stroke.color, theme::LINE_STRONG);
        assert_eq!(frame.stroke.width, metric::HAIRLINE);
        assert_eq!(frame.corner_radius, theme::RADIUS);
        assert_eq!(frame.inner_margin.left, space::M as i8);
        assert_eq!(frame.inner_margin.top, space::S as i8);
    }

    #[test]
    fn a_labelled_field_puts_its_caption_above_the_well() {
        // The one grammar: caption, then the field, full width. Nine screens used
        // to place the caption themselves, which is how the field/heading
        // distinction drifted.
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(640.0, 400.0),
            )),
            ..egui::RawInput::default()
        };
        let mut value = String::from("/dev/ttyUSB0");
        let mut rect = None;
        let output = ctx.run_ui(input, |ui| {
            rect = Some(labelled_field(ui, "ポート", &mut value).rect);
        });
        output.drop_without_applying_deltas();
        assert!(
            rect.is_some_and(|rect| rect.width() > 100.0),
            "the labelled field did not lay out at the parent's width"
        );
    }

    #[test]
    fn the_epoch_is_the_first_of_january_1970() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(split_utc(0), (1970, 1, 1, 0, 0, 0));
    }

    #[test]
    fn a_timestamp_formats_in_utc() {
        // 1_700_000_000 is 2023-11-14T22:13:20Z, a figure that can be checked
        // against any `date -u` on any machine.
        assert_eq!(utc_stamp(Some(1_700_000_000)), "2023-11-14 22:13:20 UTC");
    }

    #[test]
    fn a_leap_day_survives_the_conversion() {
        // 2000-02-29T00:00:00Z. A hand-rolled calendar gets this wrong first,
        // because 2000 is a leap year for a rule (divisible by 400) that a simple
        // `year % 4` check does not cover.
        assert_eq!(utc_stamp(Some(951_782_400)), "2000-02-29 00:00:00 UTC");
    }

    #[test]
    fn figures_are_grouped_and_units_are_named() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(406_620), "406,620 B");
        assert_eq!(metres(Some(42.4)), "42 m");
        assert_eq!(metres(None), "—");
        assert_eq!(speed(Some(18.44)), "18.4 km/h");
        assert_eq!(duration(Some(8_280)), "2時間18分");
        assert_eq!(duration(Some(90)), "1分");
        assert_eq!(duration(None), "—");
    }

    #[test]
    fn a_distance_is_measured_between_two_fixes() {
        let tokyo = TrackPoint {
            latitude: Some(35.681_236),
            longitude: Some(139.767_125),
            ..TrackPoint::default()
        };
        let yokohama = TrackPoint {
            latitude: Some(35.443_708),
            longitude: Some(139.638_042),
            ..TrackPoint::default()
        };
        let distance = distance_km(&tokyo, &yokohama).unwrap_or_default();
        // ~28 km; a degree of latitude is ~111 km, so anything outside this band
        // means the trigonometry is wrong, not that the cities moved.
        assert!(
            (27.0..30.0).contains(&distance),
            "Tokyo to Yokohama measured {distance} km"
        );
    }

    #[test]
    fn a_record_without_a_fix_has_no_distance() {
        let fix = TrackPoint {
            latitude: Some(35.0),
            longitude: Some(139.0),
            ..TrackPoint::default()
        };
        assert_eq!(distance_km(&fix, &TrackPoint::default()), None);
    }
}
