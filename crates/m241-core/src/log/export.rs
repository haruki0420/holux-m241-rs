//! Turning a parsed track into the file formats other programs read.
//!
//! Five formats, all from the same [`Track`]: GPX 1.1 and KML 2.2 for mapping
//! applications, CSV for a spreadsheet, NMEA for tools that expect a serial
//! stream, and JSON for scripts.
//!
//! # What is deliberate here
//!
//! * **Timestamps are UTC with a trailing `Z`.** The log stores Unix seconds;
//!   rendering them in a local zone would make an export depend on the machine
//!   that produced it.
//! * **Points without a position are dropped from GPX and KML.** Both formats
//!   require coordinates, and a track segment stitched through the missing points
//!   would draw a straight line across the place where the unit had no fix.
//! * **Nothing is sorted.** The device writes oldest-first, and an export that
//!   reordered points would hide a log that wrapped.
//! * **XML is escaped.** A track name is user input; `&` in it must not produce a
//!   file that no parser will open.

use std::fmt::Write as _;

use crate::types::{ExportFormat, Track, TrackPoint};

/// Name embedded in a GPX or KML document.
pub const DOCUMENT_NAME: &str = "M-241 log";

/// CSV header, in the order the columns are written.
pub const CSV_COLUMNS: [&str; 9] = [
    "index",
    "utc",
    "lat",
    "lon",
    "altitude_m",
    "speed_kmh",
    "rcr",
    "fix",
    "waypoint",
];

/// Render `track` in `format`.
///
/// Every [`ExportFormat`] has a writer, so this cannot fail. It returns the text
/// rather than writing a file: choosing the destination, an existing-file check
/// and the encoding of a non-UTF-8 path belong to the CLI, and keeping them out
/// of here is what lets an export be tested by comparing strings.
#[must_use]
pub fn export(track: &Track, format: ExportFormat) -> String {
    match format {
        ExportFormat::Gpx => to_gpx(track),
        ExportFormat::Kml => to_kml(track),
        ExportFormat::Csv => to_csv(track),
        ExportFormat::Nmea => to_nmea(track),
        ExportFormat::Json => to_json(track),
    }
}

/// Render `track` as a GPX 1.1 document.
///
/// Waypoints (`RCR & 0x0008`) become `<wpt>` elements so a button press survives
/// the conversion; everything else becomes a `<trkpt>` in one segment.
#[must_use]
pub fn to_gpx(track: &Track) -> String {
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str(
        "<gpx version=\"1.1\" creator=\"m241\" \
         xmlns=\"http://www.topografix.com/GPX/1/1\" \
         xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" \
         xsi:schemaLocation=\"http://www.topografix.com/GPX/1/1 \
         http://www.topografix.com/GPX/1/1/gpx.xsd\">\n",
    );
    let _ = writeln!(
        out,
        "  <metadata>\n    <name>{}</name>\n  </metadata>",
        escape_xml(DOCUMENT_NAME)
    );

    for (index, point) in track.points.iter().enumerate() {
        let (Some(lat), Some(lon)) = (point.latitude, point.longitude) else {
            continue;
        };
        if !point.is_waypoint() {
            continue;
        }
        let _ = write!(out, "  <wpt lat=\"{lat:.8}\" lon=\"{lon:.8}\">");
        let _ = write!(out, "<name>WP{index:06}</name>");
        if let Some(utc) = point.utc {
            let _ = write!(out, "<time>{}</time>", utc_iso(utc));
        }
        out.push_str("</wpt>\n");
    }

    let _ = writeln!(
        out,
        "  <trk>\n    <name>{}</name>\n    <trkseg>",
        escape_xml(DOCUMENT_NAME)
    );
    for point in &track.points {
        let (Some(lat), Some(lon)) = (point.latitude, point.longitude) else {
            continue;
        };
        out.push_str(&format!(
            "      <trkpt lat=\"{lat:.8}\" lon=\"{lon:.8}\">\n"
        ));
        if let Some(height) = point.height_m {
            let _ = writeln!(out, "        <ele>{height:.2}</ele>");
        }
        if let Some(utc) = point.utc {
            let _ = writeln!(out, "        <time>{}</time>", utc_iso(utc));
        }
        if let Some(speed) = point.speed_kmh {
            // GPX 1.1 has no speed element; the toolkit's own extension keeps
            // the value instead of silently dropping the field.
            out.push_str(&format!(
                "        <extensions><speed_kmh>{speed:.3}</speed_kmh></extensions>\n"
            ));
        }
        out.push_str("      </trkpt>\n");
    }
    out.push_str("    </trkseg>\n  </trk>\n</gpx>\n");
    out
}

/// Render `track` as a KML 2.2 document with a single `LineString`.
#[must_use]
pub fn to_kml(track: &Track) -> String {
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str("<kml xmlns=\"http://www.opengis.net/kml/2.2\">\n  <Document>\n");
    let _ = writeln!(out, "    <name>{}</name>", escape_xml(DOCUMENT_NAME));
    out.push_str("    <Placemark>\n");
    let _ = writeln!(out, "      <name>{}</name>", escape_xml(DOCUMENT_NAME));
    out.push_str("      <LineString>\n        <tessellate>1</tessellate>\n");
    out.push_str("        <coordinates>\n");
    for point in &track.points {
        let (Some(lat), Some(lon)) = (point.latitude, point.longitude) else {
            continue;
        };
        // KML orders coordinates lon,lat,altitude — the opposite of GPX, and the
        // most common reason a hand-built KML shows a track in the wrong place.
        let height = point.height_m.unwrap_or(0.0);
        let _ = writeln!(out, "          {lon:.8},{lat:.8},{height:.2}");
    }
    out.push_str("        </coordinates>\n      </LineString>\n    </Placemark>\n");
    out.push_str("  </Document>\n</kml>\n");
    out
}

/// Render `track` as CSV with the [`CSV_COLUMNS`] header.
#[must_use]
pub fn to_csv(track: &Track) -> String {
    let mut out = String::new();
    out.push_str(&CSV_COLUMNS.join(","));
    out.push('\n');
    for (index, point) in track.points.iter().enumerate() {
        let fields = [
            index.to_string(),
            point.utc.map(utc_iso).unwrap_or_default(),
            point
                .latitude
                .map(|v| format!("{v:.8}"))
                .unwrap_or_default(),
            point
                .longitude
                .map(|v| format!("{v:.8}"))
                .unwrap_or_default(),
            point
                .height_m
                .map(|v| format!("{v:.2}"))
                .unwrap_or_default(),
            point
                .speed_kmh
                .map(|v| format!("{v:.3}"))
                .unwrap_or_default(),
            point.rcr.map(|v| format!("0x{v:04X}")).unwrap_or_default(),
            if point.has_position() { "1" } else { "0" }.to_owned(),
            if point.is_waypoint() { "1" } else { "0" }.to_owned(),
        ];
        out.push_str(&fields.join(","));
        out.push('\n');
    }
    out
}

/// Render `track` as reconstructed `$GPRMC` sentences.
///
/// RMC is the one sentence that carries a full date and position together, so it
/// needs nothing but a track point. Points without a time or a position are
/// skipped: RMC cannot express them, and a sentence with empty fields would be
/// worse than no sentence.
#[must_use]
pub fn to_nmea(track: &Track) -> String {
    let mut out = String::new();
    for point in &track.points {
        let Some(sentence) = rmc_sentence(point) else {
            continue;
        };
        let _ = writeln!(out, "{sentence}");
    }
    out
}

/// One `$GPRMC` sentence for `point`, or `None` when it has no time or position.
#[must_use]
pub fn rmc_sentence(point: &TrackPoint) -> Option<String> {
    let utc = point.utc?;
    let lat = point.latitude?;
    let lon = point.longitude?;
    let seconds_of_day = utc % 86_400;
    let hours = seconds_of_day / 3_600;
    let minutes = (seconds_of_day % 3_600) / 60;
    let seconds = seconds_of_day % 60;
    let (day, month, year) = utc_date(utc);

    let (lat_text, lat_hemisphere) = degrees_minutes(lat, 'N', 'S');
    let (lon_text, lon_hemisphere) = degrees_minutes(lon, 'E', 'W');
    let speed_knots = point
        .speed_kmh
        .map(|kmh| format!("{:.2}", f64::from(kmh) / 1.852))
        .unwrap_or_default();

    let date = format!("{day:02}{month:02}{:02}", year % 100);
    let body = format!(
        "GPRMC,{hours:02}{minutes:02}{seconds:02}.00,A,{lat_text},{lat_hemisphere},\
         {lon_text},{lon_hemisphere},{speed_knots},,{date},,"
    );
    Some(format!(
        "${body}*{:02X}",
        crate::protocol::pmtk::checksum(&body)
    ))
}

/// `ddmm.mmmm` plus a hemisphere letter.
fn degrees_minutes(value: f64, positive: char, negative: char) -> (String, char) {
    let hemisphere = if value.is_sign_negative() {
        negative
    } else {
        positive
    };
    let absolute = value.abs();
    let degrees = absolute.trunc();
    let minutes = (absolute - degrees) * 60.0;
    // Latitude is two degree digits, longitude three; the caller's format is the
    // same for both because degrees < 100 for latitude and < 180 for longitude.
    (format!("{degrees:02.0}{minutes:07.4}"), hemisphere)
}

/// Render `track` as one JSON object.
#[must_use]
pub fn to_json(track: &Track) -> String {
    let mut out = String::new();
    out.push_str("{\n");
    let _ = writeln!(out, "  \"format\": \"m241-track\",");
    let _ = writeln!(out, "  \"version\": 1,");
    let summary = crate::types::LogSummary::of(track);
    let _ = writeln!(
        out,
        "  \"summary\": {{\"records\": {}, \"fixes\": {}, \"damaged_blocks\": {}, \
         \"started_at\": {}, \"ended_at\": {}}},",
        summary.records,
        summary.fixes,
        summary.damaged_blocks,
        json_optional(summary.started_at.map(i64::from)),
        json_optional(summary.ended_at.map(i64::from)),
    );
    out.push_str("  \"points\": [\n");
    for (index, point) in track.points.iter().enumerate() {
        let fields = [
            format!("\"index\": {index}"),
            format!("\"utc\": {}", json_optional(point.utc.map(i64::from))),
            format!(
                "\"utc_iso\": {}",
                point
                    .utc
                    .map(|utc| format!("\"{}\"", utc_iso(utc)))
                    .unwrap_or_else(|| String::from("null"))
            ),
            format!("\"lat\": {}", json_optional_float(point.latitude)),
            format!("\"lon\": {}", json_optional_float(point.longitude)),
            format!(
                "\"height_m\": {}",
                json_optional_float(point.height_m.map(f64::from))
            ),
            format!(
                "\"speed_kmh\": {}",
                json_optional_float(point.speed_kmh.map(f64::from))
            ),
            format!("\"rcr\": {}", json_optional(point.rcr.map(i64::from))),
            format!("\"waypoint\": {}", point.is_waypoint()),
        ];
        let comma = if index + 1 == track.points.len() {
            ""
        } else {
            ","
        };
        let _ = writeln!(out, "    {{{}}}{comma}", fields.join(", "));
    }
    out.push_str("  ]\n}\n");
    out
}

fn json_optional(value: Option<i64>) -> String {
    value.map_or_else(|| String::from("null"), |value| value.to_string())
}

fn json_optional_float(value: Option<f64>) -> String {
    match value {
        // A non-finite coordinate is not representable in JSON, and writing
        // `NaN` would produce a file no parser accepts.
        Some(value) if value.is_finite() => format!("{value}"),
        _ => String::from("null"),
    }
}

/// Escape the five characters that cannot appear literally in XML text.
fn escape_xml(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
    out
}

/// `1970-01-01T00:00:00Z` for a Unix timestamp.
///
/// The crate has no date library (and does not want one for five format
/// writers), so the civil-date conversion is done by hand from the days-since-
/// epoch count. The algorithm is Howard Hinnant's `civil_from_days`, which is
/// exact for the whole range of `i64` days; here the input is a `u32` so the
/// arithmetic cannot overflow.
#[must_use]
pub fn utc_iso(seconds: u32) -> String {
    let days = i64::from(seconds / 86_400);
    let time = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        time / 3_600,
        (time % 3_600) / 60,
        time % 60
    )
}

/// `(day, month, year)` a Unix timestamp falls on, in UTC.
#[must_use]
pub fn utc_date(seconds: u32) -> (u32, u32, u32) {
    let (year, month, day) = civil_from_days(i64::from(seconds / 86_400));
    (
        u32::try_from(day).unwrap_or(1),
        u32::try_from(month).unwrap_or(1),
        u32::try_from(year).unwrap_or(1970),
    )
}

/// Days since 1970-01-01 to a proleptic Gregorian civil date.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted / 146_097
    } else {
        (shifted - 146_096) / 146_097
    };
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    };
    let year = if month <= 2 { year + 1 } else { year };
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TrackPoint;

    /// The city-centre example the repository uses everywhere; never real data.
    fn tokyo_point() -> TrackPoint {
        TrackPoint {
            utc: Some(1_700_000_000),
            latitude: Some(35.681_236),
            longitude: Some(139.767_125),
            height_m: Some(80.0),
            speed_kmh: Some(42.5),
            rcr: Some(0),
        }
    }

    fn one_point_track() -> Track {
        Track {
            points: vec![tokyo_point()],
            damaged_blocks: Vec::new(),
        }
    }

    #[test]
    fn every_format_produces_output_and_none_is_empty() {
        let track = one_point_track();
        for format in ExportFormat::ALL {
            let text = export(&track, format);
            assert!(!text.is_empty(), "{format:?} produced nothing");
        }
    }

    #[test]
    fn a_gpx_trackpoint_carries_latitude_longitude_height_and_time() {
        let gpx = to_gpx(&one_point_track());
        assert!(gpx.contains("<gpx version=\"1.1\""), "got {gpx}");
        assert!(gpx.contains("lat=\"35.68123600\""), "got {gpx}");
        assert!(gpx.contains("lon=\"139.76712500\""), "got {gpx}");
        assert!(gpx.contains("<ele>80.00</ele>"), "got {gpx}");
        assert!(
            gpx.contains("<time>2023-11-14T22:13:20Z</time>"),
            "got {gpx}"
        );
        assert!(gpx.contains("</gpx>"));
    }

    #[test]
    fn a_kml_coordinate_is_longitude_comma_latitude_comma_height() {
        // The order is the opposite of GPX and getting it backwards puts the
        // track in the wrong hemisphere.
        let kml = to_kml(&one_point_track());
        assert!(kml.contains("139.76712500,35.68123600,80.00"), "got {kml}");
        assert!(kml.contains("<LineString>"));
    }

    #[test]
    fn a_csv_row_has_the_documented_columns() {
        let csv = to_csv(&one_point_track());
        let header = csv.lines().next().expect("a header line");
        assert_eq!(header, CSV_COLUMNS.join(","));
        let row = csv.lines().nth(1).expect("one row");
        let fields: Vec<&str> = row.split(',').collect();
        assert_eq!(fields.len(), CSV_COLUMNS.len());
        assert_eq!(fields[0], "0");
        assert_eq!(fields[1], "2023-11-14T22:13:20Z");
        assert_eq!(fields[7], "1", "the point has a position");
        assert_eq!(fields[8], "0", "it is not a waypoint");
    }

    #[test]
    fn an_nmea_sentence_has_a_valid_checksum_and_the_measured_layout() {
        let nmea = to_nmea(&one_point_track());
        let line = nmea.trim_end();
        assert!(line.starts_with("$GPRMC,221320.00,A,"), "got {line}");
        let (body, carried) = line
            .trim_start_matches('$')
            .split_once('*')
            .expect("a checksum");
        assert_eq!(
            u8::from_str_radix(carried, 16).expect("hex"),
            crate::protocol::pmtk::checksum(body)
        );
        assert_eq!(
            crate::protocol::pmtk::parse_sentence(line)
                .expect("a valid sentence")
                .kind,
            "GPRMC"
        );
    }

    #[test]
    fn a_point_without_a_position_is_absent_from_gpx_and_kml_but_kept_in_csv() {
        let track = Track {
            points: vec![
                TrackPoint {
                    utc: Some(1_700_000_000),
                    ..TrackPoint::default()
                },
                tokyo_point(),
            ],
            damaged_blocks: Vec::new(),
        };
        assert_eq!(to_gpx(&track).matches("<trkpt").count(), 1);
        assert_eq!(to_kml(&track).matches(",35.68").count(), 1);
        assert_eq!(to_csv(&track).lines().count(), 3, "header plus two rows");
        assert_eq!(to_nmea(&track).lines().count(), 1);
    }

    #[test]
    fn a_waypoint_becomes_its_own_gpx_element() {
        let track = Track {
            points: vec![TrackPoint {
                rcr: Some(0x0008),
                ..tokyo_point()
            }],
            damaged_blocks: Vec::new(),
        };
        let gpx = to_gpx(&track);
        assert_eq!(gpx.matches("<wpt ").count(), 1);
        assert_eq!(gpx.matches("<trkpt").count(), 1);
        assert!(gpx.contains("<name>WP000000</name>"));
    }

    #[test]
    fn a_document_name_is_escaped() {
        // The name is a constant today, but escaping is a property of the writer:
        // the moment a caller passes a track name, this must already hold.
        assert_eq!(escape_xml("a & b <c>"), "a &amp; b &lt;c&gt;");
        assert_eq!(escape_xml("\"'"), "&quot;&apos;");
    }

    #[test]
    fn a_json_document_is_valid_and_carries_the_summary() {
        let json = to_json(&one_point_track());
        assert!(json.contains("\"format\": \"m241-track\""));
        assert!(json.contains("\"records\": 1"));
        assert!(json.contains("\"fixes\": 1"));
        assert!(json.contains("\"lat\": 35.681236"));
        assert!(json.contains("\"utc_iso\": \"2023-11-14T22:13:20Z\""));
        assert!(json.trim_end().ends_with('}'));
    }

    #[test]
    fn a_missing_field_is_null_in_json_not_zero() {
        let track = Track {
            points: vec![TrackPoint {
                utc: Some(1_700_000_000),
                ..TrackPoint::default()
            }],
            damaged_blocks: Vec::new(),
        };
        let json = to_json(&track);
        assert!(json.contains("\"lat\": null"), "got {json}");
        assert!(json.contains("\"speed_kmh\": null"), "got {json}");
    }

    #[test]
    fn the_epoch_renders_as_the_unix_epoch() {
        assert_eq!(utc_iso(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc_iso(1), "1970-01-01T00:00:01Z");
        assert_eq!(utc_iso(86_400), "1970-01-02T00:00:00Z");
    }

    #[test]
    fn a_leap_day_and_a_leap_year_boundary_are_right() {
        // 2024-02-29 and 2024-03-01; an off-by-one in the civil calendar shows
        // up here and nowhere else.
        let leap = 1_709_164_800; // 2024-02-29T00:00:00Z
        assert_eq!(utc_iso(leap), "2024-02-29T00:00:00Z");
        assert_eq!(utc_iso(leap + 86_400), "2024-03-01T00:00:00Z");
    }

    #[test]
    fn the_wnro_era_dates_render_correctly() {
        // The bug this whole tool exists for: a stock unit reports 2006 dates.
        assert_eq!(utc_iso(315_964_800), "1980-01-06T00:00:00Z");
        assert_eq!(utc_iso(935_280_000), "1999-08-22T00:00:00Z");
        assert_eq!(utc_iso(1_700_000_000), "2023-11-14T22:13:20Z");
    }

    #[test]
    fn nmea_degrees_and_minutes_are_formatted_to_four_decimals() {
        let (text, hemisphere) = degrees_minutes(35.681_236, 'N', 'S');
        assert_eq!(&text[..2], "35");
        assert_eq!(hemisphere, 'N');
        let (south, hemisphere) = degrees_minutes(-35.681_236, 'N', 'S');
        assert_eq!(&south[..2], "35");
        assert_eq!(hemisphere, 'S');
        let (east, hemisphere) = degrees_minutes(139.767_125, 'E', 'W');
        assert_eq!(&east[..3], "139");
        assert_eq!(hemisphere, 'E');
    }
}
