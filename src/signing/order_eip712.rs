//! EIP-712 typed-data signing for Polymarket CTF Exchange orders.
//!
//! Domain: { name: "Polymarket CTF Exchange", version: "1", chainId: 137,
//!           verifyingContract: 0x4bFb41d5B3570DeFd03C39a9A4D8dE6Bd8B8982E }
//!
//! Order struct (12 fields): salt, maker, signer, taker, tokenId, makerAmount,
//! takerAmount, expiration, nonce, feeRateBps, side, signatureType.

use alloy::primitives::{Address, U256, hex};
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::Signer;
use alloy::sol;
use alloy::sol_types::{Eip712Domain, SolStruct};
use rust_decimal::Decimal;
use std::str::FromStr;

const CTF_EXCHANGE_POLYGON: &str = "0x4bFb41d5B3570DeFd03C39a9A4D8dE6Bd8B8982E";

sol! {
    #[derive(Debug)]
    struct Order {
        uint256 salt;
        address maker;
        address signer;
        address taker;
        uint256 tokenId;
        uint256 makerAmount;
        uint256 takerAmount;
        uint256 expiration;
        uint256 nonce;
        uint256 feeRateBps;
        uint8 side;
        uint8 signatureType;
    }
}

fn domain(chain_id: u64) -> Eip712Domain {
    Eip712Domain {
        name: Some("Polymarket CTF Exchange".into()),
        version: Some("1".into()),
        chain_id: Some(U256::from(chain_id)),
        verifying_contract: Some(
            Address::from_str(CTF_EXCHANGE_POLYGON).expect("valid address"),
        ),
        salt: None,
    }
}

/// Parameters for building a signed order.
pub struct OrderParams {
    pub salt: u128,
    pub maker: String,
    pub signer_addr: String,
    pub token_id: String,
    pub maker_amount: Decimal,
    pub taker_amount: Decimal,
    pub side: u8,         // 0 = BUY, 1 = SELL
    pub fee_rate_bps: u64,
    pub chain_id: u64,
    pub signature_type: u8,
}

fn dec_to_u256(d: Decimal) -> U256 {
    // Amounts are in raw units (already scaled by caller — e.g., USDC * 1e6).
    let s = d.trunc().to_string();
    U256::from_str(&s).unwrap_or(U256::ZERO)
}

/// Sign an order and return the hex-encoded signature.
pub async fn sign_order(
    signer: &PrivateKeySigner,
    params: &OrderParams,
) -> Result<String, String> {
    let order = Order {
        salt: U256::from(params.salt),
        maker: Address::from_str(&params.maker).map_err(|e| format!("maker: {e}"))?,
        signer: Address::from_str(&params.signer_addr).map_err(|e| format!("signer: {e}"))?,
        taker: Address::ZERO,
        tokenId: U256::from_str(&params.token_id).map_err(|e| format!("tokenId: {e}"))?,
        makerAmount: dec_to_u256(params.maker_amount),
        takerAmount: dec_to_u256(params.taker_amount),
        expiration: U256::ZERO,
        nonce: U256::ZERO,
        feeRateBps: U256::from(params.fee_rate_bps),
        side: params.side,
        signatureType: params.signature_type,
    };

    let dom = domain(params.chain_id);
    let hash = order.eip712_signing_hash(&dom);
    let sig = signer
        .sign_hash(&hash)
        .await
        .map_err(|e| format!("sign_hash: {e}"))?;
    Ok(format!("0x{}", hex::encode(sig.as_bytes())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sign_order_produces_hex_signature() {
        // Deterministic test key (DO NOT use in production).
        let signer: PrivateKeySigner =
            "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
                .parse()
                .unwrap();
        let params = OrderParams {
            salt: 12345,
            maker: format!("{:?}", signer.address()),
            signer_addr: format!("{:?}", signer.address()),
            token_id: "1234567890".to_string(),
            maker_amount: Decimal::from(1_000_000u64),
            taker_amount: Decimal::from(4_000_000u64),
            side: 0,
            fee_rate_bps: 100,
            chain_id: 137,
            signature_type: 0,
        };
        let sig = sign_order(&signer, &params).await.unwrap();
        assert!(sig.starts_with("0x"));
        assert!(sig.len() > 100); // 65 bytes = 130 hex chars + "0x"
    }
}
