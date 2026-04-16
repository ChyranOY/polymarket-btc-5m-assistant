use crate::config::PolymarketConfig;
use crate::error::{BotError, Result};
use reqwest::Client;
use rust_decimal::Decimal;
use serde::Deserialize;
use std::str::FromStr;
use std::time::Duration;

#[derive(Debug, Clone, Copy)]
pub enum PriceSide {
    Buy,
    Sell,
}

impl PriceSide {
    fn as_str(&self) -> &'static str {
        match self {
            PriceSide::Buy => "buy",
            PriceSide::Sell => "sell",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct BookLevel {
    pub price: String,
    pub size: String,
}

impl BookLevel {
    pub fn price(&self) -> Option<Decimal> {
        Decimal::from_str(&self.price).ok()
    }
    pub fn size(&self) -> Option<Decimal> {
        Decimal::from_str(&self.size).ok()
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct OrderBook {
    #[serde(default)]
    pub bids: Vec<BookLevel>,
    #[serde(default)]
    pub asks: Vec<BookLevel>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct BookSummary {
    pub best_bid: Option<Decimal>,
    pub best_ask: Option<Decimal>,
    pub bid_depth: Decimal,
    pub ask_depth: Decimal,
}

impl OrderBook {
    pub fn summarize(&self) -> BookSummary {
        // Polymarket CLOB returns bids ascending and asks descending in some deployments,
        // so we compute the extremum explicitly rather than trusting order.
        let best_bid = self
            .bids
            .iter()
            .filter_map(BookLevel::price)
            .max();
        let best_ask = self
            .asks
            .iter()
            .filter_map(BookLevel::price)
            .min();
        let bid_depth: Decimal = self
            .bids
            .iter()
            .filter_map(BookLevel::size)
            .sum();
        let ask_depth: Decimal = self
            .asks
            .iter()
            .filter_map(BookLevel::size)
            .sum();
        BookSummary {
            best_bid,
            best_ask,
            bid_depth,
            ask_depth,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ClobRest {
    http: Client,
    clob_host: String,
}

#[derive(Debug, Deserialize)]
struct PriceResp {
    price: serde_json::Value,
}

impl ClobRest {
    pub fn new(cfg: &PolymarketConfig) -> Result<Self> {
        let http = Client::builder()
            .timeout(Duration::from_secs(5))
            .build()?;
        Ok(Self {
            http,
            clob_host: cfg.clob_host.trim_end_matches('/').to_string(),
        })
    }

    pub async fn price(&self, token_id: &str, side: PriceSide) -> Result<Decimal> {
        let url = format!("{}/price", self.clob_host);
        let resp = self
            .http
            .get(&url)
            .query(&[("token_id", token_id), ("side", side.as_str())])
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(BotError::Clob(format!("/price {} {}", status.as_u16(), body)));
        }
        let pr: PriceResp = resp.json().await?;
        let dec = match pr.price {
            serde_json::Value::String(s) => Decimal::from_str(&s)
                .map_err(|e| BotError::parse(format!("/price decimal: {e}")))?,
            serde_json::Value::Number(n) => {
                let f = n
                    .as_f64()
                    .ok_or_else(|| BotError::parse("/price: number not f64"))?;
                Decimal::from_str(&format!("{f}"))
                    .map_err(|e| BotError::parse(format!("/price decimal: {e}")))?
            }
            other => return Err(BotError::parse(format!("/price unexpected: {other:?}"))),
        };
        Ok(dec)
    }

    // ---- Private endpoints (require ClobAuth) ----

    /// Submit a signed order to the CLOB. Returns the order ID on success.
    pub async fn post_order(
        &self,
        auth: &crate::signing::api_auth::ClobAuth,
        order_body: &serde_json::Value,
    ) -> Result<String> {
        let url = format!("{}/orders", self.clob_host);
        let body_str = serde_json::to_string(order_body)?;
        let headers = auth.headers("POST", "/orders", &body_str);
        let resp = self
            .http
            .post(&url)
            .headers(headers)
            .header("Content-Type", "application/json")
            .body(body_str)
            .send()
            .await?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(BotError::Clob(format!("POST /orders {} {}", status.as_u16(), text)));
        }
        let v: serde_json::Value = serde_json::from_str(&text)?;
        let order_id = v
            .get("orderID")
            .or_else(|| v.get("id"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        Ok(order_id)
    }

    /// Poll the status of a submitted order.
    pub async fn get_order(
        &self,
        auth: &crate::signing::api_auth::ClobAuth,
        order_id: &str,
    ) -> Result<serde_json::Value> {
        let path = format!("/orders/{order_id}");
        let url = format!("{}{}", self.clob_host, path);
        let headers = auth.headers("GET", &path, "");
        let resp = self
            .http
            .get(&url)
            .headers(headers)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(BotError::Clob(format!("GET /orders/{order_id} {} {}", status.as_u16(), body)));
        }
        Ok(resp.json().await?)
    }

    /// Cancel an open order.
    pub async fn cancel_order(
        &self,
        auth: &crate::signing::api_auth::ClobAuth,
        order_id: &str,
    ) -> Result<()> {
        let path = format!("/orders/{order_id}");
        let url = format!("{}{}", self.clob_host, path);
        let headers = auth.headers("DELETE", &path, "");
        let resp = self
            .http
            .delete(&url)
            .headers(headers)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(BotError::Clob(format!("DELETE /orders/{order_id} {} {}", status.as_u16(), body)));
        }
        Ok(())
    }

    /// Get USDC balance + allowance for the authenticated user.
    pub async fn balance_allowance(
        &self,
        auth: &crate::signing::api_auth::ClobAuth,
    ) -> Result<(Decimal, Decimal)> {
        let path = "/balance-allowance?asset_type=COLLATERAL";
        let url = format!("{}{}", self.clob_host, path);
        let headers = auth.headers("GET", path, "");
        let resp = self
            .http
            .get(&url)
            .headers(headers)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(BotError::Clob(format!("/balance-allowance {} {}", status.as_u16(), body)));
        }
        let v: serde_json::Value = resp.json().await?;
        let balance = v
            .get("balance")
            .and_then(|b| b.as_str())
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or_default();
        let allowance = v
            .get("allowance")
            .and_then(|a| a.as_str())
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or_default();
        Ok((balance, allowance))
    }

    /// Get open positions for the authenticated user.
    pub async fn positions(
        &self,
        auth: &crate::signing::api_auth::ClobAuth,
    ) -> Result<Vec<serde_json::Value>> {
        let path = "/positions";
        let url = format!("{}{}", self.clob_host, path);
        let headers = auth.headers("GET", path, "");
        let resp = self
            .http
            .get(&url)
            .headers(headers)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(BotError::Clob(format!("/positions {} {}", status.as_u16(), body)));
        }
        Ok(resp.json().await?)
    }

    // ---- Public endpoints ----

    pub async fn book(&self, token_id: &str) -> Result<OrderBook> {
        let url = format!("{}/book", self.clob_host);
        let resp = self
            .http
            .get(&url)
            .query(&[("token_id", token_id)])
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(BotError::Clob(format!("/book {} {}", status.as_u16(), body)));
        }
        let book: OrderBook = resp.json().await?;
        Ok(book)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn summarize_picks_extrema() {
        let b = OrderBook {
            bids: vec![
                BookLevel { price: "0.24".into(), size: "10".into() },
                BookLevel { price: "0.25".into(), size: "5".into() },
            ],
            asks: vec![
                BookLevel { price: "0.27".into(), size: "8".into() },
                BookLevel { price: "0.28".into(), size: "12".into() },
            ],
        };
        let s = b.summarize();
        assert_eq!(s.best_bid.unwrap(), dec!(0.25));
        assert_eq!(s.best_ask.unwrap(), dec!(0.27));
        assert_eq!(s.bid_depth, dec!(15));
        assert_eq!(s.ask_depth, dec!(20));
    }
}
