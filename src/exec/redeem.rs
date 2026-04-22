//! On-chain CTF token redemption for Polymarket winning positions.
//!
//! After a binary market settles, winning conditional tokens can be redeemed for
//! $1 of pUSD each via the ConditionalTokens contract on Polygon.
//!
//! CTF contract: 0x4D97DCd97eC945f40cF65F87097ACe5EA0476045
//! pUSD (V2 collateral, Polygon): 0xC011a7E12a19f7B1f670d46F03B03f3342E82DFB
//!
//! Flow:
//! 1. Query Polymarket data-api for redeemable positions
//! 2. For each, call CTF.redeemPositions(collateralToken, parentCollectionId, conditionId, [1,2])
//! 3. Wait for transaction receipt

use alloy::primitives::{Address, Bytes, FixedBytes, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::signers::local::PrivateKeySigner;
use alloy::sol;
use rust_decimal::Decimal;
use std::str::FromStr;

const CTF_ADDRESS: &str = "0x4D97DCd97eC945f40cF65F87097ACe5EA0476045";
/// Polymarket V2 collateral (pUSD). Replaced USDC.e `0x2791…84174` at the
/// 2026-04-28 V2 cutover — CTF positions are now denominated in pUSD.
const PUSD_ADDRESS: &str = "0xC011a7E12a19f7B1f670d46F03B03f3342E82DFB";

sol! {
    #[derive(Debug)]
    interface ICTF {
        function redeemPositions(
            address collateralToken,
            bytes32 parentCollectionId,
            bytes32 conditionId,
            uint256[] calldata indexSets
        ) external;
    }
}

/// Query the Polymarket data-api for positions that are ready to redeem.
pub async fn fetch_redeemable_positions(
    wallet: &str,
) -> Result<Vec<RedeemablePosition>, String> {
    let url = format!(
        "https://data-api.polymarket.com/positions?user={wallet}&sizeThreshold=0.01&limit=100"
    );
    let resp = reqwest::get(&url)
        .await
        .map_err(|e| format!("data-api fetch: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("data-api {}", resp.status()));
    }
    let body: serde_json::Value = resp.json().await.map_err(|e| format!("parse: {e}"))?;
    let positions = body.as_array().cloned().unwrap_or_default();

    let mut out = Vec::new();
    for p in &positions {
        let redeemable = p
            .get("redeemable")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if !redeemable {
            continue;
        }
        let condition_id = p
            .get("conditionId")
            .or_else(|| p.get("condition_id"))
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let size = p
            .get("size")
            .and_then(|v| v.as_str().or_else(|| v.as_f64().map(|_| "")))
            .and_then(|s| if s.is_empty() { None } else { Decimal::from_str(s).ok() })
            .or_else(|| {
                p.get("size")
                    .and_then(|v| v.as_f64())
                    .and_then(|f| Decimal::from_str(&f.to_string()).ok())
            })
            .unwrap_or_default();
        if !condition_id.is_empty() && size > Decimal::ZERO {
            out.push(RedeemablePosition { condition_id, size });
        }
    }
    Ok(out)
}

#[derive(Debug, Clone)]
pub struct RedeemablePosition {
    pub condition_id: String,
    pub size: Decimal,
}

/// Submit a CTF redeemPositions transaction on Polygon.
pub async fn redeem_position_onchain(
    rpc_url: &str,
    signer: &PrivateKeySigner,
    condition_id: &str,
) -> Result<String, String> {
    let collateral = Address::from_str(PUSD_ADDRESS).unwrap();
    let parent = FixedBytes::<32>::ZERO;
    let cond = FixedBytes::<32>::from_str(condition_id)
        .map_err(|e| format!("conditionId parse: {e}"))?;
    let index_sets = vec![U256::from(1), U256::from(2)];

    // ABI-encode the function call.
    let call = ICTF::redeemPositionsCall {
        collateralToken: collateral,
        parentCollectionId: parent,
        conditionId: cond,
        indexSets: index_sets,
    };
    let calldata = Bytes::from(alloy::sol_types::SolCall::abi_encode(&call));

    let ctf_addr = Address::from_str(CTF_ADDRESS).unwrap();

    // Build provider with signer.
    let wallet = alloy::network::EthereumWallet::from(signer.clone());
    let provider = ProviderBuilder::new()
        .with_recommended_fillers()
        .wallet(wallet)
        .on_http(rpc_url.parse().map_err(|e| format!("rpc url: {e}"))?);

    // Build + send transaction.
    let tx = alloy::rpc::types::TransactionRequest::default()
        .to(ctf_addr)
        .input(calldata.into())
        .gas_limit(300_000);

    let pending = provider
        .send_transaction(tx)
        .await
        .map_err(|e| format!("send_transaction: {e}"))?;
    let tx_hash = format!("{:?}", pending.tx_hash());
    tracing::info!(tx_hash = %tx_hash, condition_id, "CTF redeem tx submitted");

    // Wait for receipt.
    let receipt = pending
        .get_receipt()
        .await
        .map_err(|e| format!("get_receipt: {e}"))?;
    if receipt.status() {
        tracing::info!(tx_hash = %tx_hash, "CTF redeem confirmed");
        Ok(tx_hash)
    } else {
        Err(format!("CTF redeem reverted: {tx_hash}"))
    }
}
