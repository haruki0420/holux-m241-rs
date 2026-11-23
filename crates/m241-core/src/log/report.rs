//! The summary rows a report shows, and the RTF writer that prints them.
//!
//! ezTour printed a track report (`07-eztour-spec.ja.md` §2) without recording
//! its layout, so the rows here are this project's own: [`TrackStats`]'s
//! recomputed distance and average speed, plus the device's own maximum speed
//! and altitudes. The HTML snapshot (`--html`) uses the same rows, so the two
//! documents cannot disagree about what a trip's numbers are called.
//!
//! RTF is written as ASCII with every non-ASCII character escaped as a `\uN?`
//! control word, and `\`, `{` and `}` escaped so a value cannot end the
//! document early. The file declares one font and no stylesheet: any word
//! processor opens it, and nothing is copied from the vendor's templates.

use std::fmt::Write as _;

use crate::log::export::{DOCUMENT_NAME, utc_date_and_time, utc_iso};
use crate::types::{Track, TrackStats};

/// Render `track` as an RTF report: a summary block and a point table.
///
/// The summary is [`summary_rows`]'s ten rows; the table has one row per point,
/// with a missing timestamp, height or speed written as `-` rather than as a
/// zero that would read like a measurement.
#[must_use]
pub fn to_rtf(track: &Track) -> String {
    let stats = TrackStats::of(track);
    let span = track.time_span();
    let mut out = String::from(
        "{\\rtf1\\ansi\\ansicpg1252\\deff0\n\
         {\\fonttbl{\\f0\\fswiss Helvetica;}{\\f1\\fmodern Courier New;}}\n\\fs24\n",
    );
    let _ = writeln!(out, "{{\\b {}}}\\par", escape_rtf(DOCUMENT_NAME));
    out.push_str("\\par\n{\\b Summary}\\par\n");
    for (key, value) in summary_rows(&stats, span) {
        let _ = writeln!(out, "{}: {}\\par", escape_rtf(key), escape_rtf(&value));
    }
    out.push_str("\\par\n{\\b Points}\\par\n\\fs16\n");
    out.push_str(
        "{\\b INDEX}\\tab {\\b UTC DATE}\\tab {\\b UTC TIME}\\tab {\\b LATITUDE}\\tab \
         {\\b LONGITUDE}\\tab {\\b ALTITUDE M}\\tab {\\b SPEED KMH}\\par\n",
    );
    for (index, point) in track.points.iter().enumerate() {
        let (date, time) = point
            .utc
            .map_or_else(|| (String::from("-"), String::from("-")), utc_date_and_time);
        let latitude = report_number(point.latitude, 6);
        let longitude = report_number(point.longitude, 6);
        let altitude = report_number(point.height_m.map(f64::from), 2);
        let speed = report_number(point.speed_kmh.map(f64::from), 3);
        let _ = writeln!(
            out,
            "{index}\\tab {date}\\tab {time}\\tab {latitude}\\tab {longitude}\\tab {altitude}\\tab \
             {speed}\\par"
        );
    }
    out.push('}');
    out
}

/// The report's summary rows, shared by the RTF report and the HTML snapshot.
///
/// Ten fixed labels: points, fixes, the two ends of the time span, distance,
/// duration, average and maximum speed, and the altitude range. A fact the
/// track does not carry is `-`, never `0`, which would be a measurement the log
/// did not record.
pub(crate) fn summary_rows(
    stats: &TrackStats,
    span: Option<(u32, u32)>,
) -> [(&'static str, String); 10] {
    [
        ("Points", stats.records.to_string()),
        ("Fixes", stats.fixes.to_string()),
        (
            "Start (UTC)",
            span.map_or_else(|| String::from("-"), |(start, _)| utc_iso(start)),
        ),
        (
            "End (UTC)",
            span.map_or_else(|| String::from("-"), |(_, end)| utc_iso(end)),
        ),
        ("Distance", format!("{:.3} km", stats.distance_m / 1000.0)),
        (
            "Duration",
            stats
                .duration_seconds
                .map_or_else(|| String::from("-"), format_duration),
        ),
        (
            "Average speed",
            stats
                .average_speed_kmh
                .map_or_else(|| String::from("-"), |value| format!("{value:.3} km/h")),
        ),
        (
            "Max speed",
            stats
                .max_speed_kmh
                .filter(|value| value.is_finite())
                .map_or_else(|| String::from("-"), |value| format!("{value:.3} km/h")),
        ),
        (
            "Min altitude",
            stats
                .min_altitude_m
                .filter(|value| value.is_finite())
                .map_or_else(|| String::from("-"), |value| format!("{value:.2} m")),
        ),
        (
            "Max altitude",
            stats
                .max_altitude_m
                .filter(|value| value.is_finite())
                .map_or_else(|| String::from("-"), |value| format!("{value:.2} m")),
        ),
    ]
}

/// A duration as words a report can print: `1 h 02 min 03 s`, or `2 min 03 s`.
pub(crate) fn format_duration(seconds: u32) -> String {
    let hours = seconds / 3_600;
    let minutes = (seconds % 3_600) / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        format!("{hours} h {minutes:02} min {seconds:02} s")
    } else {
        format!("{minutes} min {seconds:02} s")
    }
}

fn report_number(value: Option<f64>, digits: usize) -> String {
    value
        .filter(|value| value.is_finite())
        .map_or_else(|| String::from("-"), |value| format!("{value:.digits$}"))
}

/// Escape one RTF text run.
///
/// `\`, `{` and `}` are the format's own control characters; a newline becomes
/// `\par ` because RTF has no literal line break. Every non-ASCII character is
/// written as UTF-16 code units, each as the signed `\uN?` control word with an
/// ASCII fallback, which is how RTF carries text outside `ansicpg1252` without
/// declaring a code page per run.
fn escape_rtf(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut utf16 = [0u16; 2];
    for character in text.chars() {
        match character {
            '\\' => out.push_str("\\\\"),
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            '\n' => out.push_str("\\par "),
            '\r' => {}
            '\t' => out.push_str("\\tab "),
            ' '..='~' => out.push(character),
            _ => {
                for unit in character.encode_utf16(&mut utf16) {
                    let _ = write!(out, "\\u{}?", *unit as i16);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_track() -> Track {
        Track {
            points: vec![crate::types::TrackPoint {
                utc: Some(1_700_000_000),
                latitude: Some(35.681_236),
                longitude: Some(139.767_125),
                height_m: Some(42.5),
                speed_kmh: Some(18.5),
                rcr: None,
            }],
            damaged_blocks: Vec::new(),
        }
    }

    #[test]
    fn rtf_escapes_its_own_control_characters() {
        assert_eq!(escape_rtf("a\\b{c}d"), "a\\\\b\\{c\\}d");
    }

    #[test]
    fn rtf_writes_non_ascii_as_unicode_control_words() {
        // U+2192 RIGHTWARDS ARROW is decimal 8594, inside the signed 16-bit
        // range RTF's `\uN` expects, and the `?` is its ASCII fallback.
        assert_eq!(escape_rtf("→"), "\\u8594?");
    }

    #[test]
    fn the_report_starts_with_rtf_and_carries_the_summary_and_the_table() {
        let rtf = to_rtf(&sample_track());
        assert!(rtf.starts_with("{\\rtf1"), "got {rtf}");
        assert!(rtf.trim_end().ends_with('}'), "unbalanced document");
        assert!(rtf.contains("{\\b M-241 log}"), "no title");
        assert!(rtf.contains("Summary"), "no summary");
        assert!(rtf.contains("Points: 1"), "no point count");
        assert!(rtf.contains("INDEX"), "no table header");
        assert!(rtf.contains("0\\tab 2023-11-14\\tab"), "no point row");
        assert!(rtf.contains("35.681236"), "latitude not printed");
    }

    #[test]
    fn a_missing_speed_is_a_dash_not_a_zero() {
        let mut track = sample_track();
        if let Some(point) = track.points.first_mut() {
            point.speed_kmh = None;
        }
        let rtf = to_rtf(&track);
        assert!(
            rtf.contains("-\\par"),
            "a missing speed printed as a number"
        );
        assert!(
            !rtf.contains("18.500"),
            "a missing speed printed as its old value"
        );
    }
}
