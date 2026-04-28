use crate::config::TradingConfig;
use crate::engine::state::EngineState;
use crate::model::{MarketSnapshot, OpenPosition};
use chrono::{DateTime, Utc};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitReason {
    StopLoss,
    TakeProfit,
    MarketRolled,
    ManualKillSwitch,
}

impl ExitReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            ExitReason::StopLoss => "stop_loss",
            ExitReason::TakeProfit => "take_profit",
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

    let mark = snapshot
        .bid_for(position.side)
        .unwrap_or(snapshot.price_for(position.side));
    let pnl = position.unrealized_pnl(mark);

    // Trailing take-profit removed 2026-04-27. 124 trades over the prior 48h
    // showed avg TP win ($9.90) was barely half avg SL loss ($17.47), and
    // trades that escaped the trailer to settle realized ~3× larger wins
    // ($27.38). Letting every position ride to stop-loss or settlement.
    // `cfg.take_profit_*` fields stay defined but go unused here.

    // Stop-loss: unrealized pnl <= -(contract_size * stop_loss_pct)
    let stop_loss_threshold = -(position.contract_size * cfg.stop_loss_pct);
    if pnl <= stop_loss_threshold {
        return ExitDecision::Exit(ExitReason::StopLoss);
    }

    ExitDecision::Hold
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::KellyConfig;
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
            take_profit_activation_pct: dec!(0.20),
            take_profit_giveback_pct: dec!(0.50),
            cheap_side_min: dec!(0.15),
            cheap_side_max: dec!(0.45),
            max_entry_spread: dec!(0.04),
            cooldown_after_exit_sec: 300,
            warmup_ticks: 0,
            time_left_min_minutes: 1.5,
            trading_hours_start_pst: 6,
            trading_hours_end_pst: 17,
            allow_weekends: false,
            paper_fee_rate: dec!(0.02),
            kelly: KellyConfig {
                enabled: false,
                estimated_prob: dec!(0.50),
                fraction: dec!(0.25),
                max_pct: dec!(0.08),
                edge_capture: dec!(0.40),
            },
        }
    }

    fn pos(slug: &str, side: Side, entry: rust_decimal::Decimal, shares: rust_decimal::Decimal) -> OpenPosition {
        let now = chrono::Utc::now();
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
            entry_strategy: None,
        }
    }

    fn snap(slug: &str, end_in_sec: i64, up_bid: rust_decimal::Decimal) -> MarketSnapshot {
        let now = chrono::Utc::now();
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
        let p = pos("m", Side::Up, dec!(0.25), dec!(100));
        let s = snap("m", 180, dec!(0.175));
        let state = EngineState::default();
        let d = evaluate_exit(&state, &p, &s, &cfg(), chrono::Utc::now());
        assert!(matches!(d, ExitDecision::Exit(ExitReason::StopLoss)));
    }

    #[test]
    fn stop_loss_not_tripped_above_threshold() {
        let p = pos("m", Side::Up, dec!(0.25), dec!(100));
        let s = snap("m", 180, dec!(0.20));
        let state = EngineState::default();
        let d = evaluate_exit(&state, &p, &s, &cfg(), chrono::Utc::now());
        assert!(matches!(d, ExitDecision::Hold));
    }

    #[test]
    fn losing_position_near_settlement_holds_for_volatility() {
        // 30s left, losing — still holds. Only stop-loss or settlement can exit.
        let p = pos("m", Side::Up, dec!(0.25), dec!(100));
        let s = snap("m", 30, dec!(0.20));
        let state = EngineState::default();
        let d = evaluate_exit(&state, &p, &s, &cfg(), chrono::Utc::now());
        assert!(matches!(d, ExitDecision::Hold));
    }

    #[test]
    fn rollover_fires_when_slug_changes_after_end() {
        let p = pos("old", Side::Up, dec!(0.25), dec!(100));
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
    fn tp_disabled_holds_through_giveback() {
        // Pre-removal, peak +$20 with current pnl back at +$10 would trip the
        // 50% giveback. Now the trailing-TP is gone — the position must HOLD
        // until SL or rollover. This locks in the new behavior.
        let mut p = pos("m", Side::Up, dec!(0.25), dec!(100));
        p.max_unrealized_pnl = dec!(20);
        let s = snap("m", 180, dec!(0.35));
        let state = EngineState::default();
        let d = evaluate_exit(&state, &p, &s, &cfg(), chrono::Utc::now());
        assert!(matches!(d, ExitDecision::Hold));
    }

    #[test]
    fn stop_loss_still_fires_when_giveback_would_have() {
        // Same mark that previously made TakeProfit pre-empt SL — now SL fires
        // because there's no TP path left.
        let mut p = pos("m", Side::Up, dec!(0.25), dec!(100));
        p.max_unrealized_pnl = dec!(20);
        let s = snap("m", 180, dec!(0.17)); // pnl = -8, below SL threshold of -7.5
        let state = EngineState::default();
        let d = evaluate_exit(&state, &p, &s, &cfg(), chrono::Utc::now());
        assert!(matches!(d, ExitDecision::Exit(ExitReason::StopLoss)));
    }

    #[test]
    fn kill_switch_exits_immediately() {
        let p = pos("m", Side::Up, dec!(0.25), dec!(100));
        let s = snap("m", 180, dec!(0.25));
        let mut state = EngineState::default();
        state.kill_switch = true;
        assert!(matches!(
            evaluate_exit(&state, &p, &s, &cfg(), chrono::Utc::now()),
            ExitDecision::Exit(ExitReason::ManualKillSwitch)
        ));
    }
}
