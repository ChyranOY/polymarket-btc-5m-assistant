use crate::config::TradingConfig;
use crate::engine::state::EngineState;
use crate::model::{MarketSnapshot, OpenPosition};
use chrono::{DateTime, Utc};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitReason {
    StopLoss,
    SettlementImminent,
    MarketRolled,
    ManualKillSwitch,
}

impl ExitReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            ExitReason::StopLoss => "stop_loss",
            ExitReason::SettlementImminent => "settlement_imminent",
            ExitReason::MarketRolled => "market_rolled",
            ExitReason::ManualKillSwitch => "manual_kill_switch",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ExitDecision {
    Exit(ExitReason),
    Hold,
}

const SETTLEMENT_IMMINENT_SEC: i64 = 60;

pub fn evaluate_exit(
    state: &EngineState,
    position: &OpenPosition,
    snapshot: &MarketSnapshot,
    cfg: &TradingConfig,
    now: DateTime<Utc>,
) -> ExitDecision {
    if state.kill_switch {
        return ExitDecision::Exit(ExitReason::ManualKillSwitch);
    }

    // Rollover: snapshot now references a different market than the one we entered.
    if snapshot.market_slug != position.market_slug && now >= position.market_end_date {
        return ExitDecision::Exit(ExitReason::MarketRolled);
    }

    // Settlement imminent: <60s left on the market we're in.
    if snapshot.market_slug == position.market_slug
        && snapshot.time_left_sec(now) < SETTLEMENT_IMMINENT_SEC
    {
        return ExitDecision::Exit(ExitReason::SettlementImminent);
    }

    // Stop-loss: unrealized pnl <= -(contract_size * stop_loss_pct)
    let mark = snapshot
        .bid_for(position.side)
        .unwrap_or(snapshot.price_for(position.side));
    let pnl = position.unrealized_pnl(mark);
    let stop_loss_threshold = -(position.contract_size * cfg.stop_loss_pct);
    if pnl <= stop_loss_threshold {
        return ExitDecision::Exit(ExitReason::StopLoss);
    }

    ExitDecision::Hold
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Mode, Side};
    use rust_decimal_macros::dec;

    fn cfg() -> TradingConfig {
        TradingConfig {
            mode: Mode::Paper,
            enabled_on_boot: true,
            stake_pct: dec!(0.08),
            min_stake_usd: dec!(25),
            max_stake_usd: dec!(250),
            starting_balance: dec!(1000),
            stop_loss_pct: dec!(0.30),
            cheap_side_min: dec!(0.15),
            cheap_side_max: dec!(0.45),
            time_left_min_minutes: 1.5,
            trading_hours_start_pst: 6,
            trading_hours_end_pst: 17,
            allow_weekends: false,
            paper_fee_rate: dec!(0.02),
        }
    }

    fn pos(slug: &str, side: Side, entry: rust_decimal::Decimal, shares: rust_decimal::Decimal) -> OpenPosition {
        let now = Utc::now();
        OpenPosition {
            id: "t1".into(),
            side,
            entry_price: entry,
            shares,
            contract_size: entry * shares,
            entry_time: now,
            market_slug: slug.into(),
            market_end_date: now + chrono::Duration::minutes(4),
            token_id: "1".into(),
            mode: Mode::Paper,
            max_unrealized_pnl: dec!(0),
            min_unrealized_pnl: dec!(0),
        }
    }

    fn snap(slug: &str, end_in_sec: i64, up_bid: rust_decimal::Decimal) -> MarketSnapshot {
        let now = Utc::now();
        MarketSnapshot {
            market_slug: slug.into(),
            up_token_id: "1".into(),
            down_token_id: "2".into(),
            end_date: now + chrono::Duration::seconds(end_in_sec),
            up_price: up_bid,
            down_price: dec!(1) - up_bid,
            up_ask: Some(up_bid),
            down_ask: Some(dec!(1) - up_bid),
            up_bid: Some(up_bid),
            down_bid: Some(dec!(1) - up_bid),
            fetched_at: now,
        }
    }

    #[test]
    fn stop_loss_at_minus_30pct() {
        // Position: 100 shares @ 0.25 → contract_size = 25; -30% = -7.5
        // Mark at 0.175 → pnl = (0.175 - 0.25) * 100 = -7.5 → exactly at threshold.
        let p = pos("m", Side::Up, dec!(0.25), dec!(100));
        let s = snap("m", 180, dec!(0.175));
        let state = EngineState::default();
        let d = evaluate_exit(&state, &p, &s, &cfg(), Utc::now());
        assert!(matches!(d, ExitDecision::Exit(ExitReason::StopLoss)));
    }

    #[test]
    fn stop_loss_not_tripped_above_threshold() {
        let p = pos("m", Side::Up, dec!(0.25), dec!(100));
        let s = snap("m", 180, dec!(0.20)); // pnl = -5, above -7.5
        let state = EngineState::default();
        let d = evaluate_exit(&state, &p, &s, &cfg(), Utc::now());
        assert!(matches!(d, ExitDecision::Hold));
    }

    #[test]
    fn settlement_imminent_under_60s() {
        let p = pos("m", Side::Up, dec!(0.25), dec!(100));
        let s = snap("m", 30, dec!(0.25));
        let state = EngineState::default();
        let d = evaluate_exit(&state, &p, &s, &cfg(), Utc::now());
        assert!(matches!(d, ExitDecision::Exit(ExitReason::SettlementImminent)));
    }

    #[test]
    fn rollover_fires_when_slug_changes_after_end() {
        let p = pos("old", Side::Up, dec!(0.25), dec!(100));
        // Snapshot now shows a new slug, and now > p.market_end_date.
        let later = p.market_end_date + chrono::Duration::seconds(5);
        let s = MarketSnapshot {
            market_slug: "new".into(),
            up_token_id: "3".into(),
            down_token_id: "4".into(),
            end_date: later + chrono::Duration::minutes(5),
            up_price: dec!(0.5),
            down_price: dec!(0.5),
            up_ask: Some(dec!(0.5)),
            down_ask: Some(dec!(0.5)),
            up_bid: Some(dec!(0.5)),
            down_bid: Some(dec!(0.5)),
            fetched_at: later,
        };
        let state = EngineState::default();
        let d = evaluate_exit(&state, &p, &s, &cfg(), later);
        assert!(matches!(d, ExitDecision::Exit(ExitReason::MarketRolled)));
    }

    #[test]
    fn kill_switch_exits_immediately() {
        let p = pos("m", Side::Up, dec!(0.25), dec!(100));
        let s = snap("m", 180, dec!(0.25));
        let mut state = EngineState::default();
        state.kill_switch = true;
        assert!(matches!(
            evaluate_exit(&state, &p, &s, &cfg(), Utc::now()),
            ExitDecision::Exit(ExitReason::ManualKillSwitch)
        ));
    }
}
