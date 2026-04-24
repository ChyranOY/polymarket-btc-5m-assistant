//! Coinbase Exchange spot-price WebSocket client.
//!
//! Connects to `wss://ws-feed.exchange.coinbase.com`, subscribes to the
//! `ticker` channel for BTC-USD, and maintains a short rolling history
//! (~2 minutes) of `(timestamp, price)` samples. The engine reads the
//! latest price and short-window deltas to add an independent directional
//! signal alongside Polymarket's own quote.
//!
//! Binance is blocked from the DO region the bot runs in (same reason the
//! kronos backfill uses Coinbase — see commit 9316d85), so Coinbase is the
//! sanctioned spot feed. Latency is ~50-150ms worse than Binance but fine
//! for 5-minute contracts.
//!
//! Protocol (from Coinbase Exchange docs):
//!   subscribe:  `{"type":"subscribe","product_ids":["BTC-USD"],"channels":["ticker"]}`
//!   ticker:     `{"type":"ticker","product_id":"BTC-USD","price":"67423.12",
//!                  "time":"2026-04-23T21:00:00.123Z", ...}`

use chrono::{DateTime, Utc};
use futures_util::{SinkExt, StreamExt};
use rust_decimal::Decimal;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::{sleep, sleep_until, Duration, Instant};
use tokio_tungstenite::{connect_async, tungstenite::Message};

const WS_URL: &str = "wss://ws-feed.exchange.coinbase.com";
const PRODUCT: &str = "BTC-USD";
/// How much history to retain. Longest delta window we expose is 2 minutes
/// (momentum entry). Keep noticeably more so `delta_abs(120)` lookups don't
/// fail on the edge when the oldest sample was just trimmed.
const HISTORY_RETENTION: Duration = Duration::from_secs(240);
/// Socket gets a reconnect if no frame arrives in this long. Coinbase sends
/// matches + heartbeats frequently on BTC-USD; 30s of silence means dead TCP.
const READ_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct SpotSample {
    pub price: Decimal,
    pub at: DateTime<Utc>,
}

pub type SpotHistory = Arc<RwLock<VecDeque<SpotSample>>>;

#[derive(Clone)]
pub struct CoinbaseWs {
    history: SpotHistory,
}

impl CoinbaseWs {
    /// Start the background WS task and return a read-only handle.
    pub fn start() -> Self {
        let history: SpotHistory = Arc::new(RwLock::new(VecDeque::new()));
        let task_history = history.clone();
        tokio::spawn(async move {
            run_ws(task_history).await;
        });
        Self { history }
    }

    /// Most recent spot price, if any. `None` until the first ticker frame lands.
    pub async fn latest(&self) -> Option<SpotSample> {
        self.history.read().await.back().cloned()
    }

    /// Percent change vs. the most recent sample that is at least `window` old.
    /// Returns `None` when no sample in history is old enough (bot just booted
    /// or the feed is reconnecting), or on zero/negative prices.
    pub async fn delta_pct(&self, window: Duration) -> Option<Decimal> {
        let (latest, past) = self.anchor_pair(window).await?;
        if past.price <= Decimal::ZERO {
            return None;
        }
        Some(((latest.price - past.price) / past.price) * Decimal::from(100))
    }

    /// Absolute USD change vs. the most recent sample at least `window` old.
    /// Positive = price went up. None when history is too short.
    pub async fn delta_abs(&self, window: Duration) -> Option<Decimal> {
        let (latest, past) = self.anchor_pair(window).await?;
        Some(latest.price - past.price)
    }

    async fn anchor_pair(&self, window: Duration) -> Option<(SpotSample, SpotSample)> {
        let history = self.history.read().await;
        let latest = history.back()?.clone();
        let window_dur = chrono::Duration::from_std(window).ok()?;
        let past = history
            .iter()
            .rev()
            .find(|s| latest.at - s.at >= window_dur)?
            .clone();
        Some((latest, past))
    }
}

async fn run_ws(history: SpotHistory) {
    let mut backoff = Duration::from_millis(500);
    const MAX_BACKOFF: Duration = Duration::from_secs(30);

    loop {
        tracing::debug!(url = WS_URL, "coinbase_ws: connecting");
        let conn = connect_async(WS_URL).await;
        let (mut ws_stream, _resp) = match conn {
            Ok(c) => {
                backoff = Duration::from_millis(500);
                tracing::info!("coinbase_ws: connected");
                c
            }
            Err(e) => {
                tracing::warn!(err = %e, backoff_ms = backoff.as_millis() as u64, "coinbase_ws: connect failed");
                sleep(backoff).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
                continue;
            }
        };

        // Subscribe to BTC-USD ticker.
        let sub = json!({
            "type": "subscribe",
            "product_ids": [PRODUCT],
            "channels": ["ticker"],
        })
        .to_string();
        if let Err(e) = ws_stream.send(Message::Text(sub.into())).await {
            tracing::warn!(err = %e, "coinbase_ws: subscribe send failed");
            sleep(backoff).await;
            backoff = (backoff * 2).min(MAX_BACKOFF);
            continue;
        }

        let mut last_frame_at = Instant::now();
        let disconnect_reason = loop {
            tokio::select! {
                msg = ws_stream.next() => {
                    last_frame_at = Instant::now();
                    match msg {
                        Some(Ok(Message::Text(txt))) => handle_msg(&history, txt.as_ref()).await,
                        Some(Ok(Message::Binary(bin))) => {
                            if let Ok(s) = std::str::from_utf8(&bin) {
                                handle_msg(&history, s).await;
                            }
                        }
                        Some(Ok(Message::Ping(p))) => {
                            if ws_stream.send(Message::Pong(p)).await.is_err() {
                                break "pong-send-error";
                            }
                        }
                        Some(Ok(Message::Pong(_))) => {}
                        Some(Ok(Message::Close(frame))) => {
                            tracing::info!(?frame, "coinbase_ws: server closed");
                            break "server-close";
                        }
                        Some(Ok(Message::Frame(_))) => {}
                        Some(Err(e)) => {
                            tracing::warn!(err = %e, "coinbase_ws: stream error");
                            break "stream-error";
                        }
                        None => {
                            tracing::info!("coinbase_ws: stream ended");
                            break "stream-end";
                        }
                    }
                }
                _ = sleep_until(last_frame_at + READ_IDLE_TIMEOUT) => {
                    tracing::warn!(
                        idle_s = READ_IDLE_TIMEOUT.as_secs(),
                        "coinbase_ws: read-idle timeout; reconnecting"
                    );
                    break "read-idle-timeout";
                }
            }
        };

        tracing::debug!(reason = disconnect_reason, "coinbase_ws: reconnecting");
        sleep(backoff).await;
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

async fn handle_msg(history: &SpotHistory, raw: &str) {
    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return;
    };
    let Some(kind) = value.get("type").and_then(Value::as_str) else {
        return;
    };
    if kind != "ticker" {
        return;
    }
    if value.get("product_id").and_then(Value::as_str) != Some(PRODUCT) {
        return;
    }
    let Some(price) = value
        .get("price")
        .and_then(Value::as_str)
        .and_then(|s| Decimal::from_str(s).ok())
    else {
        return;
    };
    let at = value
        .get("time")
        .and_then(Value::as_str)
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(Utc::now);

    let sample = SpotSample { price, at };
    let mut h = history.write().await;
    // Drop expired samples from the front.
    let cutoff = at - chrono::Duration::from_std(HISTORY_RETENTION).unwrap_or_default();
    while let Some(front) = h.front() {
        if front.at < cutoff {
            h.pop_front();
        } else {
            break;
        }
    }
    h.push_back(sample);
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn sample(price: Decimal, ago_secs: i64) -> SpotSample {
        SpotSample {
            price,
            at: Utc::now() - chrono::Duration::seconds(ago_secs),
        }
    }

    #[tokio::test]
    async fn delta_pct_positive_move() {
        let history: SpotHistory = Arc::new(RwLock::new(VecDeque::new()));
        {
            let mut h = history.write().await;
            h.push_back(sample(dec!(67000), 60));
            h.push_back(sample(dec!(67335), 0));
        }
        let ws = CoinbaseWs { history };
        let d = ws.delta_pct(Duration::from_secs(60)).await.unwrap();
        // 335 / 67000 * 100 = 0.5
        assert!((d - dec!(0.5)).abs() < dec!(0.01), "got {d}");
    }

    #[tokio::test]
    async fn delta_pct_none_when_history_too_short() {
        let history: SpotHistory = Arc::new(RwLock::new(VecDeque::new()));
        {
            let mut h = history.write().await;
            // Only a sample from 5s ago — can't answer a 60s window.
            h.push_back(sample(dec!(67000), 5));
            h.push_back(sample(dec!(67050), 0));
        }
        let ws = CoinbaseWs { history };
        assert!(ws.delta_pct(Duration::from_secs(60)).await.is_none());
    }

    #[tokio::test]
    async fn handle_msg_pushes_ticker_and_drops_expired() {
        let history: SpotHistory = Arc::new(RwLock::new(VecDeque::new()));
        let old_ts = (Utc::now() - chrono::Duration::seconds(300)).to_rfc3339();
        let new_ts = Utc::now().to_rfc3339();
        // An old sample pre-seeded, then a fresh ticker comes in — old should be dropped.
        history.write().await.push_back(SpotSample {
            price: dec!(65000),
            at: DateTime::parse_from_rfc3339(&old_ts).unwrap().with_timezone(&Utc),
        });
        let msg = format!(
            r#"{{"type":"ticker","product_id":"BTC-USD","price":"67000.25","time":"{new_ts}"}}"#
        );
        handle_msg(&history, &msg).await;
        let h = history.read().await;
        assert_eq!(h.len(), 1); // old one dropped
        assert_eq!(h.back().unwrap().price, dec!(67000.25));
    }

    #[tokio::test]
    async fn handle_msg_ignores_wrong_product() {
        let history: SpotHistory = Arc::new(RwLock::new(VecDeque::new()));
        let msg = r#"{"type":"ticker","product_id":"ETH-USD","price":"3200","time":"2026-04-23T21:00:00Z"}"#;
        handle_msg(&history, msg).await;
        assert!(history.read().await.is_empty());
    }
}
