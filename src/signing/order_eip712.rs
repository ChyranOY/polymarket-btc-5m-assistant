//! EIP-712 typed-data signing for Polymarket CTF Exchange V2 orders.
//!
//! Domain: { name: "Polymarket CTF Exchange", version: "2", chainId: 137,
//!           verifyingContract: 0xE111180000d2663C0091e4f400237545B87B996B }
//!
//! V2 Order struct (11 fields): salt, maker, signer, tokenId, makerAmount,
//! takerAmount, side, signatureType, timestamp (ms), metadata (bytes32),
//! builder (bytes32).
//!
//! V1 fields removed: taker, expiration, nonce, feeRateBps (fees are now
//! protocol-set at match time; see docs.polymarket.com/v2-migration).

use alloy::primitives::{Address, FixedBytes, U256, hex};
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::Signer;
use alloy::sol;
use alloy::sol_types::{Eip712Domain, SolStruct};
use rust_decimal::Decimal;
use std::str::FromStr;

pub const CTF_EXCHANGE_V2_POLYGON: &str = "0xE111180000d2663C0091e4f400237545B87B996B";

sol! {
    #[derive(Debug)]
    struct Order {
        uint256 salt;
        address maker;
        address signer;
        uint256 tokenId;
        uint256 makerAmount;
        uint256 takerAmount;
        uint8 side;
        uint8 signatureType;
        uint256 timestamp;
        bytes32 metadata;
        bytes32 builder;
    }
}

fn domain(chain_id: u64) -> Eip712Domain {
    Eip712Domain {
        name: Some("Polymarket CTF Exchange".into()),
        version: Some("2".into()),
        chain_id: Some(U256::from(chain_id)),
        verifying_contract: Some(
            Address::from_str(CTF_EXCHANGE_V2_POLYGON).expect("valid address"),
        ),
        salt: None,
    }
}

/// Parameters for building a signed V2 order.
pub struct OrderParams {
    pub salt: u128,
    pub maker: String,
    pub signer_addr: String,
    pub token_id: String,
    pub maker_amount: Decimal,
    pub taker_amount: Decimal,
    pub side: u8,         // 0 = BUY, 1 = SELL
    pub chain_id: u64,
    pub signature_type: u8,
    /// Milliseconds since epoch; included in the signed struct as `timestamp`.
    pub timestamp_ms: u64,
    /// 32-byte metadata blob; zero for standard orders.
    pub metadata: [u8; 32],
    /// 32-byte builder code; zero when no builder attribution.
    pub builder: [u8; 32],
}

fn dec_to_u256(d: Decimal) -> U256 {
    // Amounts are in raw units (already scaled by caller — e.g., pUSD * 1e6).
    let s = d.trunc().to_string();
    U256::from_str(&s).unwrap_or(U256::ZERO)
}

/// Sign a V2 order and return the hex-encoded signature.
pub async fn sign_order(
    signer: &PrivateKeySigner,
    params: &OrderParams,
) -> Result<String, String> {
    let order = Order {
        salt: U256::from(params.salt),
        maker: Address::from_str(&params.maker).map_err(|e| format!("maker: {e}"))?,
        signer: Address::from_str(&params.signer_addr).map_err(|e| format!("signer: {e}"))?,
        tokenId: U256::from_str(&params.token_id).map_err(|e| format!("tokenId: {e}"))?,
        makerAmount: dec_to_u256(params.maker_amount),
        takerAmount: dec_to_u256(params.taker_amount),
        side: params.side,
        signatureType: params.signature_type,
        timestamp: U256::from(params.timestamp_ms),
        metadata: FixedBytes::<32>::from(params.metadata),
        builder: FixedBytes::<32>::from(params.builder),
    };

    let dom = domain(params.chain_id);
    let hash = order.eip712_signing_hash(&dom);
    let sig = signer
        .sign_hash(&hash)
        .await
        .map_err(|e| format!("sign_hash: {e}"))?;
    Ok(format!("0x{}", hex::encode(sig.as_bytes())))
}

/// Parse a 0x-prefixed 32-byte hex string into a [u8; 32].
/// Returns all-zero for empty input.
pub fn parse_bytes32(s: &str) -> Result<[u8; 32], String> {
    let s = s.trim();
    if s.is_empty() {
        return Ok([0u8; 32]);
    }
    let stripped = s.strip_prefix("0x").unwrap_or(s);
    let bytes = hex::decode(stripped).map_err(|e| format!("hex decode: {e}"))?;
    if bytes.len() != 32 {
        return Err(format!("expected 32 bytes, got {}", bytes.len()));
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
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
            chain_id: 137,
            signature_type: 0,
            timestamp_ms: 1_700_000_000_000,
            metadata: [0u8; 32],
            builder: [0u8; 32],
        };
        let sig = sign_order(&signer, &params).await.unwrap();
        assert!(sig.starts_with("0x"));
        assert!(sig.len() > 100); // 65 bytes = 130 hex chars + "0x"
    }

    #[test]
    fn parse_bytes32_roundtrip() {
        assert_eq!(parse_bytes32("").unwrap(), [0u8; 32]);
        let full = "0x00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let b = parse_bytes32(full).unwrap();
        assert_eq!(b[0], 0x00);
        assert_eq!(b[1], 0x11);
        assert_eq!(b[31], 0xff);
        assert!(parse_bytes32("0x1234").is_err());
    }
}
