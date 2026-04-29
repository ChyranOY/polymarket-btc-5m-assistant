//! Tick-level SL/TP replay.
//!
//! Unlike `replay_sltp` (which only has MFE/MAE snapshots per trade and so
//! approximates a trailing-TP exit as `MFE × giveback`), this binary walks
//! the actual tick-by-tick mark path that the `signal_ticks` table captures
//! between each trade's entryTime and exitTime. At each candidate
//! (SL, activation, giveback) combo it simulates the exact tick at which a
//! real SL / trailing-TP would have fired, using the *real* mark at that
//! tick — catching the execution slip the MFE-based replay misses.
//!
//! Requires the signal_ticks meta JSONB column populated by the Rust
//! tick_recorder. Trades entered before the recorder was wired up are
//! skipped automatically (no ticks in the window).
//!
//! Usage:
//!   cargo run --release --bin replay_sltp_ticks
//!   cargo run --release --bin replay_sltp_ticks -- --mode both --top 30

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use polymarket_btc_5m::config::AppConfig;
use polymarket_btc_5m::model::{Mode, Trade};
use polymarket_btc_5m::store::supabase::SupabaseClient;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde_json::Value;
use std::collections::HashMap;
use std::env;
use std::str::FromStr;

const DEFAULT_SLUG_PREFIX: &str = "btc-updown-15m";
const DEFAULT_LIMIT: usize = 2000;
const DEFAULT_TICK_LIMIT: usize = 500_000;
const DEFAULT_TOP: usize = 20;

struct Args {
    mode: ModeFilter,
    slug_prefix: String,
    limit: usize,
    tick_limit: usize,
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
    let mut limit = DEFAULT_LIMIT;
    let mut tick_limit = DEFAULT_TICK_LIMIT;
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
            "--limit" => limit = val("--limit")?.parse()?,
            "--tick-limit" => tick_limit = val("--tick-limit")?.parse()?,
            "--top" => top = val("--top")?.parse()?,
            "-h" | "--help" => {
                eprintln!(
                    "replay_sltp_ticks — walk signal_ticks to compute exact SL/TP exits\n\
                     \n\
                     --mode paper|live|both     (default: both)\n\
                     --slug-prefix <str>        (default: {DEFAULT_SLUG_PREFIX})\n\
                     --limit <n>                max trades to load (default: {DEFAULT_LIMIT})\n\
                     --tick-limit <n>           max tick rows to fetch (default: {DEFAULT_TICK_LIMIT})\n\
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
        limit,
        tick_limit,
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

    // Load trades.
    let modes: &[Mode] = match args.mode {
        ModeFilter::Paper => &[Mode::Paper],
        ModeFilter::Live => &[Mode::Live],
        ModeFilter::Both => &[Mode::Paper, Mode::Live],
    };
    let mut trades: Vec<Trade> = Vec::new();
    for m in modes {
        let rows = supabase
            .fetch_recent_closed_trades(*m, &args.slug_prefix, args.limit)
            .await?;
        trades.extend(rows);
    }
    trades.retain(|t| t.pnl.is_some() && t.contract_size > Decimal::ZERO);
    if trades.is_empty() {
        return Err(anyhow!("no closed trades loaded"));
    }

    // Fetch ticks spanning the trade window.
    let earliest_entry = trades
        .iter()
        .map(|t| t.entry_time)
        .min()
        .expect("trades non-empty");
    let latest_exit = trades
        .iter()
        .filter_map(|t| t.exit_time)
        .max()
        .unwrap_or_else(Utc::now);
    // Ticks are batch-flushed to Supabase up to ~10s after the tick fires,
    // and `created_at` is set to flush time. Pad the fetch window so ticks
    // that occurred just before `latest_exit` but were flushed later still
    // land in the result. The replay filters per-trade on recordedAt, so
    // extra tail rows are harmless.
    let fetch_end = latest_exit + chrono::Duration::minutes(5);
    let fetch_start = earliest_entry - chrono::Duration::minutes(1);

    let raw_ticks = supabase
        .fetch_signal_ticks_window(fetch_start, fetch_end, args.tick_limit)
        .await?;

    // Index ticks by market_slug.
    let mut by_slug: HashMap<String, Vec<TickPoint>> = HashMap::new();
    for row in raw_ticks {
        if let Some(tp) = parse_tick(&row) {
            by_slug.entry(tp.market_slug.clone()).or_default().push(tp);
        }
    }
    for v in by_slug.values_mut() {
        v.sort_by_key(|t| t.created_at);
    }

    // Only keep trades that have at least one matching tick between entry and exit.
    let original_trade_count = trades.len();
    trades.retain(|t| {
        let Some(ticks) = by_slug.get(&t.market_slug) else { return false };
        let exit = t.exit_time.unwrap_or(latest_exit);
        ticks.iter().any(|tp| tp.created_at >= t.entry_time && tp.created_at <= exit)
    });

    println!(
        "\nreplay_sltp_ticks — {} trades ({} skipped: no ticks in window), {} tick rows, {} distinct markets",
        trades.len(),
        original_trade_count - trades.len(),
        by_slug.values().map(|v| v.len()).sum::<usize>(),
        by_slug.len(),
    );
    if trades.is_empty() {
        return Err(anyhow!(
            "no trades have tick coverage — run the bot longer with the tick_recorder on"
        ));
    }

    let fee_rate = cfg.trading.paper_fee_rate;
    let actual_total: Decimal = trades.iter().filter_map(|t| t.pnl).sum();

    // Current params baseline.
    let cur = simulate_all(
        &trades,
        &by_slug,
        cfg.trading.stop_loss_pct,
        cfg.trading.take_profit_activation_pct,
        cfg.trading.take_profit_giveback_pct,
        fee_rate,
    );
    println!(
        "\nactual net pnl (Supabase):           {:>10.2}",
        actual_total
    );
    println!(
        "current params {:.2}/{:.2}/{:.2} replay: {:>10.2}  (SL={} TP={} settle={} other={})",
        cfg.trading.stop_loss_pct,
        cfg.trading.take_profit_activation_pct,
        cfg.trading.take_profit_giveback_pct,
        cur.total,
        cur.n_sl,
        cur.n_tp,
        cur.n_settle,
        cur.n_other,
    );
    let divergence = (cur.total - actual_total).abs();
    println!(
        "divergence from actual:              {:>10.2}   (sanity: should be small)\n",
        divergence
    );

    // Grid sweep.
    let grid_sl: Vec<Decimal> = ["0.15", "0.20", "0.25", "0.30", "0.35", "0.40", "0.50"]
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

    let mut results: Vec<(Decimal, Decimal, Decimal, Sim)> = Vec::new();
    for sl in &grid_sl {
        for act in &grid_act {
            for give in &grid_give {
                let sim = simulate_all(&trades, &by_slug, *sl, *act, *give, fee_rate);
                results.push((*sl, *act, *give, sim));
            }
        }
    }
    results.sort_by(|a, b| b.3.total.cmp(&a.3.total));

    println!(
        "top {} by total simulated pnl  [grid: {}×{}×{} = {} combos]",
        args.top,
        grid_sl.len(),
        grid_act.len(),
        grid_give.len(),
        results.len()
    );
    println!("  rank   SL    act   give    total      SL    TP    settle  other");
    for (i, (sl, act, give, sim)) in results.iter().take(args.top).enumerate() {
        println!(
            "  {:>4}  {:<5} {:<5} {:<5}  {:>9.2}   {:>4}  {:>4}  {:>5}   {:>4}",
            i + 1,
            sl,
            act,
            give,
            sim.total,
            sim.n_sl,
            sim.n_tp,
            sim.n_settle,
            sim.n_other,
        );
    }

    if let Some(best) = results.first() {
        println!(
            "\nrecommended: SL={} activation={} giveback={}  total={:.2}",
            best.0, best.1, best.2, best.3.total,
        );
    }

    println!(
        "\nNOTES: \n\
         - Simulated SL exit uses the mark at the tick that first breaches -(contract × SL).\n\
         - Simulated TP exit uses the mark at the first tick where running_max ≥ contract × activation\n\
           AND current pnl ≤ running_max × giveback. Real slip (vs MFE × giveback) is captured.\n\
         - When neither fires, the trade's actual pnl is used (settlement / rollover / manual).\n\
         - Fees subtracted only when the exit differs from reality.\n"
    );

    Ok(())
}

#[derive(Clone)]
struct TickPoint {
    market_slug: String,
    created_at: DateTime<Utc>,
    /// The bid the engine used as its running mark for the held side.
    /// Sourced from meta.position.mark (Decimal-as-string or null).
    mark: Option<Decimal>,
}

fn parse_tick(row: &Value) -> Option<TickPoint> {
    let market_slug = row.get("market_slug")?.as_str()?.to_string();
    // Prefer the per-tick timestamp carried in meta.recordedAt. `created_at`
    // at the table level is the flush time (all rows in a batch share it),
    // which collapses a held-for-N-seconds path to one point.
    let ts_str = row
        .get("meta")
        .and_then(|m| m.get("recordedAt"))
        .and_then(|v| v.as_str())
        .or_else(|| row.get("created_at").and_then(|v| v.as_str()))?;
    let created_at = DateTime::parse_from_rfc3339(ts_str)
        .ok()
        .map(|d| d.with_timezone(&Utc))?;
    let mark = row
        .get("meta")
        .and_then(|m| m.get("position"))
        .and_then(|p| p.get("mark"))
        .and_then(parse_decimal);
    Some(TickPoint {
        market_slug,
        created_at,
        mark,
    })
}

fn parse_decimal(v: &Value) -> Option<Decimal> {
    match v {
        Value::String(s) => Decimal::from_str(s).ok(),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(Decimal::from(i))
            } else {
                n.as_f64().and_then(|f| Decimal::from_str(&f.to_string()).ok())
            }
        }
        _ => None,
    }
}

#[derive(Default, Clone, Copy)]
struct Sim {
    total: Decimal,
    n_sl: usize,
    n_tp: usize,
    n_settle: usize,
    n_other: usize,
}

enum ReplayOutcome {
    Sl(Decimal),
    Tp(Decimal),
}

fn simulate_all(
    trades: &[Trade],
    by_slug: &HashMap<String, Vec<TickPoint>>,
    sl_pct: Decimal,
    activation_pct: Decimal,
    giveback_pct: Decimal,
    fee_rate: Decimal,
) -> Sim {
    let mut s = Sim::default();
    for t in trades {
        let Some(ticks) = by_slug.get(&t.market_slug) else {
            s.n_other += 1;
            s.total += t.pnl.unwrap_or(Decimal::ZERO);
            continue;
        };
        let exit = t.exit_time.unwrap_or_else(Utc::now);
        let contract = t.contract_size;
        let entry_price = t.entry_price;
        let shares = t.shares;
        let fees = contract * fee_rate * dec!(2);

        let sl_threshold = -(contract * sl_pct);
        let tp_activation = contract * activation_pct;

        let mut running_max = dec!(0);
        let mut outcome: Option<ReplayOutcome> = None;

        for tp in ticks.iter().filter(|tp| {
            tp.created_at >= t.entry_time && tp.created_at <= exit
        }) {
            let Some(mark) = tp.mark else { continue };
            let pnl = (mark - entry_price) * shares;
            if pnl > running_max {
                running_max = pnl;
            }
            if pnl <= sl_threshold {
                outcome = Some(ReplayOutcome::Sl(pnl));
                break;
            }
            if running_max >= tp_activation && pnl <= running_max * giveback_pct {
                outcome = Some(ReplayOutcome::Tp(pnl));
                break;
            }
        }

        match outcome {
            Some(ReplayOutcome::Sl(pnl)) => {
                s.total += pnl - fees;
                s.n_sl += 1;
            }
            Some(ReplayOutcome::Tp(pnl)) => {
                s.total += pnl - fees;
                s.n_tp += 1;
            }
            None => {
                // Neither SL nor TP triggered in replay → fall through to actual
                // exit (settlement / rollover). Actual pnl already has fees netted.
                s.total += t.pnl.unwrap_or(Decimal::ZERO);
                s.n_settle += 1;
            }
        }
    }
    s
}
