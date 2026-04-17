use crate::error::{BotError, Result};
use crate::model::Mode;
use rust_decimal::Decimal;
use std::env;
use std::str::FromStr;

fn env_str(key: &str) -> Result<String> {
    env::var(key).map_err(|_| BotError::cfg(format!("missing env {key}")))
}

fn env_opt(key: &str) -> Option<String> {
    env::var(key).ok().filter(|s| !s.is_empty())
}

fn env_or<T: FromStr>(key: &str, default: T) -> Result<T>
where
    T::Err: std::fmt::Display,
{
    match env::var(key) {
        Ok(v) if !v.is_empty() => v
            .parse::<T>()
            .map_err(|e| BotError::cfg(format!("bad {key}: {e}"))),
        _ => Ok(default),
    }
}

fn env_dec(key: &str, default: &str) -> Result<Decimal> {
    let raw = env::var(key).unwrap_or_else(|_| default.to_string());
    Decimal::from_str(&raw).map_err(|e| BotError::cfg(format!("bad {key}: {e}")))
}

#[derive(Debug, Clone)]
pub struct PolymarketConfig {
    pub series_slug: String,
    pub series_id: Option<String>,
    pub gamma_url: String,
    pub clob_host: String,
    pub ws_market_url: String,
    pub chain_id: u64,
    pub signature_type: u8,
}

#[derive(Debug, Clone)]
pub struct LiveCreds {
    pub private_key: String,
    pub funder_address: String,
    pub api_key: String,
    pub api_secret: String,
    pub passphrase: String,
    pub polygon_rpc_url: Option<String>,
}

#[derive(Debug, Clone)]
pub struct KellyConfig {
    pub enabled: bool,
    pub estimated_prob: Decimal,
    pub fraction: Decimal,
    pub max_pct: Decimal,
    pub edge_capture: Decimal,
}

#[derive(Debug, Clone)]
pub struct TradingConfig {
    pub mode: Mode,
    pub enabled_on_boot: bool,
    pub stake_pct: Decimal,
    pub min_stake_usd: Decimal,
    pub max_stake_usd: Decimal,
    pub starting_balance: Decimal,
    pub stop_loss_pct: Decimal,
    pub cheap_side_min: Decimal,
    pub cheap_side_max: Decimal,
    pub max_entry_spread: Decimal,
    pub time_left_min_minutes: f64,
    pub trading_hours_start_pst: u32,
    pub trading_hours_end_pst: u32,
    pub allow_weekends: bool,
    pub paper_fee_rate: Decimal,
    pub cooldown_after_exit_sec: u32,
    pub warmup_ticks: u32,
    pub kelly: KellyConfig,
}

#[derive(Debug, Clone)]
pub struct SupabaseConfig {
    pub url: Option<String>,
    pub service_role_key: Option<String>,
}

impl SupabaseConfig {
    pub fn is_configured(&self) -> bool {
        self.url.is_some() && self.service_role_key.is_some()
    }
}

#[derive(Debug, Clone)]
pub struct HttpConfig {
    pub port: u16,
    pub control_token: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub polymarket: PolymarketConfig,
    pub live_creds: Option<LiveCreds>,
    pub trading: TradingConfig,
    pub supabase: SupabaseConfig,
    pub http: HttpConfig,
    pub paper_ledger_path: String,
}

impl AppConfig {
    pub fn from_env() -> Result<Self> {
        let polymarket = PolymarketConfig {
            series_slug: env_or("POLYMARKET_SERIES_SLUG", "btc-up-or-down-5m".to_string())?,
            series_id: env_opt("POLYMARKET_SERIES_ID").or(Some("10684".to_string())),
            gamma_url: env_or(
                "POLYMARKET_GAMMA_URL",
                "https://gamma-api.polymarket.com".to_string(),
            )?,
            clob_host: env_or("CLOB_HOST", "https://clob.polymarket.com".to_string())?,
            ws_market_url: env_or(
                "POLYMARKET_WS_MARKET_URL",
                "wss://ws-subscriptions-clob.polymarket.com/ws/market".to_string(),
            )?,
            chain_id: env_or("CHAIN_ID", 137u64)?,
            signature_type: env_or("SIGNATURE_TYPE", 2u8)?,
        };

        let live_creds = match (
            env_opt("PRIVATE_KEY"),
            env_opt("FUNDER_ADDRESS"),
            env_opt("CLOB_API_KEY"),
            env_opt("CLOB_SECRET"),
            env_opt("CLOB_PASSPHRASE"),
        ) {
            (Some(pk), Some(fa), Some(ak), Some(sk), Some(pp)) => Some(LiveCreds {
                private_key: pk,
                funder_address: fa,
                api_key: ak,
                api_secret: sk,
                passphrase: pp,
                polygon_rpc_url: env_opt("POLYGON_RPC_URL"),
            }),
            _ => None,
        };

        let trading = TradingConfig {
            mode: env_or("TRADING_MODE", Mode::Paper)?,
            enabled_on_boot: env_or("TRADING_ENABLED_ON_BOOT", false)?,
            stake_pct: env_dec("STAKE_PCT", "0.08")?,
            min_stake_usd: env_dec("MIN_STAKE_USD", "25")?,
            max_stake_usd: env_dec("MAX_STAKE_USD", "250")?,
            starting_balance: env_dec("STARTING_BALANCE", "1000")?,
            stop_loss_pct: env_dec("STOP_LOSS_PCT", "0.30")?,
            cheap_side_min: env_dec("CHEAP_SIDE_MIN", "0.15")?,
            cheap_side_max: env_dec("CHEAP_SIDE_MAX", "0.45")?,
            max_entry_spread: env_dec("MAX_ENTRY_SPREAD", "0.04")?,
            time_left_min_minutes: env_or("TIME_LEFT_MIN_MINUTES", 1.5f64)?,
            trading_hours_start_pst: env_or("TRADING_HOURS_START_PST", 6u32)?,
            trading_hours_end_pst: env_or("TRADING_HOURS_END_PST", 17u32)?,
            allow_weekends: env_or("ALLOW_WEEKENDS", false)?,
            paper_fee_rate: env_dec("PAPER_FEE_RATE", "0.02")?,
            cooldown_after_exit_sec: env_or("COOLDOWN_AFTER_EXIT_SEC", 300u32)?, // 5 min
            warmup_ticks: env_or("WARMUP_SECS", 30u32)?,
            kelly: KellyConfig {
                enabled: env_or("KELLY_ENABLED", false)?,
                estimated_prob: env_dec("ESTIMATED_PROB", "0.50")?,
                fraction: env_dec("KELLY_FRACTION", "0.25")?,
                max_pct: env_dec("KELLY_MAX_PCT", "0.08")?,
                edge_capture: env_dec("LIMIT_EDGE_CAPTURE", "0.40")?,
            },
        };

        let supabase = SupabaseConfig {
            url: env_opt("SUPABASE_URL"),
            service_role_key: env_opt("SUPABASE_SERVICE_ROLE_KEY"),
        };

        // DO App Platform injects `PORT`; fall back to `HTTP_PORT` for local dev.
        let port: u16 = match env_opt("PORT").or_else(|| env_opt("HTTP_PORT")) {
            Some(s) => s
                .parse()
                .map_err(|e| BotError::cfg(format!("bad PORT/HTTP_PORT: {e}")))?,
            None => 3000,
        };
        let http = HttpConfig {
            port,
            control_token: env_opt("CONTROL_TOKEN"),
        };

        let paper_ledger_path = env_or(
            "PAPER_LEDGER_PATH",
            "./live_trading/paper_ledger.json".to_string(),
        )?;

        // Sanity: if mode=live require creds.
        if matches!(trading.mode, Mode::Live) && live_creds.is_none() {
            return Err(BotError::cfg(
                "TRADING_MODE=live but live credentials missing (PRIVATE_KEY, FUNDER_ADDRESS, CLOB_API_KEY, CLOB_SECRET, CLOB_PASSPHRASE)",
            ));
        }

        Ok(Self {
            polymarket,
            live_creds,
            trading,
            supabase,
            http,
            paper_ledger_path,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_mode() {
        assert_eq!(Mode::from_str("paper").unwrap(), Mode::Paper);
        assert_eq!(Mode::from_str("LIVE").unwrap(), Mode::Live);
        assert!(Mode::from_str("nope").is_err());
    }
}
