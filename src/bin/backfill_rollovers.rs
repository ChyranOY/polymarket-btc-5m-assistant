//! One-shot backfill: fix the PnL / exitReason / exitPrice on every
//! market_rolled_{won,lost} trade that was decided using the buggy successor-
//! market bid. Looks up the true settlement from Gamma's `outcomePrices` on
//! the (now-settled) market and PATCHes any row whose current answer disagrees.
//!
//! Usage:
//!   cargo run --bin backfill_rollovers -- --dry-run
//!   cargo run --bin backfill_rollovers
//!
//! Safe to rerun: a row already matching the expected settlement is skipped.

use anyhow::{anyhow, Context, Result};
use polymarket_btc_5m::config::AppConfig;
use polymarket_btc_5m::data::gamma::GammaClient;
use polymarket_btc_5m::model::Mode;
use polymarket_btc_5m::store::supabase::SupabaseClient;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde_json::{json, Value};
use std::str::FromStr;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let dry_run = std::env::args().any(|a| a == "--dry-run");
    let cfg = AppConfig::from_env()?;
    let gamma = GammaClient::new(&cfg.polymarket)?;
    let supabase = SupabaseClient::new(&cfg.supabase)?;
    if !supabase.enabled() {
        return Err(anyhow!(
            "Supabase not configured (SUPABASE_URL / SUPABASE_SERVICE_ROLE_KEY)"
        ));
    }

    // Backfill both modes — the bug affects every rollover regardless of paper/live.
    for mode in [Mode::Paper, Mode::Live] {
        let rows = supabase
            .fetch_rollover_trades(mode, "btc-updown-5m-")
            .await
            .with_context(|| format!("fetch rollover trades for {}", mode.as_str()))?;
        tracing::info!(mode = mode.as_str(), count = rows.len(), "scanning");
        let mut flipped = 0usize;
        let mut matched = 0usize;
        let mut skipped = 0usize;

        for row in rows {
            match process_row(&gamma, &supabase, &row, dry_run).await {
                Ok(ProcessOutcome::Fixed) => flipped += 1,
                Ok(ProcessOutcome::AlreadyCorrect) => matched += 1,
                Ok(ProcessOutcome::Skipped(reason)) => {
                    skipped += 1;
                    tracing::debug!(reason = %reason, "skipped");
                }
                Err(e) => {
                    skipped += 1;
                    tracing::warn!(err = %e, "row error");
                }
            }
        }

        tracing::info!(
            mode = mode.as_str(),
            flipped,
            matched,
            skipped,
            "{}",
            if dry_run { "dry-run summary" } else { "patched" }
        );
    }
    Ok(())
}

enum ProcessOutcome {
    Fixed,
    AlreadyCorrect,
    Skipped(String),
}

async fn process_row(
    gamma: &GammaClient,
    supabase: &SupabaseClient,
    row: &Value,
    dry_run: bool,
) -> Result<ProcessOutcome> {
    let id = row.get("id").and_then(Value::as_str).ok_or_else(|| anyhow!("row missing id"))?;
    let slug = row
        .get("marketSlug")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("row {id}: missing marketSlug"))?;
    let side = row
        .get("side")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("row {id}: missing side"))?
        .to_ascii_uppercase();
    let entry_price = decimal_field(row, "entryPrice")
        .ok_or_else(|| anyhow!("row {id}: missing entryPrice"))?;
    let shares = decimal_field(row, "shares")
        .ok_or_else(|| anyhow!("row {id}: missing shares"))?;
    let current_exit_price = decimal_field(row, "exitPrice");
    let current_pnl = decimal_field(row, "pnl");
    let current_reason = row.get("exitReason").and_then(Value::as_str).unwrap_or("");

    let market = match gamma.fetch_market_by_slug(slug).await? {
        Some(m) => m,
        None => return Ok(ProcessOutcome::Skipped(format!("gamma no market {slug}"))),
    };
    let up_price = market
        .up_price
        .ok_or_else(|| anyhow!("row {id}: gamma {slug} missing up outcomePrice"))?;
    let down_price = market
        .down_price
        .ok_or_else(|| anyhow!("row {id}: gamma {slug} missing down outcomePrice"))?;

    // Settled markets have outcomePrices ∈ {0, 1}. If it's not {0,1}/{1,0}, market
    // hasn't resolved yet — skip rather than trust a live mid-price.
    if !is_binary_settled(up_price, down_price) {
        return Ok(ProcessOutcome::Skipped(format!(
            "{slug} not yet settled ({up_price}/{down_price})"
        )));
    }

    let our_side_won = match side.as_str() {
        "UP" => up_price == dec!(1),
        "DOWN" => down_price == dec!(1),
        other => return Err(anyhow!("row {id}: unknown side {other}")),
    };
    let expected_settlement = if our_side_won { dec!(1) } else { dec!(0) };
    let expected_pnl = (expected_settlement - entry_price) * shares;
    let expected_reason = if our_side_won {
        "market_rolled_won"
    } else {
        "market_rolled_lost"
    };

    // 1-cent tolerance on pnl — stored values are rounded to 3–4 decimals so the
    // derived (1-entry)*shares can differ by < $0.01 even when the settlement
    // decision is already correct. Not worth a PATCH.
    let price_matches = current_exit_price.map(|p| p == expected_settlement).unwrap_or(false);
    let pnl_close_enough = current_pnl
        .map(|p| (p - expected_pnl).abs() <= dec!(0.01))
        .unwrap_or(false);
    let reason_matches = current_reason == expected_reason;
    if price_matches && pnl_close_enough && reason_matches {
        return Ok(ProcessOutcome::AlreadyCorrect);
    }

    let delta = current_pnl.map(|p| expected_pnl - p).unwrap_or(expected_pnl);
    tracing::info!(
        id = id,
        slug = slug,
        side = side,
        entry = %entry_price,
        shares = %shares,
        from_reason = current_reason,
        to_reason = expected_reason,
        from_exit = ?current_exit_price,
        to_exit = %expected_settlement,
        from_pnl = ?current_pnl,
        to_pnl = %expected_pnl,
        delta = %delta,
        "{}",
        if dry_run { "WOULD FIX" } else { "FIXING" },
    );

    if !dry_run {
        let patch = json!({
            "exitPrice": expected_settlement,
            "pnl": expected_pnl,
            "exitReason": expected_reason,
            "extraJson": json!({
                "backfill": "rollover_pnl_fix",
                "previous_exit_price": current_exit_price.map(|d| d.to_string()),
                "previous_pnl": current_pnl.map(|d| d.to_string()),
                "previous_exit_reason": current_reason,
                "gamma_up_price": up_price.to_string(),
                "gamma_down_price": down_price.to_string(),
            }).to_string(),
            "updatedAt": chrono::Utc::now(),
        });
        supabase.patch_trade(id, &patch).await?;
    }
    Ok(ProcessOutcome::Fixed)
}

fn decimal_field(row: &Value, key: &str) -> Option<Decimal> {
    match row.get(key)? {
        Value::String(s) => Decimal::from_str(s).ok(),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(Decimal::from(i))
            } else {
                n.as_f64().and_then(|f| Decimal::from_str(&format!("{f}")).ok())
            }
        }
        _ => None,
    }
}

/// Only treat as settled if both outcome prices are integral 0 or 1 and they
/// sum to 1. Mid-resolution or live markets have fractional prices.
fn is_binary_settled(up: Decimal, down: Decimal) -> bool {
    let allowed = |d: Decimal| d == dec!(0) || d == dec!(1);
    allowed(up) && allowed(down) && (up + down == dec!(1))
}
