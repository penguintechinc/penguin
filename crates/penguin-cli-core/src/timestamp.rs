//! Formats a `LogLine.at_unix_nano` timestamp for `penguin logs`.
//!
//! # Divergence from Go
//!
//! `cmdLogs` (`go-client/cmd/penguin/main.go`) formats via
//! `time.Unix(0, line.AtUnixNano).Format("2006-01-02 15:04:05")` — the host's
//! *local* time zone. Reproducing that exactly would mean either adding a
//! timezone-database dependency (no `chrono`/`time` crate is in the
//! workspace, and this crate must not add root-level dependencies — see the
//! crate's `Cargo.toml`) or shelling out to the OS, both disproportionate for
//! one log timestamp format. [`format_log_timestamp`] renders the same
//! `YYYY-MM-DD HH:MM:SS` shape in **UTC** instead; see `docs/PARITY.md`.

/// Formats a Unix-epoch timestamp in nanoseconds as `YYYY-MM-DD HH:MM:SS` in
/// UTC — everything [`crate::render::render_log_line`] needs from a
/// `LogLine.at_unix_nano` value.
pub fn format_log_timestamp(at_unix_nano: i64) -> String {
    let total_seconds = at_unix_nano.div_euclid(1_000_000_000);
    let days = total_seconds.div_euclid(86_400);
    let secs_of_day = total_seconds.rem_euclid(86_400);
    let hour = secs_of_day / 3600;
    let minute = (secs_of_day % 3600) / 60;
    let second = secs_of_day % 60;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}")
}

/// Converts a day count since the Unix epoch (1970-01-01) into a
/// `(year, month, day)` proleptic-Gregorian civil date.
///
/// Howard Hinnant's `civil_from_days` algorithm (public domain,
/// <https://howardhinnant.github.io/date_algorithms.html>), exact over the
/// entire `i64` range relevant to real timestamps — chosen over a date/time
/// crate specifically so this stays dependency-free.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097); // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    let year = if month <= 2 { y + 1 } else { y };
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_epoch_formats_as_midnight_jan_first_1970() {
        assert_eq!(format_log_timestamp(0), "1970-01-01 00:00:00");
    }

    #[test]
    fn a_known_timestamp_formats_correctly() {
        // 2024-01-15 12:34:56 UTC.
        let seconds: i64 = 1_705_322_096;
        assert_eq!(
            format_log_timestamp(seconds * 1_000_000_000),
            "2024-01-15 12:34:56"
        );
    }

    #[test]
    fn sub_second_nanos_are_truncated_not_rounded() {
        let seconds: i64 = 1_705_322_096;
        assert_eq!(
            format_log_timestamp(seconds * 1_000_000_000 + 999_999_999),
            "2024-01-15 12:34:56"
        );
    }

    #[test]
    fn end_of_year_rolls_over_correctly() {
        // 2023-12-31 23:59:59 UTC.
        let seconds: i64 = 1_704_067_199;
        assert_eq!(
            format_log_timestamp(seconds * 1_000_000_000),
            "2023-12-31 23:59:59"
        );
    }

    #[test]
    fn leap_day_formats_correctly() {
        // 2024-02-29 00:00:00 UTC.
        let seconds: i64 = 1_709_164_800;
        assert_eq!(
            format_log_timestamp(seconds * 1_000_000_000),
            "2024-02-29 00:00:00"
        );
    }
}
