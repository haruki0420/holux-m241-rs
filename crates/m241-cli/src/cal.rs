//! Civil dates, so the offset flags need no date library.
//!
//! Two places need the same conversion: `log read --utc-offset-days` shifts Unix
//! seconds, and `nmea repair` rewrites a `ddmmyy` (RMC) or `dd mm yyyy` (ZDA)
//! field. Both are day arithmetic, and the crate's dependency budget does not
//! stretch to a date crate for it.
//!
//! The conversion is Howard Hinnant's `days_from_civil` / `civil_from_days`,
//! exact for the whole proleptic Gregorian calendar. It is duplicated from
//! `m241_core::log::export` rather than shared because the core's copy is
//! private and turning it public to save fifteen lines would put a CLI concern
//! in the library's API.

/// One GPS week-number-rollover cycle, in days: `1024 × 7`.
///
/// The firmware hard-codes the GPS epoch and never accounts for the second
/// rollover (2019-04-06), so every date it emits is this far in the past.
pub const WNRO_DAYS: i32 = 7_168;

/// The first date a rollover-broken M-241 can emit, as days since 1970-01-01.
const STALE_FROM: i64 = 10_825; // 1999-08-22
/// The last date a rollover-broken M-241 can emit, as days since 1970-01-01.
///
/// The rollover itself: a healthy unit is far beyond this, and a unit still in
/// this window has not had the fix applied.
const STALE_TO: i64 = 17_992; // 2019-04-06

/// How `--utc-offset-days` / `--offset-days` was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OffsetSpec {
    /// Decide from the dates themselves: shift only a stale log.
    Auto,
    /// An explicit number of days, always applied when it is not zero.
    Days(i32),
}

impl OffsetSpec {
    /// The shift to apply to a date, given the date itself.
    ///
    /// `Auto` shifts a stale date by one rollover and leaves anything else
    /// alone; an explicit number is applied as written, so `--utc-offset-days 0`
    /// is how a user asks for "do not touch my timestamps".
    #[must_use]
    pub fn shift_for(self, days_since_epoch: i64) -> i32 {
        match self {
            Self::Auto if is_stale(days_since_epoch) => WNRO_DAYS,
            Self::Auto => 0,
            Self::Days(days) => days,
        }
    }
}

/// Whether `days_since_epoch` falls inside the window a broken unit can emit.
#[must_use]
pub fn is_stale(days_since_epoch: i64) -> bool {
    (STALE_FROM..=STALE_TO).contains(&days_since_epoch)
}

/// Parse an offset flag value: a signed number of days, or `auto`.
///
/// # Errors
///
/// The message clap shows for a usage error (exit code 2).
pub fn parse_offset(text: &str) -> Result<OffsetSpec, String> {
    let trimmed = text.trim();
    if trimmed.eq_ignore_ascii_case("auto") {
        return Ok(OffsetSpec::Auto);
    }
    let days = trimmed
        .parse::<i32>()
        .map_err(|_| format!("{text:?} is not a number of days; write a number or `auto`"))?;
    Ok(OffsetSpec::Days(days))
}

/// Days since 1970-01-01 for a civil date in the proleptic Gregorian calendar.
#[must_use]
pub fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = i64::from(year) - i64::from(u32::from(month <= 2));
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let day = i64::from(day);
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The civil date `days_since_epoch` falls on.
#[must_use]
pub fn civil_from_days(days_since_epoch: i64) -> (i32, u32, u32) {
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

/// A timestamp shifted by whole days, clamped to the `u32` range.
///
/// Clamping rather than wrapping matters at the edges: a shift that runs off the
/// end must not turn 2026 into 2106 minus a few years.
#[must_use]
pub fn shift_seconds(utc: u32, days: i32) -> u32 {
    let shifted = i64::from(utc) + i64::from(days) * 86_400;
    let clamped = shifted.clamp(0, i64::from(u32::MAX));
    u32::try_from(clamped).unwrap_or(utc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_epoch_is_day_zero() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(civil_from_days(0), (1970, 1, 1));
    }

    #[test]
    fn the_rollover_boundaries_are_the_documented_dates() {
        assert_eq!(civil_from_days(STALE_FROM), (1999, 8, 22));
        assert_eq!(civil_from_days(STALE_TO), (2019, 4, 6));
        assert_eq!(days_from_civil(1999, 8, 22), STALE_FROM);
        assert_eq!(days_from_civil(2019, 4, 6), STALE_TO);
    }

    #[test]
    fn the_rollover_shift_is_7168_days() {
        // The measured defect: 2006-09-17 is what a broken unit says for
        // 2026-05-04, and 7168 days is the gap.
        let broken = days_from_civil(2006, 9, 17);
        let fixed = civil_from_days(broken + i64::from(WNRO_DAYS));
        assert_eq!(fixed, (2026, 5, 3));
    }

    #[test]
    fn dates_round_trip_across_a_leap_day_and_a_century_rule() {
        for (year, month, day) in [
            (2000, 2, 29),
            (1900, 3, 1),
            (2026, 11, 20),
            (1980, 1, 6),
            (2100, 12, 31),
            (1999, 12, 31),
        ] {
            let days = days_from_civil(year, month, day);
            assert_eq!(
                civil_from_days(days),
                (year, month, day),
                "{year}-{month}-{day}"
            );
        }
    }

    #[test]
    fn auto_shifts_only_a_stale_date() {
        assert_eq!(OffsetSpec::Auto.shift_for(STALE_FROM), WNRO_DAYS);
        assert_eq!(OffsetSpec::Auto.shift_for(STALE_TO), WNRO_DAYS);
        assert_eq!(OffsetSpec::Auto.shift_for(STALE_TO + 1), 0);
        assert_eq!(OffsetSpec::Auto.shift_for(STALE_FROM - 1), 0);
        // Today is long past the stale window.
        assert_eq!(OffsetSpec::Auto.shift_for(days_from_civil(2026, 11, 20)), 0);
    }

    #[test]
    fn an_explicit_offset_is_never_second_guessed() {
        assert_eq!(OffsetSpec::Days(3).shift_for(STALE_FROM), 3);
        assert_eq!(OffsetSpec::Days(0).shift_for(STALE_FROM), 0);
        assert_eq!(OffsetSpec::Days(-1).shift_for(0), -1);
    }

    #[test]
    fn the_offset_flag_accepts_a_number_or_auto() {
        assert_eq!(parse_offset("auto"), Ok(OffsetSpec::Auto));
        assert_eq!(parse_offset(" AUTO "), Ok(OffsetSpec::Auto));
        assert_eq!(parse_offset("7168"), Ok(OffsetSpec::Days(7_168)));
        assert_eq!(parse_offset("-7168"), Ok(OffsetSpec::Days(-7_168)));
        assert!(parse_offset("tomorrow").is_err());
    }

    #[test]
    fn shifting_seconds_clamps_at_the_ends_of_the_range() {
        assert_eq!(shift_seconds(0, -1), 0);
        assert_eq!(shift_seconds(u32::MAX, 1), u32::MAX);
        assert_eq!(shift_seconds(0, 1), 86_400);
        assert_eq!(shift_seconds(86_400, -1), 0);
    }
}
