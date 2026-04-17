use crate::model::{OpenPosition, Trade};
use chrono::{DateTime, NaiveDate, Utc};
use chrono_tz::America::Los_Angeles;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

/// Date used by the daily-PnL counter. The trading window is PST, so we roll the
/// counter at PST midnight rather than UTC midnight.
pub fn today_pst(now: DateTime<Utc>) -> NaiveDate {
    now.with_timezone(&Los_Angeles).date_naive()
}

const CIRCUIT_TRIP_LOSSES: u32 = 3;
const CIRCUIT_BASE_COOLDOWN_SEC: i64 = 5;
const CIRCUIT_MAX_COOLDOWN_SEC: i64 = 60;

#[derive(Debug, Clone)]
pub struct CircuitBreaker {
    pub consecutive_losses: u32,
    pub cooldown_until: Option<DateTime<Utc>>,
}

impl Default for CircuitBreaker {
    fn default() -> Self {
        Self {
            consecutive_losses: 0,
            cooldown_until: None,
        }
    }
}

impl CircuitBreaker {
    pub fn is_tripped(&self, now: DateTime<Utc>) -> bool {
        self.cooldown_until.map(|t| now < t).unwrap_or(false)
    }

    /// Reset after a win.
    pub fn reset(&mut self) {
        self.consecutive_losses = 0;
        self.cooldown_until = None;
    }

    /// Record a loss and possibly trip.
    pub fn record_loss(&mut self, now: DateTime<Utc>) {
        self.consecutive_losses += 1;
        if self.consecutive_losses >= CIRCUIT_TRIP_LOSSES {
            let extra = self.consecutive_losses - CIRCUIT_TRIP_LOSSES;
            let secs = (CIRCUIT_BASE_COOLDOWN_SEC << extra.min(5))
                .min(CIRCUIT_MAX_COOLDOWN_SEC);
            self.cooldown_until = Some(now + chrono::Duration::seconds(secs));
        }
    }
}

#[derive(Debug, Clone)]
pub struct EngineState {
    pub trading_enabled: bool,
    pub kill_switch: bool,
    pub balance: Decimal,
    pub daily_pnl: Decimal,
    /// The PST date that `daily_pnl` is scoped to. When it changes, the counter resets.
    pub daily_pnl_date: Option<NaiveDate>,
    pub position: Option<OpenPosition>,
    pub circuit_breaker: CircuitBreaker,
    pub last_tick: Option<DateTime<Utc>>,
    pub last_skip: Option<String>,
    /// Current unrealized PnL, updated every tick by the engine. `/status` reads this
    /// instead of doing its own WS book lookup (avoids key-mismatch / timing races).
    pub unrealized_pnl: Option<Decimal>,
    /// Slug of the most recently closed trade. Entry gate blocks re-entry into the same
    /// 5m market — one trade per market cycle.
    pub last_traded_slug: Option<String>,
    pub recent_trades: Vec<Trade>, // small in-memory cache for the UI
}

impl Default for EngineState {
    fn default() -> Self {
        Self {
            trading_enabled: false,
            kill_switch: false,
            balance: dec!(0),
            daily_pnl: dec!(0),
            daily_pnl_date: None,
            position: None,
            circuit_breaker: CircuitBreaker::default(),
            last_tick: None,
            last_skip: None,
            unrealized_pnl: None,
            last_traded_slug: None,
            recent_trades: Vec::new(),
        }
    }
}

impl EngineState {
    pub fn circuit_breaker_tripped(&self, now: DateTime<Utc>) -> bool {
        self.circuit_breaker.is_tripped(now)
    }

    /// Clear `daily_pnl` if the PST date has changed since it was last stamped.
    /// Safe to call from the tick loop — idempotent within a PST-day.
    pub fn maybe_roll_daily_pnl(&mut self, now: DateTime<Utc>) {
        let today = today_pst(now);
        match self.daily_pnl_date {
            Some(prev) if prev == today => {}
            _ => {
                self.daily_pnl = dec!(0);
                self.daily_pnl_date = Some(today);
            }
        }
    }

    pub fn record_trade_closed(&mut self, trade: Trade, now: DateTime<Utc>) {
        self.maybe_roll_daily_pnl(now);
        if let Some(p) = trade.pnl {
            self.daily_pnl += p;
            self.balance += p;
            if p < dec!(0) {
                self.circuit_breaker.record_loss(now);
            } else {
                self.circuit_breaker.reset();
            }
        }
        self.position = None;
        self.last_traded_slug = Some(trade.market_slug.clone());
        self.recent_trades.push(trade);
        // cap recent trades in-memory
        let max = 100;
        if self.recent_trades.len() > max {
            let drop = self.recent_trades.len() - max;
            self.recent_trades.drain(0..drop);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Mode, Side, TradeStatus};
    use chrono::TimeZone;

    fn closed_trade(pnl: Decimal, exit_time: DateTime<Utc>) -> Trade {
        Trade {
            id: "t".into(),
            timestamp: exit_time,
            status: TradeStatus::Closed,
            side: Side::Up,
            mode: Mode::Paper,
            entry_price: dec!(0.25),
            shares: dec!(100),
            contract_size: dec!(25),
            entry_time: exit_time,
            market_slug: "m".into(),
            entry_phase: None,
            exit_price: Some(dec!(0.30)),
            exit_time: Some(exit_time),
            exit_reason: Some("x".into()),
            pnl: Some(pnl),
            max_unrealized_pnl: dec!(0),
            min_unrealized_pnl: dec!(0),
            entry_gate_snapshot: None,
            extra_json: None,
            created_at: exit_time,
            updated_at: exit_time,
        }
    }

    #[test]
    fn daily_pnl_resets_on_pst_day_change() {
        let mut s = EngineState::default();
        // Day 1 PST: 2026-04-15 22:00 PST = 2026-04-16 05:00 UTC
        let day1 = Utc.with_ymd_and_hms(2026, 4, 16, 5, 0, 0).unwrap();
        s.record_trade_closed(closed_trade(dec!(10), day1), day1);
        assert_eq!(s.daily_pnl, dec!(10));
        // Day 2 PST: 2026-04-16 08:00 PST = 2026-04-16 15:00 UTC — next PST day
        let day2 = Utc.with_ymd_and_hms(2026, 4, 16, 15, 0, 0).unwrap();
        s.record_trade_closed(closed_trade(dec!(3), day2), day2);
        assert_eq!(s.daily_pnl, dec!(3)); // reset + new trade
    }

    #[test]
    fn daily_pnl_accumulates_within_pst_day() {
        let mut s = EngineState::default();
        let t1 = Utc.with_ymd_and_hms(2026, 4, 16, 16, 0, 0).unwrap();
        let t2 = Utc.with_ymd_and_hms(2026, 4, 16, 20, 0, 0).unwrap();
        s.record_trade_closed(closed_trade(dec!(5), t1), t1);
        s.record_trade_closed(closed_trade(dec!(-2), t2), t2);
        assert_eq!(s.daily_pnl, dec!(3));
    }

    #[test]
    fn circuit_trips_after_three_losses() {
        let now = Utc::now();
        let mut cb = CircuitBreaker::default();
        assert!(!cb.is_tripped(now));
        cb.record_loss(now);
        cb.record_loss(now);
        assert!(!cb.is_tripped(now));
        cb.record_loss(now);
        assert!(cb.is_tripped(now));
    }

    #[test]
    fn circuit_resets_after_win() {
        let now = Utc::now();
        let mut cb = CircuitBreaker::default();
        cb.record_loss(now);
        cb.record_loss(now);
        cb.record_loss(now);
        assert!(cb.is_tripped(now));
        cb.reset();
        assert!(!cb.is_tripped(now));
        assert_eq!(cb.consecutive_losses, 0);
    }

    #[test]
    fn backoff_grows() {
        let now = Utc::now();
        let mut cb = CircuitBreaker::default();
        cb.record_loss(now); cb.record_loss(now); cb.record_loss(now);
        let first = cb.cooldown_until.unwrap();
        cb.record_loss(now);
        let second = cb.cooldown_until.unwrap();
        assert!(second > first);
    }
}
