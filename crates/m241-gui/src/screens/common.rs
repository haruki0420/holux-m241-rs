//! Furniture the three screens share: the status voice, the ruled panel, and the
//! formatting of times and figures.
//!
//! Why the status voice is a type rather than a habit: DESIGN.md §3 says a status
//! colour is never the only signal. If every screen chose a colour at its own call
//! site, one call site would eventually paint a red dot with no word beside it and
//! nothing would notice. [`Status::voice`] is the only place a status colour is
//! chosen, and it returns the word with it, so a caller cannot take one without
//! the other.

use egui::{Color32, RichText, Sense, Stroke, Ui, Vec2};
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

/// A label/value line: label in mute, value right-aligned in monospace.
///
/// The manual's figure line (DESIGN.md §3). Kept here rather than in the rail
/// widget because it is screen furniture, not navigation, and it is used by two
/// screens with the same rhythm.
pub fn figure_line(ui: &mut Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).size(size::CAPTION).color(theme::MUTE));
        ui.add_space(space::S);
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

/// The 「見本」 marker, for any value that did not come from a device.
///
/// PRODUCT.md makes "say what the device is doing" a rule; sample data is the
/// one case where the honest sentence is "no device is doing anything, this is a
/// drawing of one". Every screen that can show sample data calls this first.
pub fn sample_marker(ui: &mut Ui) {
    ui.label(
        RichText::new("見本 — 実機から読んだ値ではありません")
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
