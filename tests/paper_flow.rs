//! Integration test for the paper-mode executor + engine pure-fn pipeline.
//! We don't boot the HTTP server here — we exercise the executor directly
//! and verify that a full open → close cycle settles balance + ledger correctly.

use chrono::{Duration, Utc};
use rust_decimal_macros::dec;
use std::path::PathBuf;
use uuid::Uuid;

use polymarket_btc_5m as bot;
use bot::exec::paper::PaperExecutor;
use bot::exec::{CloseRequest, Executor, OpenRequest};
use bot::model::Side;

fn tmp_ledger() -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("paper_integration_{}.json", Uuid::new_v4()));
    p
}

#[tokio::test]
async fn open_then_profitable_close_updates_balance() {
    let path = tmp_ledger();
    let exec = PaperExecutor::new(dec!(1000), dec!(0.02), path.clone()).unwrap();

    let start_balance = exec.balance().await.unwrap().available_usd;
    assert_eq!(start_balance, dec!(1000));

    let open = exec
        .open_position(OpenRequest {
            side: Side::Up,
            market_slug: "btc-updown-5m-x".into(),
            market_end_date: Utc::now() + Duration::minutes(4),
            token_id: "1".into(),
            quoted_price: dec!(0.25),
            limit_price: None,
            shares: dec!(50),
        })
        .await
        .unwrap();

    let after_open = exec.balance().await.unwrap();
    assert!(after_open.available_usd < start_balance);
    assert!(after_open.locked_usd > dec!(0));

    let close = exec
        .close_position(CloseRequest {
            position: open.position,
            exit_reason: "take_profit".into(),
            mark_price: dec!(0.50),
        })
        .await
        .unwrap();

    assert!(close.pnl > dec!(0));
    let after_close = exec.balance().await.unwrap();
    assert!(after_close.available_usd > start_balance);
    assert_eq!(after_close.locked_usd, dec!(0));

    let _ = tokio::fs::remove_file(path).await;
}

#[tokio::test]
async fn open_then_losing_close_reduces_balance() {
    let path = tmp_ledger();
    let exec = PaperExecutor::new(dec!(1000), dec!(0.02), path.clone()).unwrap();

    let open = exec
        .open_position(OpenRequest {
            side: Side::Up,
            market_slug: "btc-updown-5m-x".into(),
            market_end_date: Utc::now() + Duration::minutes(4),
            token_id: "1".into(),
            quoted_price: dec!(0.25),
            limit_price: None,
            shares: dec!(50),
        })
        .await
        .unwrap();

    let close = exec
        .close_position(CloseRequest {
            position: open.position,
            exit_reason: "stop_loss".into(),
            mark_price: dec!(0.10),
        })
        .await
        .unwrap();

    assert!(close.pnl < dec!(0));
    let after = exec.balance().await.unwrap();
    assert!(after.available_usd < dec!(1000));

    let _ = tokio::fs::remove_file(path).await;
}
