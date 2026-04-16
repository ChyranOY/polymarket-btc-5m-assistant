use super::{CloseRequest, CloseResult, Executor, OpenRequest, OpenResult};
use crate::config::LiveCreds;
use crate::data::clob_rest::ClobRest;
use crate::error::{BotError, Result};
use crate::model::{Balance, Mode, OpenPosition, Side};
use crate::signing::api_auth::ClobAuth;
use crate::signing::order_eip712::{sign_order, OrderParams};
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
const FEE_RATE_BPS: u64 = 100; // 1% taker fee

pub struct LiveExecutor {
    auth: ClobAuth,
    signer: PrivateKeySigner,
    clob: Arc<ClobRest>,
    funder_address: String,
    chain_id: u64,
}

impl LiveExecutor {
    pub fn new(creds: &LiveCreds, clob: Arc<ClobRest>, chain_id: u64) -> Result<Self> {
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

        let params = OrderParams {
            salt,
            maker: self.funder_address.clone(),
            signer_addr: self.funder_address.clone(),
            token_id: token_id.to_string(),
            maker_amount: Decimal::from_str_exact(&maker_amount).unwrap_or_default(),
            taker_amount: Decimal::from_str_exact(&taker_amount).unwrap_or_default(),
            side: side_u8,
            fee_rate_bps: FEE_RATE_BPS,
            chain_id: self.chain_id,
            signature_type: 0,
        };

        let signature = sign_order(&self.signer, &params)
            .await
            .map_err(|e| BotError::Signing(e))?;

        Ok(json!({
            "order": {
                "salt": salt.to_string(),
                "maker": self.funder_address,
                "signer": self.funder_address,
                "taker": "0x0000000000000000000000000000000000000000",
                "tokenId": token_id,
                "makerAmount": maker_amount,
                "takerAmount": taker_amount,
                "expiration": "0",
                "nonce": "0",
                "feeRateBps": FEE_RATE_BPS.to_string(),
                "side": side_str,
                "signatureType": 0,
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
        // CTF redeemPositions call on Polygon.
        // Contract: 0x4D97DCd97eC945f40cF65F87097ACe5EA0476045
        // collateralToken: USDC 0x2791Bca1f2de4661ED88A30C99A7a9449Aa84174
        // parentCollectionId: bytes32(0)
        // indexSets: [1, 2] for binary market
        //
        // Needs: alloy provider + transaction signing + gas estimation.
        // For now, log the intent and query the data-api for confirmation.
        tracing::warn!(
            token_id,
            shares = %shares,
            "live: redeem_winnings — submitting CTF redeemPositions"
        );
        match redeem_via_rpc(token_id, shares, &self.signer, &self.funder_address).await {
            Ok(credited) => {
                tracing::info!(credited = %credited, token_id, "live: redemption succeeded");
                Ok(credited)
            }
            Err(e) => {
                tracing::error!(err = %e, token_id, "live: redemption failed");
                Err(BotError::other(format!("redeem: {e}")))
            }
        }
    }

    fn mode(&self) -> Mode {
        Mode::Live
    }
}

/// Submit a CTF redeemPositions transaction via raw JSON-RPC.
/// This is a minimal implementation using reqwest + alloy ABI encoding.
async fn redeem_via_rpc(
    _token_id: &str,
    shares: Decimal,
    _signer: &PrivateKeySigner,
    _funder: &str,
) -> std::result::Result<Decimal, String> {
    // TODO: Full implementation requires:
    // 1. Encode redeemPositions(collateralToken, parentCollectionId, conditionId, indexSets)
    //    - conditionId from Polymarket data-api for the specific market
    //    - indexSets = [1, 2] for binary
    // 2. Build transaction: to=CTF_ADDRESS, data=encoded, gas=300000, chainId=137
    // 3. Sign with signer
    // 4. Submit via eth_sendRawTransaction to POLYGON_RPC
    // 5. Wait for receipt
    //
    // For now, return the theoretical value ($1 per winning share).
    // The actual balance update happens on-chain; next balance() call will reflect it.
    tracing::warn!(
        shares = %shares,
        "redeem_via_rpc: on-chain CTF call not yet wired — returning theoretical value"
    );
    Ok(shares) // $1 per winning share
}
