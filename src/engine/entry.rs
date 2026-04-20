use crate::config::TradingConfig;
use crate::engine::state::EngineState;
use crate::model::{MarketSnapshot, Side};
use crate::time_utils::in_trading_hours;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    TradingDisabled,
    OpenPositionExists,
    AlreadyTradedThisMarket,
    CooldownActive,
    WarmingUp,
    OutsideTradingHours,
    MarketNotAlive,
    CheapSideOutOfRange,
    SpreadTooWide,
    NegativeExpectedValue,
    PricesUnavailable,
    CircuitBreakerTripped,
}

impl SkipReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            SkipReason::TradingDisabled => "trading_disabled",
            SkipReason::OpenPositionExists => "open_position_exists",
            SkipReason::AlreadyTradedThisMarket => "already_traded_this_market",
            SkipReason::CooldownActive => "cooldown_active",
            SkipReason::WarmingUp => "warming_up",
            SkipReason::OutsideTradingHours => "outside_trading_hours",
            SkipReason::MarketNotAlive => "market_not_alive",
            SkipReason::CheapSideOutOfRange => "cheap_side_out_of_range",
            SkipReason::SpreadTooWide => "spread_too_wide",
            SkipReason::NegativeExpectedValue => "negative_expected_value",
            SkipReason::PricesUnavailable => "prices_unavailable",
            SkipReason::CircuitBreakerTripped => "circuit_breaker_tripped",
        }
    }
}

/// Phase of the 5-minute market cycle. Stored on the trade record for post-mortem analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum MarketPhase {
    Early, // > 3 min left
    Mid,   // 1.5 – 3 min left
    Late,  // < 1.5 min left (entry blocked by MarketNotAlive)
}

impl MarketPhase {
    pub fn from_minutes_left(min: f64) -> Self {
        if min > 3.0 {
            MarketPhase::Early
        } else if min >= 1.5 {
            MarketPhase::Mid
        } else {
            MarketPhase::Late
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            MarketPhase::Early => "EARLY",
            MarketPhase::Mid => "MID",
            MarketPhase::Late => "LATE",
        }
    }
}

#[derive(Debug, Clone)]
pub struct EntryOrder {
    pub side: Side,
    pub price: Decimal,
    pub limit_price: Option<Decimal>,
    pub phase: MarketPhase,
}

#[derive(Debug, Clone)]
pub enum EntryDecision {
    Enter(EntryOrder),
    Skip(SkipReason),
}

#[derive(Debug, Clone, Serialize)]
pub struct GateStatus {
    pub name: &'static str,
    pub pass: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GateReport {
    pub all_pass: bool,
    pub gates: Vec<GateStatus>,
}

/// Evaluate every entry gate independently (no short-circuit) for diagnostic display
/// in the UI. Mirrors the rules in `evaluate_entry` but surfaces the full list of
/// gate pass/fail + human-readable context strings.
pub fn evaluate_gates(
    state: &EngineState,
    snapshot: Option<&MarketSnapshot>,
    cfg: &TradingConfig,
    now: DateTime<Utc>,
) -> GateReport {
    let mut gates: Vec<GateStatus> = Vec::new();
    let push = |gs: &mut Vec<GateStatus>, name, pass, detail: Option<String>| {
        gs.push(GateStatus { name, pass, detail });
    };

    push(&mut gates, "trading_enabled", state.trading_enabled, None);

    let pos_detail = state
        .position
        .as_ref()
        .map(|p| format!("{} {} @ {}", p.side.as_str(), p.shares, p.entry_price));
    push(&mut gates, "no_open_position", state.position.is_none(), pos_detail);

    let diff_ok = match (&state.last_traded_slug, snapshot) {
        (Some(last), Some(sn)) => *last != sn.market_slug,
        _ => true,
    };
    push(
        &mut gates,
        "different_market",
        diff_ok,
        state.last_traded_slug.clone().map(|s| format!("last: {s}")),
    );

    let cooldown_remain = state
        .last_exit_time
        .map(|t| cfg.cooldown_after_exit_sec as i64 - (now - t).num_seconds())
        .unwrap_or(0);
    let cooldown_ok = cooldown_remain <= 0;
    push(
        &mut gates,
        "cooldown_clear",
        cooldown_ok,
        (!cooldown_ok).then(|| format!("{cooldown_remain}s remaining")),
    );

    let uptime = (now - state.boot_time).num_seconds();
    let warmup_remain = cfg.warmup_ticks as i64 - uptime;
    let warmup_ok = warmup_remain <= 0;
    push(
        &mut gates,
        "warmup_complete",
        warmup_ok,
        (!warmup_ok).then(|| format!("{warmup_remain}s remaining")),
    );

    let cb_ok = !state.circuit_breaker_tripped(now);
    push(
        &mut gates,
        "circuit_breaker_clear",
        cb_ok,
        (!cb_ok).then(|| format!("losses: {}", state.circuit_breaker.consecutive_losses)),
    );

    let hours_ok = in_trading_hours(
        now,
        cfg.trading_hours_start_pst,
        cfg.trading_hours_end_pst,
        cfg.allow_weekends,
    );
    push(
        &mut gates,
        "in_trading_hours",
        hours_ok,
        Some(format!(
            "{}–{} PST{}",
            cfg.trading_hours_start_pst,
            cfg.trading_hours_end_pst,
            if cfg.allow_weekends { "" } else { " weekdays" }
        )),
    );

    if let Some(sn) = snapshot {
        let time_left = sn.time_left_minutes(now);
        let alive_ok = time_left >= cfg.time_left_min_minutes;
        push(
            &mut gates,
            "market_alive",
            alive_ok,
            Some(format!(
                "{:.1}m left (min {:.1}m)",
                time_left, cfg.time_left_min_minutes
            )),
        );

        let up_ask = sn.up_ask;
        let dn_ask = sn.down_ask;
        let prices_ok = up_ask.is_some() || dn_ask.is_some();
        let fmt_ask = |p: Option<Decimal>| p.map(|d| d.to_string()).unwrap_or_else(|| "—".into());
        push(
            &mut gates,
            "prices_available",
            prices_ok,
            Some(format!("up={} down={}", fmt_ask(up_ask), fmt_ask(dn_ask))),
        );

        let up_in = up_ask
            .map(|p| p >= cfg.cheap_side_min && p <= cfg.cheap_side_max)
            .unwrap_or(false);
        let dn_in = dn_ask
            .map(|p| p >= cfg.cheap_side_min && p <= cfg.cheap_side_max)
            .unwrap_or(false);
        push(
            &mut gates,
            "cheap_side_in_range",
            up_in || dn_in,
            Some(format!(
                "bounds [{}, {}]",
                cfg.cheap_side_min, cfg.cheap_side_max
            )),
        );

        // Evaluate spread on the side that `evaluate_entry` would pick.
        let (side_ask, side_bid) = if up_in && (!dn_in || up_ask <= dn_ask) {
            (sn.up_ask, sn.up_bid)
        } else if dn_in {
            (sn.down_ask, sn.down_bid)
        } else {
            (None, None)
        };
        let (spread_ok, spread_detail) = match (side_ask, side_bid) {
            (Some(a), Some(b)) => {
                let spread = a - b;
                (
                    spread <= cfg.max_entry_spread,
                    Some(format!("{} (max {})", spread, cfg.max_entry_spread)),
                )
            }
            _ => (true, None),
        };
        push(&mut gates, "spread_ok", spread_ok, spread_detail);
    } else {
        push(
            &mut gates,
            "market_alive",
            false,
            Some("no market loaded".into()),
        );
    }

    let all_pass = gates.iter().all(|g| g.pass);
    GateReport { all_pass, gates }
}

/// Pure entry-gate function. The engine calls this every tick; no I/O, no mutation.
pub fn evaluate_entry(
    state: &EngineState,
    snapshot: &MarketSnapshot,
    cfg: &TradingConfig,
    now: DateTime<Utc>,
) -> EntryDecision {
    if !state.trading_enabled {
        return EntryDecision::Skip(SkipReason::TradingDisabled);
    }
    if state.position.is_some() {
        return EntryDecision::Skip(SkipReason::OpenPositionExists);
    }
    if state
        .last_traded_slug
        .as_deref()
        .map(|s| s == snapshot.market_slug)
        .unwrap_or(false)
    {
        return EntryDecision::Skip(SkipReason::AlreadyTradedThisMarket);
    }
    // 5-minute cooldown after any exit to prevent revenge-trading on the next market.
    if let Some(exit_time) = state.last_exit_time {
        let cooldown_sec = cfg.cooldown_after_exit_sec as i64;
        if (now - exit_time).num_seconds() < cooldown_sec {
            return EntryDecision::Skip(SkipReason::CooldownActive);
        }
    }
    // Warmup period after boot: let WS book + market scheduler stabilize.
    let uptime_sec = (now - state.boot_time).num_seconds();
    if uptime_sec < cfg.warmup_ticks as i64 {
        return EntryDecision::Skip(SkipReason::WarmingUp);
    }
    if state.circuit_breaker_tripped(now) {
        return EntryDecision::Skip(SkipReason::CircuitBreakerTripped);
    }
    if !in_trading_hours(
        now,
        cfg.trading_hours_start_pst,
        cfg.trading_hours_end_pst,
        cfg.allow_weekends,
    ) {
        return EntryDecision::Skip(SkipReason::OutsideTradingHours);
    }
    if snapshot.time_left_minutes(now) < cfg.time_left_min_minutes {
        return EntryDecision::Skip(SkipReason::MarketNotAlive);
    }

    // Need at least one side's ask price to judge "cheap side".
    let up_ask = snapshot.up_ask;
    let down_ask = snapshot.down_ask;
    if up_ask.is_none() && down_ask.is_none() {
        return EntryDecision::Skip(SkipReason::PricesUnavailable);
    }

    let up_ok = up_ask
        .map(|p| p >= cfg.cheap_side_min && p <= cfg.cheap_side_max)
        .unwrap_or(false);
    let down_ok = down_ask
        .map(|p| p >= cfg.cheap_side_min && p <= cfg.cheap_side_max)
        .unwrap_or(false);

    let pick = match (up_ok, down_ok) {
        (false, false) => return EntryDecision::Skip(SkipReason::CheapSideOutOfRange),
        (true, false) => Some((Side::Up, up_ask.unwrap())),
        (false, true) => Some((Side::Down, down_ask.unwrap())),
        (true, true) => {
            // Both sides are cheap — take the cheaper one.
            let up = up_ask.unwrap();
            let dn = down_ask.unwrap();
            if up <= dn {
                Some((Side::Up, up))
            } else {
                Some((Side::Down, dn))
            }
        }
    };

    let Some((side, price)) = pick else {
        return EntryDecision::Skip(SkipReason::CheapSideOutOfRange);
    };

    // Spread gate: bid-ask must be tight enough for a good fill.
    if let (Some(ask), Some(bid)) = (snapshot.ask_for(side), snapshot.bid_for(side)) {
        if ask - bid > cfg.max_entry_spread {
            return EntryDecision::Skip(SkipReason::SpreadTooWide);
        }
    }

    let phase = MarketPhase::from_minutes_left(snapshot.time_left_minutes(now));
    EntryDecision::Enter(EntryOrder {
        side,
        price,
        limit_price: None, // tick loop fills this in via Kelly if enabled
        phase,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Mode;
    use chrono::TimeZone;
    use rust_decimal_macros::dec;

    fn cfg() -> TradingConfig {
        TradingConfig {
            mode: Mode::Paper,
            enabled_on_boot: false,
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
            max_entry_spread: dec!(0.04),
            cooldown_after_exit_sec: 300,
            warmup_ticks: 0,
            kelly: crate::config::KellyConfig {
                enabled: false,
                estimated_prob: dec!(0.50),
                fraction: dec!(0.25),
                max_pct: dec!(0.08),
                edge_capture: dec!(0.40),
            },
        }
    }

    fn snapshot_at(
        up: Option<Decimal>,
        down: Option<Decimal>,
        end_in_min: i64,
        now: DateTime<Utc>,
    ) -> MarketSnapshot {
        MarketSnapshot {
            market_slug: "btc-updown-5m-x".into(),
            up_token_id: "1".into(),
            down_token_id: "2".into(),
            end_date: now + chrono::Duration::minutes(end_in_min),
            up_price: up.unwrap_or(dec!(0.5)),
            down_price: down.unwrap_or(dec!(0.5)),
            up_ask: up,
            down_ask: down,
            up_bid: up,
            down_bid: down,
            fetched_at: now,
        }
    }

    fn snapshot(up: Option<Decimal>, down: Option<Decimal>, end_in_min: i64) -> MarketSnapshot {
        snapshot_at(up, down, end_in_min, weekday_active_now())
    }

    fn weekday_active_now() -> DateTime<Utc> {
        // Wed 2026-04-15 17:00 UTC == 10:00 PST (inside window)
        Utc.with_ymd_and_hms(2026, 4, 15, 17, 0, 0).unwrap()
    }

    fn enabled_state() -> EngineState {
        let mut s = EngineState::default();
        s.trading_enabled = true;
        // Set boot_time far enough in the past that warmup never blocks in tests.
        s.boot_time = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        s
    }

    #[test]
    fn trading_disabled_blocks() {
        let s = EngineState::default();
        let snap = snapshot(Some(dec!(0.25)), Some(dec!(0.75)), 4);
        let d = evaluate_entry(&s, &snap, &cfg(), weekday_active_now());
        assert!(matches!(d, EntryDecision::Skip(SkipReason::TradingDisabled)));
    }

    #[test]
    fn happy_path_picks_cheap_side() {
        let s = enabled_state();
        let snap = snapshot(Some(dec!(0.25)), Some(dec!(0.75)), 4);
        match evaluate_entry(&s, &snap, &cfg(), weekday_active_now()) {
            EntryDecision::Enter(o) => {
                assert_eq!(o.side, Side::Up);
                assert_eq!(o.price, dec!(0.25));
            }
            other => panic!("expected Enter, got {other:?}"),
        }
    }

    #[test]
    fn picks_cheaper_of_two_in_range() {
        let s = enabled_state();
        let snap = snapshot(Some(dec!(0.40)), Some(dec!(0.30)), 4);
        match evaluate_entry(&s, &snap, &cfg(), weekday_active_now()) {
            EntryDecision::Enter(o) => {
                assert_eq!(o.side, Side::Down);
                assert_eq!(o.price, dec!(0.30));
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn out_of_range_skips() {
        let s = enabled_state();
        let snap = snapshot(Some(dec!(0.10)), Some(dec!(0.90)), 4);
        assert!(matches!(
            evaluate_entry(&s, &snap, &cfg(), weekday_active_now()),
            EntryDecision::Skip(SkipReason::CheapSideOutOfRange)
        ));
    }

    #[test]
    fn no_prices_skips() {
        let s = enabled_state();
        let snap = snapshot(None, None, 4);
        assert!(matches!(
            evaluate_entry(&s, &snap, &cfg(), weekday_active_now()),
            EntryDecision::Skip(SkipReason::PricesUnavailable)
        ));
    }

    #[test]
    fn near_settlement_skips() {
        let s = enabled_state();
        let snap = snapshot(Some(dec!(0.25)), Some(dec!(0.75)), 1); // 1 min left, need 1.5
        assert!(matches!(
            evaluate_entry(&s, &snap, &cfg(), weekday_active_now()),
            EntryDecision::Skip(SkipReason::MarketNotAlive)
        ));
    }

    #[test]
    fn outside_hours_skips() {
        let s = enabled_state();
        // Wed 02:00 UTC == Tue 18:00 PST (after window)
        let after_hours = Utc.with_ymd_and_hms(2026, 4, 15, 2, 0, 0).unwrap();
        let snap = snapshot_at(Some(dec!(0.25)), Some(dec!(0.75)), 4, after_hours);
        assert!(matches!(
            evaluate_entry(&s, &snap, &cfg(), after_hours),
            EntryDecision::Skip(SkipReason::OutsideTradingHours)
        ));
    }

    #[test]
    fn open_position_skips() {
        use crate::model::OpenPosition;
        let mut s = enabled_state();
        s.position = Some(OpenPosition {
            id: "t1".into(),
            side: Side::Up,
            entry_price: dec!(0.25),
            shares: dec!(100),
            contract_size: dec!(25),
            entry_time: Utc::now(),
            market_slug: "btc-updown-5m-x".into(),
            market_end_date: Utc::now() + chrono::Duration::minutes(4),
            token_id: "1".into(),
            mode: Mode::Paper,
            max_unrealized_pnl: dec!(0),
            min_unrealized_pnl: dec!(0),
        });
        let snap = snapshot(Some(dec!(0.25)), Some(dec!(0.75)), 4);
        assert!(matches!(
            evaluate_entry(&s, &snap, &cfg(), weekday_active_now()),
            EntryDecision::Skip(SkipReason::OpenPositionExists)
        ));
    }
}
