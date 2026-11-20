//! Importing GPX 1.0/1.1 and NMEA 0183 logs back into [`Track`].
//!
//! Together with [`crate::log::export`], this gives the toolkit round-trip
//! interoperability with standard GPS files and ezTour exports without adding
//! any external XML parser crate.

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
/// # Errors
///
/// Returns [`Error::Log`] when the text does not contain a `<gpx` root element,
/// has an unclosed `<trkpt>` tag, or carries invalid coordinates.
pub fn import_gpx(text: &str) -> Result<Track> {
    let cleaned = strip_xml_comments(text);
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

/// Parse NMEA 0183 text (`$GPRMC` and `$GPGGA` sentences) into a [`Track`].
///
/// Each line's NMEA checksum is verified via [`parse_sentence`]; malformed
/// lines, broken checksums, unsupported sentence kinds, and no-fix sentences
/// (`V` in RMC, quality `0` in GGA) are skipped cleanly. Consecutive `$GPRMC`
/// and `$GPGGA` sentences sharing the same `HHMMSS` timestamp are merged so
/// both date/speed (RMC) and altitude (GGA) survive together.
///
/// # Errors
///
/// Currently infallible for well-typed `&str` input, returning `Result<Track>`
/// for symmetry with [`import_gpx`].
pub fn import_nmea(text: &str) -> Result<Track> {
    let mut points: Vec<TrackPoint> = Vec::new();
    let mut last_hhmmss: Option<String> = None;

    for raw_line in text.lines() {
        let line = raw_line.trim();
        if !line.starts_with('$') {
            continue;
        }
        let Ok(sentence) = parse_sentence(line) else {
            continue;
        };
        match sentence.kind.as_str() {
            "GPRMC" => {
                let Some((hhmmss, parsed)) = parse_gprmc(&sentence.fields) else {
                    continue;
                };
                if last_hhmmss.as_deref() == Some(hhmmss.as_str())
                    && let Some(prev) = points.last_mut()
                {
                    if prev.utc.is_none() {
                        prev.utc = parsed.utc;
                    }
                    if prev.speed_kmh.is_none() {
                        prev.speed_kmh = parsed.speed_kmh;
                    }
                    continue;
                }
                last_hhmmss = Some(hhmmss);
                points.push(parsed);
            }
            "GPGGA" => {
                let Some((hhmmss, parsed)) = parse_gpgga(&sentence.fields) else {
                    continue;
                };
                if last_hhmmss.as_deref() == Some(hhmmss.as_str())
                    && let Some(prev) = points.last_mut()
                {
                    if prev.height_m.is_none() {
                        prev.height_m = parsed.height_m;
                    }
                    continue;
                }
                last_hhmmss = Some(hhmmss);
                points.push(parsed);
            }
            _ => {}
        }
    }

    Ok(Track {
        points,
        damaged_blocks: Vec::new(),
    })
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
    let sign = if hemi == pos_hemi.encode_utf8(&mut [0u8; 4]) {
        1.0
    } else if hemi == neg_hemi.encode_utf8(&mut [0u8; 4]) {
        -1.0
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
// Lightweight GPX XML helpers
// ---------------------------------------------------------------------------

fn strip_xml_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<!--") {
        out.push_str(rest.get(..start).unwrap_or_default());
        let after = rest.get(start + 4..).unwrap_or_default();
        if let Some(end) = after.find("-->") {
            rest = after.get(end + 3..).unwrap_or_default();
        } else {
            rest = "";
            break;
        }
    }
    out.push_str(rest);
    out
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
        assert_eq!(imported.points.len(), 2);

        for (got, want) in imported.points.iter().zip(&original.points) {
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
        let track = import_nmea(&lines.join("\r\n")).expect("import nmea");
        assert_eq!(track.points.len(), 1);
        let p = track.points[0];
        assert_eq!(p.utc, Some(1_700_000_000));
        assert_eq!(p.height_m, Some(80.5));
        assert!((p.speed_kmh.unwrap() - 18.52).abs() < 0.01);
        assert!((p.latitude.unwrap() - 35.681_236).abs() < 1e-4);
        assert!((p.longitude.unwrap() - 139.767_125).abs() < 1e-4);
    }
}
