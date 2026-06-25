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

/// Parses an IMF-fixdate (`Sun, 06 Nov 1994 08:49:37 GMT`) into a Unix
/// timestamp, or `None` if it is not a well-formed fixed-date in GMT. Only the
/// preferred IMF-fixdate form is accepted (the obsolete RFC 850 and asctime
/// forms are not), which is what modern clients send for `If-Modified-Since`.
pub fn parse_http_date(value: &str) -> Option<u64> {
    // "Wdy, DD Mon YYYY HH:MM:SS GMT" -> ["Wdy,", "DD", "Mon", "YYYY", "HH:MM:SS", "GMT"].
    let mut parts = value.split(' ');
    let _weekday = parts.next()?;
    let day: i64 = parts.next()?.parse().ok()?;
    let month_tok = parts.next()?;
    let month = MONTHS.iter().position(|&m| m == month_tok)? as i64 + 1;
    let year: i64 = parts.next()?.parse().ok()?;
    let mut time = parts.next()?.split(':');
    let hour: u64 = time.next()?.parse().ok()?;
    let minute: u64 = time.next()?.parse().ok()?;
    let second: u64 = time.next()?.parse().ok()?;
    if time.next().is_some() || parts.next() != Some("GMT") || parts.next().is_some() {
        return None;
    }
    if !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let days = days_from_civil(year, month, day);
    if days < 0 {
        return None;
    }
    Some(days as u64 * 86_400 + hour * 3_600 + minute * 60 + second)
}

/// Converts a civil (year, month, day) to a day count since the Unix epoch.
/// Howard Hinnant's `days_from_civil`, the inverse of [`civil_from_days`].
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = if month > 2 { month - 3 } else { month + 9 }; // [0, 11]
    let doy = (153 * mp + 2) / 5 + day - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
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

    #[test]
    fn parse_round_trips_format() {
        // parse_http_date must invert http_date so conditional comparisons line up.
        for secs in [0_u64, 1_234_567_890, 1_582_934_400] {
            assert_eq!(parse_http_date(&http_date(secs)), Some(secs));
        }
    }

    #[test]
    fn parse_rejects_malformed_and_non_gmt() {
        assert_eq!(parse_http_date("not a date"), None);
        assert_eq!(parse_http_date("Fri, 13 Feb 2009 23:31:30 UTC"), None);
        assert_eq!(parse_http_date("Fri, 13 Xxx 2009 23:31:30 GMT"), None);
    }
}
