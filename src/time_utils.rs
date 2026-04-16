use chrono::{DateTime, Datelike, Timelike, Utc, Weekday};
use chrono_tz::America::Los_Angeles;

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

#[cfg(test)]
mod tests {
    use super::*;
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
}
