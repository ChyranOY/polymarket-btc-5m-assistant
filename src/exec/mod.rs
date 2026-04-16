pub mod paper;

use crate::error::Result;
use crate::model::{Balance, Mode, OpenPosition, Side};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

#[derive(Debug, Clone)]
pub struct OpenRequest {
    pub side: Side,
    pub market_slug: String,
    pub market_end_date: DateTime<Utc>,
    pub token_id: String,
    pub quoted_price: Decimal, // ask we saw when deciding to enter
    pub shares: Decimal,
}

#[derive(Debug, Clone)]
pub struct OpenResult {
    pub position: OpenPosition,
    pub fill_price: Decimal,
    pub fees_paid: Decimal,
}

#[derive(Debug, Clone)]
pub struct CloseRequest {
    pub position: OpenPosition,
    pub exit_reason: String,
    pub mark_price: Decimal, // current bid we'd sell into
}

#[derive(Debug, Clone)]
pub struct CloseResult {
    pub exit_price: Decimal,
    pub exit_time: DateTime<Utc>,
    pub pnl: Decimal,
    pub fees_paid: Decimal,
}

#[async_trait]
pub trait Executor: Send + Sync {
    async fn open_position(&self, req: OpenRequest) -> Result<OpenResult>;
    async fn close_position(&self, req: CloseRequest) -> Result<CloseResult>;
    async fn balance(&self) -> Result<Balance>;
    fn mode(&self) -> Mode;
}
