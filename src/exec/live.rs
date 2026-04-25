use super::{CloseRequest, CloseResult, Executor, OpenRequest, OpenResult};
use crate::config::LiveCreds;
use crate::data::clob_rest::ClobRest;
use crate::error::{BotError, Result};
use crate::model::{Balance, Mode, OpenPosition, Side};
use crate::signing::api_auth::ClobAuth;
use crate::signing::order_eip712::{parse_bytes32, sign_order, OrderParams};
use alloy::signers::local::PrivateKeySigner;
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
    signer: PrivateKeySigner,
    clob: Arc<ClobRest>,
    funder_address: String,
    chain_id: u64,
    polygon_rpc_url: Option<String>,
    /// V2 builder code (bytes32). Zero = no builder attribution.
    builder: [u8; 32],
}

impl LiveExecutor {
    pub fn new(creds: &LiveCreds, clob: Arc<ClobRest>, chain_id: u64) -> Result<Self> {
        let builder = parse_bytes32(creds.builder_code.as_deref().unwrap_or(""))
            .map_err(|e| BotError::cfg(format!("BUILDER_CODE: {e}")))?;
        let auth = ClobAuth::new(
            &creds.funder_address,
            &creds.api_key,
            &creds.api_secret,
            &creds.passphrase,
        )
        .map_err(|e| BotError::cfg(format!("ClobAuth: {e}")))?;

        let signer: PrivateKeySigner = creds
            .private_key
            .parse()
            .map_err(|e| BotError::cfg(format!("PrivateKeySigner: {e}")))?;

        Ok(Self {
            auth,
            signer,
            clob,
            funder_address: creds.funder_address.clone(),
            chain_id,
            polygon_rpc_url: creds.polygon_rpc_url.clone(),
            builder,
        })
    }

    async fn build_signed_order(
        &self,
        token_id: &str,
        price: Decimal,
        size: Decimal,
        side_str: &str,
    ) -> Result<serde_json::Value> {
        let side_u8: u8 = if side_str == "BUY" { 0 } else { 1 };
        let salt = Uuid::new_v4().as_u128();

        let (maker_amount, taker_amount) = if side_str == "BUY" {
            let usdc = (size * price).round_dp(6);
            (to_raw_units(usdc), to_raw_units(size))
        } else {
            let usdc = (size * price).round_dp(6);
            (to_raw_units(size), to_raw_units(usdc))
        };

        let timestamp_ms = Utc::now().timestamp_millis().max(0) as u64;
        let metadata = [0u8; 32];

        let params = OrderParams {
            salt,
            maker: self.funder_address.clone(),
            signer_addr: self.funder_address.clone(),
            token_id: token_id.to_string(),
            maker_amount: Decimal::from_str_exact(&maker_amount).unwrap_or_default(),
            taker_amount: Decimal::from_str_exact(&taker_amount).unwrap_or_default(),
            side: side_u8,
            chain_id: self.chain_id,
            signature_type: 0,
            timestamp_ms,
            metadata,
            builder: self.builder,
        };

        let signature = sign_order(&self.signer, &params)
            .await
            .map_err(|e| BotError::Signing(e))?;

        let metadata_hex = format!("0x{}", alloy::primitives::hex::encode(metadata));
        let builder_hex = format!("0x{}", alloy::primitives::hex::encode(self.builder));

        Ok(json!({
            "order": {
                "salt": salt.to_string(),
                "maker": self.funder_address,
                "signer": self.funder_address,
                "tokenId": token_id,
                "makerAmount": maker_amount,
                "takerAmount": taker_amount,
                "side": side_str,
                "signatureType": 0,
                "timestamp": timestamp_ms.to_string(),
                "metadata": metadata_hex,
                "builder": builder_hex,
                "signature": signature
            },
            "orderType": "GTC"
        }))
    }

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

fn to_raw_units(amount: Decimal) -> String {
    let scaled = amount * Decimal::from(1_000_000u64);
    scaled.trunc().to_string()
}

use rust_decimal::prelude::FromStr as _;

#[async_trait]
impl Executor for LiveExecutor {
    async fn open_position(&self, req: OpenRequest) -> Result<OpenResult> {
        let price = req.limit_price.unwrap_or(req.quoted_price);
        let side_str = "BUY";
        let body = self
            .build_signed_order(&req.token_id, price, req.shares, side_str)
            .await?;
        tracing::info!(
            token = %req.token_id,
            price = %price,
            shares = %req.shares,
            "live: submitting signed order"
        );

        let order_id = self.clob.post_order(&self.auth, &body).await?;
        tracing::debug!(order_id = %order_id, "live: order accepted, polling for fill");

        let fill_status = self.poll_until_filled(&order_id).await?;
        let fill_price = fill_status
            .get("price")
            .and_then(|p| p.as_str())
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or(price);
        let filled_size = fill_status
            .get("size_matched")
            .or_else(|| fill_status.get("sizeMatched"))
            .and_then(|s| s.as_str())
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or(req.shares);

        let notional = fill_price * filled_size;
        let fees = notional * dec!(0.01);

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
            entry_strategy: None,
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
        let body = self
            .build_signed_order(&req.position.token_id, price, req.position.shares, "SELL")
            .await?;
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
            .and_then(|s| Decimal::from_str(s).ok())
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
            locked_usd: dec!(0),
        })
    }

    async fn redeem_winnings(&self, token_id: &str, shares: Decimal) -> Result<Decimal> {
        let Some(rpc_url) = &self.polygon_rpc_url else {
            tracing::warn!(
                token_id,
                shares = %shares,
                "live: redeem skipped — POLYGON_RPC_URL not configured"
            );
            return Ok(shares); // return theoretical value
        };

        tracing::info!(token_id, shares = %shares, "live: checking redeemable positions");

        // Query the Polymarket data-api for redeemable positions.
        let redeemable = super::redeem::fetch_redeemable_positions(&self.funder_address)
            .await
            .map_err(|e| BotError::other(format!("fetch redeemable: {e}")))?;

        if redeemable.is_empty() {
            tracing::debug!("live: no redeemable positions found");
            return Ok(Decimal::ZERO);
        }

        let mut total_redeemed = Decimal::ZERO;
        for pos in &redeemable {
            tracing::info!(
                condition_id = %pos.condition_id,
                size = %pos.size,
                "live: redeeming position"
            );
            match super::redeem::redeem_position_onchain(
                rpc_url,
                &self.signer,
                &pos.condition_id,
            )
            .await
            {
                Ok(tx_hash) => {
                    tracing::info!(tx_hash, size = %pos.size, "live: redeemed");
                    total_redeemed += pos.size;
                }
                Err(e) => {
                    tracing::error!(
                        condition_id = %pos.condition_id,
                        err = %e,
                        "live: redeem tx failed"
                    );
                }
            }
        }
        Ok(total_redeemed)
    }

    fn mode(&self) -> Mode {
        Mode::Live
    }
}

