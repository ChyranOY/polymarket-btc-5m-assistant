use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Paper,
    Live,
}

impl Mode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Mode::Paper => "paper",
            Mode::Live => "live",
        }
    }
}

impl std::str::FromStr for Mode {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "paper" => Ok(Mode::Paper),
            "live" => Ok(Mode::Live),
            other => Err(format!("unknown mode: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Side {
    Up,
    Down,
}

impl Side {
    pub fn opposite(self) -> Self {
        match self {
            Side::Up => Side::Down,
            Side::Down => Side::Up,
        }
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            Side::Up => "UP",
            Side::Down => "DOWN",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum TradeStatus {
    Open,
    Closed,
}

/// Snapshot of a Polymarket 5m market at a single tick.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketSnapshot {
    pub market_slug: String,
    pub up_token_id: String,
    pub down_token_id: String,
    pub end_date: DateTime<Utc>,
    pub up_price: Decimal,
    pub down_price: Decimal,
    /// Best ask (price to pay) for each side — what we'd pay to buy.
    pub up_ask: Option<Decimal>,
    pub down_ask: Option<Decimal>,
    /// Best bid (price to receive) for each side — what we'd collect on sell.
    pub up_bid: Option<Decimal>,
    pub down_bid: Option<Decimal>,
    pub fetched_at: DateTime<Utc>,
}

impl MarketSnapshot {
    pub fn time_left(&self, now: DateTime<Utc>) -> chrono::Duration {
        self.end_date - now
    }
    pub fn time_left_sec(&self, now: DateTime<Utc>) -> i64 {
        self.time_left(now).num_seconds()
    }
    pub fn time_left_minutes(&self, now: DateTime<Utc>) -> f64 {
        (self.end_date - now).num_milliseconds() as f64 / 60_000.0
    }
    pub fn ask_for(&self, side: Side) -> Option<Decimal> {
        match side {
            Side::Up => self.up_ask,
            Side::Down => self.down_ask,
        }
    }
    pub fn bid_for(&self, side: Side) -> Option<Decimal> {
        match side {
            Side::Up => self.up_bid,
            Side::Down => self.down_bid,
        }
    }
    pub fn price_for(&self, side: Side) -> Decimal {
        match side {
            Side::Up => self.up_price,
            Side::Down => self.down_price,
        }
    }
    pub fn token_id_for(&self, side: Side) -> &str {
        match side {
            Side::Up => &self.up_token_id,
            Side::Down => &self.down_token_id,
        }
    }
}

/// An open position the engine currently holds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenPosition {
    pub id: String,
    pub side: Side,
    pub entry_price: Decimal,
    pub shares: Decimal,
    pub contract_size: Decimal, // notional = shares * entry_price
    pub entry_time: DateTime<Utc>,
    pub market_slug: String,
    pub market_end_date: DateTime<Utc>,
    pub token_id: String,
    pub mode: Mode,
    pub max_unrealized_pnl: Decimal,
    pub min_unrealized_pnl: Decimal,
}

impl OpenPosition {
    pub fn unrealized_pnl(&self, current_price: Decimal) -> Decimal {
        (current_price - self.entry_price) * self.shares
    }

    pub fn update_mfe_mae(&mut self, current_price: Decimal) {
        let pnl = self.unrealized_pnl(current_price);
        if pnl > self.max_unrealized_pnl {
            self.max_unrealized_pnl = pnl;
        }
        if pnl < self.min_unrealized_pnl {
            self.min_unrealized_pnl = pnl;
        }
    }
}

/// A completed trade record, matching the dashboard's Supabase `trades` schema
/// (camelCase column names).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trade {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    pub status: TradeStatus,
    pub side: Side,
    pub mode: Mode,

    // Entry
    #[serde(rename = "entryPrice")]
    pub entry_price: Decimal,
    pub shares: Decimal,
    #[serde(rename = "contractSize")]
    pub contract_size: Decimal,
    #[serde(rename = "entryTime")]
    pub entry_time: DateTime<Utc>,
    #[serde(rename = "marketSlug")]
    pub market_slug: String,
    #[serde(rename = "entryPhase", skip_serializing_if = "Option::is_none")]
    pub entry_phase: Option<String>,

    // Exit (None while open)
    #[serde(rename = "exitPrice", skip_serializing_if = "Option::is_none")]
    pub exit_price: Option<Decimal>,
    #[serde(rename = "exitTime", skip_serializing_if = "Option::is_none")]
    pub exit_time: Option<DateTime<Utc>>,
    #[serde(rename = "exitReason", skip_serializing_if = "Option::is_none")]
    pub exit_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pnl: Option<Decimal>,

    // MFE / MAE
    #[serde(rename = "maxUnrealizedPnl")]
    pub max_unrealized_pnl: Decimal,
    #[serde(rename = "minUnrealizedPnl")]
    pub min_unrealized_pnl: Decimal,

    // Metadata
    #[serde(rename = "entryGateSnapshot", skip_serializing_if = "Option::is_none")]
    pub entry_gate_snapshot: Option<String>,
    #[serde(rename = "extraJson", skip_serializing_if = "Option::is_none")]
    pub extra_json: Option<String>,
    #[serde(rename = "createdAt")]
    pub created_at: DateTime<Utc>,
    #[serde(rename = "updatedAt")]
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct Balance {
    pub available_usd: Decimal,
    pub locked_usd: Decimal,
}

impl Balance {
    pub fn total(&self) -> Decimal {
        self.available_usd + self.locked_usd
    }
}
