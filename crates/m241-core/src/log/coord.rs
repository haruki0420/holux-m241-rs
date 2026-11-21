//! Coordinate display formatting (`Decimal`, `Dms`, `Utm`).
//!
//! Covers the three practical coordinate systems: decimal degrees with
//! hemisphere letters, degrees-minutes-seconds, and WGS84 UTM. The formats are
//! the ones `07-eztour-spec.ja.md` §3.3 and §3.6 record for ezTour's display;
//! the code does not depend on that document to explain itself.

use crate::error::{Error, Result};

/// Display representation for a geographic coordinate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoordFormat {
    /// Hemisphere + signed-free decimal degrees to 8 places (`N 35.68123600°`).
    Decimal,
    /// Hemisphere + degrees, minutes, and seconds to 3 places (`N 35°40'52.450"`).
    Dms,
    /// WGS84 UTM zone + latitude band + integer easting (`54S 388436E`) and
    /// integer northing (`3949294N`).
    Utm,
}

/// Format `(lat, lon)` in WGS84 degrees according to `format`.
///
/// # Errors
///
/// Returns [`Error::State`] when `lat` or `lon` is non-finite or outside
/// `[-90, 90]` / `[-180, 180]`, and when `format == CoordFormat::Utm` and `lat`
/// is outside `[-80, 84]`: UTM has no band beyond those latitudes, so `Utm`
/// accepts a narrower range than the other two formats.
pub fn format_coordinate(lat: f64, lon: f64, format: CoordFormat) -> Result<(String, String)> {
    if !lat.is_finite() || lat.abs() > 90.0 {
        return Err(Error::State(format!(
            "latitude {lat} is not a number between -90 and 90 degrees"
        )));
    }
    if !lon.is_finite() || lon.abs() > 180.0 {
        return Err(Error::State(format!(
            "longitude {lon} is not a number between -180 and 180 degrees"
        )));
    }

    match format {
        CoordFormat::Decimal => Ok((
            format_decimal_axis(lat, 'N', 'S'),
            format_decimal_axis(lon, 'E', 'W'),
        )),
        CoordFormat::Dms => Ok((
            format_dms_axis(lat, 'N', 'S'),
            format_dms_axis(lon, 'E', 'W'),
        )),
        CoordFormat::Utm => format_utm(lat, lon),
    }
}

fn format_decimal_axis(value: f64, positive: char, negative: char) -> String {
    let hemi = if value < 0.0 { negative } else { positive };
    format!("{hemi} {:.8}°", value.abs())
}

fn format_dms_axis(value: f64, positive: char, negative: char) -> String {
    let hemi = if value < 0.0 { negative } else { positive };
    let total_ms = (value.abs() * 3_600_000.0).round() as u64;
    let sec_ms = total_ms % 60_000;
    let total_min = total_ms / 60_000;
    let min = total_min % 60;
    let deg = total_min / 60;
    let sec = (sec_ms as f64) / 1000.0;
    // Degrees are not zero-padded: the hemisphere letter already fixes the
    // field's width, and `N 5°` reads better than `N 05°`. Minutes and seconds
    // are padded because they are parts of a degree, not a standalone number.
    format!("{hemi} {deg}°{min:02}'{sec:06.3}\"")
}

/// WGS84 Transverse Mercator UTM projection (`k0 = 0.9996`, `FE = 500_000`,
/// southern hemisphere false northing `10_000_000`).
///
/// The series expansions are the standard ones given in Snyder, *Map
/// Projections — A Working Manual* (USGS Professional Paper 1395), §8-9, with
/// the 4th-order tangent and 6th-order meridional terms carried far enough for
/// errors well under a metre inside a zone; the display rounds to whole metres.
fn format_utm(lat: f64, lon: f64) -> Result<(String, String)> {
    if !(-80.0..=84.0).contains(&lat) {
        return Err(Error::State(format!(
            "latitude {lat} is outside the UTM band range (-80 to 84 degrees)"
        )));
    }

    const BANDS: &[u8; 20] = b"CDEFGHJKLMNPQRSTUVWX";
    // 8-degree bands from -80: band 0 is C. Latitude 84, the top of band X,
    // computes to (84 + 80) / 8 = 20.5 -> 20, one past the table, because X is
    // 12 degrees tall where every other band is 8; `min(19)` folds it into X.
    let band_idx = (((lat + 80.0) / 8.0).floor() as usize).min(19);
    let band = char::from(BANDS.get(band_idx).copied().unwrap_or(b'X'));

    let mut zone = (((lon + 180.0) / 6.0).floor() as i32 + 1).clamp(1, 60);
    // Standard Norway and Svalbard UTM zone exceptions.
    if (56.0..64.0).contains(&lat) && (3.0..12.0).contains(&lon) {
        zone = 32;
    } else if (72.0..=84.0).contains(&lat) {
        if (0.0..9.0).contains(&lon) {
            zone = 31;
        } else if (9.0..21.0).contains(&lon) {
            zone = 33;
        } else if (21.0..33.0).contains(&lon) {
            zone = 35;
        } else if (33.0..42.0).contains(&lon) {
            zone = 37;
        }
    }

    const A: f64 = 6_378_137.0;
    const INV_F: f64 = 298.257_223_563;
    const K0: f64 = 0.9996;
    const FALSE_EASTING: f64 = 500_000.0;

    let f = 1.0 / INV_F;
    let e2 = f * (2.0 - f);
    let e4 = e2 * e2;
    let e6 = e4 * e2;
    let ep2 = e2 / (1.0 - e2);

    let lon0_deg = f64::from(zone) * 6.0 - 183.0;
    let phi = lat.to_radians();
    let lam = (lon - lon0_deg).to_radians();

    let sin_phi = phi.sin();
    let cos_phi = phi.cos();
    let tan_phi = phi.tan();

    let n = A / (1.0 - e2 * sin_phi * sin_phi).sqrt();
    let t = tan_phi * tan_phi;
    let c = ep2 * cos_phi * cos_phi;
    let a_term = cos_phi * lam;

    let m = A
        * ((1.0 - e2 / 4.0 - 3.0 * e4 / 64.0 - 5.0 * e6 / 256.0) * phi
            - (3.0 * e2 / 8.0 + 3.0 * e4 / 32.0 + 45.0 * e6 / 1024.0) * (2.0 * phi).sin()
            + (15.0 * e4 / 256.0 + 45.0 * e6 / 1024.0) * (4.0 * phi).sin()
            - (35.0 * e6 / 3072.0) * (6.0 * phi).sin());

    let easting = FALSE_EASTING
        + K0 * n
            * (a_term
                + (1.0 - t + c) * a_term.powi(3) / 6.0
                + (5.0 - 18.0 * t + t * t + 72.0 * c - 58.0 * ep2) * a_term.powi(5) / 120.0);

    let false_northing = if lat < 0.0 { 10_000_000.0 } else { 0.0 };
    let northing = false_northing
        + K0 * (m + n
            * tan_phi
            * (a_term.powi(2) / 2.0
                + (5.0 - t + 9.0 * c + 4.0 * c * c) * a_term.powi(4) / 24.0
                + (61.0 - 58.0 * t + t * t + 600.0 * c - 330.0 * ep2) * a_term.powi(6) / 720.0));

    let easting_m = easting.round() as i64;
    let northing_m = northing.round() as i64;
    Ok((
        format!("{zone}{band} {easting_m}E"),
        format!("{northing_m}N"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKYO_LAT: f64 = 35.681_236;
    const TOKYO_LON: f64 = 139.767_125;

    #[test]
    fn tokyo_station_formats_as_decimal_dms_and_utm_54s() {
        let (lat_dec, lon_dec) =
            format_coordinate(TOKYO_LAT, TOKYO_LON, CoordFormat::Decimal).expect("decimal");
        assert_eq!(lat_dec, "N 35.68123600°");
        assert_eq!(lon_dec, "E 139.76712500°");

        let (lat_dms, lon_dms) =
            format_coordinate(TOKYO_LAT, TOKYO_LON, CoordFormat::Dms).expect("dms");
        assert_eq!(lat_dms, "N 35°40'52.450\"");
        assert_eq!(lon_dms, "E 139°46'01.650\"");

        let (utm_e, utm_n) =
            format_coordinate(TOKYO_LAT, TOKYO_LON, CoordFormat::Utm).expect("utm");
        assert!(utm_e.starts_with("54S "), "got {utm_e}");
        let easting: i64 = utm_e
            .strip_prefix("54S ")
            .unwrap()
            .strip_suffix('E')
            .unwrap()
            .parse()
            .unwrap();
        let northing: i64 = utm_n.strip_suffix('N').unwrap().parse().unwrap();
        // ±1 m is the display's rounding to whole metres, not an allowance for
        // the projection: the series error inside a zone is far below that.
        // The expected values come from an independent UTM implementation for
        // the same WGS84 input, not from this code.
        assert!((easting - 388_436).abs() <= 1, "easting {easting}");
        assert!((northing - 3_949_294).abs() <= 1, "northing {northing}");
    }

    #[test]
    fn southern_and_western_hemispheres_use_s_w_and_false_northing() {
        let (lat_dec, lon_dec) =
            format_coordinate(-35.5, -70.25, CoordFormat::Decimal).expect("decimal sw");
        assert_eq!(lat_dec, "S 35.50000000°");
        assert_eq!(lon_dec, "W 70.25000000°");

        let (lat_dms, lon_dms) =
            format_coordinate(-35.5, -70.25, CoordFormat::Dms).expect("dms sw");
        assert_eq!(lat_dms, "S 35°30'00.000\"");
        assert_eq!(lon_dms, "W 70°15'00.000\"");

        // (-35.5, -70.25) -> zone 19, band H, southern hemisphere false northing (~6.07M m).
        // This expectation is this implementation's own output, not a value
        // checked against an external calculator; the zone/band and the
        // hemisphere offset are what the assertion is really about.
        let (utm_e, utm_n) = format_coordinate(-35.5, -70.25, CoordFormat::Utm).expect("utm sw");
        assert!(utm_e.starts_with("19H "), "got {utm_e}");
        let northing: i64 = utm_n.strip_suffix('N').unwrap().parse().unwrap();
        assert!(
            northing > 5_000_000 && northing < 10_000_000,
            "southern hemisphere northing must include +10,000,000 offset: {northing}"
        );
    }

    #[test]
    fn dms_carries_sixty_seconds_into_minutes() {
        // 35 deg + 59 min + 59.9999 sec -> rounds to 36°00'00.000"
        let near_boundary = 35.0 + 59.0 / 60.0 + 59.999_9 / 3600.0;
        let (lat_dms, _) = format_coordinate(near_boundary, 0.0, CoordFormat::Dms).expect("carry");
        assert_eq!(lat_dms, "N 36°00'00.000\"");
    }

    #[test]
    fn the_norway_and_svalbard_zone_exceptions_are_applied() {
        // Norway: 56-64 N, 3-12 E belongs to zone 32 even where the plain
        // 6-degree rule would put it in 31 (5 E) or 33 (11 E).
        let (east, _) = format_coordinate(60.0, 5.0, CoordFormat::Utm).expect("norway");
        assert_eq!(east, "32V 276980E", "Norway's 5 E is band 32, not 31");
        let (east, _) = format_coordinate(60.0, 11.0, CoordFormat::Utm).expect("norway east");
        assert!(east.starts_with("32V "), "Norway's 11 E is also 32: {east}");

        // Svalbard: above 72 N the zones are widened, so 10 E is 33 and 22 E
        // is 35 rather than 32 and 34.
        let (east, _) = format_coordinate(78.0, 10.0, CoordFormat::Utm).expect("svalbard");
        assert!(east.starts_with("33X "), "Svalbard's 10 E is 33: {east}");
        let (east, _) = format_coordinate(78.0, 22.0, CoordFormat::Utm).expect("svalbard east");
        assert!(east.starts_with("35X "), "Svalbard's 22 E is 35: {east}");

        // The 276980 easting for (60 N, 5 E) is an independent calculation; it
        // pins the projection at the edge of its own widened zone.
        let (_, north) = format_coordinate(60.0, 5.0, CoordFormat::Utm).expect("norway");
        assert_eq!(north, "6658157N");
    }

    #[test]
    fn polar_and_non_finite_coordinates_are_rejected() {
        for bad_lat in [84.1, -80.1, 90.0, -90.0] {
            assert!(matches!(
                format_coordinate(bad_lat, 0.0, CoordFormat::Utm),
                Err(Error::State(_))
            ));
        }
        for bad in [
            (f64::NAN, 0.0),
            (0.0, f64::INFINITY),
            (91.0, 0.0),
            (0.0, -181.0),
        ] {
            for fmt in [CoordFormat::Decimal, CoordFormat::Dms, CoordFormat::Utm] {
                assert!(matches!(
                    format_coordinate(bad.0, bad.1, fmt),
                    Err(Error::State(_))
                ));
            }
        }
    }
}
