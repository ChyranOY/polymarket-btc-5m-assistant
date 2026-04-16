//! HMAC-SHA256 authentication for Polymarket CLOB private endpoints (L2 auth).
//!
//! Every private request carries five headers:
//!   POLY_ADDRESS    — wallet address (checksummed)
//!   POLY_API_KEY    — from createApiKey / deriveApiKey
//!   POLY_PASSPHRASE — from same
//!   POLY_TIMESTAMP  — unix seconds as string
//!   POLY_SIGNATURE  — HMAC-SHA256(decoded_secret, timestamp + method + path + body),
//!                      base64-encoded then made URL-safe (+ → -, / → _)

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use hmac::{Hmac, Mac};
use reqwest::header::{HeaderMap, HeaderValue};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone)]
pub struct ClobAuth {
    address: String,
    api_key: String,
    secret_decoded: Vec<u8>,
    passphrase: String,
}

impl ClobAuth {
    pub fn new(
        address: impl Into<String>,
        api_key: impl Into<String>,
        api_secret_b64: &str,
        passphrase: impl Into<String>,
    ) -> Result<Self, String> {
        let normalized = api_secret_b64.replace('-', "+").replace('_', "/");
        let decoded = B64
            .decode(&normalized)
            .map_err(|e| format!("api_secret base64 decode: {e}"))?;
        Ok(Self {
            address: address.into(),
            api_key: api_key.into(),
            secret_decoded: decoded,
            passphrase: passphrase.into(),
        })
    }

    /// Build the auth headers for a CLOB private request.
    pub fn headers(&self, method: &str, path: &str, body: &str) -> HeaderMap {
        let timestamp = chrono::Utc::now().timestamp().to_string();
        let message = format!("{timestamp}{method}{path}{body}");
        let signature = self.sign(&message);

        let mut h = HeaderMap::new();
        h.insert("POLY_ADDRESS", val(&self.address));
        h.insert("POLY_API_KEY", val(&self.api_key));
        h.insert("POLY_PASSPHRASE", val(&self.passphrase));
        h.insert("POLY_TIMESTAMP", val(&timestamp));
        h.insert("POLY_SIGNATURE", val(&signature));
        h
    }

    fn sign(&self, message: &str) -> String {
        let mut mac =
            HmacSha256::new_from_slice(&self.secret_decoded).expect("HMAC accepts any key size");
        mac.update(message.as_bytes());
        let raw = mac.finalize().into_bytes();
        let b64 = B64.encode(raw);
        // URL-safe base64.
        b64.replace('+', "-").replace('/', "_")
    }
}

fn val(s: &str) -> HeaderValue {
    HeaderValue::from_str(s).unwrap_or_else(|_| HeaderValue::from_static(""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_sign_is_deterministic() {
        let auth = ClobAuth::new("0xABC", "key", "dGVzdHNlY3JldA==", "pass").unwrap();
        let s1 = auth.sign("hello");
        let s2 = auth.sign("hello");
        assert_eq!(s1, s2);
        assert!(!s1.is_empty());
        assert!(!s1.contains('+'));
        assert!(!s1.contains('/'));
    }

    #[test]
    fn headers_contain_all_five() {
        let auth = ClobAuth::new("0xABC", "mykey", "dGVzdHNlY3JldA==", "mypass").unwrap();
        let h = auth.headers("GET", "/orders", "");
        assert_eq!(h.get("POLY_ADDRESS").unwrap(), "0xABC");
        assert_eq!(h.get("POLY_API_KEY").unwrap(), "mykey");
        assert_eq!(h.get("POLY_PASSPHRASE").unwrap(), "mypass");
        assert!(h.get("POLY_TIMESTAMP").is_some());
        assert!(h.get("POLY_SIGNATURE").is_some());
    }
}
