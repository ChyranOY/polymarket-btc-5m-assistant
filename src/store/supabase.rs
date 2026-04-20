use crate::config::SupabaseConfig;
use crate::error::{BotError, Result};
use crate::model::{Mode, Trade};
use chrono::{DateTime, Utc};
use reqwest::header::{HeaderMap, HeaderValue};
use reqwest::Client;
use rust_decimal::Decimal;
use serde_json::Value;
use std::str::FromStr;
use std::time::Duration;

/// Matches only trades written by this Rust bot. The old JS bot shares the same
/// `trades` table; its `entryGateSnapshot` JSON never contains `up_ask` (which is
/// unique to `engine::tick::new_open_trade`). Apply to every read query so
/// hydrated state / UI lists don't mix the two bots' rows.
const RUST_ORIGIN_FILTER: &str = "like.*up_ask*";

/// Thin PostgREST client for the `trades` and `signal_ticks` tables.
/// No-ops when SUPABASE_URL is not configured — callers don't need to branch.
#[derive(Debug, Clone)]
pub struct SupabaseClient {
    http: Client,
    base_url: Option<String>,
    enabled: bool,
}

impl SupabaseClient {
    pub fn new(cfg: &SupabaseConfig) -> Result<Self> {
        if !cfg.is_configured() {
            tracing::warn!("supabase: disabled (SUPABASE_URL / SUPABASE_SERVICE_ROLE_KEY unset)");
            return Ok(Self {
                http: Client::new(),
                base_url: None,
                enabled: false,
            });
        }

        let key = cfg.service_role_key.as_deref().unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            "apikey",
            HeaderValue::from_str(key).map_err(|e| BotError::cfg(format!("apikey: {e}")))?,
        );
        headers.insert(
            "Authorization",
            HeaderValue::from_str(&format!("Bearer {key}"))
                .map_err(|e| BotError::cfg(format!("auth: {e}")))?,
        );
        headers.insert(
            "Content-Type",
            HeaderValue::from_static("application/json"),
        );

        let http = Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_secs(10))
            .build()?;

        Ok(Self {
            http,
            base_url: cfg.url.as_ref().map(|u| u.trim_end_matches('/').to_string()),
            enabled: true,
        })
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    fn rest(&self, path: &str) -> Option<String> {
        self.base_url
            .as_ref()
            .map(|base| format!("{}/rest/v1/{}", base, path.trim_start_matches('/')))
    }

    /// Upsert a trade row (PK is `id`). Safe to call for both OPEN and CLOSED states.
    pub async fn upsert_trade(&self, trade: &Trade) -> Result<()> {
        let Some(url) = self.rest("trades") else { return Ok(()) };
        let resp = self
            .http
            .post(&url)
            .header("Prefer", "resolution=merge-duplicates,return=minimal")
            .json(trade)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(BotError::Supabase { status: status.as_u16(), body });
        }
        Ok(())
    }

    /// Patch a trade by id. Used on close to attach exit fields without re-sending the whole row.
    pub async fn patch_trade(&self, trade_id: &str, patch: &Value) -> Result<()> {
        let Some(url) = self.rest("trades") else { return Ok(()) };
        let resp = self
            .http
            .patch(&url)
            .query(&[("id", format!("eq.{trade_id}"))])
            .header("Prefer", "return=minimal")
            .json(patch)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(BotError::Supabase { status: status.as_u16(), body });
        }
        Ok(())
    }

    /// Recent trades for the UI: ?marketSlug=like.<prefix>*&order=entryTime.desc&limit=<n>
    pub async fn list_trades(
        &self,
        market_slug_prefix: &str,
        limit: usize,
    ) -> Result<Vec<Value>> {
        let Some(url) = self.rest("trades") else { return Ok(Vec::new()) };
        let resp = self
            .http
            .get(&url)
            .query(&[
                ("marketSlug", format!("like.{market_slug_prefix}*")),
                ("entryGateSnapshot", RUST_ORIGIN_FILTER.to_string()),
                ("order", "entryTime.desc".to_string()),
                ("limit", limit.to_string()),
            ])
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(BotError::Supabase { status: status.as_u16(), body });
        }
        let v: Value = resp.json().await?;
        match v {
            Value::Array(a) => Ok(a),
            other => Err(BotError::parse(format!(
                "supabase trades: expected array, got {other:?}"
            ))),
        }
    }

    /// Fetch recent closed trades for a mode. Used to hydrate in-memory stats
    /// (total trades, win rate, recent_trades cache) on boot.
    pub async fn fetch_recent_closed_trades(
        &self,
        mode: Mode,
        slug_prefix: &str,
        limit: usize,
    ) -> Result<Vec<Trade>> {
        let Some(url) = self.rest("trades") else { return Ok(Vec::new()) };
        let resp = self
            .http
            .get(&url)
            .query(&[
                ("status", "eq.CLOSED".to_string()),
                ("mode", format!("eq.{}", mode.as_str())),
                ("marketSlug", format!("like.{slug_prefix}*")),
                ("entryGateSnapshot", RUST_ORIGIN_FILTER.to_string()),
                ("order", "exitTime.desc".to_string()),
                ("limit", limit.to_string()),
            ])
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(BotError::Supabase { status: status.as_u16(), body });
        }
        let rows: Vec<Value> = resp.json().await?;
        let mut trades = Vec::with_capacity(rows.len());
        for row in rows {
            match serde_json::from_value::<Trade>(row) {
                Ok(t) => trades.push(t),
                Err(e) => tracing::debug!(err = %e, "skipping unparseable trade row"),
            }
        }
        Ok(trades)
    }

    /// Most recent OPEN trade for the given mode — used for boot-time reconciliation
    /// after a redeploy wipes in-memory state.
    pub async fn fetch_open_trade(&self, mode: Mode, slug_prefix: &str) -> Result<Option<Trade>> {
        let Some(url) = self.rest("trades") else { return Ok(None) };
        let resp = self
            .http
            .get(&url)
            .query(&[
                ("status", "eq.OPEN".to_string()),
                ("mode", format!("eq.{}", mode.as_str())),
                ("marketSlug", format!("like.{slug_prefix}*")),
                ("entryGateSnapshot", RUST_ORIGIN_FILTER.to_string()),
                ("order", "entryTime.desc".to_string()),
                ("limit", "1".to_string()),
            ])
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(BotError::Supabase { status: status.as_u16(), body });
        }
        let v: Value = resp.json().await?;
        let rows = v.as_array().cloned().unwrap_or_default();
        let first = rows.into_iter().next();
        match first {
            None => Ok(None),
            Some(row) => Ok(Some(serde_json::from_value(row)?)),
        }
    }

    /// Sum of realized pnl for closed trades whose `exitTime >= since`. Used to hydrate
    /// the in-memory `daily_pnl` counter on boot so it survives redeploys.
    pub async fn sum_realized_pnl_since(
        &self,
        mode: Mode,
        slug_prefix: &str,
        since: DateTime<Utc>,
    ) -> Result<Decimal> {
        let Some(url) = self.rest("trades") else { return Ok(Decimal::ZERO) };
        let resp = self
            .http
            .get(&url)
            .query(&[
                ("status", "eq.CLOSED".to_string()),
                ("mode", format!("eq.{}", mode.as_str())),
                ("marketSlug", format!("like.{slug_prefix}*")),
                ("entryGateSnapshot", RUST_ORIGIN_FILTER.to_string()),
                ("exitTime", format!("gte.{}", since.to_rfc3339())),
                ("select", "pnl".to_string()),
            ])
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(BotError::Supabase { status: status.as_u16(), body });
        }
        let rows: Vec<Value> = resp.json().await?;
        let mut total = Decimal::ZERO;
        for row in rows {
            let raw = row.get("pnl");
            let p = match raw {
                Some(Value::String(s)) => Decimal::from_str(s).ok(),
                Some(Value::Number(n)) => n
                    .as_f64()
                    .and_then(|f| Decimal::from_str(&f.to_string()).ok()),
                _ => None,
            };
            if let Some(p) = p {
                total += p;
            }
        }
        Ok(total)
    }

    /// Batched signal_ticks insert. Accepts any JSON array; schema is loose.
    pub async fn insert_signal_ticks(&self, rows: &[Value]) -> Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let Some(url) = self.rest("signal_ticks") else { return Ok(()) };
        let resp = self
            .http
            .post(&url)
            .header("Prefer", "return=minimal")
            .json(rows)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(BotError::Supabase { status: status.as_u16(), body });
        }
        Ok(())
    }
}
