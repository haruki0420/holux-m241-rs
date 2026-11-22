//! Turning a parsed track into the file formats other programs read.
//!
//! Five formats, all from the same [`Track`]: GPX 1.1 and KML 2.0 for mapping
//! applications, CSV for a spreadsheet, NMEA for tools that expect a serial
//! stream, and JSON for scripts. [`to_csv_report`] writes the second CSV kind
//! ezTour offers — a summary beside the point table — without adding a sixth
//! [`ExportFormat`].
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
//! * **GPX coordinates carry nine decimals** (`%.9f`), which is what
//!   `07-eztour-spec.ja.md` §2 records for ezTour's template. Eight places are
//!   enough for a whole millimetre, but the log's latitude and longitude are
//!   4-byte floats, so the extra digit preserves the decoded value exactly and
//!   lets a round trip through this writer and [`crate::log::import`] keep the
//!   bits it started with.
//! * **The GPX metadata carries a link and a time.** GPX 1.1's `metadataType`
//!   has both, and §2 records that ezTour's header writes them. The time is the
//!   track's first recorded instant: this library has no clock of its own, and a
//!   file's mtime is not a property of the track. Trkpt children follow the
//!   schema order where the schema has the element (`ele`, `time`, `name`, then
//!   `extensions`); the bare `<speed>` between `name` and `extensions` is the
//!   exception described below, and a waypoint additionally carries a `desc`
//!   saying what kind of point it is.
//!
//! # GPX speed: a documented exception
//!
//! GPX 1.1 removed the `<speed>` element that GPX 1.0 defined, and the 1.1
//! schema does not accept one inside `<trkpt>`. ezTour writes it anyway (§2), and
//! so does this writer, for two reasons: a consumer that reads GPX 1.0-style
//! speed is common, and dropping the field would lose the only per-point speed a
//! stock M-241 records. The value is m/s (GPX 1.0's unit), and the same speed is
//! also written inside `<extensions><speed_kmh>` — a schema-valid position —
//! so a strict 1.1 reader loses nothing and [`crate::log::import::import_gpx`]
//! still round-trips the km/h value it stores. A future writer may drop the bare
//! element once no caller of that extension remains.
//!
//! # KML version
//!
//! Both KML writers emit the **2.0** namespace. `07-eztour-spec.ja.md` §2
//! records that ezTour's KML export is 2.0, the KMZ writer has always used it,
//! and a KMZ's `doc.kml` and a standalone `.kml` for the same track must not
//! advertise different versions. KML 2.2 readers accept the 2.0 namespace.

use std::fmt::Write as _;

use crate::types::{ExportFormat, Track, TrackPoint, TrackStats, haversine_meters};

/// Name embedded in a GPX or KML document.
pub const DOCUMENT_NAME: &str = "M-241 log";

/// The KML namespace both KML writers declare.
///
/// See the module doc's "KML version": 2.0 is ezTour's version and the KMZ
/// writer's, and one constant keeps the two documents from disagreeing.
pub(crate) const KML_NAMESPACE: &str = "http://earth.google.com/kml/2.0";

/// Default track line colour as KML's `<color>` string: `aabbggrr`.
///
/// Read right to left in byte pairs: `ff` is red, `80` green, `00` blue — a
/// purple — and the leading `b3` is alpha, 179/255 ≈ 0.7 opacity. Width 5 and
/// this alpha come from `07-eztour-spec.ja.md` §5, which records what ezTour's
/// map drew (purple, 5 px, 0.7 opacity).
///
/// The hex value is **this project's choice**, not ezTour's: the spec names the
/// colours in words and says they were carried as `#rrggbb` strings, and
/// nothing in this repository records the exact bytes ezTour emitted. So this
/// matches the *description*, and a side-by-side against real ezTour output is
/// unverified. A single colour is all one track needs; the ten-colour palette is
/// a multi-track feature (§5) that this exporter has no caller for yet, so
/// adding the other nine now would be untested colour data.
pub(crate) const KML_LINE_COLOR: &str = "b3ff0080";
/// Default track polyline width in pixels, from the same §5 measurement.
pub(crate) const KML_LINE_WIDTH: u8 = 5;

/// The `<Style id="trackStyle">` block both KML writers share.
///
/// Returned as text rather than written by each caller so a colour or width
/// change cannot land in one document and not the other. The indentation is
/// four spaces for a child of `<Document>`, which is where both use it.
pub(crate) fn track_style() -> String {
    format!(
        "    <Style id=\"trackStyle\">\n      <LineStyle>\n        \
         <color>{KML_LINE_COLOR}</color>\n        <width>{KML_LINE_WIDTH}</width>\n      \
         </LineStyle>\n    </Style>"
    )
}

/// The icon the standalone KML uses for a manual waypoint.
///
/// A fixed Google-hosted pushpin, which is what the spec's `IconStyle`/`Icon`/
/// `href` describes (§2) and what most KML viewers resolve without a local
/// asset. The specification does not record which URL ezTour wrote, so this is a
/// choice, not a copy; the standalone writer is the only caller because a KMZ
/// has photos rather than waypoint pins.
const WAYPOINT_ICON_HREF: &str = "http://maps.google.com/mapfiles/kml/pushpin/ylw-pushpin.png";

/// CSV header in ezTour's column order (`docs/dev/07-eztour-spec.ja.md` §2).
pub const CSV_COLUMNS: [&str; 12] = [
    "INDEX",
    "TRACK NUMBER",
    "UTC DATE",
    "UTC TIME",
    "LOCAL DATE",
    "LOCAL TIME",
    "LATITUDE",
    "N/S",
    "LONGITUDE",
    "E/W",
    "ALTITUDE",
    "SPEED",
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
/// Coordinates carry nine decimals and the header carries a link and the
/// track's start time, as `07-eztour-spec.ja.md` §2 records for ezTour's
/// template. See the module doc for the `<speed>` exception and for why the
/// metadata time is the track's start rather than the file's creation. An
/// advertised link is a fixed reference to this project, not user input.
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
        "  <metadata>\n    <name>{}</name>",
        escape_xml(DOCUMENT_NAME)
    );
    out.push_str(
        "    <link href=\"https://github.com/haruki0420/holux-m241-rs\">\n      \
         <text>Holux M-241 toolkit</text>\n    </link>\n",
    );
    if let Some((start, _)) = track.time_span() {
        let _ = writeln!(out, "    <time>{}</time>", utc_iso(start));
    }
    out.push_str("  </metadata>\n");

    for (index, point) in track.points.iter().enumerate() {
        let (Some(lat), Some(lon)) = (point.latitude, point.longitude) else {
            continue;
        };
        if !point.is_waypoint() {
            continue;
        }
        let _ = write!(out, "  <wpt lat=\"{lat:.9}\" lon=\"{lon:.9}\">");
        // GPX 1.1's waypoint children are ordered ele, time, …, name, cmt,
        // desc; the time therefore comes before the name.
        if let Some(utc) = point.utc {
            let _ = write!(out, "<time>{}</time>", utc_iso(utc));
        }
        let _ = write!(out, "<name>WP{index:06}</name>");
        out.push_str("<desc>manual waypoint</desc>");
        out.push_str("</wpt>\n");
    }

    let _ = writeln!(
        out,
        "  <trk>\n    <name>{}</name>\n    <trkseg>",
        escape_xml(DOCUMENT_NAME)
    );
    for (index, point) in track.points.iter().enumerate() {
        let (Some(lat), Some(lon)) = (point.latitude, point.longitude) else {
            continue;
        };
        let _ = writeln!(out, "      <trkpt lat=\"{lat:.9}\" lon=\"{lon:.9}\">");
        if let Some(height) = point.height_m {
            let _ = writeln!(out, "        <ele>{height:.2}</ele>");
        }
        if let Some(utc) = point.utc {
            let _ = writeln!(out, "        <time>{}</time>", utc_iso(utc));
        }
        // A name on every trkpt is what `metadataType`'s template writes, and it
        // lets a reader refer to a point without inventing an index. A `desc`
        // would only repeat it, so the description is reserved for waypoints.
        let _ = writeln!(out, "        <name>P{index:06}</name>");
        if let Some(speed) = point.speed_kmh.filter(|speed| speed.is_finite()) {
            // See the module doc: the bare element is GPX 1.0's, kept for the
            // consumers ezTour feeds; the km/h value also goes in `extensions`.
            let _ = writeln!(out, "        <speed>{:.3}</speed>", f64::from(speed) / 3.6);
            let _ = writeln!(
                out,
                "        <extensions><speed_kmh>{speed:.3}</speed_kmh></extensions>"
            );
        }
        out.push_str("      </trkpt>\n");
    }
    out.push_str("    </trkseg>\n  </trk>\n</gpx>\n");
    out
}

/// Render `track` as a KML 2.0 document.
///
/// The document carries the same `trackStyle` the KMZ writer uses, the
/// `waypointStyle` its waypoint placemarks reference, a `LookAt` framed on the
/// track, the track's `LineString`, and one `<Point>` placemark per manual
/// waypoint (`07-eztour-spec.ja.md` §2, §8 P0-4).
#[must_use]
pub fn to_kml(track: &Track) -> String {
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    let _ = writeln!(out, "<kml xmlns=\"{KML_NAMESPACE}\">\n  <Document>");
    let _ = writeln!(out, "    <name>{}</name>", escape_xml(DOCUMENT_NAME));
    out.push_str(&track_style());
    out.push('\n');
    out.push_str(
        "    <Style id=\"waypointStyle\">\n      <IconStyle>\n        <scale>1</scale>\n        \
         <Icon>\n          ",
    );
    let _ = writeln!(out, "<href>{WAYPOINT_ICON_HREF}</href>");
    out.push_str("        </Icon>\n      </IconStyle>\n    </Style>\n");
    if let Some((lon, lat, range)) = look_at(track) {
        let _ = writeln!(
            out,
            "    <LookAt>\n      <longitude>{lon:.8}</longitude>\n      \
             <latitude>{lat:.8}</latitude>\n      <altitude>0</altitude>\n      \
             <heading>0</heading>\n      <tilt>0</tilt>\n      <range>{range:.0}</range>\n    \
             </LookAt>"
        );
    }
    out.push_str("    <Placemark>\n");
    let _ = writeln!(out, "      <name>{}</name>", escape_xml(DOCUMENT_NAME));
    out.push_str("      <styleUrl>#trackStyle</styleUrl>\n");
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

    for (index, point) in track.points.iter().enumerate() {
        let (Some(lat), Some(lon)) = (point.latitude, point.longitude) else {
            continue;
        };
        if !point.is_waypoint() {
            continue;
        }
        let height = point.height_m.unwrap_or(0.0);
        let _ = writeln!(
            out,
            "    <Placemark>\n      <name>WP{index:06}</name>\n      \
             <description>manual waypoint</description>\n      \
             <styleUrl>#waypointStyle</styleUrl>\n      <Point>\n        \
             <coordinates>{lon:.8},{lat:.8},{height:.2}</coordinates>\n      </Point>\n    \
             </Placemark>"
        );
    }
    out.push_str("  </Document>\n</kml>\n");
    out
}

/// Centre and camera range for a KML `LookAt`, framed on every positioned point.
///
/// `(longitude, latitude, range_m)`. The range is three times the bounding box's
/// diagonal — enough that a 45-degree field of view contains the whole track —
/// with a one-kilometre floor so a single point does not put the camera inside
/// it. This is a framing heuristic, not a measured field of view; the
/// specification records that ezTour writes a `LookAt` (§2) but not the range
/// formula, so the numbers are this writer's choice. `None` when no point has a
/// finite position, because a `LookAt` needs a place to look at.
fn look_at(track: &Track) -> Option<(f64, f64, f64)> {
    let mut bounds: Option<(f64, f64, f64, f64)> = None;
    for point in &track.points {
        let (Some(lat), Some(lon)) = (point.latitude, point.longitude) else {
            continue;
        };
        if !lat.is_finite() || !lon.is_finite() {
            continue;
        }
        bounds = Some(match bounds {
            Some((min_lat, max_lat, min_lon, max_lon)) => (
                min_lat.min(lat),
                max_lat.max(lat),
                min_lon.min(lon),
                max_lon.max(lon),
            ),
            None => (lat, lat, lon, lon),
        });
    }
    let (min_lat, max_lat, min_lon, max_lon) = bounds?;
    let center_lat = (min_lat + max_lat) / 2.0;
    let center_lon = (min_lon + max_lon) / 2.0;
    let north_south = haversine_meters(min_lat, center_lon, max_lat, center_lon);
    let east_west = haversine_meters(center_lat, min_lon, center_lat, max_lon);
    let diagonal = north_south.hypot(east_west);
    Some((center_lon, center_lat, (diagonal * 3.0).max(1_000.0)))
}

/// `TRACK NUMBER` for every CSV row.
///
/// ezTour numbers tracks; a [`Track`] is exactly one track, so the column is a
/// constant rather than a per-point value.
const TRACK_NUMBER: &str = "1";

/// Render `track` as CSV with the [`CSV_COLUMNS`] header.
///
/// The columns and order follow the ezTour layout recorded in
/// `docs/dev/07-eztour-spec.ja.md` §2; that specification is the source, and a
/// byte-for-byte comparison with an ezTour export is 未検証.
///
/// `INDEX` is the zero-based index of the point in the track. `LOCAL DATE` and
/// `LOCAL TIME` stay empty because the time-zone basis ezTour used is
/// unrecorded. The dump's `rcr` / `fix` / `waypoint` fields have no column in
/// ezTour's layout: `rcr` and `waypoint` are in the JSON export, and `fix` is
/// the presence of a coordinate ([`TrackPoint::has_position`]).
#[must_use]
pub fn to_csv(track: &Track) -> String {
    let mut out = String::new();
    out.push_str(&CSV_COLUMNS.join(","));
    out.push('\n');
    for (index, point) in track.points.iter().enumerate() {
        let (utc_date, utc_time) = point
            .utc
            .map_or_else(|| (String::new(), String::new()), utc_date_and_time);
        let (latitude, north_south) = csv_coordinate(point.latitude, 'N', 'S');
        let (longitude, east_west) = csv_coordinate(point.longitude, 'E', 'W');
        let fields = [
            index.to_string(),
            String::from(TRACK_NUMBER),
            utc_date,
            utc_time,
            String::new(),
            String::new(),
            latitude,
            north_south,
            longitude,
            east_west,
            csv_optional(point.height_m, 2),
            csv_optional(point.speed_kmh, 3),
        ];
        out.push_str(&fields.join(","));
        out.push('\n');
    }
    out
}

/// `(unsigned decimal degrees, hemisphere letter)`: the sign is its own column.
fn csv_coordinate(value: Option<f64>, positive: char, negative: char) -> (String, String) {
    let Some(value) = value.filter(|value| value.is_finite()) else {
        return (String::new(), String::new());
    };
    let letter = if value.is_sign_negative() {
        negative
    } else {
        positive
    };
    (format!("{:.8}", value.abs()), letter.to_string())
}

/// An optional numeric column: empty when absent.
///
/// `digits` is the per-column precision (`ALTITUDE` 2, `SPEED` 3) rather than a
/// constant because the two columns carry different units.
fn csv_optional(value: Option<f32>, digits: usize) -> String {
    value.map_or_else(String::new, |value| format!("{value:.digits$}"))
}

/// Render `track` as the CSV *report*: a summary block, then the point table.
///
/// ezTour's CSV export offers two kinds, point rows and a report
/// (`07-eztour-spec.ja.md` §2); the specification records the twelve columns once
/// and does not define the report's own layout. So this writer reuses
/// [`to_csv`]'s header and rows for the table and puts a `KEY,VALUE` summary
/// above it. **The summary's keys are this project's own and the report's
/// columns are inferred, not specified**; a byte comparison with ezTour is
/// 未検証. The `REPORT` marker and the blank line separate the two tables so a
/// reader can split them without guessing where the point table begins.
///
/// The numbers are [`TrackStats`]'s recomputed facts, which is what ezTour's
/// report showed (spec §4.2): a log whose records carry no speed still gets a
/// distance and an average.
///
/// This is a text writer rather than a sixth [`ExportFormat`]: the report is the
/// same track rendered differently, and the format enum is a public selector the
/// GUI and CLI both match on.
#[must_use]
pub fn to_csv_report(track: &Track) -> String {
    let stats = TrackStats::of(track);
    let span = track.time_span();
    let optional = |value: Option<f64>, digits: usize| {
        value.map_or_else(String::new, |value| format!("{value:.digits$}"))
    };
    let rows: [(&str, String); 12] = [
        ("GENERATED BY", String::from("m241")),
        ("TRACK NAME", String::from(DOCUMENT_NAME)),
        ("RECORDS", stats.records.to_string()),
        ("FIXES", stats.fixes.to_string()),
        (
            "START",
            span.map_or_else(String::new, |(start, _)| utc_iso(start)),
        ),
        (
            "END",
            span.map_or_else(String::new, |(_, end)| utc_iso(end)),
        ),
        ("DISTANCE M", format!("{:.2}", stats.distance_m)),
        (
            "DURATION S",
            stats
                .duration_seconds
                .map_or_else(String::new, |seconds| seconds.to_string()),
        ),
        ("AVERAGE SPEED KMH", optional(stats.average_speed_kmh, 3)),
        (
            "MAX SPEED KMH",
            stats
                .max_speed_kmh
                .map_or_else(String::new, |speed| format!("{speed:.3}")),
        ),
        (
            "MIN ALTITUDE M",
            stats
                .min_altitude_m
                .map_or_else(String::new, |height| format!("{height:.2}")),
        ),
        (
            "MAX ALTITUDE M",
            stats
                .max_altitude_m
                .map_or_else(String::new, |height| format!("{height:.2}")),
        ),
    ];
    let mut out = String::from("REPORT\n");
    for (key, value) in rows {
        let _ = writeln!(out, "{key},{}", csv_field(value.as_str()));
    }
    out.push('\n');
    out.push_str(&to_csv(track));
    out
}

/// Quote one CSV field when it could otherwise split a row.
///
/// The summary's values are fixed names and formatted numbers today, so this
/// changes nothing yet; it is here because a report that silently breaks when a
/// future field holds a comma is the failure mode CSV quoting exists to prevent.
fn csv_field(text: &str) -> String {
    if text
        .chars()
        .any(|character| matches!(character, ',' | '"' | '\n' | '\r'))
    {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_string()
    }
}

/// Render `track` as reconstructed NMEA 0183 sentences.
///
/// One `$GPGGA`, one `$GPGSA` and one `$GPRMC` per point that has a time and a
/// position, in that order — the set `07-eztour-spec.ja.md` §2 records ezTour
/// rebuilding from a log, and the same set a live M-241 streams
/// (`docs/dev/03-hardware-facts.md`, Serial). Points without a time or a
/// position are skipped: no sentence can express them, and a sentence with empty
/// fields would be worse than no sentence.
///
/// # What the sentences can and cannot say
///
/// A log record carries a time, a position, a height and sometimes a speed. It
/// carries **no satellite count, no satellite list, no DOP and no geoid
/// separation**. Those fields are written empty — NMEA's "not available" — or,
/// where the field must hold a digit, as `0`; they are not filled with a
/// plausible value, because a fabricated satellite count is data the unit never
/// measured and a reader cannot tell it from a real one. In particular:
///
/// * GGA's fix quality is `1` (a GPS fix): a record only exists with coordinates,
///   which is the unit's own statement that it had a fix. Its satellite count is
///   `00`, its HDOP and geoid separation are empty, and its altitude is the
///   record's height with unit `M` (empty when the record has none).
/// * GSA's mode is `A` (automatic selection). Its fix type is `3` when the
///   record has an altitude and `2` when it does not: a record with a height is
///   the only evidence this writer has of a three-dimensional fix, and the
///   satellite geometry that would settle it is not in the log. Its twelve
///   satellite fields and all three DOPs are empty.
///
/// # Errors
///
/// This function cannot fail; a point that cannot form a sentence is skipped.
#[must_use]
pub fn to_nmea(track: &Track) -> String {
    let mut out = String::new();
    for point in &track.points {
        for sentence in [
            gga_sentence(point),
            gsa_sentence(point),
            rmc_sentence(point),
        ]
        .into_iter()
        .flatten()
        {
            let _ = writeln!(out, "{sentence}");
        }
    }
    out
}

/// One `$GPRMC` sentence for `point`, or `None` when it has no time or position.
#[must_use]
pub fn rmc_sentence(point: &TrackPoint) -> Option<String> {
    let utc = point.utc?;
    let lat = point.latitude?;
    let lon = point.longitude?;
    let (day, month, year) = utc_date(utc);

    let (lat_text, lat_hemisphere) = degrees_minutes(lat, 'N', 'S');
    let (lon_text, lon_hemisphere) = degrees_minutes(lon, 'E', 'W');
    let speed_knots = point
        .speed_kmh
        .filter(|kmh| kmh.is_finite())
        .map(|kmh| format!("{:.2}", f64::from(kmh) / 1.852))
        .unwrap_or_default();

    let date = format!("{day:02}{month:02}{:02}", year % 100);
    let body = format!(
        "GPRMC,{},A,{lat_text},{lat_hemisphere},{lon_text},{lon_hemisphere},{speed_knots},,\
         {date},,",
        nmea_time(utc)
    );
    Some(nmea_sentence(&body))
}

/// One `$GPGGA` sentence for `point`, or `None` when it has no time or position.
///
/// The satellite count, HDOP and geoid separation are empty or `0` because the
/// log does not record them; see [`to_nmea`] for why they are not guessed.
#[must_use]
pub fn gga_sentence(point: &TrackPoint) -> Option<String> {
    let utc = point.utc?;
    let lat = point.latitude?;
    let lon = point.longitude?;
    let (lat_text, lat_hemisphere) = degrees_minutes(lat, 'N', 'S');
    let (lon_text, lon_hemisphere) = degrees_minutes(lon, 'E', 'W');
    let altitude = point
        .height_m
        .filter(|height| height.is_finite())
        .map(|height| format!("{height:.1}"))
        .unwrap_or_default();
    // Fields: time, lat, N/S, lon, E/W, quality, satellites, HDOP, altitude,
    // altitude unit, geoid separation, separation unit, age, station id.
    let body = format!(
        "GPGGA,{},{lat_text},{lat_hemisphere},{lon_text},{lon_hemisphere},1,00,,{altitude},M,,M,,",
        nmea_time(utc)
    );
    Some(nmea_sentence(&body))
}

/// One `$GPGSA` sentence for `point`, or `None` when it has no time or position.
///
/// The fix type is derived from the record's altitude (`3` for a fix with a
/// height, `2` without); the satellite list and DOPs are empty. See [`to_nmea`].
#[must_use]
pub fn gsa_sentence(point: &TrackPoint) -> Option<String> {
    point.utc?;
    point.latitude?;
    point.longitude?;
    let fix_type = if point.height_m.is_some_and(|height| height.is_finite()) {
        3
    } else {
        2
    };
    // Fields: mode, fix type, twelve satellite ids, PDOP, HDOP, VDOP. The 15
    // empty slots stay empty rather than becoming zeros: NMEA uses an empty
    // field for "no data", and a zero DOP would be an excellent, not unknown,
    // dilution of precision.
    let body = format!("GPGSA,A,{fix_type},,,,,,,,,,,,,,,");
    Some(nmea_sentence(&body))
}

/// `HHMMSS.ss` for the UTC second a track point carries.
fn nmea_time(utc: u32) -> String {
    let seconds_of_day = utc % 86_400;
    format!(
        "{:02}{:02}{:02}.00",
        seconds_of_day / 3_600,
        (seconds_of_day % 3_600) / 60,
        seconds_of_day % 60
    )
}

/// `$<body>*<checksum>`, the one way every sentence here is terminated.
fn nmea_sentence(body: &str) -> String {
    format!("${body}*{:02X}", crate::protocol::pmtk::checksum(body))
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
        "  \"summary\": {{\"records\": {}, \"fixes\": {}, \"damaged_block_count\": {}, \
         \"started_at\": {}, \"ended_at\": {}}},",
        summary.records,
        summary.fixes,
        summary.damaged_block_count,
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
pub(crate) fn escape_xml(text: &str) -> String {
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

/// `(YYYY-MM-DD, HH:MM:SS)` for a Unix timestamp, in UTC.
///
/// The two halves the CSV columns need. [`utc_iso`] renders the same instant
/// with a `T` and a trailing `Z`, and both go through this conversion, so the
/// date surgery in the CSV writer cannot drift from the ISO form.
#[must_use]
pub fn utc_date_and_time(seconds: u32) -> (String, String) {
    let days = i64::from(seconds / 86_400);
    let time = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    (
        format!("{year:04}-{month:02}-{day:02}"),
        format!(
            "{:02}:{:02}:{:02}",
            time / 3_600,
            (time % 3_600) / 60,
            time % 60
        ),
    )
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
    let (date, time) = utc_date_and_time(seconds);
    format!("{date}T{time}Z")
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
        // Nine decimals, ezTour's `%.9f` (spec §2): eight would truncate the
        // decoded 4-byte float's last digit.
        assert!(gpx.contains("lat=\"35.681236000\""), "got {gpx}");
        assert!(gpx.contains("lon=\"139.767125000\""), "got {gpx}");
        assert!(gpx.contains("<ele>80.00</ele>"), "got {gpx}");
        assert!(
            gpx.contains("<time>2023-11-14T22:13:20Z</time>"),
            "got {gpx}"
        );
        assert!(gpx.contains("</gpx>"));
    }

    #[test]
    fn a_gpx_header_carries_the_metadata_link_and_the_track_start() {
        let gpx = to_gpx(&one_point_track());
        assert!(gpx.contains("<metadata>"), "got {gpx}");
        assert!(
            gpx.contains("<link href=\"https://github.com/haruki0420/holux-m241-rs\">"),
            "a GPX 1.1 metadata link must have an href: {gpx}"
        );
        assert!(
            gpx.contains("<time>2023-11-14T22:13:20Z</time>"),
            "the metadata time is the track's first instant: {gpx}"
        );
        assert!(gpx.contains("<name>P000000</name>"), "got {gpx}");
        // ezTour's `<speed>` is m/s; 42.5 km/h is 11.806 m/s, and the exact km/h
        // value stays in the extension for a round trip.
        assert!(gpx.contains("<speed>11.806</speed>"), "got {gpx}");
        assert!(
            gpx.contains("<extensions><speed_kmh>42.500</speed_kmh></extensions>"),
            "got {gpx}"
        );
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
    fn a_kml_document_carries_the_track_style_look_at_and_waypoint_point() {
        let track = Track {
            points: vec![
                tokyo_point(),
                TrackPoint {
                    rcr: Some(0x0008),
                    ..tokyo_point()
                },
            ],
            damaged_blocks: Vec::new(),
        };
        let kml = to_kml(&track);
        // One namespace for the standalone KML and the KMZ's doc.kml (spec §2).
        assert!(
            kml.contains(&format!("<kml xmlns=\"{KML_NAMESPACE}\">")),
            "got {kml}"
        );
        assert!(kml.contains("<Style id=\"trackStyle\">"), "got {kml}");
        assert!(kml.contains("<LineStyle>"), "got {kml}");
        assert!(kml.contains("<width>5</width>"), "got {kml}");
        assert!(
            kml.contains("<styleUrl>#trackStyle</styleUrl>"),
            "got {kml}"
        );
        assert!(kml.contains("<Style id=\"waypointStyle\">"), "got {kml}");
        assert!(kml.contains("<IconStyle>"), "got {kml}");
        assert!(
            kml.contains(
                "<href>http://maps.google.com/mapfiles/kml/pushpin/ylw-pushpin.png</href>"
            ),
            "got {kml}"
        );
        assert!(kml.contains("<LookAt>"), "got {kml}");
        assert!(kml.contains("<range>1000</range>"), "got {kml}");
        // The waypoint is drawn as its own `<Point>`, not only merged into the
        // line, so a reader can present the button press.
        assert!(kml.contains("<Point>"), "got {kml}");
        assert!(
            kml.contains("<coordinates>139.76712500,35.68123600,80.00</coordinates>"),
            "got {kml}"
        );
        assert!(kml.contains("<name>WP000001</name>"), "got {kml}");
    }

    #[test]
    fn a_kml_without_a_position_has_no_look_at() {
        let track = Track {
            points: vec![TrackPoint {
                utc: Some(1_700_000_000),
                ..TrackPoint::default()
            }],
            damaged_blocks: Vec::new(),
        };
        let kml = to_kml(&track);
        assert!(!kml.contains("<LookAt>"), "nothing to look at: {kml}");
        assert!(kml.contains("</kml>"));
    }

    #[test]
    fn a_csv_row_has_the_documented_columns() {
        // The header is the contract: a spreadsheet written against ezTour
        // looks these columns up by name and position.
        assert_eq!(
            CSV_COLUMNS,
            [
                "INDEX",
                "TRACK NUMBER",
                "UTC DATE",
                "UTC TIME",
                "LOCAL DATE",
                "LOCAL TIME",
                "LATITUDE",
                "N/S",
                "LONGITUDE",
                "E/W",
                "ALTITUDE",
                "SPEED",
            ],
            "the CSV column list and order are ezTour's"
        );
        let csv = to_csv(&one_point_track());
        let header = csv.lines().next().expect("a header line");
        assert_eq!(header, CSV_COLUMNS.join(","));
        let row = csv.lines().nth(1).expect("one row");
        let fields: Vec<&str> = row.split(',').collect();
        assert_eq!(fields.len(), CSV_COLUMNS.len());
        assert_eq!(fields[0], "0");
        assert_eq!(fields[1], "1");
        assert_eq!(fields[2], "2023-11-14");
        assert_eq!(fields[3], "22:13:20");
        assert_eq!(
            (fields[4], fields[5]),
            ("", ""),
            "a local time with no zone basis is left empty, not guessed"
        );
        assert_eq!(fields[6], "35.68123600");
        assert_eq!(fields[7], "N");
        assert_eq!(fields[8], "139.76712500");
        assert_eq!(fields[9], "E");
        assert_eq!(fields[10], "80.00");
        assert_eq!(fields[11], "42.500");
    }

    #[test]
    fn a_csv_coordinate_carries_its_sign_as_a_hemisphere_column() {
        // The number is unsigned because N/S and E/W say the sign; `-35.5`
        // would otherwise appear as a negative latitude with an S beside it.
        let track = Track {
            points: vec![TrackPoint {
                utc: None,
                latitude: Some(-35.5),
                longitude: Some(-70.25),
                ..TrackPoint::default()
            }],
            damaged_blocks: Vec::new(),
        };
        let csv = to_csv(&track);
        let row = csv.lines().nth(1).expect("one row");
        let fields: Vec<&str> = row.split(',').collect();
        assert_eq!(fields[6], "35.50000000");
        assert_eq!(fields[7], "S");
        assert_eq!(fields[8], "70.25000000");
        assert_eq!(fields[9], "W");
        assert_eq!(
            (fields[4], fields[5]),
            ("", ""),
            "a local time with no zone basis is left empty"
        );
        assert_eq!(
            (fields[2], fields[3]),
            ("", ""),
            "no time is an empty UTC pair"
        );
        assert_eq!((fields[10], fields[11]), ("", ""), "no height or speed");
    }

    #[test]
    fn a_csv_report_has_a_summary_block_then_the_twelve_point_columns() {
        let report = to_csv_report(&one_point_track());
        let lines: Vec<&str> = report.lines().collect();
        assert_eq!(lines[0], "REPORT", "the report starts with its marker");
        assert!(
            lines[1..]
                .iter()
                .take_while(|line| !line.is_empty())
                .any(|line| { *line == format!("RECORDS,{}", 1) }),
            "the summary carries the recomputed facts: {report}"
        );
        let header = lines
            .iter()
            .position(|line| *line == CSV_COLUMNS.join(","))
            .expect("the point table header follows the summary");
        assert!(
            lines[header + 1].starts_with("0,1,"),
            "the point rows reuse the point CSV columns: {report}"
        );
        assert_eq!(
            lines.len(),
            header + 2,
            "summary, one point row and nothing else: {report}"
        );
    }

    #[test]
    fn a_csv_report_summary_names_the_units_the_numbers_are_in() {
        let report = to_csv_report(&one_point_track());
        for key in [
            "DISTANCE M",
            "DURATION S",
            "AVERAGE SPEED KMH",
            "MAX SPEED KMH",
            "MIN ALTITUDE M",
            "MAX ALTITUDE M",
        ] {
            assert!(report.contains(key), "missing {key} in {report}");
        }
        assert!(
            report.contains("MAX SPEED KMH,42.500"),
            "the recorded maximum speed is carried: {report}"
        );
    }

    #[test]
    fn an_nmea_sentence_has_a_valid_checksum_and_the_measured_layout() {
        let nmea = to_nmea(&one_point_track());
        // GGA and GSA come first, RMC last; every line must carry a checksum the
        // shared sentence parser accepts.
        let lines: Vec<&str> = nmea.lines().collect();
        assert_eq!(lines.len(), 3, "one GGA, one GSA, one RMC: {nmea}");
        for kind in ["GPGGA", "GPGSA", "GPRMC"] {
            assert!(
                lines
                    .iter()
                    .any(|line| line.starts_with(&format!("${kind},"))),
                "missing {kind}: {nmea}"
            );
        }
        let line = lines.last().copied().expect("an RMC line");
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
    fn an_nmea_gga_carries_the_altitude_and_says_what_the_log_cannot() {
        let nmea = to_nmea(&one_point_track());
        let gga = nmea
            .lines()
            .find(|line| line.starts_with("$GPGGA,"))
            .expect("a GGA line");
        let (body, carried) = gga
            .trim_start_matches('$')
            .split_once('*')
            .expect("a checksum");
        assert_eq!(
            u8::from_str_radix(carried, 16).expect("hex"),
            crate::protocol::pmtk::checksum(body)
        );
        let fields: Vec<&str> = body.split(',').collect();
        assert_eq!(fields.len(), 15, "GGA has 15 fields: {body}");
        assert_eq!(fields[0], "GPGGA");
        assert_eq!(fields[6], "1", "a record with a position is a GPS fix");
        assert_eq!(fields[7], "00", "the log records no satellite count");
        assert_eq!(fields[8], "", "the log records no HDOP");
        assert_eq!(fields[9], "80.0", "the altitude is the record's height");
        assert_eq!(fields[10], "M");
        assert_eq!(fields[11], "", "the log records no geoid separation");

        let gsa = nmea
            .lines()
            .find(|line| line.starts_with("$GPGSA,"))
            .expect("a GSA line");
        let (body, carried) = gsa
            .trim_start_matches('$')
            .split_once('*')
            .expect("a checksum");
        assert_eq!(
            u8::from_str_radix(carried, 16).expect("hex"),
            crate::protocol::pmtk::checksum(body)
        );
        let fields: Vec<&str> = body.split(',').collect();
        assert_eq!(fields.len(), 18, "GSA has 18 fields: {body}");
        assert_eq!(fields[0], "GPGSA");
        assert_eq!(fields[1], "A", "automatic satellite selection");
        assert_eq!(fields[2], "3", "a record with an altitude is a 3D fix");
        assert!(
            fields[3..15].iter().all(|field| field.is_empty()),
            "the log records no satellite list: {body}"
        );
        assert!(
            fields[15..].iter().all(|field| field.is_empty()),
            "the log records no DOPs: {body}"
        );
    }

    #[test]
    fn a_point_without_an_altitude_gives_a_two_dimensional_gsa() {
        let track = Track {
            points: vec![TrackPoint {
                height_m: None,
                ..tokyo_point()
            }],
            damaged_blocks: Vec::new(),
        };
        let nmea = to_nmea(&track);
        let gsa = nmea
            .lines()
            .find(|line| line.starts_with("$GPGSA,"))
            .expect("a GSA line");
        let body = gsa
            .trim_start_matches('$')
            .split_once('*')
            .expect("a checksum")
            .0;
        assert_eq!(body.split(',').nth(2), Some("2"), "got {body}");
        let gga = nmea
            .lines()
            .find(|line| line.starts_with("$GPGGA,"))
            .expect("a GGA line");
        let body = gga
            .trim_start_matches('$')
            .split_once('*')
            .expect("a checksum")
            .0;
        assert_eq!(
            body.split(',').nth(9),
            Some(""),
            "an unknown altitude is empty, not zero: {body}"
        );
    }

    #[test]
    fn the_exported_nmea_round_trips_back_into_a_track_with_its_altitude() {
        // The GGA/RMC pair is what lets an import recover a height that RMC
        // alone cannot carry; GSA is ignored by the reader and must stay that
        // way (docs/reference/02-cli.md counts it as `ignored`).
        let imported = crate::log::import::import_nmea(&to_nmea(&one_point_track()))
            .expect("import our own export");
        assert_eq!(
            (imported.imported, imported.dropped, imported.ignored),
            (2, 0, 1),
            "one GGA and one RMC imported, one GSA ignored"
        );
        assert_eq!(imported.track.points.len(), 1);
        let point = imported.track.points[0];
        assert_eq!(point.utc, Some(1_700_000_000));
        assert_eq!(point.height_m, Some(80.0));
        assert!((point.speed_kmh.expect("speed") - 42.5).abs() < 0.05);
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
        assert_eq!(
            to_nmea(&track).lines().count(),
            3,
            "GGA, GSA and RMC for the one point that has a position"
        );
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
    fn the_csv_date_and_time_agree_with_the_iso_rendering() {
        let seconds = 1_700_000_000;
        let (date, time) = utc_date_and_time(seconds);
        assert_eq!(date, "2023-11-14");
        assert_eq!(time, "22:13:20");
        assert_eq!(format!("{date}T{time}Z"), utc_iso(seconds));
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
