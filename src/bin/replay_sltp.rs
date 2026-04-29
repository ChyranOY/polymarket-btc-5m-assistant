//! Replay closed trades from Supabase against a grid of (SL, TP-activation,
//! TP-giveback) parameter combinations and print a ranked summary.
//!
//! We do NOT have tick-by-tick price history (signal_ticks writer is still
//! TODO), only MFE/MAE snapshots per trade. So we can't know whether the
//! drawdown or peak came first within a trade. The binary computes BOTH
//! orderings (SL-first and TP-first) plus the midpoint, and ranks by the
//! ordering-robust metric `min(worst, midpoint)`.
//!
//! Usage:
//!   cargo run --release --bin replay_sltp
//!   cargo run --release --bin replay_sltp -- --mode both --limit 2000 --top 30
//!   cargo run --release --bin replay_sltp -- --since 2026-04-01T00:00:00Z
//!
//! Config comes from the usual .env (SUPABASE_URL / SUPABASE_SERVICE_ROLE_KEY,
//! STOP_LOSS_PCT, TAKE_PROFIT_ACTIVATION_PCT, TAKE_PROFIT_GIVEBACK_PCT,
//! PAPER_FEE_RATE).

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use polymarket_btc_5m::config::AppConfig;
use polymarket_btc_5m::model::{Mode, Trade};
use polymarket_btc_5m::store::supabase::SupabaseClient;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use std::env;
use std::str::FromStr;

const DEFAULT_SLUG_PREFIX: &str = "btc-updown-15m";
const DEFAULT_LIMIT: usize = 2000;
const DEFAULT_TOP: usize = 20;

struct Args {
    mode: ModeFilter,
    slug_prefix: String,
    since: Option<DateTime<Utc>>,
    limit: usize,
    top: usize,
}

#[derive(Clone, Copy)]
enum ModeFilter {
    Paper,
    Live,
    Both,
}

fn parse_args() -> Result<Args> {
    let mut mode = ModeFilter::Both;
    let mut slug_prefix = DEFAULT_SLUG_PREFIX.to_string();
    let mut since: Option<DateTime<Utc>> = None;
    let mut limit = DEFAULT_LIMIT;
    let mut top = DEFAULT_TOP;

    let mut it = env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut val = |name: &str| {
            it.next()
                .ok_or_else(|| anyhow!("{name} requires a value"))
        };
        match flag.as_str() {
            "--mode" => {
                mode = match val("--mode")?.to_ascii_lowercase().as_str() {
                    "paper" => ModeFilter::Paper,
                    "live" => ModeFilter::Live,
                    "both" => ModeFilter::Both,
                    other => return Err(anyhow!("bad --mode {other}")),
                }
            }
            "--slug-prefix" => slug_prefix = val("--slug-prefix")?,
            "--since" => {
                let s = val("--since")?;
                since = Some(
                    DateTime::parse_from_rfc3339(&s)
                        .map_err(|e| anyhow!("bad --since: {e}"))?
                        .with_timezone(&Utc),
                );
            }
            "--limit" => limit = val("--limit")?.parse()?,
            "--top" => top = val("--top")?.parse()?,
            "-h" | "--help" => {
                eprintln!(
                    "replay_sltp — sweep SL/TP params against historical closed trades\n\
                     \n\
                     --mode paper|live|both     (default: both)\n\
                     --slug-prefix <str>        (default: {DEFAULT_SLUG_PREFIX})\n\
                     --since <rfc3339>          (default: no lower bound)\n\
                     --limit <n>                (default: {DEFAULT_LIMIT})\n\
                     --top <n>                  (default: {DEFAULT_TOP})\n"
                );
                std::process::exit(0);
            }
            other => return Err(anyhow!("unknown flag {other}")),
        }
    }
    Ok(Args {
        mode,
        slug_prefix,
        since,
        limit,
        top,
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    let args = parse_args()?;
    let cfg = AppConfig::from_env()?;
    let supabase = SupabaseClient::new(&cfg.supabase)?;
    if !supabase.enabled() {
        return Err(anyhow!(
            "Supabase not configured (SUPABASE_URL / SUPABASE_SERVICE_ROLE_KEY)"
        ));
    }

    let mut trades: Vec<Trade> = Vec::new();
    let modes: &[Mode] = match args.mode {
        ModeFilter::Paper => &[Mode::Paper],
        ModeFilter::Live => &[Mode::Live],
        ModeFilter::Both => &[Mode::Paper, Mode::Live],
    };
    for m in modes {
        let rows = supabase
            .fetch_recent_closed_trades(*m, &args.slug_prefix, args.limit)
            .await?;
        trades.extend(rows);
    }

    if let Some(since) = args.since {
        trades.retain(|t| t.entry_time >= since);
    }
    // Keep only usable rows: must have pnl, contract_size > 0, and non-missing MFE/MAE.
    trades.retain(|t| {
        t.pnl.is_some() && t.contract_size > Decimal::ZERO
    });

    if trades.is_empty() {
        return Err(anyhow!("no closed trades matched the filters"));
    }

    let fee_rate = cfg.trading.paper_fee_rate; // used as 2× per trade
    let per_trade_fees = |contract: Decimal| contract * fee_rate * dec!(2);

    // Baseline: the live actuals.
    let actual_total: Decimal = trades.iter().filter_map(|t| t.pnl).sum();

    // Current config params — the baseline the user is running today.
    let current_sl = cfg.trading.stop_loss_pct;
    let current_activation = cfg.trading.take_profit_activation_pct;
    let current_giveback = cfg.trading.take_profit_giveback_pct;

    let grid_sl: Vec<Decimal> = [
        "0.15", "0.20", "0.25", "0.30", "0.35", "0.40", "0.50",
    ]
    .iter()
    .map(|s| Decimal::from_str(s).unwrap())
    .collect();
    let grid_act: Vec<Decimal> = ["0.05", "0.08", "0.10", "0.15", "0.20", "0.30"]
        .iter()
        .map(|s| Decimal::from_str(s).unwrap())
        .collect();
    let grid_give: Vec<Decimal> = ["0.40", "0.50", "0.60", "0.70", "0.80", "0.90"]
        .iter()
        .map(|s| Decimal::from_str(s).unwrap())
        .collect();

    println!(
        "\nreplay_sltp — {} trades loaded (mode={}, slug={}, limit={}, since={})",
        trades.len(),
        match args.mode {
            ModeFilter::Paper => "paper",
            ModeFilter::Live => "live",
            ModeFilter::Both => "both",
        },
        args.slug_prefix,
        args.limit,
        args.since
            .map(|d| d.to_rfc3339())
            .unwrap_or_else(|| "none".into()),
    );
    println!(
        "fee model: contract_size × PAPER_FEE_RATE({}) × 2 per trade\n",
        fee_rate,
    );
    println!("actual net pnl (sum of Supabase pnl column): {:>12.2}\n", actual_total);

    // Sanity check: replay current params against the same set.
    let cur = simulate(
        &trades,
        current_sl,
        current_activation,
        current_giveback,
        per_trade_fees,
    );
    println!(
        "current params  SL={} act={} give={}    best={:>10.2}  mid={:>10.2}  worst={:>10.2}",
        current_sl, current_activation, current_giveback, cur.best, cur.mid, cur.worst
    );
    println!(
        "                                                       actual={:>10.2}  (sanity: mid should be close)\n",
        actual_total
    );

    // Grid sweep.
    let mut results: Vec<(Decimal, Decimal, Decimal, Sim)> = Vec::new();
    for sl in &grid_sl {
        for act in &grid_act {
            for give in &grid_give {
                let sim = simulate(&trades, *sl, *act, *give, per_trade_fees);
                results.push((*sl, *act, *give, sim));
            }
        }
    }
    // Rank by min(worst, mid) — ordering-robust.
    results.sort_by(|a, b| {
        let a_score = a.3.worst.min(a.3.mid);
        let b_score = b.3.worst.min(b.3.mid);
        b_score.cmp(&a_score)
    });

    println!(
        "top {} by min(worst, mid)  [grid: {}×{}×{} = {} combos]",
        args.top,
        grid_sl.len(),
        grid_act.len(),
        grid_give.len(),
        results.len()
    );
    println!("  rank   SL    act   give    best       mid       worst     hit_sl  hit_tp  unaff");
    for (i, (sl, act, give, sim)) in results.iter().take(args.top).enumerate() {
        println!(
            "  {:>4}  {:<5} {:<5} {:<5}  {:>9.2} {:>9.2} {:>9.2}   {:>4}   {:>4}   {:>4}",
            i + 1,
            sl,
            act,
            give,
            sim.best,
            sim.mid,
            sim.worst,
            sim.hit_sl,
            sim.hit_tp,
            sim.unaffected,
        );
    }

    if let Some(best) = results.first() {
        println!(
            "\nrecommended (robust): SL={} activation={} giveback={}  (best={:.2} mid={:.2} worst={:.2})",
            best.0, best.1, best.2, best.3.best, best.3.mid, best.3.worst,
        );
    }

    println!(
        "\nLIMITATION: trailing-TP exit approximated as MFE * giveback. Without tick-by-tick\n\
         price history (signal_ticks writer not yet wired per CLAUDE.md), we cannot simulate\n\
         the exact mid-trade retracement path. Treat ranked deltas as directional, not absolute.\n"
    );

    Ok(())
}

#[derive(Default, Clone, Copy)]
struct Sim {
    best: Decimal,
    mid: Decimal,
    worst: Decimal,
    hit_sl: usize,
    hit_tp: usize,
    unaffected: usize,
}

/// Replay every trade under one param combo and sum pnl across three ordering
/// assumptions (tp-first = best case, sl-first = worst case, midpoint).
fn simulate(
    trades: &[Trade],
    sl_pct: Decimal,
    activation_pct: Decimal,
    giveback_pct: Decimal,
    fees_fn: impl Fn(Decimal) -> Decimal,
) -> Sim {
    let mut s = Sim::default();
    for t in trades {
        let contract = t.contract_size;
        let mfe = t.max_unrealized_pnl;
        let mae = t.min_unrealized_pnl;
        let actual = t.pnl.unwrap_or(Decimal::ZERO);

        let sl_threshold = -(contract * sl_pct); // negative
        let tp_activation = contract * activation_pct; // positive

        let hit_sl = mae <= sl_threshold;
        let hit_tp = mfe >= tp_activation;

        let tp_pnl = mfe * giveback_pct;
        let sl_pnl = sl_threshold;
        // `actual` is already net of fees (Supabase stores realized pnl).
        // Our simulated SL/TP pnls are gross, so subtract fees only when we
        // replace the actual exit.
        let fees = fees_fn(contract);

        let (best, best_fees) = if hit_tp {
            (tp_pnl, fees)
        } else if hit_sl {
            (sl_pnl, fees)
        } else {
            (actual, Decimal::ZERO)
        };
        let (worst, worst_fees) = if hit_sl {
            (sl_pnl, fees)
        } else if hit_tp {
            (tp_pnl, fees)
        } else {
            (actual, Decimal::ZERO)
        };
        let mid_gross = (best + worst) / dec!(2);
        let mid_fees = (best_fees + worst_fees) / dec!(2);

        s.best += best - best_fees;
        s.mid += mid_gross - mid_fees;
        s.worst += worst - worst_fees;

        if hit_sl {
            s.hit_sl += 1;
        }
        if hit_tp {
            s.hit_tp += 1;
        }
        if !hit_sl && !hit_tp {
            s.unaffected += 1;
        }
    }
    s
}
