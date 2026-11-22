//! The NMEA date repair and the day arithmetic it shares with `log read`.
//!
//! `m241 nmea repair` lives in the CLI crate. This is the same algorithm in the
//! GUI crate for two reasons: `m241-gui` depends on `m241-core` only, and the
//! core has no NMEA rewriter to call. The arithmetic is small, integer-only and
//! deterministic, so a copy with its own tests is honest here — and the GUI and
//! the CLI must agree byte-for-byte on a repaired sentence, which the tests pin
//! with the same measured examples the CLI uses.
//!
//! One GPS week-number-rollover cycle is [`WNRO_DAYS`]: the 2006 firmware
//! converts GPS time with the 1980 epoch constant and never accounts for the
//! second rollover (2019-04-06), so every date it emits is that far in the past.

use m241_core::Track;

/// One GPS week-number-rollover cycle, in days (`1024 × 7`).
pub const WNRO_DAYS: i32 = 7_168;

/// The first date a rollover-broken M-241 can emit, as days since 1970-01-01.
const STALE_FROM: i64 = 10_825; // 1999-08-22
/// The last date a rollover-broken M-241 can emit, as days since 1970-01-01.
const STALE_TO: i64 = 17_992; // 2019-04-06

/// Where the `ddmmyy` date sits in a `$GPRMC` field list (address at index 0).
const RMC_DATE: usize = 9;

/// The `dd`, `mm` and `yyyy` fields of `$GPZDA` (address at index 0).
const ZDA_DAY: usize = 2;
const ZDA_MONTH: usize = 3;
const ZDA_YEAR: usize = 4;

/// How a date offset was written: `auto` decides from the dates themselves, a
/// number is applied as given.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OffsetSpec {
    /// Shift only a stale date by one rollover; leave anything else alone.
    #[default]
    Auto,
    /// A number of days, applied to every date, including negative values.
    Days(i32),
}

impl OffsetSpec {
    /// Parse `auto` or a signed number of days.
    ///
    /// # Errors
    ///
    /// The Japanese sentence the control shows for an unreadable value.
    pub fn parse(text: &str) -> Result<Self, String> {
        let trimmed = text.trim();
        if trimmed.eq_ignore_ascii_case("auto") {
            return Ok(Self::Auto);
        }
        trimmed
            .parse::<i32>()
            .map(Self::Days)
            .map_err(|_| format!("{text:?} は日数ではありません。数字か `auto` を入力してください"))
    }

    /// The shift to apply to a date, given the date itself.
    #[must_use]
    fn shift_for(self, days_since_epoch: i64) -> i32 {
        match self {
            Self::Auto if is_stale(days_since_epoch) => WNRO_DAYS,
            Self::Auto => 0,
            Self::Days(days) => days,
        }
    }
}

/// Whether `days_since_epoch` falls inside the window a broken unit can emit.
fn is_stale(days_since_epoch: i64) -> bool {
    (STALE_FROM..=STALE_TO).contains(&days_since_epoch)
}

/// Apply `spec` to every timestamp in `track` and return the applied day shift.
///
/// The decision is made from the newest record, not the last one in file order,
/// because a wrapped ring buffer places older blocks after newer ones. A shift of
/// zero changes nothing and is reported as zero.
pub fn apply_to_track(track: &mut Track, spec: OffsetSpec) -> i32 {
    let shift = track
        .points
        .iter()
        .filter_map(|point| point.utc)
        .max()
        .map_or(0, |utc| spec.shift_for(i64::from(utc / 86_400)));
    if shift != 0 {
        for point in &mut track.points {
            point.utc = point.utc.map(|utc| shift_seconds(utc, shift));
        }
    }
    shift
}

/// A timestamp shifted by whole days, clamped to the `u32` range.
fn shift_seconds(utc: u32, days: i32) -> u32 {
    let shifted = i64::from(utc) + i64::from(days) * 86_400;
    u32::try_from(shifted.clamp(0, i64::from(u32::MAX))).unwrap_or(utc)
}

/// Repair one line, preserving its terminator.
///
/// Returns the line and whether it changed. A line that is not a date-bearing
/// sentence is returned untouched, so the caller does not have to know which
/// sentences matter. The sentence is never re-rendered from scratch: only the
/// date fields are replaced and the checksum recomputed, so fields this build
/// does not understand survive.
#[must_use]
pub fn repair_line(line: &str, offset: OffsetSpec) -> (String, bool) {
    let (text, ending) = split_ending(line);
    let Some(body) = text.strip_prefix('$') else {
        return (line.to_owned(), false);
    };
    let Some((payload, _carried)) = body.split_once('*') else {
        // A sentence with no checksum cannot be repaired without inventing one.
        return (line.to_owned(), false);
    };
    let mut fields: Vec<String> = payload.split(',').map(str::to_owned).collect();
    let Some(address) = fields.first() else {
        return (line.to_owned(), false);
    };
    // The talker is not checked: `$GPRMC`, `$GNRMC` and `$INRMC` all carry the
    // date in the same position, so the sentence type alone decides.
    let upper = address.to_ascii_uppercase();
    let changed = if upper.ends_with("RMC") {
        repair_rmc(&mut fields, offset)
    } else if upper.ends_with("ZDA") {
        repair_zda(&mut fields, offset)
    } else {
        false
    };
    if !changed {
        return (line.to_owned(), false);
    }
    let rebuilt = fields.join(",");
    (
        format!("${rebuilt}*{:02X}{ending}", xor_checksum(&rebuilt)),
        true,
    )
}

/// Replace the `ddmmyy` field of an RMC sentence.
fn repair_rmc(fields: &mut [String], offset: OffsetSpec) -> bool {
    let Some(text) = fields.get(RMC_DATE) else {
        return false;
    };
    let Some((day, month, year)) = parse_ddmmyy(text) else {
        return false;
    };
    let days = days_from_civil(year, month, day);
    let shift = offset.shift_for(days);
    if shift == 0 {
        return false;
    }
    let (year, month, day) = civil_from_days(days + i64::from(shift));
    let replacement = format!("{day:02}{month:02}{:02}", year.rem_euclid(100));
    let Some(slot) = fields.get_mut(RMC_DATE) else {
        return false;
    };
    *slot = replacement;
    true
}

/// Replace the day, month and year fields of a ZDA sentence.
fn repair_zda(fields: &mut [String], offset: OffsetSpec) -> bool {
    let (Some(day), Some(month), Some(year)) = (
        fields.get(ZDA_DAY),
        fields.get(ZDA_MONTH),
        fields.get(ZDA_YEAR),
    ) else {
        return false;
    };
    let (Ok(day), Ok(month), Ok(year)) = (
        day.parse::<u32>(),
        month.parse::<u32>(),
        year.parse::<i32>(),
    ) else {
        return false;
    };
    if !is_valid_date(year, month, day) {
        return false;
    }
    let days = days_from_civil(year, month, day);
    let shift = offset.shift_for(days);
    if shift == 0 {
        return false;
    }
    let (year, month, day) = civil_from_days(days + i64::from(shift));
    let replacements = [
        format!("{day:02}"),
        format!("{month:02}"),
        format!("{year:04}"),
    ];
    for (index, text) in [ZDA_DAY, ZDA_MONTH, ZDA_YEAR].into_iter().zip(replacements) {
        let Some(slot) = fields.get_mut(index) else {
            return false;
        };
        *slot = text;
    }
    true
}

/// Parse a `ddmmyy` field. Two-digit years follow the NMEA convention: 70–99
/// are 1970–1999, 00–69 are 2000–2069.
fn parse_ddmmyy(text: &str) -> Option<(u32, u32, i32)> {
    if text.len() != 6 || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let day: u32 = text.get(0..2)?.parse().ok()?;
    let month: u32 = text.get(2..4)?.parse().ok()?;
    let short: i32 = text.get(4..6)?.parse().ok()?;
    if day == 0 || month == 0 || month > 12 || day > 31 {
        return None;
    }
    let year = if short < 70 {
        2000 + short
    } else {
        1900 + short
    };
    Some((day, month, year))
}

/// Split the line terminator from the sentence body.
fn split_ending(line: &str) -> (&str, &str) {
    let body = line.trim_end_matches(['\r', '\n']);
    let ending = line.get(body.len()..).unwrap_or("");
    (body, ending)
}

/// XOR of every byte between `$` and `*`.
fn xor_checksum(body: &str) -> u8 {
    body.as_bytes().iter().fold(0u8, |acc, byte| acc ^ byte)
}

/// Whether `(year, month, day)` is a valid proleptic Gregorian date.
fn is_valid_date(year: i32, month: u32, day: u32) -> bool {
    if !(1..=9999).contains(&year) || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return false;
    }
    civil_from_days(days_from_civil(year, month, day)) == (year, month, day)
}

/// Days since 1970-01-01 for a civil date (Howard Hinnant's `days_from_civil`).
fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = i64::from(year) - i64::from(u32::from(month <= 2));
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let day = i64::from(day);
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The civil date `days_since_epoch` falls on (Hinnant's `civil_from_days`).
fn civil_from_days(days_since_epoch: i64) -> (i32, u32, u32) {
    let shifted = days_since_epoch + 719_468;
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
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    };
    let year = year + i64::from(month <= 2);
    (
        i32::try_from(year).unwrap_or(1970),
        u32::try_from(month).unwrap_or(1),
        u32::try_from(day).unwrap_or(1),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A no-fix sentence dated 2006-09-17, exactly the CLI's worked example.
    const BROKEN_RMC: &str = "$GPRMC,022127.542,V,8960.0000,N,00000.0000,E,0.00,0.00,170906,,,N*7F";
    /// [`BROKEN_RMC`] after one rollover: 2026-05-03 with a recomputed checksum.
    const FIXED_RMC: &str = "$GPRMC,022127.542,V,8960.0000,N,00000.0000,E,0.00,0.00,030526,,,N*74";

    #[test]
    fn the_broken_example_becomes_its_expected_7168_days_later_date() {
        let (repaired, changed) = repair_line(BROKEN_RMC, OffsetSpec::Auto);
        assert!(changed);
        assert_eq!(repaired, FIXED_RMC);
    }

    #[test]
    fn a_modern_date_is_left_alone_by_auto() {
        let modern = "$GPRMC,022127.542,A,3540.87416,N,13946.02750,E,0.00,0.00,201126,,,A*68";
        let (repaired, changed) = repair_line(modern, OffsetSpec::Auto);
        assert!(!changed, "auto must not touch a date that is not stale");
        assert_eq!(repaired, modern);
    }

    #[test]
    fn an_explicit_offset_is_applied_to_a_modern_date() {
        let modern = "$GPRMC,022127.542,A,3540.87416,N,13946.02750,E,0.00,0.00,201126,,,A*68";
        let (repaired, changed) = repair_line(modern, OffsetSpec::Days(-2));
        assert!(changed, "an explicit offset is never second-guessed");
        assert!(repaired.contains(",181126,"), "got {repaired}");
    }

    #[test]
    fn a_zda_sentence_has_all_three_date_fields_rewritten() {
        let (repaired, changed) =
            repair_line("$GPZDA,022127.54,17,09,2006,00,00*00", OffsetSpec::Auto);
        assert!(changed);
        assert!(repaired.contains(",03,05,2026,"), "got {repaired}");
        let (body, carried) = repaired.split_once('*').expect("checksum");
        let body = body.strip_prefix('$').expect("a sentence body");
        assert_eq!(format!("{:02X}", xor_checksum(body)), carried);
    }

    #[test]
    fn lines_that_are_not_date_bearing_pass_through_byte_for_byte() {
        for line in [
            "$GPGGA,022127.54,3540.87416,N,13946.02750,E,1,08,0.9,80.0,M,0.0,M,,*00\r\n",
            "======Setting_ProcessKeys========\r\n",
            "",
            "\r\n",
            "$PMTK705,B-core_1.20,0043,01017-00F,1.0*72",
        ] {
            let (repaired, changed) = repair_line(line, OffsetSpec::Auto);
            assert!(!changed, "{line:?} must not change");
            assert_eq!(repaired, line);
        }
    }

    #[test]
    fn a_sentence_without_a_checksum_is_not_invented() {
        let line = "$GPRMC,022127.542,V,8960.0000,N,00000.0000,E,0.00,0.00,170906,,,N";
        let (repaired, changed) = repair_line(line, OffsetSpec::Auto);
        assert!(!changed);
        assert_eq!(repaired, line);
    }

    #[test]
    fn an_out_of_range_zda_date_is_left_alone() {
        for line in [
            "$GPZDA,022127.54,00,00,2006,00,00*00",
            "$GPZDA,022127.54,32,01,2006,00,00*00",
            "$GPZDA,022127.54,17,13,2006,00,00*00",
        ] {
            let (repaired, changed) = repair_line(line, OffsetSpec::Auto);
            assert!(!changed, "{line:?} must not be rewritten");
            assert_eq!(repaired, line);
        }
    }

    #[test]
    fn the_two_digit_year_uses_the_nmea_century_rule() {
        assert_eq!(parse_ddmmyy("010170"), Some((1, 1, 1970)));
        assert_eq!(parse_ddmmyy("010169"), Some((1, 1, 2069)));
        assert_eq!(parse_ddmmyy("290200"), Some((29, 2, 2000)));
        assert_eq!(parse_ddmmyy("000101"), None, "day zero is not a date");
    }

    #[test]
    fn the_offset_accepts_a_number_or_auto() {
        assert_eq!(OffsetSpec::parse("auto"), Ok(OffsetSpec::Auto));
        assert_eq!(OffsetSpec::parse(" AUTO "), Ok(OffsetSpec::Auto));
        assert_eq!(OffsetSpec::parse("7168"), Ok(OffsetSpec::Days(7_168)));
        assert_eq!(OffsetSpec::parse("-7168"), Ok(OffsetSpec::Days(-7_168)));
        assert!(OffsetSpec::parse("tomorrow").is_err());
    }

    #[test]
    fn auto_shifts_only_a_stale_track_and_reports_the_days() {
        // 2006-09-17: inside the stale window, so `auto` adds one rollover.
        let mut track = Track {
            points: vec![m241_core::TrackPoint {
                utc: Some(1_159_444_800),
                ..m241_core::TrackPoint::default()
            }],
            ..Track::default()
        };
        let shift = apply_to_track(&mut track, OffsetSpec::Auto);
        assert_eq!(shift, WNRO_DAYS);
        assert_eq!(
            track.points.first().and_then(|point| point.utc),
            Some(1_159_444_800 + 7_168 * 86_400)
        );

        let mut modern = Track {
            points: vec![m241_core::TrackPoint {
                utc: Some(1_700_000_000),
                ..m241_core::TrackPoint::default()
            }],
            ..Track::default()
        };
        assert_eq!(apply_to_track(&mut modern, OffsetSpec::Auto), 0);
        assert_eq!(
            modern.points.first().and_then(|point| point.utc),
            Some(1_700_000_000)
        );
    }

    #[test]
    fn the_rollover_shift_is_7168_days() {
        let broken = days_from_civil(2006, 9, 17);
        assert_eq!(civil_from_days(broken + i64::from(WNRO_DAYS)), (2026, 5, 3));
    }
}
