//! The reader's own calendar.
//!
//! macOS keeps this in `TimestampFormatting`. This client starts it where the
//! first screen needs it: the Server buckets every Dashboard day boundary in
//! the reader's IANA time zone and answers in Unix seconds, so the client has
//! to name that zone for the request and turn the seconds back into a local
//! date for the axis. Reviews still slices RFC 3339 text until it wants the
//! same pair.

use chrono::{Local, TimeZone as _};

/// The system's IANA time zone, such as `Asia/Shanghai`.
///
/// The Server requires one — every day boundary it computes is a date in it —
/// and the engine groups its own telemetry by the boundaries the Server named,
/// so naming the same zone here is what keeps the two halves of a Dashboard
/// describing the same days. A machine that will not say which zone it is in is
/// reported as UTC, which is a real zone rather than a guess at an offset.
pub fn time_zone() -> String {
    iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".to_owned())
}

/// One day of the local calendar, as macOS's abbreviated month and day writes
/// it: `Sep 26`.
pub fn day(epoch_seconds: i64) -> String {
    match Local.timestamp_opt(epoch_seconds, 0).earliest() {
        Some(moment) => moment.format("%b %-d").to_string(),
        None => epoch_seconds.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zone_is_always_named() {
        // Whatever this machine answers, the Server is given a name it accepts
        // rather than an empty query parameter.
        assert!(!time_zone().is_empty());
    }

    #[test]
    fn a_day_is_a_short_local_date() {
        // 2026-09-26T12:00:00Z, which every zone west of UTC+12 still calls the
        // 26th or the 25th; the shape is what is asserted, not the day, because
        // the answer depends on where the machine is.
        let label = day(1_790_452_800);
        assert!(label.len() >= 4, "unexpected day label: {label}");
        assert!(!label.contains("1790452800"), "the seconds leaked: {label}");
    }
}
