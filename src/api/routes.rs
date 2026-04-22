use crate::engine::tick::EngineHandle;
use crate::exec::Executor;
use crate::exec::live::LiveExecutor;
use crate::exec::paper::PaperExecutor;
use crate::model::Mode;
use axum::{
    Json, Router,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use tower_http::services::ServeDir;

const PKG_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone)]
pub struct AppState {
    pub engine: Arc<EngineHandle>,
    pub boot_at: DateTime<Utc>,
}

pub fn build_router(state: AppState, static_dir: &str) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/status", get(status))
        .route("/trades", get(trades))
        .route("/positions", get(positions))
        .route("/trading/start", post(trading_start))
        .route("/trading/stop", post(trading_stop))
        .route("/mode", post(set_mode))
        .nest_service("/ui", ServeDir::new(static_dir))
        .route("/", get(root_redirect))
        .with_state(state)
}

async fn root_redirect() -> impl IntoResponse {
    axum::response::Redirect::permanent("/ui/")
}

async fn health(State(s): State<AppState>) -> Json<Value> {
    let uptime = (Utc::now() - s.boot_at).num_seconds();
    Json(json!({
        "ok": true,
        "version": PKG_VERSION,
        "uptime_s": uptime,
    }))
}

async fn status(State(s): State<AppState>) -> Json<Value> {
    let h = &s.engine;
    let now = Utc::now();
    let engine_state = h.state.lock().await.clone();
    let market = h.tracker.current().await;
    let mode = h.current_mode().await;
    let balance = h.executor.read().await.balance().await.ok();

    // WS book diagnostics only — the /status endpoint never makes its own REST
    // calls. For gate evaluation we use the tick loop's last snapshot (which
    // does use REST fallback) so the UI matches what the engine actually sees.
    let ws_diag = if let (Some(ws), Some(m)) = (h.clob_ws.as_ref(), market.as_ref()) {
        let up = ws.peek(&m.up_token_id).await;
        let dn = ws.peek(&m.down_token_id).await;
        let fresh = |s: &Option<crate::data::clob_ws::BookSnapshot>| {
            s.as_ref()
                .map(|b| (now - b.updated_at).num_milliseconds())
        };
        json!({
            "up_best_bid": up.as_ref().and_then(|b| b.best_bid),
            "up_best_ask": up.as_ref().and_then(|b| b.best_ask),
            "up_ms_ago": fresh(&up),
            "down_best_bid": dn.as_ref().and_then(|b| b.best_bid),
            "down_best_ask": dn.as_ref().and_then(|b| b.best_ask),
            "down_ms_ago": fresh(&dn),
        })
    } else {
        Value::Null
    };

    let gates = crate::engine::entry::evaluate_gates(
        &engine_state,
        engine_state.last_snapshot.as_ref(),
        &h.cfg.trading,
        now,
    );

    let market_json = market.as_ref().map(|m| {
        let time_left_sec = (m.end_date - now).num_seconds();
        json!({
            "slug": m.slug,
            "end_date": m.end_date,
            "time_left_sec": time_left_sec,
        })
    });

    let pos_json = engine_state.position.as_ref().map(|p| {
        json!({
            "id": p.id,
            "side": p.side.as_str(),
            "entry_price": p.entry_price,
            "shares": p.shares,
            "contract_size": p.contract_size,
            "entry_time": p.entry_time,
            "unrealized_pnl": engine_state.unrealized_pnl,
            "market_slug": p.market_slug,
        })
    });

    // Trade stats from in-memory history (hydrated from Supabase on boot).
    let closed_trades: Vec<_> = engine_state
        .recent_trades
        .iter()
        .filter(|t| t.pnl.is_some())
        .collect();
    let total_trades = closed_trades.len();
    let wins = closed_trades
        .iter()
        .filter(|t| t.pnl.unwrap() > rust_decimal_macros::dec!(0))
        .count();
    let win_rate = if total_trades > 0 {
        Some((wins as f64) / (total_trades as f64))
    } else {
        None
    };
    // Running total for the last 100 closed trades held in memory — drives the
    // small stats card only. The dashboard's hero "Realized PnL" uses
    // `all_time_realized_pnl` instead so it never drifts as the cache rolls.
    let total_pnl: rust_decimal::Decimal = closed_trades
        .iter()
        .filter_map(|t| t.pnl)
        .sum();

    let last_tick_ms_ago = engine_state
        .last_tick
        .map(|t| (now - t).num_milliseconds());

    Json(json!({
        "mode": mode.as_str(),
        "trading_enabled": engine_state.trading_enabled,
        "kill_switch": engine_state.kill_switch,
        "market": market_json,
        "position": pos_json,
        "balance": balance.as_ref().map(|b| json!({
            "available_usd": b.available_usd,
            "locked_usd": b.locked_usd,
        })),
        "daily_pnl": engine_state.daily_pnl,
        "last_tick_ms_ago": last_tick_ms_ago,
        "last_skip": engine_state.last_skip,
        "circuit_breaker": {
            "consecutive_losses": engine_state.circuit_breaker.consecutive_losses,
            "cooldown_until": engine_state.circuit_breaker.cooldown_until,
        },
        "book": ws_diag,
        "gates": gates,
        "total_trades": total_trades,
        "wins": wins,
        "win_rate": win_rate,
        "total_pnl": total_pnl,
        // Session-scoped realized PnL = current equity − starting balance. Matches
        // what the paper executor's balance would tell you (balance moved −$98
        // from $1000 → show −$98). Using the paper ledger's numbers ensures it
        // always reconciles with the Balance tile on the dashboard; a Supabase
        // sum would mix in prior sessions since the paper ledger resets on
        // every DO redeploy.
        "realized_pnl": balance.as_ref().map(|b| {
            b.available_usd + b.locked_usd - h.cfg.trading.starting_balance
        }),
    }))
}

async fn trades(
    State(s): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let limit = params
        .get("limit")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(20)
        .min(200);

    // Try Supabase first; fall back to in-memory recent trades if it's disabled or failing.
    let prefix = s.engine.cfg.polymarket.series_slug.replace("btc-up-or-down-", "btc-updown-");
    match s.engine.supabase.list_trades(&prefix, limit).await {
        Ok(rows) if !rows.is_empty() => (StatusCode::OK, Json(Value::Array(rows))),
        Ok(_) | Err(_) => {
            let fallback = s.engine.state.lock().await.recent_trades.clone();
            let rows: Vec<Value> = fallback
                .into_iter()
                .take(limit)
                .map(|t| serde_json::to_value(t).unwrap_or(Value::Null))
                .collect();
            (StatusCode::OK, Json(Value::Array(rows)))
        }
    }
}

async fn positions(State(s): State<AppState>) -> Json<Value> {
    let state = s.engine.state.lock().await;
    match &state.position {
        None => Json(json!([])),
        Some(p) => Json(json!([{
            "id": p.id,
            "side": p.side.as_str(),
            "entry_price": p.entry_price,
            "shares": p.shares,
            "market_slug": p.market_slug,
            "entry_time": p.entry_time,
        }])),
    }
}

fn require_token(headers: &HeaderMap, expected: &Option<String>) -> Result<(), StatusCode> {
    let Some(expected) = expected else { return Ok(()) };
    let got = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    if got == Some(expected.as_str()) {
        Ok(())
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

async fn trading_start(
    State(s): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, StatusCode> {
    require_token(&headers, &s.engine.cfg.http.control_token)?;
    let mut state = s.engine.state.lock().await;
    state.trading_enabled = true;
    Ok(Json(json!({ "trading_enabled": true })))
}

async fn trading_stop(
    State(s): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, StatusCode> {
    require_token(&headers, &s.engine.cfg.http.control_token)?;
    let mut state = s.engine.state.lock().await;
    state.trading_enabled = false;
    Ok(Json(json!({ "trading_enabled": false })))
}

#[derive(Debug, Deserialize)]
struct ModeBody {
    mode: String,
}

#[derive(Debug, Serialize)]
struct ModeResp {
    mode: String,
}

async fn set_mode(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ModeBody>,
) -> Result<Json<ModeResp>, (StatusCode, String)> {
    require_token(&headers, &s.engine.cfg.http.control_token)
        .map_err(|c| (c, "unauthorized".into()))?;

    let mode: Mode = body
        .mode
        .parse()
        .map_err(|e: String| (StatusCode::BAD_REQUEST, e))?;

    // Refuse to flip to live unless creds are loaded and no paper position is open.
    let state_copy = s.engine.state.lock().await.clone();
    if state_copy.position.is_some() {
        return Err((
            StatusCode::CONFLICT,
            "cannot switch mode while a position is open".into(),
        ));
    }
    if matches!(mode, Mode::Live) && s.engine.cfg.live_creds.is_none() {
        return Err((
            StatusCode::BAD_REQUEST,
            "live credentials not configured".into(),
        ));
    }

    // Build the new executor. Paper is always available; Live path not yet wired (task 12).
    let new_exec: Arc<dyn Executor> = match mode {
        Mode::Paper => {
            let paper = PaperExecutor::new(
                s.engine.cfg.trading.starting_balance,
                s.engine.cfg.trading.paper_fee_rate,
                s.engine.cfg.paper_ledger_path.clone(),
            )
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            Arc::new(paper)
        }
        Mode::Live => {
            let creds = s.engine.cfg.live_creds.as_ref().ok_or((
                StatusCode::BAD_REQUEST,
                "live credentials not configured".to_string(),
            ))?;
            let live = LiveExecutor::new(
                creds,
                s.engine.clob.clone(),
                s.engine.cfg.polymarket.chain_id,
            )
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            Arc::new(live)
        }
    };
    *s.engine.executor.write().await = new_exec;

    Ok(Json(ModeResp {
        mode: mode.as_str().to_string(),
    }))
}

pub async fn serve(state: AppState, port: u16, static_dir: &str) -> std::io::Result<()> {
    let app = build_router(state, static_dir);
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
    tracing::info!(port, "http server listening");
    axum::serve(listener, app).await
}
