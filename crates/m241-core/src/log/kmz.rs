//! KMZ export: bundling KML 2.0 track geometry and photos into a Store ZIP.
//!
//! A KMZ archive is a standard PKZIP container with `doc.kml` at its root and
//! referenced photos under `photo/`. Using uncompressed Store (`method = 0`)
//! keeps this crate free of external compression dependencies while producing
//! valid archives accepted by Google Earth and standard ZIP tools.

use std::collections::HashSet;
use std::fmt::Write as _;

use crate::error::{Error, Result};
use crate::log::export::{DOCUMENT_NAME, escape_xml, utc_date, utc_iso};
use crate::types::Track;

/// ZIP local file header signature (`PK\x03\x04`).
const LFH_SIGNATURE: u32 = 0x0403_4B50;
/// ZIP central directory file header signature (`PK\x01\x02`).
const CDFH_SIGNATURE: u32 = 0x0201_4B50;
/// ZIP end of central directory record signature (`PK\x05\x06`).
const EOCD_SIGNATURE: u32 = 0x0605_4B50;
/// Compression method 0: uncompressed Store.
const METHOD_STORE: u16 = 0;
/// PKZIP version 2.0 ("made by" and "needed to extract").
const ZIP_VERSION_20: u16 = 20;
/// General purpose bit 11: filename and comment are UTF-8 encoded.
const FLAG_UTF8: u16 = 0x0800;

/// Default track style colour (`aabbggrr`): opacity `0.7` (`0xB3`) over purple,
/// matching the ezTour 10-colour palette and opacity (`07-eztour-spec.ja.md` §5).
const DEFAULT_LINE_COLOR: &str = "b3ff0080";
/// Default track polyline width in pixels (`07-eztour-spec.ja.md` §5).
const DEFAULT_LINE_WIDTH: u8 = 5;

/// A photo to embed in a KMZ archive under `photo/`.
#[derive(Debug, Clone, PartialEq)]
pub struct KmzPhoto {
    /// Original filename (sanitised before being placed in `photo/`).
    pub name: String,
    /// Latitude in signed decimal degrees.
    pub lat: f64,
    /// Longitude in signed decimal degrees.
    pub lon: f64,
    /// Optional altitude in metres.
    pub altitude_m: Option<f32>,
    /// Optional capture time as UTC Unix seconds.
    pub utc: Option<u32>,
    /// Raw JPEG bytes to store in the archive.
    pub jpeg_bytes: Vec<u8>,
}

/// Bundle `track` and `photos` into an uncompressed (Store) KMZ archive.
///
/// The resulting ZIP contains `doc.kml` (KML 2.0 with styled `<LineString>` and
/// a `<Folder><name>Photos</name>` of photo `<Placemark>` entries) and each
/// photo under `photo/<sanitised_name>`.
///
/// # Errors
///
/// Returns [`Error::State`] if a photo coordinate or altitude is non-finite or
/// out of range, or [`Error::Log`] if the archive exceeds ZIP32 limits.
pub fn export_kmz(track: &Track, photos: &[KmzPhoto]) -> Result<Vec<u8>> {
    let mut used_names = HashSet::with_capacity(photos.len());
    let mut resolved: Vec<(String, &KmzPhoto)> = Vec::with_capacity(photos.len());

    for photo in photos {
        validate_photo(photo)?;
        let sanitised = sanitize_photo_name(&photo.name);
        let unique = deduplicate_name(&sanitised, &mut used_names);
        resolved.push((unique, photo));
    }

    let kml = build_kml(track, &resolved);
    let kml_utc = track.time_span().map(|(_, end)| end);

    let mut entries: Vec<ZipInput<'_>> = Vec::with_capacity(1 + resolved.len());
    entries.push(ZipInput {
        path: String::from("doc.kml"),
        data: kml.as_bytes(),
        utc: kml_utc,
    });
    for (name, photo) in &resolved {
        entries.push(ZipInput {
            path: format!("photo/{name}"),
            data: &photo.jpeg_bytes,
            utc: photo.utc,
        });
    }

    write_zip_store(&entries)
}

fn validate_photo(photo: &KmzPhoto) -> Result<()> {
    if !photo.lat.is_finite() || photo.lat.abs() > 90.0 {
        return Err(Error::State(format!(
            "photo {:?} has invalid latitude {}",
            photo.name, photo.lat
        )));
    }
    if !photo.lon.is_finite() || photo.lon.abs() > 180.0 {
        return Err(Error::State(format!(
            "photo {:?} has invalid longitude {}",
            photo.name, photo.lon
        )));
    }
    if photo.altitude_m.is_some_and(|alt| !alt.is_finite()) {
        return Err(Error::State(format!(
            "photo {:?} has non-finite altitude",
            photo.name
        )));
    }
    Ok(())
}

/// Sanitise a photo filename so it cannot escape `photo/` or break XML/ZIP.
fn sanitize_photo_name(raw: &str) -> String {
    let candidate = raw
        .split(['/', '\\'])
        .rfind(|part| !part.is_empty() && *part != "." && *part != "..")
        .unwrap_or("photo.jpg");

    let mut cleaned = String::with_capacity(candidate.len());
    for ch in candidate.chars() {
        if ch.is_control() || matches!(ch, '/' | '\\' | ':' | '<' | '>' | '"' | '\'' | '&' | ']') {
            cleaned.push('_');
        } else {
            cleaned.push(ch);
        }
    }
    while cleaned.contains("..") {
        cleaned = cleaned.replace("..", "_");
    }
    let trimmed = cleaned.trim();
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        String::from("photo.jpg")
    } else {
        trimmed.to_owned()
    }
}

/// Ensure each archive entry under `photo/` is unique (case-insensitively).
fn deduplicate_name(base: &str, used_lower: &mut HashSet<String>) -> String {
    let lower = base.to_ascii_lowercase();
    if used_lower.insert(lower) {
        return base.to_owned();
    }

    let (stem, ext) = match base.rfind('.') {
        Some(dot) if dot > 0 => match (base.get(..dot), base.get(dot..)) {
            (Some(s), Some(e)) => (s, e),
            _ => (base, ""),
        },
        _ => (base, ""),
    };

    let mut counter = 1usize;
    loop {
        let candidate = format!("{stem}_{counter}{ext}");
        let candidate_lower = candidate.to_ascii_lowercase();
        if used_lower.insert(candidate_lower) {
            return candidate;
        }
        counter = counter.saturating_add(1);
    }
}

fn build_kml(track: &Track, photos: &[(String, &KmzPhoto)]) -> String {
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str("<kml xmlns=\"http://earth.google.com/kml/2.0\">\n");
    out.push_str("  <Document>\n");
    let _ = writeln!(out, "    <name>{}</name>", escape_xml(DOCUMENT_NAME));
    let _ = writeln!(
        out,
        "    <Style id=\"trackStyle\">\n      <LineStyle>\n        \
         <color>{DEFAULT_LINE_COLOR}</color>\n        <width>{DEFAULT_LINE_WIDTH}</width>\n      \
         </LineStyle>\n    </Style>"
    );
    out.push_str("    <Placemark>\n");
    let _ = writeln!(out, "      <name>{}</name>", escape_xml(DOCUMENT_NAME));
    out.push_str("      <styleUrl>#trackStyle</styleUrl>\n");
    out.push_str("      <LineString>\n        <tessellate>1</tessellate>\n");
    out.push_str("        <coordinates>\n");
    for point in &track.points {
        let (Some(lat), Some(lon)) = (point.latitude, point.longitude) else {
            continue;
        };
        if !lat.is_finite() || !lon.is_finite() {
            continue;
        }
        let alt = point.height_m.filter(|h| h.is_finite()).unwrap_or(0.0);
        let _ = writeln!(out, "          {lon:.8},{lat:.8},{alt:.2}");
    }
    out.push_str("        </coordinates>\n      </LineString>\n    </Placemark>\n");

    out.push_str("    <Folder>\n      <name>Photos</name>\n");
    for (archive_name, photo) in photos {
        out.push_str("      <Placemark>\n");
        let _ = writeln!(out, "        <name>{}</name>", escape_xml(archive_name));
        if let Some(utc) = photo.utc {
            let _ = writeln!(
                out,
                "        <TimeStamp><when>{}</when></TimeStamp>",
                utc_iso(utc)
            );
        }
        let _ = writeln!(
            out,
            "        <description><![CDATA[<img src=\"photo/{archive_name}\"/>]]></description>"
        );
        let alt = photo.altitude_m.unwrap_or(0.0);
        let _ = writeln!(
            out,
            "        <Point><coordinates>{:.8},{:.8},{alt:.2}</coordinates></Point>",
            photo.lon, photo.lat
        );
        out.push_str("      </Placemark>\n");
    }
    out.push_str("    </Folder>\n");
    out.push_str("  </Document>\n</kml>\n");
    out
}

// ---------------------------------------------------------------------------
// Pure-Rust uncompressed ZIP writer + IEEE CRC-32
// ---------------------------------------------------------------------------

struct ZipInput<'a> {
    path: String,
    data: &'a [u8],
    utc: Option<u32>,
}

struct CentralEntry<'a> {
    name: &'a [u8],
    crc: u32,
    size: u32,
    dos_time: u16,
    dos_date: u16,
    local_header_offset: u32,
}

/// Compute the IEEE 802.3 / PKZIP CRC-32 of `bytes`.
#[must_use]
pub fn crc32_ieee(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// Convert UTC Unix seconds into `(dos_time, dos_date)` for ZIP headers.
fn dos_datetime(utc: Option<u32>) -> (u16, u16) {
    let Some(secs) = utc else {
        // 1980-01-01 00:00:00 — earliest MS-DOS date.
        return (0, (1 << 5) | 1);
    };
    let (day, month, year) = utc_date(secs);
    if !(1980..=2107).contains(&year) {
        return (0, (1 << 5) | 1);
    }
    let sod = secs % 86_400;
    let hour = sod / 3_600;
    let minute = (sod % 3_600) / 60;
    let second_half = (sod % 60) / 2;
    let dos_time = ((hour as u16) << 11) | ((minute as u16) << 5) | (second_half as u16);
    let dos_date = (((year - 1980) as u16) << 9) | ((month as u16) << 5) | (day as u16);
    (dos_time, dos_date)
}

fn write_zip_store(entries: &[ZipInput<'_>]) -> Result<Vec<u8>> {
    let entry_count = u16::try_from(entries.len()).map_err(|_| Error::Log {
        message: format!("KMZ has {} entries, exceeding ZIP32 limit", entries.len()),
        offset: None,
    })?;

    let mut out = Vec::new();
    let mut central = Vec::with_capacity(entries.len());

    for entry in entries {
        let name = entry.path.as_bytes();
        let name_len = u16::try_from(name.len()).map_err(|_| Error::Log {
            message: format!("KMZ entry name {:?} exceeds 65535 bytes", entry.path),
            offset: None,
        })?;
        let data_size = u32::try_from(entry.data.len()).map_err(|_| Error::Log {
            message: format!("KMZ entry {:?} exceeds 4 GiB ZIP32 limit", entry.path),
            offset: None,
        })?;
        let local_offset = u32::try_from(out.len()).map_err(|_| Error::Log {
            message: String::from("KMZ archive exceeds 4 GiB ZIP32 offset limit"),
            offset: None,
        })?;

        let crc = crc32_ieee(entry.data);
        let (dos_time, dos_date) = dos_datetime(entry.utc);

        // Local File Header (30 bytes + name)
        out.extend_from_slice(&LFH_SIGNATURE.to_le_bytes());
        out.extend_from_slice(&ZIP_VERSION_20.to_le_bytes());
        out.extend_from_slice(&FLAG_UTF8.to_le_bytes());
        out.extend_from_slice(&METHOD_STORE.to_le_bytes());
        out.extend_from_slice(&dos_time.to_le_bytes());
        out.extend_from_slice(&dos_date.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&data_size.to_le_bytes());
        out.extend_from_slice(&data_size.to_le_bytes());
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra field length
        out.extend_from_slice(name);
        out.extend_from_slice(entry.data);

        central.push(CentralEntry {
            name,
            crc,
            size: data_size,
            dos_time,
            dos_date,
            local_header_offset: local_offset,
        });
    }

    let cd_offset = u32::try_from(out.len()).map_err(|_| Error::Log {
        message: String::from("KMZ central directory exceeds 4 GiB ZIP32 offset limit"),
        offset: None,
    })?;

    for item in &central {
        let name_len = u16::try_from(item.name.len()).map_err(|_| Error::Log {
            message: String::from("KMZ entry name exceeds 65535 bytes"),
            offset: None,
        })?;
        // Central Directory File Header (46 bytes + name)
        out.extend_from_slice(&CDFH_SIGNATURE.to_le_bytes());
        out.extend_from_slice(&ZIP_VERSION_20.to_le_bytes()); // version made by
        out.extend_from_slice(&ZIP_VERSION_20.to_le_bytes()); // version needed
        out.extend_from_slice(&FLAG_UTF8.to_le_bytes());
        out.extend_from_slice(&METHOD_STORE.to_le_bytes());
        out.extend_from_slice(&item.dos_time.to_le_bytes());
        out.extend_from_slice(&item.dos_date.to_le_bytes());
        out.extend_from_slice(&item.crc.to_le_bytes());
        out.extend_from_slice(&item.size.to_le_bytes());
        out.extend_from_slice(&item.size.to_le_bytes());
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra field length
        out.extend_from_slice(&0u16.to_le_bytes()); // file comment length
        out.extend_from_slice(&0u16.to_le_bytes()); // disk number start
        out.extend_from_slice(&0u16.to_le_bytes()); // internal file attrs
        out.extend_from_slice(&0u32.to_le_bytes()); // external file attrs
        out.extend_from_slice(&item.local_header_offset.to_le_bytes());
        out.extend_from_slice(item.name);
    }

    let cd_size =
        u32::try_from(out.len().saturating_sub(cd_offset as usize)).map_err(|_| Error::Log {
            message: String::from("KMZ central directory size exceeds 4 GiB ZIP32 limit"),
            offset: None,
        })?;

    // End of Central Directory Record (22 bytes)
    out.extend_from_slice(&EOCD_SIGNATURE.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // disk number
    out.extend_from_slice(&0u16.to_le_bytes()); // start disk
    out.extend_from_slice(&entry_count.to_le_bytes());
    out.extend_from_slice(&entry_count.to_le_bytes());
    out.extend_from_slice(&cd_size.to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // comment length

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TrackPoint;

    const TOKYO_LAT: f64 = 35.681_236;
    const TOKYO_LON: f64 = 139.767_125;
    const TOKYO_UTC: u32 = 1_700_000_000;

    fn sample_track() -> Track {
        Track {
            points: vec![
                TrackPoint {
                    utc: Some(TOKYO_UTC),
                    latitude: Some(TOKYO_LAT),
                    longitude: Some(TOKYO_LON),
                    height_m: Some(40.0),
                    speed_kmh: Some(12.0),
                    rcr: None,
                },
                TrackPoint {
                    utc: Some(TOKYO_UTC + 60),
                    latitude: Some(34.693_7),
                    longitude: Some(135.502_3),
                    height_m: Some(25.0),
                    speed_kmh: Some(18.0),
                    rcr: None,
                },
            ],
            damaged_blocks: Vec::new(),
        }
    }

    struct ParsedZipEntry {
        name: String,
        method: u16,
        crc: u32,
        data: Vec<u8>,
    }

    fn parse_zip_and_verify(bytes: &[u8]) -> Vec<ParsedZipEntry> {
        assert!(bytes.len() >= 22, "ZIP shorter than EOCD");
        let eocd_pos = bytes.len() - 22;
        assert_eq!(
            u32::from_le_bytes(bytes[eocd_pos..eocd_pos + 4].try_into().unwrap()),
            EOCD_SIGNATURE
        );
        let entry_count =
            u16::from_le_bytes(bytes[eocd_pos + 8..eocd_pos + 10].try_into().unwrap());
        let total_entries =
            u16::from_le_bytes(bytes[eocd_pos + 10..eocd_pos + 12].try_into().unwrap());
        assert_eq!(entry_count, total_entries);
        let cd_size = u32::from_le_bytes(bytes[eocd_pos + 12..eocd_pos + 16].try_into().unwrap());
        let cd_offset = u32::from_le_bytes(bytes[eocd_pos + 16..eocd_pos + 20].try_into().unwrap());
        assert_eq!(cd_offset as usize + cd_size as usize, eocd_pos);

        let mut entries = Vec::new();
        let mut cursor = cd_offset as usize;
        for _ in 0..entry_count {
            assert_eq!(
                u32::from_le_bytes(bytes[cursor..cursor + 4].try_into().unwrap()),
                CDFH_SIGNATURE
            );
            let method = u16::from_le_bytes(bytes[cursor + 10..cursor + 12].try_into().unwrap());
            let cd_crc = u32::from_le_bytes(bytes[cursor + 16..cursor + 20].try_into().unwrap());
            let comp_size =
                u32::from_le_bytes(bytes[cursor + 20..cursor + 24].try_into().unwrap()) as usize;
            let uncomp_size =
                u32::from_le_bytes(bytes[cursor + 24..cursor + 28].try_into().unwrap()) as usize;
            assert_eq!(comp_size, uncomp_size);
            let name_len =
                u16::from_le_bytes(bytes[cursor + 28..cursor + 30].try_into().unwrap()) as usize;
            let extra_len =
                u16::from_le_bytes(bytes[cursor + 30..cursor + 32].try_into().unwrap()) as usize;
            let comment_len =
                u16::from_le_bytes(bytes[cursor + 32..cursor + 34].try_into().unwrap()) as usize;
            let lfh_offset =
                u32::from_le_bytes(bytes[cursor + 42..cursor + 46].try_into().unwrap()) as usize;
            let cd_name = std::str::from_utf8(&bytes[cursor + 46..cursor + 46 + name_len])
                .expect("UTF-8 name")
                .to_owned();
            cursor += 46 + name_len + extra_len + comment_len;

            assert_eq!(
                u32::from_le_bytes(bytes[lfh_offset..lfh_offset + 4].try_into().unwrap()),
                LFH_SIGNATURE
            );
            let lfh_method =
                u16::from_le_bytes(bytes[lfh_offset + 8..lfh_offset + 10].try_into().unwrap());
            assert_eq!(lfh_method, method);
            let lfh_crc =
                u32::from_le_bytes(bytes[lfh_offset + 14..lfh_offset + 18].try_into().unwrap());
            assert_eq!(lfh_crc, cd_crc);
            let lfh_name_len =
                u16::from_le_bytes(bytes[lfh_offset + 26..lfh_offset + 28].try_into().unwrap())
                    as usize;
            let lfh_extra_len =
                u16::from_le_bytes(bytes[lfh_offset + 28..lfh_offset + 30].try_into().unwrap())
                    as usize;
            let lfh_name =
                std::str::from_utf8(&bytes[lfh_offset + 30..lfh_offset + 30 + lfh_name_len])
                    .expect("UTF-8 LFH name");
            assert_eq!(lfh_name, cd_name);
            let data_start = lfh_offset + 30 + lfh_name_len + lfh_extra_len;
            let data = bytes[data_start..data_start + comp_size].to_vec();
            assert_eq!(crc32_ieee(&data), cd_crc, "CRC-32 mismatch for {cd_name}");

            entries.push(ParsedZipEntry {
                name: cd_name,
                method,
                crc: cd_crc,
                data,
            });
        }
        assert_eq!(cursor, eocd_pos);
        entries
    }

    #[test]
    fn crc32_matches_standard_check_vector() {
        assert_eq!(crc32_ieee(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32_ieee(b""), 0x0000_0000);
    }

    #[test]
    fn export_kmz_produces_valid_zip_and_styled_kml_with_photos() {
        let photos = vec![KmzPhoto {
            name: String::from("tokyo.jpg"),
            lat: TOKYO_LAT,
            lon: TOKYO_LON,
            altitude_m: Some(40.0),
            utc: Some(TOKYO_UTC),
            jpeg_bytes: vec![0xFF, 0xD8, 0xFF, 0xD8 + 1],
        }];

        let kmz = export_kmz(&sample_track(), &photos).expect("export kmz");
        let entries = parse_zip_and_verify(&kmz);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "doc.kml");
        assert_eq!(entries[0].method, METHOD_STORE);
        assert_ne!(entries[0].crc, 0);

        let kml = std::str::from_utf8(&entries[0].data).expect("UTF-8 KML");
        assert!(kml.contains("http://earth.google.com/kml/2.0"));
        assert!(kml.contains("<Document>"));
        assert!(kml.contains("<LineStyle>"));
        assert!(kml.contains("<width>5</width>"));
        assert!(kml.contains("<LineString>"));
        assert!(kml.contains("139.76712500,35.68123600,40.00"));
        assert!(kml.contains("<Folder>\n      <name>Photos</name>"));
        assert!(kml.contains("<img src=\"photo/tokyo.jpg\"/>"));

        assert_eq!(entries[1].name, "photo/tokyo.jpg");
        assert_eq!(entries[1].method, METHOD_STORE);
        assert_eq!(entries[1].data, photos[0].jpeg_bytes);
    }

    #[test]
    fn export_kmz_sanitises_traversal_and_deduplicates_names() {
        let photos = vec![
            KmzPhoto {
                name: String::from("../../etc/passwd.jpg"),
                lat: TOKYO_LAT,
                lon: TOKYO_LON,
                altitude_m: None,
                utc: None,
                jpeg_bytes: vec![1, 2, 3],
            },
            KmzPhoto {
                name: String::from("sub\\dir\\passwd.jpg"),
                lat: TOKYO_LAT,
                lon: TOKYO_LON,
                altitude_m: None,
                utc: None,
                jpeg_bytes: vec![4, 5, 6],
            },
            KmzPhoto {
                name: String::from("PASSWD.JPG"),
                lat: TOKYO_LAT,
                lon: TOKYO_LON,
                altitude_m: None,
                utc: None,
                jpeg_bytes: vec![7, 8, 9],
            },
            KmzPhoto {
                name: String::from(".."),
                lat: TOKYO_LAT,
                lon: TOKYO_LON,
                altitude_m: None,
                utc: None,
                jpeg_bytes: vec![10],
            },
        ];

        let kmz = export_kmz(&sample_track(), &photos).expect("export kmz");
        let entries = parse_zip_and_verify(&kmz);
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "doc.kml",
                "photo/passwd.jpg",
                "photo/passwd_1.jpg",
                "photo/PASSWD_2.JPG",
                "photo/photo.jpg",
            ]
        );
        for name in &names[1..] {
            assert!(!name.contains(".."), "traversal survived in {name}");
            assert!(!name.contains('\\'), "backslash survived in {name}");
        }
        let kml = std::str::from_utf8(&entries[0].data).expect("UTF-8 KML");
        assert!(kml.contains("<img src=\"photo/passwd.jpg\"/>"));
        assert!(kml.contains("<img src=\"photo/passwd_1.jpg\"/>"));
        assert!(kml.contains("<img src=\"photo/PASSWD_2.JPG\"/>"));
        assert!(kml.contains("<img src=\"photo/photo.jpg\"/>"));
    }

    #[test]
    fn export_kmz_rejects_invalid_photo_coordinates() {
        let bad = KmzPhoto {
            name: String::from("bad.jpg"),
            lat: 95.0,
            lon: TOKYO_LON,
            altitude_m: None,
            utc: None,
            jpeg_bytes: vec![1, 2],
        };
        let err = export_kmz(&sample_track(), &[bad]).expect_err("invalid lat must fail");
        assert!(matches!(err, Error::State(_)));
    }
}
