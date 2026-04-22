use crate::config::AppConfig;
use crate::data::clob_rest::{ClobRest, PriceSide};
use crate::data::clob_ws::ClobWs;
use crate::engine::entry::{evaluate_entry, EntryDecision};
use crate::engine::exit::{evaluate_exit, ExitDecision, ExitReason};
use crate::engine::sizing::{kelly_size, size_trade};
use crate::engine::state::EngineState;
use crate::error::Result;
use crate::exec::{CloseRequest, Executor, OpenRequest};
use crate::market::scheduler::{MarketMeta, MarketTracker};
use crate::model::{MarketSnapshot, Mode, Trade, TradeStatus};
use crate::store::supabase::SupabaseClient;
use crate::store::tick_recorder::TickRecorder;
use chrono::Utc;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

/// How fresh a WS book snapshot must be to bypass the REST fallback.
const BOOK_FRESHNESS_SEC: i64 = 5;

pub struct EngineHandle {
    pub state: Arc<Mutex<EngineState>>,
    pub executor: Arc<RwLock<Arc<dyn Executor>>>,
    pub tracker: MarketTracker,
    pub clob: Arc<ClobRest>,
    pub clob_ws: Option<ClobWs>,
    pub supabase: Arc<SupabaseClient>,
    pub tick_recorder: Option<Arc<TickRecorder>>,
    pub cfg: Arc<AppConfig>,
}

impl EngineHandle {
    pub async fn current_mode(&self) -> Mode {
        self.executor.read().await.mode()
    }
}

pub async fn run_tick_loop(handle: Arc<EngineHandle>) {
    let active_tick = tokio::time::Duration::from_secs(1);
    let mut was_active = true;

    loop {
        let now = Utc::now();
        let in_hours = crate::time_utils::in_trading_hours(
            now,
            handle.cfg.trading.trading_hours_start_pst,
            handle.cfg.trading.trading_hours_end_pst,
            handle.cfg.trading.allow_weekends,
        );

        // Off-hours: sleep until the next trading window opens. No polling.
        if !in_hours {
            if was_active {
                tracing::info!("tick: off-hours — pausing API requests and WS subscriptions");
                if let Some(ws) = handle.clob_ws.as_ref() {
                    ws.set_subscriptions(vec![]).await;
                }
                was_active = false;
            }
            {
                let mut state = handle.state.lock().await;
                state.last_skip = Some("outside_trading_hours".into());
                state.last_tick = Some(now);
                state.maybe_roll_daily_pnl(now);
                state.unrealized_pnl = None;
            }
            let wake = crate::time_utils::next_trading_open(
                now,
                handle.cfg.trading.trading_hours_start_pst,
                handle.cfg.trading.allow_weekends,
            );
            let sleep_secs = (wake - tokio::time::Instant::now()).as_secs();
            tracing::info!(sleep_secs, "tick: sleeping until next trading window");
            tokio::time::sleep_until(wake).await;
            continue;
        }

        // Transitioning back to active: re-subscribe WS via the market tracker.
        if !was_active {
            tracing::info!("tick: trading hours resumed — reconnecting");
            if let (Some(ws), Some(meta)) =
                (handle.clob_ws.as_ref(), handle.tracker.current().await)
            {
                ws.set_subscriptions(vec![meta.up_token_id, meta.down_token_id])
                    .await;
            }
            was_active = true;
        }

        let started = tokio::time::Instant::now();
        if let Err(e) = run_one(&handle).await {
            tracing::warn!(err = %e, "tick error");
        }
        let elapsed = started.elapsed();
        if elapsed < active_tick {
            tokio::time::sleep(active_tick - elapsed).await;
        }
    }
}

pub async fn run_one(h: &EngineHandle) -> Result<()> {
    let now = Utc::now();
    let market = match h.tracker.current().await {
        Some(m) => m,
        None => {
            tracing::trace!("tick: no market yet");
            return Ok(());
        }
    };

    let snapshot = build_snapshot(&h.clob, h.clob_ws.as_ref(), &market).await?;

    // Update MFE/MAE on an open position regardless of exit decision, and roll the
    // daily PnL counter if we've crossed a PST day boundary since the last tick.
    // Only trust the snapshot's mark when it still references the position's own
    // market — once rollover has happened the snapshot is the successor market and
    // its ~0.50 opening bid must not be treated as our position's value.
    {
        let mut state = h.state.lock().await;
        state.last_tick = Some(now);
        state.last_snapshot = Some(snapshot.clone());
        state.maybe_roll_daily_pnl(now);
        if let Some(pos) = state.position.as_mut() {
            if snapshot.market_slug == pos.market_slug {
                let mark = snapshot
                    .bid_for(pos.side)
                    .unwrap_or(snapshot.price_for(pos.side));
                pos.update_mfe_mae(mark);
                state.unrealized_pnl = Some(pos.unrealized_pnl(mark));
                state.last_position_mark = Some(mark);
            }
            // Snapshot has rolled past us — keep the last valid unrealized/mark
            // so the UI shows the final pre-rollover value until the exit fires.
        } else {
            state.unrealized_pnl = None;
        }
    }

    // Capture a signal_ticks row only while a position is open. The primary
    // consumer is the SL/TP replay simulator, which needs tick-by-tick mark
    // paths between entry and exit to reconstruct trailing-TP exits — flat-
    // state ticks aren't used by any downstream analysis today.
    //
    // Top-level columns match the legacy `signal_ticks` schema (shared with
    // the Node dashboard writer). Rich detail — ask/bid per side, open-
    // position state — lives in the `meta` JSONB column added by the
    // 2026-04-22 migration `add_meta_jsonb_to_signal_ticks`.
    let position_snapshot = {
        let state = h.state.lock().await;
        state.position.as_ref().map(|pos| {
            (
                pos.side,
                json!({
                    "side": pos.side,
                    "entryPrice": pos.entry_price,
                    "shares": pos.shares,
                    "contractSize": pos.contract_size,
                    "mark": state.last_position_mark,
                    "unrealizedPnl": state.unrealized_pnl,
                    "maxUnrealizedPnl": pos.max_unrealized_pnl,
                    "minUnrealizedPnl": pos.min_unrealized_pnl,
                    "marketSlug": pos.market_slug,
                }),
            )
        })
    };
    if let (Some(recorder), Some((rec_side, position_meta))) =
        (h.tick_recorder.as_ref(), position_snapshot)
    {
        let mode = h.current_mode().await;
        let spread_up = match (snapshot.up_ask, snapshot.up_bid) {
            (Some(a), Some(b)) => Some(a - b),
            _ => None,
        };
        let spread_down = match (snapshot.down_ask, snapshot.down_bid) {
            (Some(a), Some(b)) => Some(a - b),
            _ => None,
        };
        let meta = json!({
            "mode": mode.as_str(),
            "upTokenId": snapshot.up_token_id,
            "downTokenId": snapshot.down_token_id,
            "endDate": snapshot.end_date,
            "upAsk": snapshot.up_ask,
            "upBid": snapshot.up_bid,
            "downAsk": snapshot.down_ask,
            "downBid": snapshot.down_bid,
            "position": position_meta,
        });
        recorder.record(json!({
            "timeframe": "5m",
            "market_slug": snapshot.market_slug,
            "time_left_min": snapshot.time_left_minutes(now),
            "poly_up": snapshot.up_price,
            "poly_down": snapshot.down_price,
            "spread_up": spread_up,
            "spread_down": spread_down,
            "rec_side": rec_side,
            "rec_phase": "holding",
            "meta": meta,
        }));
    }

    // If we hold a position, check exits first.
    let held = {
        let state = h.state.lock().await;
        state.position.clone()
    };

    if let Some(position) = held {
        let state_snapshot = h.state.lock().await.clone();
        let decision = evaluate_exit(
            &state_snapshot,
            &position,
            &snapshot,
            &h.cfg.trading,
            now,
        );
        drop(state_snapshot);

        if let ExitDecision::Exit(reason) = decision {
            let executor = h.executor.read().await.clone();

            // For rollover, mark is the last bid we saw while the snapshot still
            // matched our market (captured in state.last_position_mark). For every
            // other exit reason, the snapshot still references our market so its
            // current bid is correct.
            let mark = if matches!(reason, ExitReason::MarketRolled) {
                let cached = h.state.lock().await.last_position_mark;
                cached.unwrap_or(position.entry_price)
            } else {
                snapshot
                    .bid_for(position.side)
                    .unwrap_or(snapshot.price_for(position.side))
            };

            // MarketRolled means the old market has settled — you can't sell on a
            // closed market. Instead, redeem the tokens at their settlement payout:
            // $1/share if our side won (mark > 0.50), $0 if it lost.
            let (trade, log_action) = if matches!(reason, ExitReason::MarketRolled) {
                let won = mark > dec!(0.50);
                let settlement_price = if won { dec!(1) } else { dec!(0) };
                let pnl = (settlement_price - position.entry_price) * position.shares;
                let fees = dec!(0); // no trading fees on redemption

                if won {
                    match executor
                        .redeem_winnings(&position.token_id, position.shares)
                        .await
                    {
                        Ok(credited) => tracing::info!(
                            credited = %credited,
                            "auto-claim: redeemed winning tokens"
                        ),
                        Err(e) => tracing::warn!(err = %e, "auto-claim: redeem failed"),
                    }
                }

                let trade = build_settled_trade(
                    &position,
                    settlement_price,
                    pnl,
                    fees,
                    if won { "market_rolled_won" } else { "market_rolled_lost" },
                    now,
                );
                (trade, "position settled (auto-claim)")
            } else {
                // Normal exit (StopLoss, SettlementImminent, KillSwitch) — sell at mark.
                let res = executor
                    .close_position(CloseRequest {
                        position: position.clone(),
                        exit_reason: reason.as_str().into(),
                        mark_price: mark,
                    })
                    .await?;
                let trade =
                    finalize_trade(&position, &snapshot, mark, reason.as_str(), &res, now);
                (trade, "position closed")
            };

            if let Err(e) = h.supabase.upsert_trade(&trade).await {
                tracing::warn!(err = %e, "supabase upsert (close) failed");
            }
            let pnl_display = trade.pnl.unwrap_or(dec!(0));
            {
                let mut state = h.state.lock().await;
                state.record_trade_closed(trade, now);
            }
            tracing::info!(
                slug = %snapshot.market_slug,
                reason = %reason.as_str(),
                pnl = %pnl_display,
                log_action,
            );
            return Ok(());
        }
    } else {
        // No position: consider entering.
        let state_snapshot = h.state.lock().await.clone();
        let decision = evaluate_entry(&state_snapshot, &snapshot, &h.cfg.trading, now);
        drop(state_snapshot);

        match decision {
            EntryDecision::Skip(reason) => {
                h.state.lock().await.last_skip = Some(reason.as_str().into());
            }
            EntryDecision::Enter(mut order) => {
                let balance = {
                    let executor = h.executor.read().await.clone();
                    executor.balance().await?
                };

                // Size the trade: Kelly (with limit price) or flat percentage.
                let (shares, limit_price) = if h.cfg.trading.kelly.enabled {
                    match kelly_size(
                        balance.available_usd,
                        order.price,
                        h.cfg.trading.paper_fee_rate,
                        &h.cfg.trading.kelly,
                    ) {
                        Some(kr) => {
                            order.limit_price = Some(kr.limit_price);
                            tracing::debug!(
                                edge = %kr.edge,
                                raw_kelly = %kr.raw_kelly,
                                stake = %kr.stake,
                                limit = %kr.limit_price,
                                "kelly sizing"
                            );
                            (kr.shares, Some(kr.limit_price))
                        }
                        None => {
                            h.state.lock().await.last_skip =
                                Some("negative_expected_value".into());
                            return Ok(());
                        }
                    }
                } else {
                    let s = size_trade(balance.available_usd, order.price, &h.cfg.trading);
                    (s, None)
                };

                if shares <= dec!(0) {
                    h.state.lock().await.last_skip =
                        Some("sizing_returned_zero".into());
                    return Ok(());
                }

                let req = OpenRequest {
                    side: order.side,
                    market_slug: snapshot.market_slug.clone(),
                    market_end_date: snapshot.end_date,
                    token_id: snapshot.token_id_for(order.side).to_string(),
                    quoted_price: order.price,
                    limit_price,
                    shares,
                };
                let executor = h.executor.read().await.clone();
                let open = executor.open_position(req).await?;
                let trade = new_open_trade(&open.position, &snapshot, now);
                if let Err(e) = h.supabase.upsert_trade(&trade).await {
                    tracing::warn!(err = %e, "supabase upsert (open) failed");
                }
                {
                    let mut state = h.state.lock().await;
                    state.position = Some(open.position.clone());
                    state.last_skip = None;
                }
                tracing::info!(
                    slug = %snapshot.market_slug,
                    side = %order.side.as_str(),
                    phase = %order.phase.as_str(),
                    shares = %open.position.shares,
                    fill = %open.fill_price,
                    limit = ?limit_price,
                    "position opened"
                );
            }
        }
    }

    Ok(())
}

async fn build_snapshot(
    clob: &ClobRest,
    ws: Option<&ClobWs>,
    market: &MarketMeta,
) -> Result<MarketSnapshot> {
    // Prefer the WS-maintained book state when fresh. Only fall back to REST if
    // either side is missing or stale (WS disconnected for a few seconds).
    let now = Utc::now();
    let (up_ws, dn_ws) = match ws {
        Some(w) => (
            w.peek(&market.up_token_id).await,
            w.peek(&market.down_token_id).await,
        ),
        None => (None, None),
    };

    let fresh = |snap: &Option<_>| {
        snap.as_ref()
            .map(|s: &crate::data::clob_ws::BookSnapshot| {
                (now - s.updated_at).num_seconds() <= BOOK_FRESHNESS_SEC
            })
            .unwrap_or(false)
    };

    let (mut up_ask, mut up_bid) = if fresh(&up_ws) {
        let s = up_ws.as_ref().unwrap();
        (s.best_ask, s.best_bid)
    } else {
        (None, None)
    };
    let (mut dn_ask, mut dn_bid) = if fresh(&dn_ws) {
        let s = dn_ws.as_ref().unwrap();
        (s.best_ask, s.best_bid)
    } else {
        (None, None)
    };

    // REST fallback — only make the requests we actually need. Errors are
    // logged at debug so we can tell "REST failed" from "REST returned None"
    // when diagnosing a "prices unavailable" stretch.
    async fn rest_price(
        clob: &ClobRest,
        token: &str,
        side: PriceSide,
    ) -> Option<Decimal> {
        match clob.price(token, side).await {
            Ok(p) => Some(p),
            Err(e) => {
                tracing::debug!(token, ?side, err = %e, "clob /price fallback failed");
                None
            }
        }
    }

    if up_ask.is_none() {
        up_ask = rest_price(clob, &market.up_token_id, PriceSide::Buy).await;
    }
    if up_bid.is_none() {
        up_bid = rest_price(clob, &market.up_token_id, PriceSide::Sell).await;
    }
    if dn_ask.is_none() {
        dn_ask = rest_price(clob, &market.down_token_id, PriceSide::Buy).await;
    }
    if dn_bid.is_none() {
        dn_bid = rest_price(clob, &market.down_token_id, PriceSide::Sell).await;
    }

    let up_mid = midpoint(up_ask, up_bid).unwrap_or(dec!(0.5));
    let dn_mid = midpoint(dn_ask, dn_bid).unwrap_or(dec!(0.5));

    Ok(MarketSnapshot {
        market_slug: market.slug.clone(),
        up_token_id: market.up_token_id.clone(),
        down_token_id: market.down_token_id.clone(),
        end_date: market.end_date,
        up_price: up_mid,
        down_price: dn_mid,
        up_ask,
        down_ask: dn_ask,
        up_bid,
        down_bid: dn_bid,
        fetched_at: now,
    })
}

fn midpoint(ask: Option<Decimal>, bid: Option<Decimal>) -> Option<Decimal> {
    match (ask, bid) {
        (Some(a), Some(b)) => Some((a + b) / dec!(2)),
        (Some(v), None) | (None, Some(v)) => Some(v),
        _ => None,
    }
}

fn new_open_trade(
    pos: &crate::model::OpenPosition,
    snapshot: &MarketSnapshot,
    now: chrono::DateTime<chrono::Utc>,
) -> Trade {
    Trade {
        id: pos.id.clone(),
        timestamp: now,
        status: TradeStatus::Open,
        side: pos.side,
        mode: pos.mode,
        entry_price: pos.entry_price,
        shares: pos.shares,
        contract_size: pos.contract_size,
        entry_time: pos.entry_time,
        market_slug: snapshot.market_slug.clone(),
        entry_phase: None,
        exit_price: None,
        exit_time: None,
        exit_reason: None,
        pnl: None,
        max_unrealized_pnl: pos.max_unrealized_pnl,
        min_unrealized_pnl: pos.min_unrealized_pnl,
        entry_gate_snapshot: Some(
            json!({
                "up_ask": snapshot.up_ask.map(|d| d.to_string()),
                "down_ask": snapshot.down_ask.map(|d| d.to_string()),
                "time_left_sec": snapshot.time_left_sec(now),
            })
            .to_string(),
        ),
        extra_json: None,
        created_at: now,
        updated_at: now,
    }
}

fn finalize_trade(
    pos: &crate::model::OpenPosition,
    snapshot: &MarketSnapshot,
    _mark_price: Decimal,
    reason: &str,
    close: &crate::exec::CloseResult,
    now: chrono::DateTime<chrono::Utc>,
) -> Trade {
    Trade {
        id: pos.id.clone(),
        timestamp: pos.entry_time,
        status: TradeStatus::Closed,
        side: pos.side,
        mode: pos.mode,
        entry_price: pos.entry_price,
        shares: pos.shares,
        contract_size: pos.contract_size,
        entry_time: pos.entry_time,
        market_slug: pos.market_slug.clone(),
        entry_phase: None,
        exit_price: Some(close.exit_price),
        exit_time: Some(close.exit_time),
        exit_reason: Some(reason.to_string()),
        pnl: Some(close.pnl),
        max_unrealized_pnl: pos.max_unrealized_pnl,
        min_unrealized_pnl: pos.min_unrealized_pnl,
        entry_gate_snapshot: None,
        extra_json: Some(
            json!({
                "exit_slug": snapshot.market_slug,
                "exit_fees": close.fees_paid.to_string(),
            })
            .to_string(),
        ),
        created_at: pos.entry_time,
        updated_at: now,
    }
}

/// Build a trade record for a position that settled at the market's resolution price
/// (auto-claim path). Used when MarketRolled fires and we can't sell — we redeem instead.
fn build_settled_trade(
    pos: &crate::model::OpenPosition,
    settlement_price: Decimal,
    pnl: Decimal,
    fees: Decimal,
    reason: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Trade {
    Trade {
        id: pos.id.clone(),
        timestamp: pos.entry_time,
        status: TradeStatus::Closed,
        side: pos.side,
        mode: pos.mode,
        entry_price: pos.entry_price,
        shares: pos.shares,
        contract_size: pos.contract_size,
        entry_time: pos.entry_time,
        market_slug: pos.market_slug.clone(),
        entry_phase: None,
        exit_price: Some(settlement_price),
        exit_time: Some(now),
        exit_reason: Some(reason.to_string()),
        pnl: Some(pnl),
        max_unrealized_pnl: pos.max_unrealized_pnl,
        min_unrealized_pnl: pos.min_unrealized_pnl,
        entry_gate_snapshot: None,
        extra_json: Some(
            json!({
                "settlement": true,
                "won": settlement_price > dec!(0.50),
                "fees": fees.to_string(),
            })
            .to_string(),
        ),
        created_at: pos.entry_time,
        updated_at: now,
    }
}

