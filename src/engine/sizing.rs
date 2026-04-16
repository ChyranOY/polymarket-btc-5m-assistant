use crate::config::TradingConfig;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

/// Shares we're willing to buy given a price and available balance.
/// - stake = clamp(balance * stake_pct, min_stake, max_stake)
/// - shares = stake / (price * (1 + fee_rate))
/// - rounded DOWN to 0.01 shares (Polymarket min tick size)
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
            cheap_side_min: dec!(0.15),
            cheap_side_max: dec!(0.45),
            time_left_min_minutes: 1.5,
            trading_hours_start_pst: 6,
            trading_hours_end_pst: 17,
            allow_weekends: false,
            paper_fee_rate: dec!(0.02),
        }
    }

    #[test]
    fn happy_path_8pct_of_1000_at_0_25() {
        // stake = 1000 * 0.08 = 80 → within [25, 250]
        // effective price = 0.25 * 1.02 = 0.255
        // shares = 80 / 0.255 ≈ 313.725... → round_down to 313.72
        let s = size_trade(dec!(1000), dec!(0.25), &cfg());
        assert_eq!(s, dec!(313.72));
    }

    #[test]
    fn clamps_to_min_stake() {
        // 100 * 0.08 = 8 → below min 25 → stake = 25
        // shares = 25 / (0.25 * 1.02) ≈ 98.039... → 98.03
        let s = size_trade(dec!(100), dec!(0.25), &cfg());
        assert_eq!(s, dec!(98.03));
    }

    #[test]
    fn clamps_to_max_stake() {
        // 10000 * 0.08 = 800 → above max 250 → stake = 250
        // shares = 250 / (0.25 * 1.02) ≈ 980.392 → 980.39
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
