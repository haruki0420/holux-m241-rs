//! Furniture the four screens share: the status voice, the ruled panel and
//! table, the figure rows, the button hierarchy, and the formatting of times and
//! figures.
//!
//! Why the status voice is a type rather than a habit: DESIGN.md §3 says a status
//! colour is never the only signal. If every screen chose a colour at its own call
//! site, one call site would eventually paint a red dot with no word beside it and
//! nothing would notice. [`Status::voice`] is the only place a status colour is
//! chosen, and it returns the word with it, so a caller cannot take one without
//! the other.

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
/// `display` is 24 and appears once per screen (DESIGN.md §3), which is why this
/// is a function rather than two labels a screen could get subtly wrong.
pub fn heading(ui: &mut Ui, title: &str, sentence: &str) {
    ui.label(
        RichText::new(title)
            .size(size::DISPLAY)
            .color(theme::INK)
            .strong(),
    );
    ui.add_space(space::XS);
    ui.label(RichText::new(sentence).size(size::BODY).color(theme::MUTE));
}

/// A ruled panel: hairline border, no fill, no shadow (DESIGN.md §3, §7).
///
/// Elevation is a `rule` hairline declared once. A card with a drop shadow would
/// be a second elevation language, and the manual has no shadows to lend.
pub fn panel(ui: &mut Ui, title: &str, body: impl FnOnce(&mut Ui)) {
    panel_with_border(ui, title, theme::RULE, body);
}

/// The same panel, with a caller-chosen border colour.
///
/// The border is a parameter rather than a second panel function because exactly
/// one caller needs to emphasise the border — the firmware screen's write step and
/// its undefined-device state, where amber is allowed. Passing the colour keeps
/// the amber *token* referenced only in the destructive module, which is what the
/// design gate enforces, while the frame itself stays in one place.
pub fn panel_with_border(ui: &mut Ui, title: &str, border: Color32, body: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(theme::GROUND)
        .stroke(Stroke::new(metric::HAIRLINE, border))
        .corner_radius(theme::RADIUS)
        .inner_margin(space::L)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(title).size(size::CAPTION).color(theme::MUTE));
            ui.add_space(space::S);
            body(ui);
        });
}

/// The body of a screen: one vertical scroll area.
///
/// 1024×640 is a supported window size (DESIGN.md §8), and at that height the
/// screens' content is taller than the content area. Anything not in a scroll
/// area is *unreachable*, not merely cropped, which is what the review found:
/// the log's last row missing at y=639, the firmware's checklist cut in half,
/// the Connect screen's "next step" block ending at its first line.
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
/// the bottom of a small window. A hairline separates them from the scrolling
/// body, in place of a fill or a shadow (§3: elevation is a rule).
pub fn footer(ui: &mut Ui, id: &str, body: impl FnOnce(&mut Ui)) {
    egui::Panel::bottom(id)
        .show_separator_line(false)
        .frame(
            egui::Frame::new()
                .fill(theme::GROUND)
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

/// A panel whose caption row carries a right-aligned note as well as the title.
///
/// The note is where a figure about the panel as a whole belongs — the speed
/// chart's maximum, for instance. Drawing it inside the plot put it on top of
/// the series the review had to read (DESIGN.md §3: a label never overlaps the
/// data it labels).
pub fn panel_captioned(ui: &mut Ui, title: &str, note: Option<&str>, body: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(theme::GROUND)
        .stroke(Stroke::new(metric::HAIRLINE, theme::RULE))
        .corner_radius(theme::RADIUS)
        .inner_margin(space::L)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(title).size(size::CAPTION).color(theme::MUTE));
                if let Some(note) = note {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(note).size(size::CAPTION).color(theme::MUTE));
                    });
                }
            });
            ui.add_space(space::S);
            body(ui);
        });
}

/// A panel whose caption carries the word [`SAMPLE_WORD`] when the figures in it
/// are sample values.
///
/// Every panel that can show sample figures uses this instead of [`panel`], so
/// the word travels with the numbers: a screenshot cropped to one panel still
/// says the figures were not read from a device (DESIGN.md §6, PRODUCT.md). The
/// review found the word only on the toggles at the bottom of two screens — far
/// from the numbers, and absent altogether on the Log screen.
pub fn sample_panel(ui: &mut Ui, title: &str, sample: bool, body: impl FnOnce(&mut Ui)) {
    if sample {
        panel_captioned(ui, &format!("{title}／{SAMPLE_WORD}"), None, body);
    } else {
        panel(ui, title, body);
    }
}

/// A block caption, with the sample word on the right when the block is sample.
///
/// The counterpart of [`sample_panel`] for the ruled blocks that are not panels
/// (the Log screen's 集計 and 点の一覧): same word, same place, so the marker is
/// one convention rather than a per-screen habit.
pub fn block_title(ui: &mut Ui, title: &str, sample: bool) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).size(size::CAPTION).color(theme::MUTE));
        if sample {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(SAMPLE_WORD)
                        .size(size::CAPTION)
                        .color(theme::MUTE),
                );
            });
        }
    });
}

/// One status line: coloured swatch, the word for that colour, then the text.
///
/// The swatch exists because the colour is a scan aid; the word is the signal.
/// Remove the swatch and nothing is lost but speed; remove the word and the line
/// becomes unreadable in monochrome, which is what §3 forbids.
pub fn status_line(ui: &mut Ui, status: Status, text: &str) {
    let (colour, word) = status.voice();
    ui.horizontal(|ui| {
        let (rect, _ignored) = ui.allocate_exact_size(Vec2::splat(space::S), Sense::hover());
        ui.painter().rect_filled(rect, theme::RADIUS, colour);
        ui.label(RichText::new(word).size(size::CAPTION).color(colour));
        ui.label(RichText::new(text).size(size::UI).color(theme::INK));
    });
}

/// A ruled figure row: caption label on the left, tabular figure on the right,
/// a hairline underneath.
///
/// Row height 32, one step of the reference-manual rhythm (DESIGN.md §3). The
/// value is always monospace so `0x402F6` and `0x402F7` line up whether or not
/// the surrounding face has tabular digits.
pub fn figure_row(ui: &mut Ui, label: &str, value: &str) {
    let (rect, _response) = figure_row_sized(ui, label, value, metric::ROW);
    ui.painter().hline(
        rect.x_range(),
        rect.bottom(),
        Stroke::new(metric::HAIRLINE, theme::RULE),
    );
}

/// A compact figure row (28), for a subordinate rail block.
///
/// DESIGN.md §5 makes the Log screen's left column a rail block rather than a
/// second table: its rows are one step tighter than the content tables.
pub fn figure_row_compact(ui: &mut Ui, label: &str, value: &str) {
    let (rect, _response) = figure_row_sized(ui, label, value, metric::ROW_COMPACT);
    ui.painter().hline(
        rect.x_range(),
        rect.bottom(),
        Stroke::new(metric::HAIRLINE, theme::RULE),
    );
}

/// The two ends of a figure row, without the hairline.
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
        egui::FontId::proportional(size::CAPTION),
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

/// The header of a ruled table: caption columns at the given fractions of the
/// width, one table-header high (28), with the same hairline as a body row.
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
        Stroke::new(metric::HAIRLINE, theme::RULE),
    );
}

/// One ruled table body row: monospace cells on the same fractions as
/// [`table_header`], a hairline underneath.
///
/// Every cell is monospace, which is the point of the table: a column of
/// offsets is a column because the glyphs have one width (DESIGN.md §3).
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
        Stroke::new(metric::HAIRLINE, theme::RULE),
    );
}

/// The design's focus ring: 2pt `ink`, inset by the hairline (DESIGN.md §6).
///
/// egui's `Button` draws its own focus outline only when it has no explicit
/// `.stroke()` (button.rs replaces the frame style with the argument), and every
/// button in this product sets a stroke to state its role. So the ring is drawn
/// here, from the same `Response` the widget returned, and every custom control
/// calls it. Without this a focused 接続して確認 / 中止 / 書き込む was pixel-
/// identical to an unfocused one, which is the Must finding of the design review.
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

/// The primary action: a bone (`ink`) face with a `ground` word.
///
/// This is the safe action the screen recommends. When it cannot be taken it
/// becomes an outline with a mute word rather than a dimmed bone face: dimming
/// a 14:1 pair with egui's `disabled_alpha` lands under 4.5:1, which is exactly
/// the defect the review found on the destructive button.
pub fn primary_button(ui: &mut Ui, label: &str, enabled: bool) -> egui::Response {
    let (fill, ink, border) = if enabled {
        (theme::INK, theme::GROUND, theme::INK)
    } else {
        (theme::GROUND, theme::MUTE, theme::RULE)
    };
    action_button(ui, label, enabled, fill, ink, border, 1.0)
}

/// A compact control: a `rule` outline with no fill, for a toolbar or a toggle.
///
/// A fourth class the §10 hierarchy did not name. The map's playback controls and
/// the export-format chips are neither the screen's primary action nor its
/// secondary one, and leaving them on egui's default filled face made the Log
/// toolbar the brightest thing on the page (design review, Should 4). Selected is
/// an `ink` outline — the same selection language as [`choice_button`] — and
/// amber is never used.
pub fn tool_button(ui: &mut Ui, label: &str, selected: bool, enabled: bool) -> egui::Response {
    let (ink, border) = if selected {
        (theme::INK, theme::INK)
    } else {
        (theme::MUTE, theme::RULE)
    };
    ui.scope(|ui| {
        // A disabled tool keeps its own contrast pair rather than egui's dimmed
        // fill, the same rule the action buttons follow.
        ui.style_mut().visuals.disabled_alpha = 1.0;
        let response = ui.add_enabled(
            enabled,
            egui::Button::new(RichText::new(label).size(size::UI).color(ink))
                .fill(Color32::TRANSPARENT)
                .stroke(Stroke::new(metric::HAIRLINE, border))
                .corner_radius(theme::RADIUS)
                .min_size(Vec2::new(0.0, metric::ROW_COMPACT)),
        );
        focus_ring(ui, &response);
        response
    })
    .inner
}

/// One option of a segmented control: ink outline when selected, rule when not.
///
/// Selected is a bone outline, not a fill and not a status colour: selection in
/// this product is never amber (DESIGN.md §3, §7).
///
/// This control has no disabled state on purpose: the settings choices are live
/// whenever the screen shows them, so there is nothing to gate. The two action
/// buttons do have one, which is why they share [`action_button`] and this does
/// not.
pub fn choice_button(ui: &mut Ui, label: &str, selected: bool) -> egui::Response {
    let (ink, border) = if selected {
        (theme::INK, theme::INK)
    } else {
        (theme::MUTE, theme::RULE)
    };
    let response = ui.add(
        egui::Button::new(RichText::new(label).size(size::UI).color(ink))
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::new(metric::HAIRLINE, border))
            .corner_radius(theme::RADIUS)
            .min_size(Vec2::new(metric::ACTION_WIDTH * 0.75, metric::ROW)),
    );
    focus_ring(ui, &response);
    response
}

/// A secondary action: an outline on the ground, no fill.
///
/// The same control at a narrower width, so 中止 / 参照 / 再読込 read as a
/// quieter class than the one primary action a screen offers.
pub fn secondary_button(ui: &mut Ui, label: &str, enabled: bool) -> egui::Response {
    let ink = if enabled { theme::INK } else { theme::MUTE };
    action_button(
        ui,
        label,
        enabled,
        Color32::TRANSPARENT,
        ink,
        theme::RULE,
        0.7,
    )
}

/// The body [`primary_button`] and [`secondary_button`] share.
///
/// The two roles differ only in fill, ink, border and width factor, so those are
/// arguments; the disabled treatment (override `disabled_alpha`, keep the face's
/// own contrast pair) is written once here instead of twice with a chance to
/// drift.
fn action_button(
    ui: &mut Ui,
    label: &str,
    enabled: bool,
    fill: Color32,
    ink: Color32,
    border: Color32,
    width_factor: f32,
) -> egui::Response {
    ui.scope(|ui| {
        ui.style_mut().visuals.disabled_alpha = 1.0;
        let response = ui.add_enabled(
            enabled,
            egui::Button::new(RichText::new(label).size(size::UI).color(ink))
                .fill(fill)
                .stroke(Stroke::new(metric::HAIRLINE, border))
                .corner_radius(theme::RADIUS)
                .min_size(Vec2::new(metric::ACTION_WIDTH * width_factor, metric::ROW)),
        );
        focus_ring(ui, &response);
        response
    })
    .inner
}

/// The 「見本」 marker, for any value that did not come from a device.
///
/// PRODUCT.md makes "say what the device is doing" a rule; sample data is the
/// one case where the honest sentence is "no device is doing anything, this is a
/// drawing of one". The firmware screen prints this under its heading; panels
/// and blocks that carry sample figures use [`sample_panel`] and [`block_title`],
/// which put the same word in their caption instead of adding a band above them.
pub fn sample_marker(ui: &mut Ui) {
    ui.label(
        RichText::new("見本（実機から読んだ値ではありません）")
            .size(size::CAPTION)
            .color(theme::MUTE),
    );
}

/// A horizontal hairline that takes its space in the layout.
///
/// Allocated, not painted at the cursor: a rule that reserves no height is one the
/// next element starts on top of, which looks like a border rather than a rule.
pub fn hairline(ui: &mut Ui) {
    let width = ui.available_width();
    let (rect, _ignored) =
        ui.allocate_exact_size(Vec2::new(width, metric::HAIRLINE), Sense::hover());
    ui.painter().rect_filled(rect, theme::RADIUS, theme::RULE);
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

    #[test]
    fn every_custom_button_draws_the_focus_ring() {
        // The design review's Must: an explicit `.stroke()` makes egui's Button
        // skip its own focus outline, so every custom control painted the same
        // whether focused or not. The ring is drawn from the response; this drives
        // each button class through the real widget, gives it focus, and looks for
        // the 2pt ink rectangle in the frame.
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
