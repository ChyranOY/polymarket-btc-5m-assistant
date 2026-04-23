use anyhow::Result;
use chrono::{TimeZone, Utc};
use chrono_tz::America::Los_Angeles;
use polymarket_btc_5m::model::Mode;
use polymarket_btc_5m::{api, config, data, engine, exec, market, store};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::{Mutex, RwLock};

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cfg = Arc::new(config::AppConfig::from_env()?);
    tracing::info!(
        mode = cfg.trading.mode.as_str(),
        port = cfg.http.port,
        slug = %cfg.polymarket.series_slug,
        live_ready = cfg.live_creds.is_some(),
        supabase_ready = cfg.supabase.is_configured(),
        "polymarket-btc-5m boot",
    );

    let gamma = data::gamma::GammaClient::new(&cfg.polymarket)?;
    let clob = Arc::new(data::clob_rest::ClobRest::new(&cfg.polymarket)?);
    let clob_ws = data::clob_ws::ClobWs::start(cfg.polymarket.ws_market_url.clone());
    let coinbase_ws = data::coinbase_ws::CoinbaseWs::start();
    let supabase = Arc::new(store::supabase::SupabaseClient::new(&cfg.supabase)?);
    let tick_recorder = store::tick_recorder::TickRecorder::start(supabase.clone());

    let paper = exec::paper::PaperExecutor::new(
        cfg.trading.starting_balance,
        cfg.trading.paper_fee_rate,
        cfg.paper_ledger_path.clone(),
    )?;
    let executor: Arc<RwLock<Arc<dyn exec::Executor>>> =
        Arc::new(RwLock::new(Arc::new(paper) as Arc<dyn exec::Executor>));

    let tracker = market::scheduler::MarketTracker::new();
    let sched_tracker = tracker.clone();
    let sched_gamma = gamma.clone();
    tokio::spawn(async move {
        market::scheduler::run_market_scheduler(
            sched_gamma,
            sched_tracker,
            market::scheduler::SchedulerConfig::default(),
        )
        .await;
    });

    // Bridge market rollovers into WS subscription updates so the book feed always
    // tracks the active market's two outcome tokens.
    let ws_tracker = tracker.clone();
    let ws_bridge = clob_ws.clone();
    tokio::spawn(async move {
        let mut rx = ws_tracker.subscribe();
        if let Some(meta) = ws_tracker.current().await {
            ws_bridge
                .set_subscriptions(vec![meta.up_token_id, meta.down_token_id])
                .await;
        }
        while let Ok(meta) = rx.recv().await {
            ws_bridge
                .set_subscriptions(vec![meta.up_token_id, meta.down_token_id])
                .await;
        }
    });

    let mut state0 = engine::state::EngineState::default();
    state0.trading_enabled = cfg.trading.enabled_on_boot;
    state0.balance = cfg.trading.starting_balance;
    let slug_prefix = "btc-updown-5m-";
    boot_reconcile(&mut state0, &supabase, cfg.trading.mode, slug_prefix).await;
    let state = Arc::new(Mutex::new(state0));

    let handle = Arc::new(engine::tick::EngineHandle {
        state: state.clone(),
        executor: executor.clone(),
        tracker: tracker.clone(),
        clob: clob.clone(),
        clob_ws: Some(clob_ws.clone()),
        supabase: supabase.clone(),
        tick_recorder: tick_recorder.clone(),
        coinbase_ws: Some(coinbase_ws.clone()),
        cfg: cfg.clone(),
    });

    let tick_handle = handle.clone();
    tokio::spawn(async move {
        engine::tick::run_tick_loop(tick_handle).await;
    });

    let app_state = api::routes::AppState {
        engine: handle.clone(),
        boot_at: Utc::now(),
    };
    let http_port = cfg.http.port;
    let http_task = tokio::spawn(async move {
        if let Err(e) = api::routes::serve(app_state, http_port, "./static").await {
            tracing::error!(err = %e, "http server crashed");
        }
    });

    // Accept both Ctrl+C (local) and SIGTERM (DO App Platform redeploys).
    let mut term = signal(SignalKind::terminate())
        .map_err(|e| anyhow::anyhow!("SIGTERM register: {e}"))?;
    tracing::info!("engine running; SIGINT / SIGTERM to stop");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => tracing::info!("SIGINT received"),
        _ = term.recv()              => tracing::info!("SIGTERM received"),
        _ = http_task                => tracing::warn!("http task exited early"),
    }

    graceful_shutdown(
        state.clone(),
        executor.clone(),
        clob_ws.clone(),
        supabase.clone(),
    )
    .await;
    Ok(())
}

/// Hydrate EngineState from Supabase on boot so the bot survives redeploys without
/// losing daily_pnl or forgetting an abandoned open position.
async fn boot_reconcile(
    state: &mut engine::state::EngineState,
    supabase: &store::supabase::SupabaseClient,
    mode: Mode,
    slug_prefix: &str,
) {
    if !supabase.enabled() {
        return;
    }
    let now = Utc::now();
    state.maybe_roll_daily_pnl(now);

    // 1) Hydrate daily_pnl from today's closed trades (PST day boundary).
    let midnight_pst = midnight_pst_utc(now);
    match supabase.sum_realized_pnl_since(mode, slug_prefix, midnight_pst).await {
        Ok(sum) => {
            state.daily_pnl = sum;
            tracing::info!(daily_pnl = %sum, since = %midnight_pst, "reconcile: daily pnl hydrated");
        }
        Err(e) => tracing::warn!(err = %e, "reconcile: daily pnl hydrate failed"),
    }

    // 2) Handle any OPEN trade left behind by a prior crash / SIGKILL. For paper mode
    // we treat it as abandoned — the in-memory position didn't survive the restart,
    // so patch the row as CLOSED with a sentinel reason and move on. Live mode (not
    // yet implemented) would need to query CLOB /positions to reconstruct real state.
    match supabase.fetch_open_trade(mode, slug_prefix).await {
        Ok(Some(t)) => {
            tracing::warn!(
                trade_id = %t.id,
                slug = %t.market_slug,
                entry_time = %t.entry_time,
                "reconcile: found abandoned OPEN trade; marking closed",
            );
            let patch = json!({
                "status": "CLOSED",
                "exitTime": now,
                "exitReason": "abandoned_by_restart",
                "updatedAt": now,
            });
            if let Err(e) = supabase.patch_trade(&t.id, &patch).await {
                tracing::warn!(err = %e, "reconcile: patch_trade failed");
            }
        }
        Ok(None) => {}
        Err(e) => tracing::warn!(err = %e, "reconcile: fetch_open_trade failed"),
    }

    // 3) Hydrate recent_trades from Supabase so stats (total trades, win rate) are
    // accurate from the first poll, not just from trades closed this session.
    match supabase.fetch_recent_closed_trades(mode, slug_prefix, 10_000).await {
        Ok(trades) => {
            let count = trades.len();
            state.recent_trades = trades;
            tracing::info!(count, "reconcile: recent_trades hydrated from Supabase");
        }
        Err(e) => tracing::warn!(err = %e, "reconcile: recent_trades hydrate failed"),
    }
}

/// On SIGTERM/SIGINT: stop accepting entries, try to close any open position
/// within a bounded window, persist the exit row, then return so the runtime can
/// drop and the process exits cleanly.
async fn graceful_shutdown(
    state: Arc<Mutex<engine::state::EngineState>>,
    executor: Arc<RwLock<Arc<dyn exec::Executor>>>,
    clob_ws: data::clob_ws::ClobWs,
    supabase: Arc<store::supabase::SupabaseClient>,
) {
    // Phase 1: stop the engine from opening anything new.
    state.lock().await.trading_enabled = false;

    // Phase 2: if we're flat, we're done.
    let pending = state.lock().await.position.clone();
    let Some(pos) = pending else {
        tracing::info!("graceful shutdown: no open position");
        return;
    };

    tracing::info!(slug = %pos.market_slug, side = pos.side.as_str(), "graceful shutdown: closing position");

    // Mark price: latest WS bid for the position's side; fall back to entry to avoid
    // panics if the book is cold.
    let mark = match clob_ws.peek(&pos.token_id).await {
        Some(b) => b.best_bid.unwrap_or(pos.entry_price),
        None => pos.entry_price,
    };

    let req = exec::CloseRequest {
        position: pos.clone(),
        exit_reason: "shutdown".into(),
        mark_price: mark,
    };
    let exec_now = executor.read().await.clone();
    match tokio::time::timeout(
        Duration::from_secs(20),
        exec_now.close_position(req),
    )
    .await
    {
        Ok(Ok(res)) => {
            tracing::info!(pnl = %res.pnl, "graceful shutdown: closed");
            let patch = json!({
                "status": "CLOSED",
                "exitPrice": res.exit_price,
                "exitTime": res.exit_time,
                "exitReason": "shutdown",
                "pnl": res.pnl,
                "updatedAt": Utc::now(),
            });
            if let Err(e) = supabase.patch_trade(&pos.id, &patch).await {
                tracing::warn!(err = %e, "graceful shutdown: supabase patch failed");
            }
        }
        Ok(Err(e)) => tracing::warn!(err = %e, "graceful shutdown: close executor error"),
        Err(_) => tracing::warn!("graceful shutdown: close timed out"),
    }
}

/// The UTC instant corresponding to 00:00 in America/Los_Angeles on the current PST date.
fn midnight_pst_utc(now: chrono::DateTime<Utc>) -> chrono::DateTime<Utc> {
    let local_date = now.with_timezone(&Los_Angeles).date_naive();
    let naive_midnight = local_date.and_hms_opt(0, 0, 0).expect("midnight exists");
    match Los_Angeles.from_local_datetime(&naive_midnight) {
        chrono::LocalResult::Single(dt) => dt.with_timezone(&Utc),
        chrono::LocalResult::Ambiguous(dt, _) => dt.with_timezone(&Utc),
        // DST spring-forward skips midnight — fall back to 1:00 AM local.
        chrono::LocalResult::None => {
            let fallback = local_date
                .and_hms_opt(1, 0, 0)
                .expect("1am exists")
                .and_local_timezone(Los_Angeles)
                .single()
                .unwrap_or_else(|| now.with_timezone(&Los_Angeles));
            fallback.with_timezone(&Utc)
        }
    }
}
