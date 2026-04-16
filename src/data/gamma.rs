use crate::config::PolymarketConfig;
use crate::error::{BotError, Result};
use chrono::{DateTime, Utc};
use reqwest::Client;
use rust_decimal::Decimal;
use serde_json::Value;
use std::str::FromStr;
use std::time::Duration;

/// Minimal view of a Gamma market that our 5m strategy cares about.
#[derive(Debug, Clone)]
pub struct GammaMarket {
    pub slug: String,
    pub end_date: DateTime<Utc>,
    pub closed: bool,
    pub up_token_id: String,
    pub down_token_id: String,
    pub up_price: Option<Decimal>,
    pub down_price: Option<Decimal>,
    pub best_bid: Option<Decimal>,
    pub best_ask: Option<Decimal>,
}

#[derive(Debug, Clone)]
pub struct GammaClient {
    http: Client,
    gamma_url: String,
    series_slug: String,
    series_id: Option<String>,
}

impl GammaClient {
    pub fn new(cfg: &PolymarketConfig) -> Result<Self> {
        let http = Client::builder()
            .timeout(Duration::from_secs(5))
            .build()?;
        Ok(Self {
            http,
            gamma_url: cfg.gamma_url.trim_end_matches('/').to_string(),
            series_slug: cfg.series_slug.clone(),
            series_id: cfg.series_id.clone(),
        })
    }

    /// Fetch markets by flattening events returned for a given series_id. This is the path
    /// the dashboard uses in production; the /markets?seriesSlug filter on Gamma isn't
    /// reliable. Falls back to /markets?seriesSlug if no series_id is configured.
    pub async fn fetch_active_markets(&self, limit: usize) -> Result<Vec<GammaMarket>> {
        if let Some(sid) = self.series_id.as_deref() {
            return self.fetch_via_events(sid, limit).await;
        }
        self.fetch_via_markets(limit).await
    }

    async fn fetch_via_events(&self, series_id: &str, limit: usize) -> Result<Vec<GammaMarket>> {
        let url = format!("{}/events", self.gamma_url);
        let resp = self
            .http
            .get(&url)
            .query(&[
                ("series_id", series_id),
                ("active", "true"),
                ("closed", "false"),
                // Ascending by endDate: stale-but-not-marked-closed entries (a dozen or so
                // from prior months) appear first and are dropped by our `end_date > now`
                // filter; the current 5m cycle follows. Using desc would send tomorrow's
                // pre-scheduled markets first and bury today's until pagination exhausts.
                ("order", "endDate"),
                ("ascending", "true"),
                ("limit", &limit.to_string()),
            ])
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(BotError::Clob(format!("gamma /events {} {}", status.as_u16(), body)));
        }
        let value: Value = resp.json().await?;
        let events = value
            .as_array()
            .ok_or_else(|| BotError::parse("gamma /events: expected array"))?;
        let mut out = Vec::new();
        for e in events {
            let markets = e.get("markets").and_then(Value::as_array);
            let Some(markets) = markets else { continue };
            for m in markets {
                match parse_gamma_market(m) {
                    Ok(gm) => out.push(gm),
                    Err(err) => tracing::debug!(err = %err, "skipping event market"),
                }
            }
        }
        Ok(out)
    }

    async fn fetch_via_markets(&self, limit: usize) -> Result<Vec<GammaMarket>> {
        let url = format!("{}/markets", self.gamma_url);
        let resp = self
            .http
            .get(&url)
            .query(&[
                ("seriesSlug", self.series_slug.as_str()),
                ("active", "true"),
                ("closed", "false"),
                ("enableOrderBook", "true"),
                ("limit", &limit.to_string()),
            ])
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(BotError::Clob(format!("gamma /markets {} {}", status.as_u16(), body)));
        }
        let value: Value = resp.json().await?;
        let arr = value
            .as_array()
            .ok_or_else(|| BotError::parse("gamma /markets: expected array"))?;
        let mut out = Vec::with_capacity(arr.len());
        for m in arr {
            match parse_gamma_market(m) {
                Ok(gm) => out.push(gm),
                Err(err) => tracing::debug!(err = %err, "skipping market"),
            }
        }
        Ok(out)
    }

    /// Pick the active market whose `end_date` is in the future and nearest to `now`.
    ///
    /// Gamma's `active=true&closed=false` filter is leaky — it returns stale markets
    /// from prior cycles that never got marked closed (observed Dec-2025 / Jan-2026
    /// entries returned alongside today's 5m cycle). We pull a wide window and rely
    /// on the client-side `end_date > now` filter to find the live one.
    pub async fn pick_current_market(&self, now: DateTime<Utc>) -> Result<Option<GammaMarket>> {
        let markets = self.fetch_active_markets(200).await?;
        Ok(pick_nearest_future(markets, now))
    }
}

fn pick_nearest_future(
    mut markets: Vec<GammaMarket>,
    now: DateTime<Utc>,
) -> Option<GammaMarket> {
    markets.retain(|m| !m.closed && m.end_date > now);
    markets.sort_by_key(|m| m.end_date);
    markets.into_iter().next()
}

fn parse_gamma_market(v: &Value) -> Result<GammaMarket> {
    let slug = v
        .get("slug")
        .and_then(Value::as_str)
        .ok_or_else(|| BotError::parse("market.slug missing"))?
        .to_string();

    let end_date_raw = v
        .get("endDate")
        .and_then(Value::as_str)
        .ok_or_else(|| BotError::parse(format!("market.endDate missing on {slug}")))?;
    let end_date = DateTime::parse_from_rfc3339(end_date_raw)
        .map_err(|e| BotError::parse(format!("market.endDate parse ({slug}): {e}")))?
        .with_timezone(&Utc);

    let closed = v
        .get("closed")
        .map(|cv| match cv {
            Value::Bool(b) => *b,
            Value::String(s) => matches!(s.to_ascii_lowercase().as_str(), "true" | "1"),
            _ => false,
        })
        .unwrap_or(false);

    let outcomes = parse_json_string_or_array(v.get("outcomes"))?;
    let prices = parse_json_string_or_array(v.get("outcomePrices"))?;
    let tokens = parse_json_string_or_array(v.get("clobTokenIds"))?;

    if outcomes.len() != tokens.len() {
        return Err(BotError::parse(format!(
            "market {slug}: outcomes/tokens length mismatch"
        )));
    }

    let (mut up_token_id, mut down_token_id) = (None, None);
    let (mut up_price, mut down_price) = (None, None);
    for (i, o) in outcomes.iter().enumerate() {
        let lab = o.as_str().unwrap_or("").to_ascii_lowercase();
        let tok = tokens
            .get(i)
            .and_then(|t| t.as_str())
            .map(|s| s.to_string());
        let px = prices.get(i).and_then(value_to_decimal);
        match lab.as_str() {
            "up" => {
                up_token_id = tok;
                up_price = px;
            }
            "down" => {
                down_token_id = tok;
                down_price = px;
            }
            _ => {}
        }
    }

    let up_token_id = up_token_id.ok_or_else(|| {
        BotError::parse(format!("market {slug}: missing Up outcome token"))
    })?;
    let down_token_id = down_token_id.ok_or_else(|| {
        BotError::parse(format!("market {slug}: missing Down outcome token"))
    })?;

    let best_bid = v.get("bestBid").and_then(value_to_decimal);
    let best_ask = v.get("bestAsk").and_then(value_to_decimal);

    Ok(GammaMarket {
        slug,
        end_date,
        closed,
        up_token_id,
        down_token_id,
        up_price,
        down_price,
        best_bid,
        best_ask,
    })
}

/// Polymarket Gamma returns some array-shaped fields as JSON strings (e.g. `"[\"Up\",\"Down\"]"`).
/// Normalize to `Vec<Value>`.
fn parse_json_string_or_array(v: Option<&Value>) -> Result<Vec<Value>> {
    match v {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(a)) => Ok(a.clone()),
        Some(Value::String(s)) => {
            if s.is_empty() {
                return Ok(Vec::new());
            }
            let parsed: Value = serde_json::from_str(s)
                .map_err(|e| BotError::parse(format!("nested JSON: {e}")))?;
            match parsed {
                Value::Array(a) => Ok(a),
                other => Err(BotError::parse(format!(
                    "expected nested array, got {other:?}"
                ))),
            }
        }
        Some(other) => Err(BotError::parse(format!(
            "expected array or string, got {other:?}"
        ))),
    }
}

fn value_to_decimal(v: &Value) -> Option<Decimal> {
    match v {
        Value::String(s) => Decimal::from_str(s).ok(),
        Value::Number(n) => {
            if let Some(f) = n.as_f64() {
                Decimal::from_str(&format!("{f}")).ok()
            } else if let Some(i) = n.as_i64() {
                Some(Decimal::from(i))
            } else {
                None
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;

    fn make_market(slug: &str, end_iso: &str) -> Value {
        json!({
            "slug": slug,
            "endDate": end_iso,
            "closed": false,
            "outcomes": "[\"Up\",\"Down\"]",
            "outcomePrices": "[\"0.25\",\"0.75\"]",
            "clobTokenIds": "[\"111\",\"222\"]",
            "bestBid": 0.25,
            "bestAsk": 0.27
        })
    }

    #[test]
    fn parses_nested_json_string_fields() {
        let m = make_market("btc-updown-5m-x", "2026-04-16T17:00:00Z");
        let gm = parse_gamma_market(&m).unwrap();
        assert_eq!(gm.slug, "btc-updown-5m-x");
        assert_eq!(gm.up_token_id, "111");
        assert_eq!(gm.down_token_id, "222");
        assert_eq!(gm.up_price.unwrap().to_string(), "0.25");
        assert_eq!(gm.down_price.unwrap().to_string(), "0.75");
        assert!(gm.best_ask.is_some());
    }

    #[test]
    fn picks_nearest_future_market() {
        let now = Utc.with_ymd_and_hms(2026, 4, 16, 17, 0, 0).unwrap();
        let a = parse_gamma_market(&make_market("a", "2026-04-16T17:30:00Z")).unwrap();
        let b = parse_gamma_market(&make_market("b", "2026-04-16T17:05:00Z")).unwrap();
        let c_past = parse_gamma_market(&make_market("c", "2026-04-16T16:55:00Z")).unwrap();
        let picked = pick_nearest_future(vec![a, b, c_past], now).unwrap();
        assert_eq!(picked.slug, "b");
    }
}
