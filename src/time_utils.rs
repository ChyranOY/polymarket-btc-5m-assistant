use chrono::{DateTime, Datelike, Duration, NaiveTime, TimeZone, Timelike, Utc, Weekday};
use chrono_tz::America::Los_Angeles;
use std::time::Duration as StdDuration;
use tokio::time::Instant;

/// True if `now` is inside the PST trading window (inclusive start, exclusive end)
/// and (optionally) excluding weekends.
pub fn in_trading_hours(
    now: DateTime<Utc>,
    start_hour_pst: u32,
    end_hour_pst: u32,
    allow_weekends: bool,
) -> bool {
    let local = now.with_timezone(&Los_Angeles);
    if !allow_weekends {
        match local.weekday() {
            Weekday::Sat | Weekday::Sun => return false,
            _ => {}
        }
    }
    let h = local.hour();
    h >= start_hour_pst && h < end_hour_pst
}

/// Compute the tokio Instant when the next trading window opens.
/// Used to sleep precisely until market open instead of polling.
pub fn next_trading_open(
    now: DateTime<Utc>,
    start_hour_pst: u32,
    allow_weekends: bool,
) -> Instant {
    let local = now.with_timezone(&Los_Angeles);
    let start_time = NaiveTime::from_hms_opt(start_hour_pst, 0, 0).unwrap();

    let mut candidate = local.date_naive();

    // If we haven't passed the start hour today, today is the candidate.
    // Otherwise, start from tomorrow.
    if local.time() >= start_time {
        candidate += Duration::days(1);
    }

    // Skip weekends if needed.
    if !allow_weekends {
        loop {
            match candidate.weekday() {
                Weekday::Sat => candidate += Duration::days(2),
                Weekday::Sun => candidate += Duration::days(1),
                _ => break,
            }
        }
    }

    let naive_open = candidate.and_time(start_time);
    let open_utc = match Los_Angeles.from_local_datetime(&naive_open) {
        chrono::LocalResult::Single(dt) => dt.with_timezone(&Utc),
        chrono::LocalResult::Ambiguous(dt, _) => dt.with_timezone(&Utc),
        chrono::LocalResult::None => {
            // DST gap — try 1 hour later.
            let shifted = candidate.and_hms_opt(start_hour_pst + 1, 0, 0).unwrap();
            Los_Angeles
                .from_local_datetime(&shifted)
                .single()
                .unwrap_or_else(|| (now + Duration::hours(1)).with_timezone(&Los_Angeles))
                .with_timezone(&Utc)
        }
    };

    let until = (open_utc - now).num_milliseconds().max(0) as u64;
    Instant::now() + StdDuration::from_millis(until)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration as ChronoDuration;
    use chrono::TimeZone;

    #[test]
    fn weekday_inside_window() {
        // Wed 2026-04-15 10:00 PST == 17:00 UTC
        let now = Utc.with_ymd_and_hms(2026, 4, 15, 17, 0, 0).unwrap();
        assert!(in_trading_hours(now, 6, 17, false));
    }

    #[test]
    fn before_window() {
        // Wed 2026-04-15 05:00 PST == 12:00 UTC
        let now = Utc.with_ymd_and_hms(2026, 4, 15, 12, 0, 0).unwrap();
        assert!(!in_trading_hours(now, 6, 17, false));
    }

    #[test]
    fn after_window() {
        // Wed 2026-04-15 17:30 PST == Thu 00:30 UTC
        let now = Utc.with_ymd_and_hms(2026, 4, 16, 0, 30, 0).unwrap();
        assert!(!in_trading_hours(now, 6, 17, false));
    }

    #[test]
    fn weekend_blocked() {
        // Sat 2026-04-18 10:00 PST == 17:00 UTC
        let now = Utc.with_ymd_and_hms(2026, 4, 18, 17, 0, 0).unwrap();
        assert!(!in_trading_hours(now, 6, 17, false));
        assert!(in_trading_hours(now, 6, 17, true));
    }

    #[test]
    fn next_open_after_hours_weekday() {
        // Wed 2026-04-15 18:00 PST (01:00 UTC Thu) → next open is Thu 06:00 PST (13:00 UTC)
        let now = Utc.with_ymd_and_hms(2026, 4, 16, 1, 0, 0).unwrap();
        let wake = next_trading_open(now, 6, false);
        let expected = Utc.with_ymd_and_hms(2026, 4, 16, 13, 0, 0).unwrap();
        let sleep_ms = (wake - Instant::now()).as_millis() as i64;
        let expected_ms = (expected - now).num_milliseconds();
        // Allow 100ms tolerance for test execution time.
        assert!((sleep_ms - expected_ms).abs() < 100);
    }

    #[test]
    fn next_open_friday_evening_skips_to_monday() {
        // Fri 2026-04-17 18:00 PST (Sat 01:00 UTC) → next open is Mon 06:00 PST
        let now = Utc.with_ymd_and_hms(2026, 4, 18, 1, 0, 0).unwrap();
        let wake = next_trading_open(now, 6, false);
        // Mon 2026-04-20 06:00 PST = 13:00 UTC
        let expected = Utc.with_ymd_and_hms(2026, 4, 20, 13, 0, 0).unwrap();
        let sleep_ms = (wake - Instant::now()).as_millis() as i64;
        let expected_ms = (expected - now).num_milliseconds();
        assert!((sleep_ms - expected_ms).abs() < 100);
    }

    #[test]
    fn next_open_before_start_is_today() {
        // Wed 2026-04-15 05:00 PST (12:00 UTC) → next open is today 06:00 PST (13:00 UTC)
        let now = Utc.with_ymd_and_hms(2026, 4, 15, 12, 0, 0).unwrap();
        let wake = next_trading_open(now, 6, false);
        let expected = Utc.with_ymd_and_hms(2026, 4, 15, 13, 0, 0).unwrap();
        let sleep_ms = (wake - Instant::now()).as_millis() as i64;
        let expected_ms = (expected - now).num_milliseconds();
        assert!((sleep_ms - expected_ms).abs() < 100);
    }
}
