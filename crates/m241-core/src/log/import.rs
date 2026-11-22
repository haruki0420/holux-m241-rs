//! Importing GPX 1.0/1.1 and NMEA 0183 logs back into [`Track`].
//!
//! Reads `<trkpt>` elements (with `<ele>`, `<time>`, `<speed>`) and `$GPRMC` /
//! `$GPGGA` sentences, and nothing else. `<wpt>` waypoints are **skipped**: they
//! are a different kind of object (a saved place, not a recorded position), and
//! [`crate::log::export::to_gpx`] writing them is a one-way export until
//! something decides what a waypoint should become in a track. No external XML
//! parser is used, so the GPX reader is a string walk over the tags it needs.
//!
//! `<speed_kmh>` is this toolkit's own extension — [`crate::log::export::to_gpx`]
//! writes it beside the standard `<speed>` so a round trip does not lose the
//! km/h value it stores. It is read first, before the standard `<speed>` (GPX
//! 1.0, m/s), so a file that carries both keeps the exact value rather than a
//! converted one. A GPX 1.1 `<speed>` has no unit in the specification; it is
//! read as m/s, which is what GPX 1.0 defines and what most writers emit.

use crate::error::{Error, Result};
use crate::log::exif::days_from_civil;
use crate::protocol::pmtk::parse_sentence;
use crate::types::{Track, TrackPoint};

/// Parse a GPX 1.0 or 1.1 XML document into a [`Track`].
///
/// Reads every `<trkpt lat="..." lon="...">` element in document order along
/// with optional `<ele>`, `<time>` (ISO-8601 UTC `YYYY-MM-DDTHH:MM:SSZ`, plus
/// optional fractional seconds or explicit `+HH:MM`/`-HH:MM` offset), `<speed>`
/// (GPX 1.0 m/s, converted to km/h), and `<speed_kmh>` (written by
/// [`crate::log::export::to_gpx`]).
///
/// `<wpt>` waypoints and `<name>` elements are ignored; comments are removed
/// before anything is read.
///
/// # Errors
///
/// Returns [`Error::Log`] when the text does not contain a `<gpx` root element,
/// has an unclosed `<trkpt>` tag, has an `<!--` that is never closed, or carries
/// invalid coordinates. An unclosed comment is an error rather than "the rest
/// of the file is gone": the points after it would otherwise vanish silently
/// and the result would look like a track that simply ended.
pub fn import_gpx(text: &str) -> Result<Track> {
    let cleaned = strip_xml_comments(text)?;
    if !cleaned.contains("<gpx") {
        return Err(Error::Log {
            message: String::from("input is not a GPX document (missing <gpx element)"),
            offset: None,
        });
    }

    let mut points = Vec::new();
    let mut cursor = 0usize;

    while let Some(rel_start) = cleaned
        .get(cursor..)
        .and_then(|s| find_tag_start(s, "trkpt"))
    {
        let tag_start = cursor + rel_start;
        let Some(after_tag) = cleaned.get(tag_start..) else {
            break;
        };
        let Some(open_end_rel) = find_attr_tag_end(after_tag) else {
            return Err(Error::Log {
                message: String::from("unclosed <trkpt> start tag"),
                offset: Some(tag_start),
            });
        };
        let Some(open_tag) = after_tag.get(..=open_end_rel) else {
            return Err(Error::Log {
                message: String::from("unclosed <trkpt> start tag"),
                offset: Some(tag_start),
            });
        };
        let self_closing = open_tag.ends_with("/>");
        let (lat, lon) = parse_lat_lon_attrs(open_tag, tag_start)?;

        let body_start = tag_start + open_end_rel + 1;
        let (body, next_cursor) = if self_closing {
            ("", body_start)
        } else {
            let Some(close_rel) = cleaned.get(body_start..).and_then(|s| s.find("</trkpt>")) else {
                return Err(Error::Log {
                    message: String::from("missing closing </trkpt> tag"),
                    offset: Some(tag_start),
                });
            };
            let body_end = body_start + close_rel;
            let body = cleaned.get(body_start..body_end).unwrap_or_default();
            (body, body_end + "</trkpt>".len())
        };

        let height_m = extract_element(body, "ele")
            .map(|raw| {
                raw.trim()
                    .parse::<f32>()
                    .ok()
                    .filter(|v| v.is_finite())
                    .ok_or_else(|| Error::Log {
                        message: format!("invalid <ele> value {raw:?}"),
                        offset: Some(tag_start),
                    })
            })
            .transpose()?;

        let utc = extract_element(body, "time")
            .map(|raw| {
                parse_iso8601_utc(raw.trim()).ok_or_else(|| Error::Log {
                    message: format!("invalid <time> value {raw:?}"),
                    offset: Some(tag_start),
                })
            })
            .transpose()?;

        let speed_kmh = if let Some(raw) = extract_element(body, "speed_kmh") {
            Some(parse_finite_f32(raw.trim(), "speed_kmh", tag_start)?)
        } else if let Some(raw) = extract_element(body, "speed") {
            // GPX 1.0 <speed> is in m/s.
            let mps = parse_finite_f32(raw.trim(), "speed", tag_start)?;
            Some(mps * 3.6)
        } else {
            None
        };

        points.push(TrackPoint {
            utc,
            latitude: Some(lat),
            longitude: Some(lon),
            height_m,
            speed_kmh,
            rcr: None,
        });
        cursor = next_cursor;
    }

    Ok(Track {
        points,
        damaged_blocks: Vec::new(),
    })
}

/// What an NMEA import read, kept and dropped.
///
/// The counts tell an empty capture from one whose every sentence was rejected.
#[derive(Debug, Clone)]
pub struct NmeaImport {
    pub track: Track,
    /// `$GPRMC` / `$GPGGA` lines whose fields parsed, including a merged pair.
    pub imported: usize,
    /// `$GPRMC` / `$GPGGA` lines that produced no point: a broken checksum, a
    /// no-fix sentence, or missing coordinates.
    pub dropped: usize,
    /// Lines never turned into a point: blank, free text, another kind.
    pub ignored: usize,
}

/// Parse NMEA 0183 text (`$GPRMC` and `$GPGGA` sentences) into a [`NmeaImport`].
///
/// Malformed lines, broken checksums, unsupported kinds and no-fix sentences are
/// skipped and counted; sentences sharing a `HHMMSS` are merged so date/speed
/// (RMC) and altitude (GGA) survive together. Infallible for well-typed input;
/// `Result` mirrors [`import_gpx`].
pub fn import_nmea(text: &str) -> Result<NmeaImport> {
    let mut points: Vec<TrackPoint> = Vec::new();
    let mut last_bucket: Option<String> = None;
    let mut imported = 0usize;
    let mut dropped = 0usize;
    let mut ignored = 0usize;

    for raw_line in text.lines() {
        let line = raw_line.trim();
        if !line.starts_with('$') {
            ignored += 1;
            continue;
        }
        let Ok(sentence) = parse_sentence(line) else {
            dropped += 1;
            continue;
        };
        let parsed = match sentence.kind.as_str() {
            // A refused sentence (no fix, bad fields) is dropped by the merge
            // and, deliberately, does not touch `last_bucket`: a `V` line
            // between two good ones must not break their pairing.
            "GPRMC" => parse_gprmc(&sentence.fields)
                .map(|(bucket, point)| Merged::Rmc(Rmc { bucket, point })),
            "GPGGA" => parse_gpgga(&sentence.fields)
                .map(|(bucket, point)| Merged::Gga(Gga { bucket, point })),
            // A sentence of another kind is not one this reader failed to read;
            // it carries no position this importer could use.
            _ => {
                ignored += 1;
                continue;
            }
        };
        if parsed.is_some() {
            imported += 1;
        } else {
            dropped += 1;
        }
        merge_sentence(&mut points, &mut last_bucket, parsed);
    }

    Ok(NmeaImport {
        track: Track {
            points,
            damaged_blocks: Vec::new(),
        },
        imported,
        dropped,
        ignored,
    })
}

/// One parsed RMC sentence: its `HHMMSS` bucket and the point it carries.
struct Rmc {
    bucket: String,
    point: TrackPoint,
}

/// One parsed GGA sentence.
struct Gga {
    bucket: String,
    point: TrackPoint,
}

/// A sentence that parsed, of either kind.
enum Merged {
    Rmc(Rmc),
    Gga(Gga),
}

/// Add `sentence` to `points`, merging it into the previous point when both
/// share the `HHMMSS` bucket.
///
/// `last_bucket` is the bucket of the point at the end of `points`, and it is
/// the whole key: the RMC date is not part of it, so two sentences with the
/// same `HHMMSS` on different days (a log that spans midnight) would merge. NMEA
/// writes `HHMMSS` for both sentence kinds and the date only in RMC, so this is
/// the finest key the two kinds share.
///
/// Only the tags each kind owns are copied — `utc` and `speed_kmh` from RMC,
/// `height_m` from GGA — so the order the kinds arrive in does not decide which
/// fields survive.
fn merge_sentence(
    points: &mut Vec<TrackPoint>,
    last_bucket: &mut Option<String>,
    parsed: Option<Merged>,
) {
    let Some(merged) = parsed else {
        return;
    };
    let (bucket, point, height_only) = match merged {
        Merged::Rmc(rmc) => (rmc.bucket, rmc.point, false),
        Merged::Gga(gga) => (gga.bucket, gga.point, true),
    };
    let same_bucket = last_bucket.as_deref() == Some(bucket.as_str());
    if same_bucket && let Some(prev) = points.last_mut() {
        if height_only {
            if prev.height_m.is_none() {
                prev.height_m = point.height_m;
            }
        } else {
            if prev.utc.is_none() {
                prev.utc = point.utc;
            }
            if prev.speed_kmh.is_none() {
                prev.speed_kmh = point.speed_kmh;
            }
        }
        return;
    }
    *last_bucket = Some(bucket);
    points.push(point);
}

fn parse_gprmc(fields: &[String]) -> Option<(String, TrackPoint)> {
    let time_str = fields.first()?.trim();
    let status = fields.get(1)?.trim();
    if status != "A" {
        return None;
    }
    let lat_str = fields.get(2)?.trim();
    let lat_hemi = fields.get(3)?.trim();
    let lon_str = fields.get(4)?.trim();
    let lon_hemi = fields.get(5)?.trim();
    let speed_str = fields.get(6).map(String::as_str).unwrap_or("").trim();
    let date_str = fields.get(8)?.trim();

    let (hhmmss, hour, minute, second) = parse_nmea_hhmmss(time_str)?;
    let (day, month, year) = parse_nmea_ddmmyy(date_str)?;
    let days = days_from_civil(year, month, day)?;
    let unix_i64 = days
        .checked_mul(86_400)?
        .checked_add(i64::from(hour) * 3_600 + i64::from(minute) * 60 + i64::from(second))?;
    let utc = u32::try_from(unix_i64).ok()?;

    let lat = parse_nmea_coord(lat_str, lat_hemi, 'N', 'S', 90.0)?;
    let lon = parse_nmea_coord(lon_str, lon_hemi, 'E', 'W', 180.0)?;
    let speed_kmh = if speed_str.is_empty() {
        None
    } else {
        let knots: f64 = speed_str.parse().ok()?;
        knots.is_finite().then_some((knots * 1.852) as f32)
    };

    Some((
        hhmmss,
        TrackPoint {
            utc: Some(utc),
            latitude: Some(lat),
            longitude: Some(lon),
            height_m: None,
            speed_kmh,
            rcr: None,
        },
    ))
}

fn parse_gpgga(fields: &[String]) -> Option<(String, TrackPoint)> {
    let time_str = fields.first()?.trim();
    let lat_str = fields.get(1)?.trim();
    let lat_hemi = fields.get(2)?.trim();
    let lon_str = fields.get(3)?.trim();
    let lon_hemi = fields.get(4)?.trim();
    let quality = fields.get(5)?.trim();
    if quality.is_empty() || quality == "0" {
        return None;
    }

    let (hhmmss, _, _, _) = parse_nmea_hhmmss(time_str)?;
    let lat = parse_nmea_coord(lat_str, lat_hemi, 'N', 'S', 90.0)?;
    let lon = parse_nmea_coord(lon_str, lon_hemi, 'E', 'W', 180.0)?;
    let height_m = fields
        .get(8)
        .map(String::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse::<f32>().ok())
        .filter(|v| v.is_finite());

    Some((
        hhmmss,
        TrackPoint {
            utc: None,
            latitude: Some(lat),
            longitude: Some(lon),
            height_m,
            speed_kmh: None,
            rcr: None,
        },
    ))
}

fn parse_nmea_hhmmss(text: &str) -> Option<(String, u32, u32, u32)> {
    let base = text.split('.').next()?;
    if base.len() < 6 || !base.is_ascii() {
        return None;
    }
    let hh: u32 = base.get(0..2)?.parse().ok()?;
    let mm: u32 = base.get(2..4)?.parse().ok()?;
    let ss: u32 = base.get(4..6)?.parse().ok()?;
    if hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    Some((base.get(0..6)?.to_owned(), hh, mm, ss))
}

fn parse_nmea_ddmmyy(text: &str) -> Option<(u32, u32, i64)> {
    if text.len() != 6 || !text.is_ascii() {
        return None;
    }
    let day: u32 = text.get(0..2)?.parse().ok()?;
    let month: u32 = text.get(2..4)?.parse().ok()?;
    let yy: i64 = text.get(4..6)?.parse().ok()?;
    // NMEA keeps two digits of year; it has no century of its own. 80 is where
    // the century rolls, so a logger's log stays in 1980-2079.
    let year = if yy >= 80 { 1900 + yy } else { 2000 + yy };
    Some((day, month, year))
}

fn parse_nmea_coord(
    raw: &str,
    hemi: &str,
    pos_hemi: char,
    neg_hemi: char,
    max_abs: f64,
) -> Option<f64> {
    if raw.is_empty() {
        return None;
    }
    // The hemisphere is a one-character string; anything else (empty, two
    // letters, lower case) is a sentence this parser cannot sign and is refused.
    let mut letters = hemi.chars();
    let first = letters.next();
    let sign = if first.is_some() && letters.next().is_none() {
        match first {
            Some(f) if f == pos_hemi => 1.0,
            Some(f) if f == neg_hemi => -1.0,
            _ => return None,
        }
    } else {
        return None;
    };

    let split_pos = match raw.find('.') {
        Some(dot) => dot.checked_sub(2)?,
        None => raw.len().checked_sub(2)?,
    };
    let (deg_str, min_str) = raw.split_at_checked(split_pos)?;
    let deg: f64 = deg_str.parse().ok()?;
    let min: f64 = min_str.parse().ok()?;
    if !deg.is_finite() || !min.is_finite() || min < 0.0 || min >= 60.0 {
        return None;
    }
    let decimal = sign * (deg + min / 60.0);
    (decimal.is_finite() && decimal.abs() <= max_abs).then_some(decimal)
}

// ---------------------------------------------------------------------------
// String-based GPX search (no XML parser is pulled in; see the module doc)
// ---------------------------------------------------------------------------

/// Remove `<!-- ... -->` comments, keeping everything else as it was.
///
/// # Errors
///
/// [`Error::Log`] when an `<!--` is never closed. Cutting the input there would
/// drop every point after it and return a short track that looks complete, so
/// the malformed document is refused instead.
fn strip_xml_comments(text: &str) -> Result<String> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<!--") {
        out.push_str(rest.get(..start).unwrap_or_default());
        let after = rest.get(start + 4..).unwrap_or_default();
        let Some(end) = after.find("-->") else {
            return Err(Error::Log {
                message: String::from("a <!-- comment is never closed with -->"),
                offset: Some(out.len() + start),
            });
        };
        rest = after.get(end + 3..).unwrap_or_default();
    }
    out.push_str(rest);
    Ok(out)
}

fn find_tag_start(haystack: &str, tag: &str) -> Option<usize> {
    let needle = format!("<{tag}");
    let mut offset = 0usize;
    while let Some(found) = haystack.get(offset..).and_then(|s| s.find(&needle)) {
        let pos = offset + found;
        let next_char = haystack.get(pos + needle.len()..)?.chars().next()?;
        if next_char.is_ascii_whitespace() || next_char == '>' || next_char == '/' {
            return Some(pos);
        }
        offset = pos + needle.len();
    }
    None
}

fn find_attr_tag_end(tag_text: &str) -> Option<usize> {
    let mut quote: Option<char> = None;
    for (idx, ch) in tag_text.char_indices() {
        match quote {
            Some(q) if ch == q => quote = None,
            Some(_) => {}
            None if ch == '"' || ch == '\'' => quote = Some(ch),
            None if ch == '>' => return Some(idx),
            None => {}
        }
    }
    None
}

fn parse_lat_lon_attrs(tag: &str, offset: usize) -> Result<(f64, f64)> {
    let lat_str = find_attr(tag, "lat").ok_or_else(|| Error::Log {
        message: String::from("<trkpt> is missing lat attribute"),
        offset: Some(offset),
    })?;
    let lon_str = find_attr(tag, "lon").ok_or_else(|| Error::Log {
        message: String::from("<trkpt> is missing lon attribute"),
        offset: Some(offset),
    })?;
    let lat: f64 = lat_str.trim().parse().map_err(|_| Error::Log {
        message: format!("invalid <trkpt> lat {lat_str:?}"),
        offset: Some(offset),
    })?;
    let lon: f64 = lon_str.trim().parse().map_err(|_| Error::Log {
        message: format!("invalid <trkpt> lon {lon_str:?}"),
        offset: Some(offset),
    })?;
    if !lat.is_finite() || lat.abs() > 90.0 {
        return Err(Error::Log {
            message: format!("out-of-range <trkpt> lat {lat}"),
            offset: Some(offset),
        });
    }
    if !lon.is_finite() || lon.abs() > 180.0 {
        return Err(Error::Log {
            message: format!("out-of-range <trkpt> lon {lon}"),
            offset: Some(offset),
        });
    }
    Ok((lat, lon))
}

fn find_attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let mut rest = tag;
    while let Some(pos) = rest.find(name) {
        let before_ok = pos == 0
            || rest
                .get(..pos)
                .and_then(|s| s.chars().next_back())
                .is_some_and(char::is_whitespace);
        let after = rest.get(pos + name.len()..)?;
        if before_ok {
            let trimmed = after.trim_start();
            if let Some(after_eq) = trimmed.strip_prefix('=') {
                let after_eq = after_eq.trim_start();
                let quote = after_eq.chars().next()?;
                if quote == '"' || quote == '\'' {
                    let inner = after_eq.get(1..)?;
                    let end = inner.find(quote)?;
                    return inner.get(..end);
                }
            }
        }
        rest = after;
    }
    None
}

fn extract_element<'a>(body: &'a str, name: &str) -> Option<&'a str> {
    let start = find_tag_start(body, name)?;
    let after_name = body.get(start..)?;
    let open_end = after_name.find('>')?;
    if after_name.get(..open_end)?.ends_with('/') {
        return None;
    }
    let content_start = start + open_end + 1;
    let close_tag = format!("</{name}>");
    let close_rel = body.get(content_start..)?.find(&close_tag)?;
    body.get(content_start..content_start + close_rel)
}

fn parse_finite_f32(raw: &str, name: &str, offset: usize) -> Result<f32> {
    raw.parse::<f32>()
        .ok()
        .filter(|v| v.is_finite())
        .ok_or_else(|| Error::Log {
            message: format!("invalid <{name}> value {raw:?}"),
            offset: Some(offset),
        })
}

fn parse_iso8601_utc(text: &str) -> Option<u32> {
    let (date_str, time_and_zone) = text.split_once('T').or_else(|| text.split_once('t'))?;
    let mut date_parts = date_str.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: u32 = date_parts.next()?.parse().ok()?;
    let day: u32 = date_parts.next()?.parse().ok()?;
    if date_parts.next().is_some() {
        return None;
    }

    let (time_str, tz_offset_secs) = if let Some(base) = time_and_zone
        .strip_suffix('Z')
        .or_else(|| time_and_zone.strip_suffix('z'))
    {
        (base, 0i64)
    } else if let Some((base, off)) = time_and_zone.rsplit_once('+') {
        (base, parse_tz_offset(off, 1)?)
    } else if let Some((base, off)) = time_and_zone.rsplit_once('-') {
        (base, parse_tz_offset(off, -1)?)
    } else {
        (time_and_zone, 0i64)
    };

    let mut clock = time_str.split(':');
    let hour: u32 = clock.next()?.parse().ok()?;
    let minute: u32 = clock.next()?.parse().ok()?;
    let sec_part = clock.next()?;
    if clock.next().is_some() {
        return None;
    }
    let sec_whole = sec_part.split('.').next()?;
    let second: u32 = sec_whole.parse().ok()?;
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }

    let days = days_from_civil(year, month, day)?;
    let local_secs = days
        .checked_mul(86_400)?
        .checked_add(i64::from(hour) * 3_600 + i64::from(minute) * 60 + i64::from(second))?;
    let utc_secs = local_secs.checked_sub(tz_offset_secs)?;
    u32::try_from(utc_secs).ok()
}

fn parse_tz_offset(text: &str, sign: i64) -> Option<i64> {
    let (hh_str, mm_str) = text.split_once(':')?;
    let hh: i64 = hh_str.parse().ok()?;
    let mm: i64 = mm_str.parse().ok()?;
    if !(0..=23).contains(&hh) || !(0..=59).contains(&mm) {
        return None;
    }
    Some(sign * (hh * 3_600 + mm * 60))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::export::{to_gpx, to_nmea};

    fn sample_track() -> Track {
        Track {
            points: vec![
                TrackPoint {
                    utc: Some(1_700_000_000),
                    latitude: Some(35.681_236),
                    longitude: Some(139.767_125),
                    height_m: Some(42.5),
                    speed_kmh: Some(18.5),
                    rcr: Some(0x0008),
                },
                TrackPoint {
                    utc: Some(1_700_000_010),
                    latitude: Some(34.693_7),
                    longitude: Some(135.502_3),
                    height_m: Some(15.25),
                    speed_kmh: Some(36.0),
                    rcr: Some(0),
                },
            ],
            damaged_blocks: Vec::new(),
        }
    }

    #[test]
    fn import_gpx_round_trips_with_export_to_gpx() {
        let original = sample_track();
        let gpx = to_gpx(&original);
        let imported = import_gpx(&gpx).expect("import exported gpx");
        assert_eq!(imported.points.len(), 2);

        for (got, want) in imported.points.iter().zip(&original.points) {
            assert_eq!(got.utc, want.utc);
            assert!(
                (got.latitude.unwrap() - want.latitude.unwrap()).abs() < 1e-8,
                "lat mismatch"
            );
            assert!(
                (got.longitude.unwrap() - want.longitude.unwrap()).abs() < 1e-8,
                "lon mismatch"
            );
            assert!((got.height_m.unwrap() - want.height_m.unwrap()).abs() < 0.01);
            assert!((got.speed_kmh.unwrap() - want.speed_kmh.unwrap()).abs() < 0.01);
        }
    }

    #[test]
    fn import_gpx_supports_gpx10_speed_and_self_closing_and_offsets() {
        let gpx10 = r#"<?xml version="1.0" encoding="UTF-8"?>
<gpx version="1.0" creator="test">
  <trk><trkseg>
    <trkpt lon='-70.25' lat="-35.5">
      <ele>120.5</ele>
      <time>2023-11-15T07:13:20.500+09:00</time>
      <speed>10.0</speed>
    </trkpt>
    <trkpt lat="35.681236" lon="139.767125"/>
  </trkseg></trk>
</gpx>"#;
        let track = import_gpx(gpx10).expect("import gpx 1.0");
        assert_eq!(track.points.len(), 2);
        assert_eq!(track.points[0].utc, Some(1_700_000_000));
        assert!((track.points[0].latitude.unwrap() - (-35.5)).abs() < 1e-9);
        assert!((track.points[0].longitude.unwrap() - (-70.25)).abs() < 1e-9);
        assert_eq!(track.points[0].height_m, Some(120.5));
        assert!((track.points[0].speed_kmh.unwrap() - 36.0).abs() < 1e-4);

        assert_eq!(track.points[1].utc, None);
        assert_eq!(track.points[1].height_m, None);
        assert!((track.points[1].latitude.unwrap() - 35.681_236).abs() < 1e-9);
    }

    #[test]
    fn a_gpx10_speed_in_metres_per_second_arrives_as_km_per_hour() {
        // The conversion is the whole of the GPX 1.0 `<speed>` contract, so it
        // gets its own assertion (and its own test): a file with a single point
        // leaves nothing else for a survivor to hide behind.
        let gpx = r#"<gpx version="1.0"><trk><trkseg>
            <trkpt lat="35.681236" lon="139.767125"><speed>10.0</speed></trkpt>
        </trkseg></trk></gpx>"#;
        let track = import_gpx(gpx).expect("import gpx 1.0");
        let speed = track.points[0].speed_kmh.expect("a speed");
        assert!(
            (speed - 36.0).abs() < 1e-4,
            "10 m/s is 36 km/h, got {speed}"
        );
    }

    #[test]
    fn the_speed_kmh_extension_wins_over_the_standard_speed() {
        // `to_gpx` writes both; reading the extension first keeps the km/h
        // value exact instead of converting the m/s one back.
        let gpx = r#"<gpx version="1.1"><trk><trkseg>
            <trkpt lat="0.0" lon="0.0"><speed>1.0</speed><speed_kmh>42.5</speed_kmh></trkpt>
        </trkseg></trk></gpx>"#;
        let track = import_gpx(gpx).expect("import");
        assert_eq!(track.points[0].speed_kmh, Some(42.5));
    }

    #[test]
    fn an_unclosed_xml_comment_is_an_error_not_a_truncated_track() {
        let doc = r#"<gpx><trk><trkseg>
            <trkpt lat="35.0" lon="139.0"/><!-- never closed
        </trkseg></trk></gpx>"#;
        let err = import_gpx(doc).expect_err("an unclosed comment is malformed");
        assert!(matches!(err, Error::Log { .. }), "got {err:?}");
        assert!(err.to_string().contains("never closed"), "got {err}");
    }

    #[test]
    fn a_closed_comment_is_removed_without_touching_the_points() {
        let doc = r#"<gpx><!-- <trkpt lat="91.0" lon="0.0"/> --><trk><trkseg>
            <trkpt lat="35.0" lon="139.0"/>
        </trkseg></trk></gpx>"#;
        let track = import_gpx(doc).expect("import");
        assert_eq!(track.points.len(), 1, "a commented-out point is not read");
        assert!((track.points[0].latitude.unwrap() - 35.0).abs() < 1e-9);
    }

    #[test]
    fn waypoints_are_not_read_as_track_points() {
        // to_gpx writes `<wpt>` for saved places; import reads `<trkpt>` only.
        // This test pins that choice so a change to either side is visible.
        let doc = r#"<gpx><wpt lat="35.0" lon="139.0"/><trk><trkseg>
            <trkpt lat="35.1" lon="139.1"/>
        </trkseg></trk></gpx>"#;
        let track = import_gpx(doc).expect("import");
        assert_eq!(track.points.len(), 1);
        assert!((track.points[0].latitude.unwrap() - 35.1).abs() < 1e-9);
    }

    #[test]
    fn import_gpx_rejects_malformed_input() {
        assert!(matches!(import_gpx("hello"), Err(Error::Log { .. })));
        let bad_lat = r#"<gpx><trk><trkseg><trkpt lat="91.0" lon="0.0"/></trkseg></trk></gpx>"#;
        assert!(matches!(import_gpx(bad_lat), Err(Error::Log { .. })));
        let unclosed = r#"<gpx><trk><trkseg><trkpt lat="35.0" lon="139.0">"#;
        assert!(matches!(import_gpx(unclosed), Err(Error::Log { .. })));
    }

    #[test]
    fn import_nmea_round_trips_with_export_to_nmea() {
        let original = sample_track();
        let nmea = to_nmea(&original);
        let imported = import_nmea(&nmea).expect("import exported nmea");
        assert_eq!(
            (imported.imported, imported.dropped, imported.ignored),
            (2, 0, 0),
            "both exported sentences are usable"
        );
        assert_eq!(imported.track.points.len(), 2);

        for (got, want) in imported.track.points.iter().zip(&original.points) {
            assert_eq!(got.utc, want.utc);
            assert!(
                (got.latitude.unwrap() - want.latitude.unwrap()).abs() < 1e-4,
                "lat {:?} vs {:?}",
                got.latitude,
                want.latitude
            );
            assert!(
                (got.longitude.unwrap() - want.longitude.unwrap()).abs() < 1e-4,
                "lon {:?} vs {:?}",
                got.longitude,
                want.longitude
            );
            assert!((got.speed_kmh.unwrap() - want.speed_kmh.unwrap()).abs() < 0.05);
        }
    }

    #[test]
    fn import_nmea_merges_gga_and_rmc_and_skips_bad_or_void_lines() {
        use crate::protocol::pmtk::sentence;
        let lines = [
            // Bad checksum -> skipped
            String::from("$GPRMC,221320.00,A,3540.8742,N,13946.0275,E,10.00,,141123,,*00"),
            // Void fix -> skipped
            sentence("GPRMC,221320.00,V,,,,,0.00,,141123,,"),
            // GGA no-fix (quality 0) -> skipped
            sentence("GPGGA,221320.00,3540.8742,N,13946.0275,E,0,00,99.9,80.0,M,0.0,M,,"),
            // Valid GGA + RMC for same timestamp -> merged into 1 point with altitude + speed + utc
            sentence("GPGGA,221320.00,3540.8742,N,13946.0275,E,1,08,0.9,80.5,M,0.0,M,,"),
            sentence("GPRMC,221320.00,A,3540.8742,N,13946.0275,E,10.00,,141123,,"),
            // Unrelated sentence -> ignored
            sentence("GPGSA,A,3,01,02,03,,,,,,,,,1.5,1.0,1.2"),
            String::from("not an nmea line"),
        ];
        let track = import_nmea(&lines.join("\r\n")).expect("import nmea").track;
        assert_eq!(track.points.len(), 1);
        let p = track.points[0];
        assert_eq!(p.utc, Some(1_700_000_000));
        assert_eq!(p.height_m, Some(80.5));
        assert!((p.speed_kmh.unwrap() - 18.52).abs() < 0.01);
        assert!((p.latitude.unwrap() - 35.681_236).abs() < 1e-4);
        assert!((p.longitude.unwrap() - 139.767_125).abs() < 1e-4);
    }

    #[test]
    fn an_rmc_before_its_gga_still_gets_the_altitude() {
        use crate::protocol::pmtk::sentence;
        // NMEA order is not fixed; a receiver that emits RMC first must still
        // yield a point with the date, the speed and the altitude.
        let text = [
            sentence("GPRMC,221320.00,A,3540.8742,N,13946.0275,E,10.00,,141123,,"),
            sentence("GPGGA,221320.00,3540.8742,N,13946.0275,E,1,08,0.9,80.5,M,0.0,M,,"),
        ]
        .join("\r\n");
        let track = import_nmea(&text).expect("import nmea").track;
        assert_eq!(track.points.len(), 1);
        let p = track.points[0];
        assert_eq!(p.utc, Some(1_700_000_000), "the RMC supplies the date");
        assert_eq!(
            p.height_m,
            Some(80.5),
            "the later GGA supplies the altitude"
        );
        assert!((p.speed_kmh.unwrap() - 18.52).abs() < 0.01);
    }

    #[test]
    fn a_void_rmc_between_two_good_ones_changes_nothing() {
        use crate::protocol::pmtk::sentence;
        // The `V` sentence repeats the same `HHMMSS` as the good RMC around it
        // and carries different coordinates. If it were read, the merge would
        // patch nothing (a void RMC has no position) and, more importantly, it
        // must not reset the pairing key for the GGA that follows.
        let text = [
            sentence("GPRMC,221320.00,A,3540.8742,N,13946.0275,E,10.00,,141123,,"),
            sentence("GPRMC,221320.00,V,0000.0000,N,00000.0000,E,0.00,,141123,,"),
            sentence("GPGGA,221320.00,3540.8742,N,13946.0275,E,1,08,0.9,80.5,M,0.0,M,,"),
        ]
        .join("\r\n");
        let track = import_nmea(&text).expect("import nmea").track;
        assert_eq!(
            track.points.len(),
            1,
            "one place, not a phantom record for the void fix"
        );
        let p = track.points[0];
        assert!((p.latitude.unwrap() - 35.681_236).abs() < 1e-4);
        assert_eq!(p.height_m, Some(80.5));
        assert_eq!(p.utc, Some(1_700_000_000));
    }

    #[test]
    fn a_void_rmc_with_no_following_sentence_is_not_a_point() {
        use crate::protocol::pmtk::sentence;
        let text = sentence("GPRMC,221320.00,V,3540.8742,N,13946.0275,E,10.00,,141123,,");
        let track = import_nmea(&text).expect("import nmea").track;
        assert!(
            track.points.is_empty(),
            "a no-fix RMC must be dropped, not kept and overwritten: {:?}",
            track.points
        );
    }

    #[test]
    fn a_gga_of_quality_zero_is_not_a_point() {
        use crate::protocol::pmtk::sentence;
        let text = sentence("GPGGA,221320.00,3540.8742,N,13946.0275,E,0,00,99.9,80.0,M,0.0,M,,");
        assert!(import_nmea(&text).expect("import").track.points.is_empty());
    }

    #[test]
    fn nmea_hemispheres_must_be_one_of_the_four_letters() {
        // A two-letter or missing hemisphere cannot be signed; the sentence is
        // dropped rather than read as the northern/eastern hemisphere.
        let bad = [
            "$GPRMC,120000.00,A,3540.8742,NN,13946.0275,E,0.00,,141123,,",
            "$GPRMC,120000.00,A,3540.8742,x,13946.0275,E,0.00,,141123,,",
            "$GPRMC,120000.00,A,3540.8742,a,13946.0275,E,0.00,,141123,,",
        ];
        for body in bad {
            let line = crate::protocol::pmtk::sentence(body);
            assert!(
                import_nmea(&line).expect("import").track.points.is_empty(),
                "an unreadable hemisphere must drop the sentence: {body}"
            );
        }
        let south = crate::protocol::pmtk::sentence(
            "GPRMC,120000.00,A,3540.8742,S,13946.0275,W,0.00,,141123,,",
        );
        let track = import_nmea(&south).expect("import").track;
        assert!(track.points[0].latitude.unwrap() < 0.0);
        assert!(track.points[0].longitude.unwrap() < 0.0);
    }

    #[test]
    fn an_nmea_import_reports_imported_dropped_and_ignored_lines() {
        // One line of each outcome, so a reader can tell "nothing arrived" from
        // "everything was rejected" without inspecting the track.
        let lines = [
            crate::protocol::pmtk::sentence(
                "GPRMC,221320.00,A,3540.8742,N,13946.0275,E,10.00,,141123,,",
            ),
            // Broken checksum: dropped.
            String::from("$GPRMC,221320.00,A,3540.8742,N,13946.0275,E,10.00,,141123,,*00"),
            // No fix: dropped.
            crate::protocol::pmtk::sentence("GPRMC,221320.00,V,,,,,0.00,,141123,,"),
            // Another sentence kind: not this reader's, counted as ignored.
            crate::protocol::pmtk::sentence("GPGSA,A,3,01,02,03,,,,,,,,,1.5,1.0,1.2"),
            // Blank and free text: ignored.
            String::new(),
            String::from("not an nmea line"),
        ];
        let imported = import_nmea(&lines.join("\r\n")).expect("import nmea");
        assert_eq!(imported.imported, 1);
        assert_eq!(imported.dropped, 2);
        assert_eq!(imported.ignored, 3);
        assert_eq!(imported.imported + imported.dropped + imported.ignored, 6);
        assert_eq!(imported.track.points.len(), 1);
    }

    #[test]
    fn an_nmea_import_counts_a_merged_pair_as_two_imported_sentences() {
        // The pair becomes one point, but both lines were read and accepted;
        // reporting one would make the counts disagree with the input.
        let text = [
            crate::protocol::pmtk::sentence(
                "GPGGA,221320.00,3540.8742,N,13946.0275,E,1,08,0.9,80.5,M,0.0,M,,",
            ),
            crate::protocol::pmtk::sentence(
                "GPRMC,221320.00,A,3540.8742,N,13946.0275,E,10.00,,141123,,",
            ),
        ]
        .join("\r\n");
        let imported = import_nmea(&text).expect("import nmea");
        assert_eq!(imported.imported, 2);
        assert_eq!(imported.dropped, 0);
        assert_eq!(imported.track.points.len(), 1);
    }

    #[test]
    fn an_empty_nmea_capture_is_zero_lines_not_an_error() {
        let imported = import_nmea("").expect("import empty nmea");
        assert!(imported.track.points.is_empty());
        assert_eq!(imported.imported + imported.dropped + imported.ignored, 0);
    }
}
