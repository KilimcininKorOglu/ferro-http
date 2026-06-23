//! HTTP-date formatting (RFC 7231 IMF-fixdate), `no_std` and dependency-free.

use alloc::format;
use alloc::string::String;

const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Formats a Unix timestamp (seconds since 1970-01-01 UTC) as an IMF-fixdate,
/// e.g. `Sun, 06 Nov 1994 08:49:37 GMT`.
pub fn http_date(unix_secs: u64) -> String {
    let days = (unix_secs / 86_400) as i64;
    let secs_of_day = unix_secs % 86_400;
    let hour = secs_of_day / 3_600;
    let minute = (secs_of_day % 3_600) / 60;
    let second = secs_of_day % 60;

    // 1970-01-01 was a Thursday; weekday 0 == Sunday.
    let weekday = ((days % 7 + 4) % 7) as usize;
    let (year, month, day) = civil_from_days(days);

    format!(
        "{}, {:02} {} {:04} {:02}:{:02}:{:02} GMT",
        WEEKDAYS[weekday],
        day,
        MONTHS[(month - 1) as usize],
        year,
        hour,
        minute,
        second
    )
}

/// Converts a day count since the Unix epoch to a civil (year, month, day).
///
/// Howard Hinnant's `civil_from_days`, valid across the full range we need.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let day = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_is_thursday() {
        assert_eq!(http_date(0), "Thu, 01 Jan 1970 00:00:00 GMT");
    }

    #[test]
    fn known_timestamp_formats_correctly() {
        // 1234567890 -> Fri, 13 Feb 2009 23:31:30 GMT (a widely cited value).
        assert_eq!(http_date(1_234_567_890), "Fri, 13 Feb 2009 23:31:30 GMT");
    }

    #[test]
    fn leap_day_is_handled() {
        // 2020-02-29 00:00:00 UTC = 1582934400.
        assert_eq!(http_date(1_582_934_400), "Sat, 29 Feb 2020 00:00:00 GMT");
    }
}
