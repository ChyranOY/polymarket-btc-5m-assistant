//! Batched writer for the Supabase `signal_ticks` table.
//!
//! Each tick produces a compact row that can be replayed later to simulate
//! alternative SL/TP params, reconstruct spread behavior around entries, etc.
//! The tick loop pushes rows onto an mpsc channel; a background task drains
//! the channel and flushes in batches of up to `BATCH_SIZE` rows or every
//! `FLUSH_INTERVAL` (whichever fires first) so a slow PostgREST request
//! never blocks the 1-second tick loop.
//!
//! The recorder is a no-op when Supabase is disabled. On graceful shutdown
//! the sender is dropped, which causes the background task to flush any
//! remaining buffer and exit.

use crate::store::supabase::SupabaseClient;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// Max rows per PostgREST insert.
const BATCH_SIZE: usize = 10;
/// Max age (from first buffered row) before the flusher gives up waiting for
/// a full batch and posts whatever it has.
const BATCH_WINDOW: Duration = Duration::from_secs(10);
/// Upper bound on idle time between batches — triggers when no ticks arrive
/// at all (e.g., off-hours).
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
/// Max rows buffered in the channel before record() starts dropping.
const CHANNEL_CAPACITY: usize = 512;

pub struct TickRecorder {
    tx: mpsc::Sender<Value>,
}

impl TickRecorder {
    /// Start the background flush task and return a handle. Returns `None`
    /// when Supabase is disabled — callers treat `None` as "recording off".
    pub fn start(supabase: Arc<SupabaseClient>) -> Option<Arc<Self>> {
        if !supabase.enabled() {
            return None;
        }
        let (tx, rx) = mpsc::channel::<Value>(CHANNEL_CAPACITY);
        tokio::spawn(flush_loop(rx, supabase));
        Some(Arc::new(Self { tx }))
    }

    /// Non-blocking record. Drops (with a warn log) if the channel is full,
    /// so the tick loop can never be back-pressured by Supabase latency.
    pub fn record(&self, row: Value) {
        if let Err(e) = self.tx.try_send(row) {
            match e {
                mpsc::error::TrySendError::Full(_) => {
                    tracing::warn!("tick_recorder: channel full, dropping tick row")
                }
                mpsc::error::TrySendError::Closed(_) => {
                    tracing::debug!("tick_recorder: channel closed, dropping tick row")
                }
            }
        }
    }
}

async fn flush_loop(mut rx: mpsc::Receiver<Value>, supabase: Arc<SupabaseClient>) {
    let mut buf: Vec<Value> = Vec::with_capacity(BATCH_SIZE);
    loop {
        // Wait for the first row (or a shutdown via channel close).
        let first = match tokio::time::timeout(IDLE_TIMEOUT, rx.recv()).await {
            Ok(Some(row)) => row,
            Ok(None) => break, // channel closed, drain complete
            Err(_) => continue, // no rows arrived in IDLE_TIMEOUT; loop and wait again
        };
        buf.push(first);

        // Accumulate until BATCH_SIZE or BATCH_WINDOW from the first row,
        // whichever comes first. Waiting here (instead of try_recv draining)
        // is what actually batches ticks — the producer runs at ~1 Hz.
        let deadline = tokio::time::Instant::now() + BATCH_WINDOW;
        while buf.len() < BATCH_SIZE {
            match tokio::time::timeout_at(deadline, rx.recv()).await {
                Ok(Some(row)) => buf.push(row),
                Ok(None) => break,      // channel closed mid-batch
                Err(_) => break,        // BATCH_WINDOW elapsed
            }
        }

        flush(&supabase, &mut buf).await;
    }

    // Shutdown: flush whatever is buffered before exiting.
    while let Ok(row) = rx.try_recv() {
        buf.push(row);
        if buf.len() >= BATCH_SIZE {
            flush(&supabase, &mut buf).await;
        }
    }
    if !buf.is_empty() {
        flush(&supabase, &mut buf).await;
    }
    tracing::info!("tick_recorder: flush loop exited");
}

async fn flush(supabase: &SupabaseClient, buf: &mut Vec<Value>) {
    if buf.is_empty() {
        return;
    }
    let n = buf.len();
    let rows = std::mem::take(buf);
    match supabase.insert_signal_ticks(&rows).await {
        Ok(()) => tracing::debug!(count = n, "tick_recorder: flushed"),
        Err(e) => tracing::warn!(err = %e, count = n, "tick_recorder: flush failed"),
    }
}
