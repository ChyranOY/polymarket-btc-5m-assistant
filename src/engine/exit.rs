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

    // Ride ITM to settlement: once the market agrees we've almost certainly
    // won (mark ≥ 0.90) AND there's barely any time left for a reversal
    // (< 60s), skip the trailing-TP check entirely. Stop-loss still runs
    // below, so a catastrophic reversal still closes us out.
    let time_left_sec = (position.market_end_date - now).num_seconds();
    let ride_to_settlement = mark >= rust_decimal_macros::dec!(0.90) && time_left_sec < 60;

    // Momentum trades skip the trailing-TP entirely — they're meant to ride
    // to settlement (or stop-loss) since the directional thesis is the whole
    // edge. Cheap-side trades keep the trailer.
    let is_momentum = position
        .entry_strategy
        .as_deref()
        .map(|s| s == "momentum")
        .unwrap_or(false);

    // Trailing take-profit: once MFE crosses the activation threshold, exit on a
    // giveback of the peak. Checked before stop-loss so a reversal from a winning
    // peak closes at a small gain instead of riding all the way to stop-out.
    let activation_abs = position.contract_size * cfg.take_profit_activation_pct;
    if !ride_to_settlement && !is_momentum && position.max_unrealized_pnl >= activation_abs {
        let giveback_threshold = position.max_unrealized_pnl * cfg.take_profit_giveback_pct;
        if pnl <= giveback_threshold {
            return ExitDecision::Exit(ExitReason::TakeProfit);
        }
    }

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
    fn take_profit_fires_after_armed_giveback() {
        // contract_size = 0.25 * 100 = 25. activation @ 20% → arm once mfe >= 5.
        // giveback @ 50% → exit when current pnl <= 50% of peak.
        let mut p = pos("m", Side::Up, dec!(0.25), dec!(100));
        p.max_unrealized_pnl = dec!(20); // peak +$20 (80% of contract), armed
        // current mark 0.35 → pnl = (0.35-0.25)*100 = +10 → 50% of peak 20
        let s = snap("m", 180, dec!(0.35));
        let state = EngineState::default();
        let d = evaluate_exit(&state, &p, &s, &cfg(), chrono::Utc::now());
        assert!(matches!(d, ExitDecision::Exit(ExitReason::TakeProfit)));
    }

    #[test]
    fn take_profit_does_not_fire_before_activation() {
        // peak +$4 (below +5 activation). No exit even if current dropped.
        let mut p = pos("m", Side::Up, dec!(0.25), dec!(100));
        p.max_unrealized_pnl = dec!(4);
        let s = snap("m", 180, dec!(0.26)); // pnl = +1, well under peak, but not armed
        let state = EngineState::default();
        let d = evaluate_exit(&state, &p, &s, &cfg(), chrono::Utc::now());
        assert!(matches!(d, ExitDecision::Hold));
    }

    #[test]
    fn take_profit_preempts_stop_loss_when_both_trip() {
        // Peak was +20 (armed). Now at -8 which is BELOW stop-loss (-7.5).
        // Take-profit retracement check runs first → TakeProfit.
        let mut p = pos("m", Side::Up, dec!(0.25), dec!(100));
        p.max_unrealized_pnl = dec!(20);
        let s = snap("m", 180, dec!(0.17)); // pnl = -8
        let state = EngineState::default();
        let d = evaluate_exit(&state, &p, &s, &cfg(), chrono::Utc::now());
        assert!(matches!(d, ExitDecision::Exit(ExitReason::TakeProfit)));
    }

    #[test]
    fn momentum_position_skips_trailing_tp() {
        // Same setup that fires TP on a cheap-side trade — but momentum trades
        // hold instead and ride to settlement / SL.
        let mut p = pos("m", Side::Up, dec!(0.25), dec!(100));
        p.max_unrealized_pnl = dec!(20);
        p.entry_strategy = Some("momentum".into());
        let s = snap("m", 180, dec!(0.35)); // would normally trigger giveback
        let state = EngineState::default();
        let d = evaluate_exit(&state, &p, &s, &cfg(), chrono::Utc::now());
        assert!(matches!(d, ExitDecision::Hold));
    }

    #[test]
    fn ride_itm_suspends_trailing_tp_near_settlement() {
        // Peak was +20 (armed). Now at +10 which would normally trip giveback.
        // But mark is 0.95 and <60s left → ride to settlement instead.
        let mut p = pos("m", Side::Up, dec!(0.25), dec!(100));
        p.max_unrealized_pnl = dec!(20);
        let s = snap("m", 30, dec!(0.95));
        let state = EngineState::default();
        let d = evaluate_exit(&state, &p, &s, &cfg(), chrono::Utc::now());
        assert!(matches!(d, ExitDecision::Hold));
    }

    #[test]
    fn ride_itm_does_not_trigger_with_lots_of_time_left() {
        // 3 min left, same high mark — ride-ITM shouldn't apply; trailing-TP
        // works normally (giveback fires because pnl retraced 50% from peak).
        let mut p = pos("m", Side::Up, dec!(0.25), dec!(100));
        p.max_unrealized_pnl = dec!(20);
        let s = snap("m", 180, dec!(0.35)); // pnl +10 = 50% of peak 20 → giveback
        let state = EngineState::default();
        let d = evaluate_exit(&state, &p, &s, &cfg(), chrono::Utc::now());
        assert!(matches!(d, ExitDecision::Exit(ExitReason::TakeProfit)));
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
