use super::{CloseRequest, CloseResult, Executor, OpenRequest, OpenResult};
use crate::error::{BotError, Result};
use crate::model::{Balance, Mode, OpenPosition};
use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PaperLedger {
    balance: Decimal,
    position: Option<OpenPosition>,
}

impl PaperLedger {
    fn new(starting_balance: Decimal) -> Self {
        Self {
            balance: starting_balance,
            position: None,
        }
    }
}

pub struct PaperExecutor {
    inner: Arc<Mutex<PaperLedger>>,
    ledger_path: PathBuf,
    fee_rate: Decimal,
    slippage_max: Decimal, // uniform [0, slippage_max]
}

impl PaperExecutor {
    pub fn new(
        starting_balance: Decimal,
        fee_rate: Decimal,
        ledger_path: impl Into<PathBuf>,
    ) -> Result<Self> {
        let ledger_path = ledger_path.into();
        let ledger = match std::fs::read_to_string(&ledger_path) {
            Ok(s) => match serde_json::from_str::<PaperLedger>(&s) {
                Ok(l) => {
                    tracing::info!(path = %ledger_path.display(), balance = %l.balance, "paper ledger loaded");
                    l
                }
                Err(e) => {
                    tracing::warn!(err = %e, "paper ledger parse failed; starting fresh");
                    PaperLedger::new(starting_balance)
                }
            },
            Err(_) => {
                tracing::info!(balance = %starting_balance, "paper ledger seeded");
                PaperLedger::new(starting_balance)
            }
        };
        Ok(Self {
            inner: Arc::new(Mutex::new(ledger)),
            ledger_path,
            fee_rate,
            slippage_max: dec!(0.003),
        })
    }

    async fn persist_locked(&self, ledger: &PaperLedger) -> Result<()> {
        if let Some(parent) = self.ledger_path.parent() {
            if !parent.as_os_str().is_empty() {
                tokio::fs::create_dir_all(parent).await.ok();
            }
        }
        let serialized = serde_json::to_string_pretty(ledger)?;
        tokio::fs::write(&self.ledger_path, serialized).await?;
        Ok(())
    }

    fn slippage_entry(&self, price: Decimal) -> Decimal {
        // Uniform 0..=slippage_max, deterministic-but-cheap via nanos.
        let nanos = Utc::now().timestamp_subsec_nanos() as i64;
        let frac = Decimal::from(nanos) / Decimal::from(1_000_000_000i64);
        let slip = self.slippage_max * frac;
        (price * (dec!(1) + slip)).min(dec!(0.999))
    }

    fn slippage_exit(&self, price: Decimal) -> Decimal {
        let nanos = Utc::now().timestamp_subsec_nanos() as i64;
        let frac = Decimal::from(nanos) / Decimal::from(1_000_000_000i64);
        let slip = self.slippage_max * frac;
        (price * (dec!(1) - slip)).max(dec!(0.001))
    }
}

#[async_trait]
impl Executor for PaperExecutor {
    async fn open_position(&self, req: OpenRequest) -> Result<OpenResult> {
        let fill_price = self.slippage_entry(req.quoted_price);
        let notional = fill_price * req.shares;
        let fees = notional * self.fee_rate;
        let total_cost = notional + fees;

        let mut guard = self.inner.lock().await;
        if guard.position.is_some() {
            return Err(BotError::other("paper: open called while position exists"));
        }
        if guard.balance < total_cost {
            return Err(BotError::other(format!(
                "paper: insufficient balance {} < {}",
                guard.balance, total_cost
            )));
        }

        guard.balance -= total_cost;
        let now = Utc::now();
        let position = OpenPosition {
            id: Uuid::new_v4().to_string(),
            side: req.side,
            entry_price: fill_price,
            shares: req.shares,
            contract_size: notional,
            entry_time: now,
            market_slug: req.market_slug.clone(),
            market_end_date: req.market_end_date,
            token_id: req.token_id.clone(),
            mode: Mode::Paper,
            max_unrealized_pnl: dec!(0),
            min_unrealized_pnl: dec!(0),
        };
        guard.position = Some(position.clone());
        self.persist_locked(&guard).await?;
        drop(guard);

        Ok(OpenResult {
            position,
            fill_price,
            fees_paid: fees,
        })
    }

    async fn close_position(&self, req: CloseRequest) -> Result<CloseResult> {
        let exit_price = self.slippage_exit(req.mark_price);
        let proceeds = exit_price * req.position.shares;
        let fees = proceeds * self.fee_rate;
        let net_proceeds = proceeds - fees;

        let pnl = (exit_price - req.position.entry_price) * req.position.shares - fees;

        let mut guard = self.inner.lock().await;
        guard.balance += net_proceeds;
        guard.position = None;
        self.persist_locked(&guard).await?;
        drop(guard);

        Ok(CloseResult {
            exit_price,
            exit_time: Utc::now(),
            pnl,
            fees_paid: fees,
        })
    }

    async fn balance(&self) -> Result<Balance> {
        let guard = self.inner.lock().await;
        Ok(Balance {
            available_usd: guard.balance,
            locked_usd: guard
                .position
                .as_ref()
                .map(|p| p.contract_size)
                .unwrap_or(dec!(0)),
        })
    }

    fn mode(&self) -> Mode {
        Mode::Paper
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::{CloseRequest, OpenRequest};
    use crate::model::Side;
    use chrono::Duration;

    fn tmp_path(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("paper_ledger_{}_{}.json", name, Uuid::new_v4()));
        p
    }

    #[tokio::test]
    async fn open_deducts_and_close_restores_on_flat() {
        let path = tmp_path("flat");
        let ex = PaperExecutor::new(dec!(1000), dec!(0), path.clone()).unwrap();
        // slippage is non-deterministic; use fee_rate=0 and assert round-trip.
        let req = OpenRequest {
            side: Side::Up,
            market_slug: "m".into(),
            market_end_date: Utc::now() + Duration::minutes(5),
            token_id: "1".into(),
            quoted_price: dec!(0.25),
            shares: dec!(100),
        };
        let open = ex.open_position(req).await.unwrap();
        let bal_after_open = ex.balance().await.unwrap();
        assert!(bal_after_open.available_usd < dec!(1000));

        // Close at the same mark → small PnL fluctuation from slippage only.
        let close = ex
            .close_position(CloseRequest {
                position: open.position,
                exit_reason: "stop_loss".into(),
                mark_price: dec!(0.25),
            })
            .await
            .unwrap();
        // With fee=0, pnl = (exit - entry) * shares; both around 0.25 with tiny ±slip.
        assert!(close.pnl.abs() < dec!(1));
        let _ = tokio::fs::remove_file(path).await;
    }

    #[tokio::test]
    async fn profit_and_loss_move_balance_in_right_direction() {
        let path = tmp_path("pnl");
        let ex = PaperExecutor::new(dec!(1000), dec!(0), path.clone()).unwrap();
        let req = OpenRequest {
            side: Side::Up,
            market_slug: "m".into(),
            market_end_date: Utc::now() + Duration::minutes(5),
            token_id: "1".into(),
            quoted_price: dec!(0.25),
            shares: dec!(100),
        };
        let open = ex.open_position(req).await.unwrap();
        // Exit high → expect positive pnl.
        let close = ex
            .close_position(CloseRequest {
                position: open.position,
                exit_reason: "take_profit".into(),
                mark_price: dec!(0.45),
            })
            .await
            .unwrap();
        assert!(close.pnl > dec!(10));
        let _ = tokio::fs::remove_file(path).await;
    }

    #[tokio::test]
    async fn ledger_persists_across_instances() {
        let path = tmp_path("persist");
        {
            let ex = PaperExecutor::new(dec!(500), dec!(0), path.clone()).unwrap();
            let req = OpenRequest {
                side: Side::Up,
                market_slug: "m".into(),
                market_end_date: Utc::now() + Duration::minutes(5),
                token_id: "1".into(),
                quoted_price: dec!(0.25),
                shares: dec!(50),
            };
            ex.open_position(req).await.unwrap();
        }
        // New instance with bigger starting balance should IGNORE seed and use persisted file.
        let ex2 = PaperExecutor::new(dec!(99999), dec!(0), path.clone()).unwrap();
        let b = ex2.balance().await.unwrap();
        assert!(b.available_usd < dec!(500));
        assert!(b.locked_usd > dec!(0));
        let _ = tokio::fs::remove_file(path).await;
    }
}
