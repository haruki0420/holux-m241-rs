//! A static HTML snapshot: an inline SVG map of a track and its summary.
//!
//! No tiles, no JavaScript and no external request: the page draws the track's
//! own coordinates in an equirectangular projection, with a scale bar and an
//! attribution line. ezTour's HTML export embedded a Google Maps page
//! (`07-eztour-spec.ja.md` §2, §5), which needs both a network and an API key the
//! vendor's page no longer serves; a file that stops working is not an archival
//! snapshot, so this writer keeps the shape and the numbers and drops the
//! basemap. Every value in it comes from the log: distance is a great-circle
//! sum, speed and altitude are the recorder's own fields.

use std::fmt::Write as _;

use crate::log::export::DOCUMENT_NAME;
use crate::log::report::summary_rows;
use crate::types::{Track, TrackPoint, TrackStats};

/// Width of the SVG canvas in user units.
const MAP_WIDTH: f64 = 960.0;
/// Height of the SVG canvas in user units.
const MAP_HEIGHT: f64 = 540.0;
/// Space kept free at every edge of the canvas.
const MAP_PADDING: f64 = 32.0;
/// Metres in one degree of latitude at the mean Earth radius.
const METRES_PER_DEGREE: f64 = 111_319.49;
/// The scale bar aims for this many pixels, then snaps to a round distance.
const SCALE_BAR_TARGET_PX: f64 = 140.0;

/// Render `track` as one self-contained HTML document.
///
/// The summary is [`summary_rows`]'s table. A track with no positioned point
/// gets the summary and a sentence instead of an empty map.
#[must_use]
pub fn to_html(track: &Track) -> String {
    let stats = TrackStats::of(track);
    let span = track.time_span();
    let mut out = String::from(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n",
    );
    let _ = writeln!(out, "<title>{}</title>", escape_html(DOCUMENT_NAME));
    out.push_str(STYLE);
    out.push_str("</head>\n<body>\n");
    let _ = writeln!(out, "<h1>{}</h1>", escape_html(DOCUMENT_NAME));
    out.push_str("<table class=\"summary\">\n");
    for (key, value) in summary_rows(&stats, span) {
        let _ = writeln!(
            out,
            "<tr><th>{}</th><td>{}</td></tr>",
            escape_html(key),
            escape_html(&value)
        );
    }
    out.push_str("</table>\n");
    match map_svg(track) {
        Some(svg) => out.push_str(&svg),
        None => out.push_str(
            "<p class=\"empty\">No point in this track carries a position, so there is no map to \
             draw.</p>\n",
        ),
    }
    out.push_str(
        "<p class=\"attribution\">Drawn by holux-m241-rs from the track log. The map is an \
         equirectangular projection of the logged coordinates with no basemap tiles; distance is \
         a great-circle sum, and speed and altitude are the recorder's own values.</p>\n\
         </body>\n</html>\n",
    );
    out
}

/// The inline SVG map, or `None` when no point has a position.
fn map_svg(track: &Track) -> Option<String> {
    let mut min_lat = f64::INFINITY;
    let mut max_lat = f64::NEG_INFINITY;
    let mut min_lon = f64::INFINITY;
    let mut max_lon = f64::NEG_INFINITY;
    let mut positioned = 0usize;
    for point in &track.points {
        if let Some((latitude, longitude)) = point.position() {
            min_lat = min_lat.min(latitude);
            max_lat = max_lat.max(latitude);
            min_lon = min_lon.min(longitude);
            max_lon = max_lon.max(longitude);
            positioned += 1;
        }
    }
    if positioned == 0 {
        return None;
    }

    // Equirectangular: the longitude span shrinks by cos(latitude) so the shape
    // is not stretched. The clamp keeps a polar track finite; the projection is
    // a drawing, not a measurement.
    let lon_scale = ((min_lat + max_lat) / 2.0)
        .to_radians()
        .cos()
        .abs()
        .max(0.01);
    let width_degrees = (max_lon - min_lon) * lon_scale;
    let height_degrees = max_lat - min_lat;
    let usable_width = MAP_WIDTH - 2.0 * MAP_PADDING;
    let usable_height = MAP_HEIGHT - 2.0 * MAP_PADDING;
    let scale = fit_scale(usable_width, usable_height, width_degrees, height_degrees);
    let origin_x = MAP_PADDING + ((usable_width - width_degrees * scale) / 2.0).max(0.0);
    let origin_y = MAP_PADDING + ((usable_height - height_degrees * scale) / 2.0).max(0.0);
    let project = |latitude: f64, longitude: f64| -> (f64, f64) {
        (
            origin_x + (longitude - min_lon) * lon_scale * scale,
            origin_y + (max_lat - latitude) * scale,
        )
    };

    let mut svg = String::new();
    let _ = writeln!(
        svg,
        "<svg class=\"map\" viewBox=\"0 0 {MAP_WIDTH} {MAP_HEIGHT}\" role=\"img\" \
         aria-label=\"Track map\">"
    );
    let _ = writeln!(
        svg,
        "<rect class=\"map-bg\" x=\"0\" y=\"0\" width=\"{MAP_WIDTH}\" height=\"{MAP_HEIGHT}\"/>"
    );

    // One polyline per run of positioned points: a gap without a fix is not a
    // straight line the unit travelled, so it must not be drawn as one.
    let mut segments: Vec<Vec<(f64, f64)>> = Vec::new();
    let mut current: Vec<(f64, f64)> = Vec::new();
    for point in &track.points {
        match point.position() {
            Some((latitude, longitude)) => current.push(project(latitude, longitude)),
            None => {
                if !current.is_empty() {
                    segments.push(std::mem::take(&mut current));
                }
            }
        }
    }
    if !current.is_empty() {
        segments.push(current);
    }

    for segment in &segments {
        let Some(&(first_x, first_y)) = segment.first() else {
            continue;
        };
        if segment.len() > 1 {
            let mut points = String::new();
            for (x, y) in segment {
                if !points.is_empty() {
                    points.push(' ');
                }
                let _ = write!(points, "{x:.1},{y:.1}");
            }
            let _ = writeln!(svg, "<polyline class=\"track\" points=\"{points}\"/>");
        } else {
            let _ = writeln!(
                svg,
                "<circle class=\"track-dot\" cx=\"{first_x:.1}\" cy=\"{first_y:.1}\" r=\"2\"/>"
            );
        }
    }

    if let Some((latitude, longitude)) = track.points.iter().find_map(TrackPoint::position) {
        write_marker(&mut svg, &project, latitude, longitude, "Start", "start");
    }
    if let Some((latitude, longitude)) = track.points.iter().rev().find_map(TrackPoint::position) {
        write_marker(&mut svg, &project, latitude, longitude, "End", "end");
    }

    let metres_per_pixel = METRES_PER_DEGREE / scale;
    let bar_metres = nice_distance(metres_per_pixel * SCALE_BAR_TARGET_PX);
    let bar_pixels = bar_metres / metres_per_pixel;
    let bar_x = MAP_PADDING;
    let bar_y = MAP_HEIGHT - 18.0;
    let bar_end = bar_x + bar_pixels;
    let _ = writeln!(
        svg,
        "<line class=\"scale\" x1=\"{bar_x:.1}\" y1=\"{bar_y:.1}\" x2=\"{bar_end:.1}\" \
         y2=\"{bar_y:.1}\"/>"
    );
    for x in [bar_x, bar_end] {
        let _ = writeln!(
            svg,
            "<line class=\"scale\" x1=\"{x:.1}\" y1=\"{:.1}\" x2=\"{x:.1}\" y2=\"{:.1}\"/>",
            bar_y - 4.0,
            bar_y + 4.0
        );
    }
    let _ = writeln!(
        svg,
        "<text class=\"scale-label\" x=\"{bar_x:.1}\" y=\"{:.1}\">{}</text>",
        bar_y - 8.0,
        format_distance(bar_metres)
    );
    let _ = writeln!(
        svg,
        "<text class=\"credit\" x=\"{:.1}\" y=\"{:.1}\">m241 - equirectangular - no basemap \
         tiles</text>",
        MAP_WIDTH - MAP_PADDING,
        MAP_HEIGHT - 8.0
    );
    svg.push_str("</svg>\n");
    Some(svg)
}

/// One labelled start/end marker, offset so its text does not cover the dot.
fn write_marker(
    svg: &mut String,
    project: &impl Fn(f64, f64) -> (f64, f64),
    latitude: f64,
    longitude: f64,
    label: &str,
    class: &str,
) {
    let (x, y) = project(latitude, longitude);
    let _ = writeln!(
        svg,
        "<circle class=\"{class}\" cx=\"{x:.1}\" cy=\"{y:.1}\" r=\"6\"/>"
    );
    let _ = writeln!(
        svg,
        "<text class=\"marker-label\" x=\"{:.1}\" y=\"{:.1}\">{label}</text>",
        x + 10.0,
        y - 10.0
    );
}

/// The pixels-per-degree that fit both spans inside the canvas.
///
/// A span of zero (one point, or a perfectly north-south track) does not limit
/// the scale; the other axis does. Both zero means a single point, drawn at a
/// nominal one pixel per degree.
fn fit_scale(
    usable_width: f64,
    usable_height: f64,
    width_degrees: f64,
    height_degrees: f64,
) -> f64 {
    let horizontal = if width_degrees > 0.0 {
        usable_width / width_degrees
    } else {
        f64::INFINITY
    };
    let vertical = if height_degrees > 0.0 {
        usable_height / height_degrees
    } else {
        f64::INFINITY
    };
    let scale = horizontal.min(vertical);
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

/// Snap a distance to `1`, `2` or `5` times a power of ten, for the scale bar.
fn nice_distance(target_metres: f64) -> f64 {
    if !target_metres.is_finite() || target_metres <= 0.0 {
        return 1.0;
    }
    let base = 10f64.powf(target_metres.log10().floor());
    let mantissa = target_metres / base;
    let snap = if mantissa < 1.5 {
        1.0
    } else if mantissa < 3.5 {
        2.0
    } else if mantissa < 7.5 {
        5.0
    } else {
        10.0
    };
    snap * base
}

fn format_distance(metres: f64) -> String {
    if metres >= 1_000.0 {
        format!("{:.0} km", metres / 1_000.0)
    } else if metres >= 1.0 {
        format!("{metres:.0} m")
    } else {
        format!("{metres:.1} m")
    }
}

fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(character),
        }
    }
    out
}

const STYLE: &str = "<style>\n\
body{font-family:system-ui,sans-serif;margin:0;padding:24px;color:#222;background:#fafafa}\n\
h1{font-size:20px;margin:0 0 12px}\n\
table.summary{border-collapse:collapse;margin:0 0 16px}\n\
table.summary th{text-align:left;padding:2px 16px 2px 0;font-weight:600}\n\
table.summary td{padding:2px 0}\n\
svg.map{width:100%;max-width:960px;height:auto;display:block;border:1px solid #ddd;background:#fff}\n\
.map-bg{fill:#f0f4f8}\n\
.track{fill:none;stroke:#8000ff;stroke-width:3;stroke-opacity:.7;stroke-linejoin:round;stroke-linecap:round}\n\
.track-dot{fill:#8000ff;fill-opacity:.7}\n\
.start{fill:#1a7f37}\n.end{fill:#c62828}\n\
.marker-label{font:12px system-ui,sans-serif;fill:#222}\n\
.scale{stroke:#333;stroke-width:1.5}\n\
.scale-label{font:11px system-ui,sans-serif;fill:#333}\n\
.credit{font:10px system-ui,sans-serif;fill:#666}\n\
.attribution{font-size:12px;color:#666;max-width:960px}\n\
.empty{color:#666}\n\
</style>\n";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TrackPoint;

    fn sample_track() -> Track {
        Track {
            points: vec![
                TrackPoint {
                    utc: Some(1_700_000_000),
                    latitude: Some(35.681_236),
                    longitude: Some(139.767_125),
                    height_m: Some(42.5),
                    speed_kmh: Some(18.5),
                    rcr: None,
                },
                TrackPoint {
                    utc: Some(1_700_000_060),
                    latitude: Some(35.690_000),
                    longitude: Some(139.780_000),
                    height_m: Some(38.0),
                    speed_kmh: Some(20.0),
                    rcr: None,
                },
            ],
            damaged_blocks: Vec::new(),
        }
    }

    #[test]
    fn the_snapshot_carries_the_map_the_summary_and_the_attribution() {
        let html = to_html(&sample_track());
        assert!(html.starts_with("<!DOCTYPE html>"), "not an HTML document");
        assert!(html.contains("<svg class=\"map\""), "no inline SVG map");
        assert!(html.contains("<polyline class=\"track\""), "no track line");
        assert!(html.contains(">Start<"), "no start marker");
        assert!(html.contains(">End<"), "no end marker");
        assert!(html.contains("scale-label"), "no scale bar");
        assert!(html.contains("no basemap"), "no attribution");
        assert!(html.contains("Points</th><td>2</td>"), "no summary");
        assert!(html.contains("2023-11-14"), "no start time");
        assert!(html.contains("km/h"), "no speed row");
    }

    #[test]
    fn the_snapshot_has_no_javascript_and_no_remote_reference() {
        let html = to_html(&sample_track());
        // Whole-document scan, not a sample of tags: each entry is something the
        // page must never grow, whether it arrives through a template edit or a
        // future value. `src=` / `href=` cover a reference to any scheme, not
        // only `http`, and `url(` covers CSS.
        for forbidden in [
            "<script",
            "<img",
            "<link",
            "<iframe",
            "<object",
            "<embed",
            "src=",
            "href=",
            "http://",
            "https://",
            "url(",
            "javascript:",
            "@import",
            " onload=",
            " onerror=",
            "xlink:href",
        ] {
            assert!(
                !html.contains(forbidden),
                "the snapshot must not contain {forbidden:?}"
            );
        }
        assert!(
            html.trim_end().ends_with("</html>"),
            "the snapshot is not a closed document"
        );
    }

    #[test]
    fn a_gap_without_a_fix_is_not_drawn_as_a_line() {
        let mut track = sample_track();
        let mut points = track.points.clone();
        if let Some(point) = points.get_mut(1) {
            point.latitude = None;
            point.longitude = None;
        }
        points.push(TrackPoint {
            utc: Some(1_700_000_120),
            latitude: Some(35.695_000),
            longitude: Some(139.790_000),
            height_m: None,
            speed_kmh: None,
            rcr: None,
        });
        track.points = points;
        let html = to_html(&track);
        // Two runs of one point each: dots, not a polyline across the gap.
        assert!(!html.contains("<polyline"), "drew a line across a gap");
        assert_eq!(html.matches("class=\"track-dot\"").count(), 2);
    }

    #[test]
    fn a_track_without_positions_has_no_map() {
        let html = to_html(&Track::default());
        assert!(!html.contains("<svg"), "drew an empty map");
        assert!(html.contains("no map to draw"), "no explanation");
    }

    #[test]
    fn html_text_is_escaped() {
        assert_eq!(escape_html("<b>&\"'"), "&lt;b&gt;&amp;&quot;&#39;");
    }

    #[test]
    fn the_scale_bar_uses_round_distances() {
        assert_eq!(format_distance(nice_distance(137.0)), "100 m");
        assert_eq!(format_distance(nice_distance(1_800.0)), "2 km");
        assert_eq!(format_distance(nice_distance(4_000.0)), "5 km");
    }
}
