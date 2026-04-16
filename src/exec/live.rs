use super::{CloseRequest, CloseResult, Executor, OpenRequest, OpenResult};
use crate::config::LiveCreds;
use crate::data::clob_rest::ClobRest;
use crate::error::{BotError, Result};
use crate::model::{Balance, Mode, OpenPosition, Side};
use crate::signing::api_auth::ClobAuth;
use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

const POLL_INTERVAL: Duration = Duration::from_millis(500);
const POLL_TIMEOUT: Duration = Duration::from_secs(30);

pub struct LiveExecutor {
    auth: ClobAuth,
    clob: Arc<ClobRest>,
    funder_address: String,
}

impl LiveExecutor {
    pub fn new(creds: &LiveCreds, clob: Arc<ClobRest>) -> Result<Self> {
        let auth = ClobAuth::new(
            &creds.funder_address,
            &creds.api_key,
            &creds.api_secret,
            &creds.passphrase,
        )
        .map_err(|e| BotError::cfg(format!("ClobAuth: {e}")))?;
        Ok(Self {
            auth,
            clob,
            funder_address: creds.funder_address.clone(),
        })
    }

    /// Build the order JSON body for POST /orders.
    /// NOTE: This currently builds an unsigned order. Full EIP-712 signing (order_eip712.rs)
    /// is needed before this will work on mainnet. The CLOB will reject unsigned orders.
    fn build_order_body(
        &self,
        token_id: &str,
        price: Decimal,
        size: Decimal,
        side: &str,
    ) -> serde_json::Value {
        let maker_amount: String;
        let taker_amount: String;

        if side == "BUY" {
            // Buying: maker puts up USDC (size), taker delivers tokens
            let usdc_amount = (size * price).round_dp(6);
            maker_amount = to_raw_units(usdc_amount);
            taker_amount = to_raw_units(size);
        } else {
            // Selling: maker puts up tokens (size), taker delivers USDC
            maker_amount = to_raw_units(size);
            let usdc_amount = (size * price).round_dp(6);
            taker_amount = to_raw_units(usdc_amount);
        }

        json!({
            "order": {
                "salt": Uuid::new_v4().as_u128().to_string(),
                "maker": self.funder_address,
                "signer": self.funder_address,
                "taker": "0x0000000000000000000000000000000000000000",
                "tokenId": token_id,
                "makerAmount": maker_amount,
                "takerAmount": taker_amount,
                "expiration": "0",
                "nonce": "0",
                "feeRateBps": "100",
                "side": side,
                "signatureType": 0,
                "signature": "0x"  // TODO: real EIP-712 signature from order_eip712.rs
            },
            "orderType": "GTC"
        })
    }

    /// Poll GET /orders/{id} until filled, cancelled, or timeout.
    async fn poll_until_filled(&self, order_id: &str) -> Result<serde_json::Value> {
        let deadline = tokio::time::Instant::now() + POLL_TIMEOUT;
        loop {
            let status = self.clob.get_order(&self.auth, order_id).await?;
            let state = status
                .get("status")
                .and_then(|s| s.as_str())
                .unwrap_or("");
            match state {
                "filled" | "matched" => return Ok(status),
                "cancelled" | "expired" => {
                    return Err(BotError::Clob(format!("order {order_id} {state}")));
                }
                _ => {}
            }
            if tokio::time::Instant::now() >= deadline {
                tracing::warn!(order_id, "live: poll timeout, cancelling");
                let _ = self.clob.cancel_order(&self.auth, order_id).await;
                return Err(BotError::Clob(format!("order {order_id} fill timeout")));
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

/// Convert a decimal amount to raw 6-decimal-place units (USDC has 6 decimals).
fn to_raw_units(amount: Decimal) -> String {
    let scaled = amount * Decimal::from(1_000_000u64);
    scaled.trunc().to_string()
}

#[async_trait]
impl Executor for LiveExecutor {
    async fn open_position(&self, req: OpenRequest) -> Result<OpenResult> {
        let price = req.limit_price.unwrap_or(req.quoted_price);
        let side_str = match req.side {
            Side::Up | Side::Down => "BUY",
        };
        let body = self.build_order_body(&req.token_id, price, req.shares, side_str);
        tracing::info!(
            token = %req.token_id,
            price = %price,
            shares = %req.shares,
            "live: submitting order"
        );

        let order_id = self.clob.post_order(&self.auth, &body).await?;
        tracing::debug!(order_id = %order_id, "live: order accepted, polling for fill");

        let fill_status = self.poll_until_filled(&order_id).await?;
        let fill_price = fill_status
            .get("price")
            .and_then(|p| p.as_str())
            .and_then(|s| s.parse::<Decimal>().ok())
            .unwrap_or(price);
        let filled_size = fill_status
            .get("size_matched")
            .or_else(|| fill_status.get("sizeMatched"))
            .and_then(|s| s.as_str())
            .and_then(|s| s.parse::<Decimal>().ok())
            .unwrap_or(req.shares);

        let notional = fill_price * filled_size;
        let fees = notional * dec!(0.01); // 1% taker fee estimate

        let now = Utc::now();
        let position = OpenPosition {
            id: order_id.clone(),
            side: req.side,
            entry_price: fill_price,
            shares: filled_size,
            contract_size: notional,
            entry_time: now,
            market_slug: req.market_slug,
            market_end_date: req.market_end_date,
            token_id: req.token_id,
            mode: Mode::Live,
            max_unrealized_pnl: dec!(0),
            min_unrealized_pnl: dec!(0),
        };

        tracing::info!(
            order_id = %order_id,
            fill_price = %fill_price,
            filled_size = %filled_size,
            "live: position opened"
        );

        Ok(OpenResult {
            position,
            fill_price,
            fees_paid: fees,
        })
    }

    async fn close_position(&self, req: CloseRequest) -> Result<CloseResult> {
        let price = req.mark_price;
        let body = self.build_order_body(&req.position.token_id, price, req.position.shares, "SELL");
        tracing::info!(
            token = %req.position.token_id,
            price = %price,
            shares = %req.position.shares,
            "live: submitting sell order"
        );

        let order_id = self.clob.post_order(&self.auth, &body).await?;
        let fill_status = self.poll_until_filled(&order_id).await?;
        let exit_price = fill_status
            .get("price")
            .and_then(|p| p.as_str())
            .and_then(|s| s.parse::<Decimal>().ok())
            .unwrap_or(price);

        let pnl = (exit_price - req.position.entry_price) * req.position.shares;
        let fees = exit_price * req.position.shares * dec!(0.01);

        Ok(CloseResult {
            exit_price,
            exit_time: Utc::now(),
            pnl: pnl - fees,
            fees_paid: fees,
        })
    }

    async fn balance(&self) -> Result<Balance> {
        let (bal, _allowance) = self.clob.balance_allowance(&self.auth).await?;
        Ok(Balance {
            available_usd: bal,
            locked_usd: dec!(0), // TODO: sum open order collateral
        })
    }

    async fn redeem_winnings(&self, _token_id: &str, shares: Decimal) -> Result<Decimal> {
        // TODO: call CTF redeemPositions() via alloy + Polygon RPC.
        // For now, log and return 0 — the balance will reflect the redemption
        // once the on-chain call is implemented.
        tracing::warn!(
            shares = %shares,
            "live: redeem_winnings not yet implemented (needs CTF contract call)"
        );
        Ok(dec!(0))
    }

    fn mode(&self) -> Mode {
        Mode::Live
    }
}
