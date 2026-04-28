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
    FavoriteAskTooLow,    // no side priced at FAVORITE_MIN or above
    FavoriteAskTooHigh,   // favorite already at FAVORITE_MAX — no profit margin left
    SpotConfirmationMissing, // Coinbase history not yet warm enough for direction check
    SpotDirectionMismatch,   // BTC spot is moving against the favorite side
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
            SkipReason::FavoriteAskTooLow => "favorite_ask_too_low",
            SkipReason::FavoriteAskTooHigh => "favorite_ask_too_high",
            SkipReason::SpotConfirmationMissing => "spot_confirmation_missing",
            SkipReason::SpotDirectionMismatch => "spot_direction_mismatch",
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

/// Which entry path triggered this order — stored on the trade row so the UI
/// and replay tools can split performance by strategy.
pub const STRATEGY_FAVORITE: &str = "favorite";

/// Lower bound for the favorite's ask price. Below this, the outcome is still
/// uncertain enough that we don't have an edge.
const FAVORITE_MIN_ASK: rust_decimal::Decimal = rust_decimal_macros::dec!(0.75);
/// Upper bound — above this there's no profit margin worth the SL risk.
const FAVORITE_MAX_ASK: rust_decimal::Decimal = rust_decimal_macros::dec!(0.97);
/// Spot-direction confirmation window. Must agree with the chosen side.
/// Used by `tick.rs` when computing `spot_delta_30s_pct` from CoinbaseWs.
#[allow(dead_code)]
const SPOT_CONFIRMATION_SECS: u64 = 30;

#[derive(Debug, Clone)]
pub struct EntryOrder {
    pub side: Side,
    pub price: Decimal,
    pub limit_price: Option<Decimal>,
    pub phase: MarketPhase,
    pub strategy: &'static str,
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

        // Identify the favorite (the side priced higher) and check the
        // FAVORITE_MIN_ASK / FAVORITE_MAX_ASK band.
        let (fav_side, fav_ask) = match (up_ask, dn_ask) {
            (Some(u), Some(d)) if u >= d => (Some(Side::Up), Some(u)),
            (Some(_), Some(d)) => (Some(Side::Down), Some(d)),
            (Some(u), None) => (Some(Side::Up), Some(u)),
            (None, Some(d)) => (Some(Side::Down), Some(d)),
            _ => (None, None),
        };
        let in_band = fav_ask
            .map(|p| p >= FAVORITE_MIN_ASK && p <= FAVORITE_MAX_ASK)
            .unwrap_or(false);
        push(
            &mut gates,
            "favorite_in_range",
            in_band,
            Some(format!(
                "favorite={} bounds [{}, {}]",
                fmt_ask(fav_ask),
                FAVORITE_MIN_ASK,
                FAVORITE_MAX_ASK
            )),
        );

        // Evaluate spread on the favorite side.
        let (side_ask, side_bid) = match fav_side {
            Some(Side::Up) => (sn.up_ask, sn.up_bid),
            Some(Side::Down) => (sn.down_ask, sn.down_bid),
            None => (None, None),
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
///
/// **Strategy: Favorite.** Buy whichever side's ask is between
/// `FAVORITE_MIN_ASK` (0.75) and `FAVORITE_MAX_ASK` (0.97), but only when
/// BTC spot is also moving in that direction over the last
/// `SPOT_CONFIRMATION_SECS` (30s). The market has mostly priced in the
/// outcome and BTC's recent move agrees — high-conviction late entry.
///
/// `spot_delta_30s_pct` is the BTC spot percent change over the trailing
/// 30 seconds (from the Coinbase feed). `None` when history is too short
/// (just booted / reconnecting) — we skip rather than guess.
pub fn evaluate_entry(
    state: &EngineState,
    snapshot: &MarketSnapshot,
    cfg: &TradingConfig,
    now: DateTime<Utc>,
    spot_delta_30s_pct: Option<Decimal>,
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
    if let Some(exit_time) = state.last_exit_time {
        let cooldown_sec = cfg.cooldown_after_exit_sec as i64;
        if (now - exit_time).num_seconds() < cooldown_sec {
            return EntryDecision::Skip(SkipReason::CooldownActive);
        }
    }
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

    let up_ask = snapshot.up_ask;
    let down_ask = snapshot.down_ask;
    if up_ask.is_none() && down_ask.is_none() {
        return EntryDecision::Skip(SkipReason::PricesUnavailable);
    }

    // Pick the favorite — the side priced higher (closer to $1).
    let (side, price) = match (up_ask, down_ask) {
        (Some(u), Some(d)) if u >= d => (Side::Up, u),
        (Some(_), Some(d)) => (Side::Down, d),
        (Some(u), None) => (Side::Up, u),
        (None, Some(d)) => (Side::Down, d),
        _ => return EntryDecision::Skip(SkipReason::PricesUnavailable),
    };

    if price < FAVORITE_MIN_ASK {
        return EntryDecision::Skip(SkipReason::FavoriteAskTooLow);
    }
    if price > FAVORITE_MAX_ASK {
        return EntryDecision::Skip(SkipReason::FavoriteAskTooHigh);
    }

    // Spread gate.
    if let (Some(ask), Some(bid)) = (snapshot.ask_for(side), snapshot.bid_for(side)) {
        if ask - bid > cfg.max_entry_spread {
            return EntryDecision::Skip(SkipReason::SpreadTooWide);
        }
    }

    // Confirmation: BTC spot must be moving in the same direction as the
    // favorite over the trailing 30s. UP needs +delta, DOWN needs −delta.
    let delta = match spot_delta_30s_pct {
        Some(d) => d,
        None => return EntryDecision::Skip(SkipReason::SpotConfirmationMissing),
    };
    let direction_ok = match side {
        Side::Up => delta > Decimal::ZERO,
        Side::Down => delta < Decimal::ZERO,
    };
    if !direction_ok {
        return EntryDecision::Skip(SkipReason::SpotDirectionMismatch);
    }

    let phase = MarketPhase::from_minutes_left(snapshot.time_left_minutes(now));
    EntryDecision::Enter(EntryOrder {
        side,
        price,
        limit_price: None,
        phase,
        strategy: STRATEGY_FAVORITE,
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
            take_profit_activation_pct: dec!(0.20),
            take_profit_giveback_pct: dec!(0.50),
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

    /// 0.5% positive 30s spot delta — confirms an UP entry.
    fn spot_up() -> Option<Decimal> { Some(dec!(0.5)) }
    /// 0.5% negative 30s spot delta — confirms a DOWN entry.
    fn spot_down() -> Option<Decimal> { Some(dec!(-0.5)) }
    /// Flat spot — direction can't be confirmed.
    fn spot_flat() -> Option<Decimal> { Some(dec!(0)) }

    #[test]
    fn trading_disabled_blocks() {
        let s = EngineState::default();
        let snap = snapshot(Some(dec!(0.20)), Some(dec!(0.80)), 4);
        let d = evaluate_entry(&s, &snap, &cfg(), weekday_active_now(), spot_up());
        assert!(matches!(d, EntryDecision::Skip(SkipReason::TradingDisabled)));
    }

    #[test]
    fn happy_path_picks_favorite_with_spot_confirmation() {
        // DOWN at 0.80 is the favorite; spot is moving down → confirmed.
        let s = enabled_state();
        let snap = snapshot(Some(dec!(0.20)), Some(dec!(0.80)), 4);
        match evaluate_entry(&s, &snap, &cfg(), weekday_active_now(), spot_down()) {
            EntryDecision::Enter(o) => {
                assert_eq!(o.side, Side::Down);
                assert_eq!(o.price, dec!(0.80));
                assert_eq!(o.strategy, STRATEGY_FAVORITE);
            }
            other => panic!("expected Enter, got {other:?}"),
        }
    }

    #[test]
    fn favorite_below_min_skips() {
        // No side ≥ 0.75 — too uncertain to bet either way.
        let s = enabled_state();
        let snap = snapshot(Some(dec!(0.40)), Some(dec!(0.60)), 4);
        assert!(matches!(
            evaluate_entry(&s, &snap, &cfg(), weekday_active_now(), spot_up()),
            EntryDecision::Skip(SkipReason::FavoriteAskTooLow)
        ));
    }

    #[test]
    fn favorite_above_max_skips() {
        // 0.99 is too tight a margin for the SL risk.
        let s = enabled_state();
        let snap = snapshot(Some(dec!(0.01)), Some(dec!(0.99)), 4);
        assert!(matches!(
            evaluate_entry(&s, &snap, &cfg(), weekday_active_now(), spot_down()),
            EntryDecision::Skip(SkipReason::FavoriteAskTooHigh)
        ));
    }

    #[test]
    fn spot_direction_mismatch_skips() {
        // DOWN favorite but spot is moving UP — refuse to fight the trend.
        let s = enabled_state();
        let snap = snapshot(Some(dec!(0.20)), Some(dec!(0.80)), 4);
        assert!(matches!(
            evaluate_entry(&s, &snap, &cfg(), weekday_active_now(), spot_up()),
            EntryDecision::Skip(SkipReason::SpotDirectionMismatch)
        ));
    }

    #[test]
    fn flat_spot_counts_as_mismatch() {
        // Zero delta — no confirmation either way; skip rather than guess.
        let s = enabled_state();
        let snap = snapshot(Some(dec!(0.20)), Some(dec!(0.80)), 4);
        assert!(matches!(
            evaluate_entry(&s, &snap, &cfg(), weekday_active_now(), spot_flat()),
            EntryDecision::Skip(SkipReason::SpotDirectionMismatch)
        ));
    }

    #[test]
    fn missing_spot_data_skips() {
        // Coinbase feed not warm yet.
        let s = enabled_state();
        let snap = snapshot(Some(dec!(0.20)), Some(dec!(0.80)), 4);
        assert!(matches!(
            evaluate_entry(&s, &snap, &cfg(), weekday_active_now(), None),
            EntryDecision::Skip(SkipReason::SpotConfirmationMissing)
        ));
    }

    #[test]
    fn no_prices_skips() {
        let s = enabled_state();
        let snap = snapshot(None, None, 4);
        assert!(matches!(
            evaluate_entry(&s, &snap, &cfg(), weekday_active_now(), spot_up()),
            EntryDecision::Skip(SkipReason::PricesUnavailable)
        ));
    }

    #[test]
    fn near_settlement_skips() {
        let s = enabled_state();
        let snap = snapshot(Some(dec!(0.20)), Some(dec!(0.80)), 1);
        assert!(matches!(
            evaluate_entry(&s, &snap, &cfg(), weekday_active_now(), spot_down()),
            EntryDecision::Skip(SkipReason::MarketNotAlive)
        ));
    }

    #[test]
    fn outside_hours_skips() {
        let s = enabled_state();
        let after_hours = Utc.with_ymd_and_hms(2026, 4, 15, 2, 0, 0).unwrap();
        let snap = snapshot_at(Some(dec!(0.20)), Some(dec!(0.80)), 4, after_hours);
        assert!(matches!(
            evaluate_entry(&s, &snap, &cfg(), after_hours, spot_down()),
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
            entry_strategy: None,
        });
        let snap = snapshot(Some(dec!(0.20)), Some(dec!(0.80)), 4);
        assert!(matches!(
            evaluate_entry(&s, &snap, &cfg(), weekday_active_now(), spot_down()),
            EntryDecision::Skip(SkipReason::OpenPositionExists)
        ));
    }
}
