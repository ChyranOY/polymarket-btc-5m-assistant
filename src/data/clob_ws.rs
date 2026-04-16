//! CLOB orderbook WebSocket client.
//!
//! Connects to `wss://ws-subscriptions-sink.polymarket.com/ws/market`, subscribes to
//! the `book` channel for one or more CLOB token IDs, and keeps an in-memory map of
//! the latest best bid / best ask per token. The engine reads from this map each tick
//! instead of hitting the rate-limited REST `/price` endpoint.
//!
//! Reconnects with exponential backoff (500ms → cap 30s). On reconnect, re-sends the
//! current subscription set. Subscription changes at runtime are delivered via an
//! mpsc command channel.
//!
//! Protocol (reverse-engineered from the dashboard's `clobWs.js`):
//!   subscribe:   `{"type":"subscribe","channel":"book","assets_ids":["tok1","tok2"]}`
//!   unsubscribe: `{"type":"unsubscribe","channel":"book","assets_ids":["tok1"]}`
//!   book event:  `{"asset_id":"tok1","bids":[{"price":"0.25","size":"10"}, ...], "asks":[...], ...}`
//!
//! Book messages appear to be full snapshots (not deltas) — each one replaces the
//! state for its token.

use chrono::{DateTime, Utc};
use futures_util::{SinkExt, StreamExt};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc};
use tokio::time::{Duration, sleep};
use tokio_tungstenite::{connect_async, tungstenite::Message};

#[derive(Debug, Clone)]
pub struct BookSnapshot {
    pub best_bid: Option<Decimal>,
    pub best_ask: Option<Decimal>,
    pub updated_at: DateTime<Utc>,
}

pub type BookStore = Arc<RwLock<HashMap<String, BookSnapshot>>>;

#[derive(Debug)]
enum WsCommand {
    SetSubscriptions(Vec<String>),
}

#[derive(Clone)]
pub struct ClobWs {
    books: BookStore,
    cmd_tx: mpsc::Sender<WsCommand>,
}

impl ClobWs {
    /// Start the background WebSocket task. Returns a handle that gives the engine
    /// read access to per-token book state and a way to update subscriptions.
    pub fn start(ws_url: impl Into<String>) -> Self {
        let books: BookStore = Arc::new(RwLock::new(HashMap::new()));
        let (cmd_tx, cmd_rx) = mpsc::channel::<WsCommand>(8);
        let task_books = books.clone();
        let url = ws_url.into();
        tokio::spawn(async move {
            run_ws(url, task_books, cmd_rx).await;
        });
        Self { books, cmd_tx }
    }

    pub fn books(&self) -> BookStore {
        self.books.clone()
    }

    /// Replace the current subscription set with `ids`. Idempotent.
    pub async fn set_subscriptions(&self, ids: Vec<String>) {
        let _ = self.cmd_tx.send(WsCommand::SetSubscriptions(ids)).await;
    }

    /// Fast non-locking peek for the engine's tick loop. Clones the snapshot so the
    /// read lock is released before the engine does any work.
    pub async fn peek(&self, token_id: &str) -> Option<BookSnapshot> {
        self.books.read().await.get(token_id).cloned()
    }
}

async fn run_ws(url: String, books: BookStore, mut cmd_rx: mpsc::Receiver<WsCommand>) {
    let mut current_ids: Vec<String> = Vec::new();
    let mut backoff = Duration::from_millis(500);
    const MAX_BACKOFF: Duration = Duration::from_secs(30);

    loop {
        tracing::debug!(url = %url, "clob_ws: connecting");
        let conn = connect_async(&url).await;
        let (mut ws_stream, _resp) = match conn {
            Ok(c) => {
                backoff = Duration::from_millis(500);
                tracing::info!("clob_ws: connected");
                c
            }
            Err(e) => {
                tracing::warn!(err = %e, backoff_ms = backoff.as_millis() as u64, "clob_ws: connect failed");
                sleep(backoff).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
                continue;
            }
        };

        // On (re)connect, send the current subscription set if any.
        if !current_ids.is_empty() {
            if let Err(e) = send_subscribe(&mut ws_stream, &current_ids).await {
                tracing::warn!(err = %e, "clob_ws: initial subscribe send failed");
            }
        }

        // Drive reads and commands until the socket errors/closes.
        let disconnect_reason = loop {
            tokio::select! {
                biased;
                cmd = cmd_rx.recv() => {
                    match cmd {
                        Some(WsCommand::SetSubscriptions(new_ids)) => {
                            if let Err(e) = apply_subscription_diff(
                                &mut ws_stream,
                                &mut current_ids,
                                new_ids,
                            ).await {
                                tracing::warn!(err = %e, "clob_ws: subscription update failed");
                                break "sub-send-error";
                            }
                        }
                        None => {
                            tracing::info!("clob_ws: command channel closed; shutting down");
                            return;
                        }
                    }
                }
                msg = ws_stream.next() => {
                    match msg {
                        Some(Ok(Message::Text(txt))) => handle_book_msg(&books, txt.as_ref()).await,
                        Some(Ok(Message::Binary(bin))) => {
                            if let Ok(s) = std::str::from_utf8(&bin) {
                                handle_book_msg(&books, s).await;
                            }
                        }
                        Some(Ok(Message::Ping(payload))) => {
                            if ws_stream.send(Message::Pong(payload)).await.is_err() {
                                break "pong-send-error";
                            }
                        }
                        Some(Ok(Message::Pong(_))) => {}
                        Some(Ok(Message::Close(frame))) => {
                            tracing::info!(?frame, "clob_ws: server closed");
                            break "server-close";
                        }
                        Some(Ok(Message::Frame(_))) => {}
                        Some(Err(e)) => {
                            tracing::warn!(err = %e, "clob_ws: stream error");
                            break "stream-error";
                        }
                        None => {
                            tracing::info!("clob_ws: stream ended");
                            break "stream-end";
                        }
                    }
                }
            }
        };

        tracing::debug!(reason = disconnect_reason, "clob_ws: reconnecting");
        sleep(backoff).await;
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

async fn apply_subscription_diff(
    ws_stream: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    current_ids: &mut Vec<String>,
    new_ids: Vec<String>,
) -> Result<(), tokio_tungstenite::tungstenite::Error> {
    let current_set: HashSet<&String> = current_ids.iter().collect();
    let new_set: HashSet<&String> = new_ids.iter().collect();

    let to_add: Vec<String> = new_set
        .difference(&current_set)
        .map(|s| (*s).clone())
        .collect();
    let to_remove: Vec<String> = current_set
        .difference(&new_set)
        .map(|s| (*s).clone())
        .collect();

    if !to_remove.is_empty() {
        send_message(ws_stream, "unsubscribe", &to_remove).await?;
    }
    if !to_add.is_empty() {
        send_message(ws_stream, "subscribe", &to_add).await?;
    }
    *current_ids = new_ids;
    Ok(())
}

async fn send_subscribe(
    ws_stream: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    ids: &[String],
) -> Result<(), tokio_tungstenite::tungstenite::Error> {
    send_message(ws_stream, "subscribe", ids).await
}

async fn send_message(
    ws_stream: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    kind: &str,
    ids: &[String],
) -> Result<(), tokio_tungstenite::tungstenite::Error> {
    let payload = json!({
        "type": kind,
        "channel": "book",
        "assets_ids": ids,
    })
    .to_string();
    tracing::debug!(kind, count = ids.len(), "clob_ws: send");
    ws_stream.send(Message::Text(payload.into())).await
}

async fn handle_book_msg(books: &BookStore, raw: &str) {
    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return;
    };
    // The sink sometimes sends an array of events in a single frame.
    match value {
        Value::Array(items) => {
            for item in items {
                apply_book_snapshot(books, &item).await;
            }
        }
        other => apply_book_snapshot(books, &other).await,
    }
}

async fn apply_book_snapshot(books: &BookStore, msg: &Value) {
    // Filter to book events — ignore `pong`, `status`, etc.
    let is_book = msg
        .get("event_type")
        .and_then(Value::as_str)
        .map(|s| s.eq_ignore_ascii_case("book") || s.eq_ignore_ascii_case("price_change"))
        .unwrap_or(false)
        || msg.get("bids").is_some()
        || msg.get("asks").is_some();
    if !is_book {
        return;
    }

    let token_id = msg
        .get("asset_id")
        .or_else(|| msg.get("token_id"))
        .or_else(|| msg.get("market"))
        .and_then(Value::as_str);
    let Some(token_id) = token_id else { return };

    let bids = msg.get("bids").and_then(Value::as_array);
    let asks = msg.get("asks").and_then(Value::as_array);

    // Previous snapshot — `price_change` events may carry only one side; keep the other.
    let prev = books.read().await.get(token_id).cloned();

    let best_bid = bids
        .map(|levels| best_level(levels, Extremum::Max))
        .unwrap_or_else(|| prev.as_ref().and_then(|p| p.best_bid));
    let best_ask = asks
        .map(|levels| best_level(levels, Extremum::Min))
        .unwrap_or_else(|| prev.as_ref().and_then(|p| p.best_ask));

    let snap = BookSnapshot {
        best_bid,
        best_ask,
        updated_at: Utc::now(),
    };
    books.write().await.insert(token_id.to_string(), snap);
}

#[derive(Copy, Clone)]
enum Extremum {
    Min,
    Max,
}

fn best_level(levels: &[Value], which: Extremum) -> Option<Decimal> {
    let mut best: Option<Decimal> = None;
    for lvl in levels {
        let price = lvl
            .get("price")
            .and_then(|v| v.as_str().or_else(|| v.as_f64().map(|_| "").filter(|s| !s.is_empty())))
            .and_then(|s| Decimal::from_str(s).ok())
            .or_else(|| {
                lvl.get("price")
                    .and_then(Value::as_f64)
                    .and_then(|f| Decimal::from_str(&f.to_string()).ok())
            })
            .or_else(|| {
                lvl.as_array().and_then(|a| {
                    a.first()
                        .and_then(|v| v.as_str())
                        .and_then(|s| Decimal::from_str(s).ok())
                })
            });
        let Some(p) = price else { continue };
        if p <= Decimal::ZERO {
            continue;
        }
        best = Some(match (best, which) {
            (None, _) => p,
            (Some(b), Extremum::Max) => b.max(p),
            (Some(b), Extremum::Min) => b.min(p),
        });
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;
    use serde_json::json;

    #[tokio::test]
    async fn applies_snapshot_from_book_msg() {
        let books: BookStore = Arc::new(RwLock::new(HashMap::new()));
        let msg = json!({
            "event_type": "book",
            "asset_id": "tok1",
            "bids": [{"price":"0.24","size":"10"},{"price":"0.23","size":"5"}],
            "asks": [{"price":"0.27","size":"8"},{"price":"0.28","size":"12"}],
        });
        apply_book_snapshot(&books, &msg).await;
        let b = books.read().await.get("tok1").cloned().unwrap();
        assert_eq!(b.best_bid, Some(dec!(0.24)));
        assert_eq!(b.best_ask, Some(dec!(0.27)));
    }

    #[tokio::test]
    async fn price_change_without_asks_keeps_prev_ask() {
        let books: BookStore = Arc::new(RwLock::new(HashMap::new()));
        let initial = json!({
            "event_type": "book",
            "asset_id": "tok2",
            "bids": [{"price":"0.20","size":"10"}],
            "asks": [{"price":"0.22","size":"10"}],
        });
        apply_book_snapshot(&books, &initial).await;
        let delta = json!({
            "event_type": "price_change",
            "asset_id": "tok2",
            "bids": [{"price":"0.21","size":"10"}],
        });
        apply_book_snapshot(&books, &delta).await;
        let b = books.read().await.get("tok2").cloned().unwrap();
        assert_eq!(b.best_bid, Some(dec!(0.21)));
        assert_eq!(b.best_ask, Some(dec!(0.22))); // preserved
    }

    #[tokio::test]
    async fn ignores_non_book_messages() {
        let books: BookStore = Arc::new(RwLock::new(HashMap::new()));
        apply_book_snapshot(&books, &json!({"event_type":"pong"})).await;
        apply_book_snapshot(&books, &json!({"status":"ok"})).await;
        assert!(books.read().await.is_empty());
    }

    #[test]
    fn best_level_picks_correct_extremum() {
        let bids = vec![
            json!({"price":"0.23","size":"5"}),
            json!({"price":"0.25","size":"10"}),
            json!({"price":"0.24","size":"8"}),
        ];
        assert_eq!(best_level(&bids, Extremum::Max), Some(dec!(0.25)));
        assert_eq!(best_level(&bids, Extremum::Min), Some(dec!(0.23)));
    }
}
