//! UTC stamps for run ids and provenance, from [`std::time::SystemTime`] alone.
//!
//! The workspace carries no date/time crate, and one stamp per run does not justify acquiring one.
//! [`Stamp`] is the two spellings the arc needs: the compact id half and the ISO-8601 instant
//! `run.json` records.

use std::time::{SystemTime, UNIX_EPOCH};

/// One instant in the two spellings this crate writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Stamp {
    /// `20260918T142233Z` — the sortable first half of a run id.
    pub(crate) compact: String,
    /// `2026-09-18T14:22:33Z` — `run.json`'s `started`/`finished`.
    pub(crate) iso: String,
}

impl Stamp {
    /// The current instant. A clock before the Unix epoch reads as the epoch rather than failing:
    /// a run id is a label, never a measurement.
    pub(crate) fn now() -> Stamp {
        Stamp::at(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |since| since.as_secs()),
        )
    }

    /// The instant `seconds` after the Unix epoch, in UTC.
    pub(crate) fn at(seconds: u64) -> Stamp {
        let days = (seconds / 86_400) as i64;
        let rest = seconds % 86_400;
        let (hour, minute, second) = (rest / 3600, (rest % 3600) / 60, rest % 60);
        let (year, month, day) = civil_from_days(days);
        Stamp {
            compact: format!("{year:04}{month:02}{day:02}T{hour:02}{minute:02}{second:02}Z"),
            iso: format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z"),
        }
    }
}

/// Days since 1970-01-01 to a proleptic-Gregorian `(year, month, day)` — Howard Hinnant's
/// `civil_from_days`, which is exact for every date this crate can see and needs no table.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = (z - era * 146_097) as u64;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_and_known_instants_round_trip() {
        assert_eq!(Stamp::at(0).iso, "1970-01-01T00:00:00Z");
        assert_eq!(Stamp::at(0).compact, "19700101T000000Z");
        // 2026-09-18T14:22:33Z
        assert_eq!(Stamp::at(1_789_741_353).iso, "2026-09-18T14:22:33Z");
        // A leap day, the case a naive 365-day walk gets wrong.
        assert_eq!(Stamp::at(1_709_164_800).iso, "2024-02-29T00:00:00Z");
    }
}
