use crate::config::{KellyConfig, TradingConfig};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

#[derive(Debug, Clone)]
pub struct KellyResult {
    pub stake: Decimal,
    pub shares: Decimal,
    pub limit_price: Decimal,
    pub edge: Decimal,
    pub raw_kelly: Decimal,
}

/// Quarter-Kelly sizing for binary prediction markets.
///
/// `f* = [(p × b) - (1-p)] / b × fraction`
///
/// Returns `None` if edge ≤ 0 (negative EV — entry gate should skip).
pub fn kelly_size(
    balance: Decimal,
    price: Decimal,
    fee_rate: Decimal,
    kelly: &KellyConfig,
) -> Option<KellyResult> {
    if balance <= dec!(0) || price <= dec!(0) || price >= dec!(1) {
        return None;
    }
    let p = kelly.estimated_prob;
    let edge = p - price;
    if edge <= dec!(0) {
        return None;
    }

    let b = (dec!(1) - price) / price;
    let raw_kelly = (p * b - (dec!(1) - p)) / b;
    if raw_kelly <= dec!(0) {
        return None;
    }

    let size_pct = raw_kelly * kelly.fraction;
    let capped_pct = size_pct.min(kelly.max_pct);
    let stake = balance * capped_pct;

    let limit_price = price + (edge * kelly.edge_capture);
    let effective = limit_price * (dec!(1) + fee_rate);
    if effective <= dec!(0) {
        return None;
    }
    let shares = round_down(stake / effective, 2);
    if shares <= dec!(0) {
        return None;
    }

    Some(KellyResult {
        stake,
        shares,
        limit_price: round_down(limit_price, 3),
        edge,
        raw_kelly,
    })
}

/// Flat percentage sizing (original strategy, used when Kelly is disabled).
pub fn size_trade(balance: Decimal, price: Decimal, cfg: &TradingConfig) -> Decimal {
    if balance <= dec!(0) || price <= dec!(0) {
        return dec!(0);
    }
    let raw_stake = balance * cfg.stake_pct;
    let stake = raw_stake.clamp(cfg.min_stake_usd, cfg.max_stake_usd);
    let effective = price * (dec!(1) + cfg.paper_fee_rate);
    if effective <= dec!(0) {
        return dec!(0);
    }
    let shares = stake / effective;
    round_down(shares, 2)
}

fn round_down(value: Decimal, dp: u32) -> Decimal {
    value.round_dp_with_strategy(dp, rust_decimal::RoundingStrategy::ToZero)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TradingConfig;
    use crate::model::Mode;

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
                enabled: true,
                estimated_prob: dec!(0.50),
                fraction: dec!(0.25),
                max_pct: dec!(0.08),
                edge_capture: dec!(0.40),
            },
        }
    }

    fn kelly_cfg() -> KellyConfig {
        cfg().kelly
    }

    #[test]
    fn kelly_positive_edge_at_cheap_price() {
        // prob=0.50, price=0.25: edge=0.25, b=3.0
        // raw_kelly = (0.50*3 - 0.50)/3 = 0.333
        // size_pct = 0.333 * 0.25 = 0.0833 → capped at 0.08
        // stake = 1000 * 0.08 = 80
        // limit_price = 0.25 + 0.25*0.4 = 0.35
        let r = kelly_size(dec!(1000), dec!(0.25), dec!(0.02), &kelly_cfg()).unwrap();
        assert_eq!(r.stake, dec!(80));
        assert_eq!(r.limit_price, dec!(0.350));
        assert!(r.shares > dec!(0));
    }

    #[test]
    fn kelly_returns_none_on_no_edge() {
        // prob=0.20, price=0.25 → edge = -0.05 → None
        let mut k = kelly_cfg();
        k.estimated_prob = dec!(0.20);
        assert!(kelly_size(dec!(1000), dec!(0.25), dec!(0.02), &k).is_none());
    }

    #[test]
    fn kelly_scales_with_probability() {
        let mut k = kelly_cfg();
        k.max_pct = dec!(1); // lift cap to see scaling
        k.estimated_prob = dec!(0.40);
        let r1 = kelly_size(dec!(1000), dec!(0.25), dec!(0), &k).unwrap();
        k.estimated_prob = dec!(0.60);
        let r2 = kelly_size(dec!(1000), dec!(0.25), dec!(0), &k).unwrap();
        assert!(r2.stake > r1.stake);
    }

    #[test]
    fn kelly_respects_hard_cap() {
        // prob=0.90, price=0.25 → huge raw Kelly → should cap at 8%
        let mut k = kelly_cfg();
        k.estimated_prob = dec!(0.90);
        let r = kelly_size(dec!(1000), dec!(0.25), dec!(0.02), &k).unwrap();
        assert_eq!(r.stake, dec!(80)); // 1000 * 0.08
    }

    #[test]
    fn kelly_limit_price_inside_spread() {
        let r = kelly_size(dec!(1000), dec!(0.25), dec!(0), &kelly_cfg()).unwrap();
        assert!(r.limit_price > dec!(0.25));
        assert!(r.limit_price < dec!(0.50));
    }

    // --- Flat sizing (existing tests preserved) ---

    #[test]
    fn happy_path_8pct_of_1000_at_0_25() {
        let s = size_trade(dec!(1000), dec!(0.25), &cfg());
        assert_eq!(s, dec!(313.72));
    }

    #[test]
    fn clamps_to_min_stake() {
        let s = size_trade(dec!(100), dec!(0.25), &cfg());
        assert_eq!(s, dec!(98.03));
    }

    #[test]
    fn clamps_to_max_stake() {
        let s = size_trade(dec!(10000), dec!(0.25), &cfg());
        assert_eq!(s, dec!(980.39));
    }

    #[test]
    fn zero_balance_returns_zero() {
        assert_eq!(size_trade(dec!(0), dec!(0.25), &cfg()), dec!(0));
    }

    #[test]
    fn zero_price_returns_zero() {
        assert_eq!(size_trade(dec!(1000), dec!(0), &cfg()), dec!(0));
    }
}
