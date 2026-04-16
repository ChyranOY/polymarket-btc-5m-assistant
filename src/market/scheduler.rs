use crate::data::gamma::{GammaClient, GammaMarket};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use std::sync::Arc;
use tokio::sync::{RwLock, broadcast};
use tokio::time::{Duration, Instant, sleep, sleep_until};

/// Minimal metadata the rest of the bot needs about the current active market.
#[derive(Debug, Clone)]
pub struct MarketMeta {
    pub slug: String,
    pub end_date: DateTime<Utc>,
    pub up_token_id: String,
    pub down_token_id: String,
}

impl From<&GammaMarket> for MarketMeta {
    fn from(m: &GammaMarket) -> Self {
        Self {
            slug: m.slug.clone(),
            end_date: m.end_date,
            up_token_id: m.up_token_id.clone(),
            down_token_id: m.down_token_id.clone(),
        }
    }
}

/// Shared handle readable by the tick loop / API. The scheduler task writes; consumers read.
#[derive(Debug, Clone)]
pub struct MarketTracker {
    inner: Arc<RwLock<Option<MarketMeta>>>,
    tx: broadcast::Sender<MarketMeta>,
}

impl MarketTracker {
    pub fn new() -> Self {
        let (tx, _rx) = broadcast::channel(16);
        Self {
            inner: Arc::new(RwLock::new(None)),
            tx,
        }
    }

    pub async fn current(&self) -> Option<MarketMeta> {
        self.inner.read().await.clone()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<MarketMeta> {
        self.tx.subscribe()
    }

    async fn set(&self, meta: MarketMeta) {
        *self.inner.write().await = Some(meta.clone());
        let _ = self.tx.send(meta); // ok to drop if no subscribers
    }
}

impl Default for MarketTracker {
    fn default() -> Self {
        Self::new()
    }
}

pub struct SchedulerConfig {
    pub rollover_buffer: Duration,
    pub safety_tick: Duration,
    pub max_retry_backoff: Duration,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            rollover_buffer: Duration::from_secs(2),
            safety_tick: Duration::from_secs(5 * 60),
            max_retry_backoff: Duration::from_secs(15),
        }
    }
}

/// Run the event-driven rollover loop until the task is aborted.
/// Design: sleep_until(end_date + buffer), refetch, broadcast, repeat.
/// Safety net: on every iteration we also race a 5-minute interval so clock drift
/// or a skipped market can't leave us stranded on a settled slug.
pub async fn run_market_scheduler(
    gamma: GammaClient,
    tracker: MarketTracker,
    cfg: SchedulerConfig,
) {
    // Boot: acquire an initial active market (with retry).
    loop {
        match gamma.pick_current_market(Utc::now()).await {
            Ok(Some(m)) => {
                let meta: MarketMeta = (&m).into();
                tracing::info!(slug = %meta.slug, end_date = %meta.end_date, "market: initial");
                tracker.set(meta).await;
                break;
            }
            Ok(None) => {
                tracing::warn!("market: no active 5m market yet; retrying in 2s");
                sleep(Duration::from_secs(2)).await;
            }
            Err(e) => {
                tracing::warn!(err = %e, "market: gamma fetch failed; retrying in 2s");
                sleep(Duration::from_secs(2)).await;
            }
        }
    }

    loop {
        let current = match tracker.current().await {
            Some(c) => c,
            None => {
                // Shouldn't happen after the boot loop but guard anyway.
                sleep(Duration::from_secs(2)).await;
                continue;
            }
        };

        let wake_at = wake_instant_for(current.end_date, cfg.rollover_buffer);
        let safety = sleep(cfg.safety_tick);

        tokio::select! {
            _ = sleep_until(wake_at) => {
                tracing::debug!(slug = %current.slug, "market: rollover timer fired");
            }
            _ = safety => {
                tracing::debug!(slug = %current.slug, "market: safety interval tick");
            }
        }

        // Refetch with exponential backoff until we see a different active market
        // (or we've waited long enough).
        let new_meta = fetch_new_market(&gamma, &current.slug, cfg.max_retry_backoff).await;
        match new_meta {
            Some(m) => {
                tracing::info!(
                    prev = %current.slug,
                    next = %m.slug,
                    end_date = %m.end_date,
                    "market: rolled over"
                );
                tracker.set(m).await;
            }
            None => {
                tracing::warn!(
                    slug = %current.slug,
                    "market: no successor after backoff; looping"
                );
                // Fall through to loop; we'll try again on next safety tick.
            }
        }
    }
}

fn wake_instant_for(end_date: DateTime<Utc>, buffer: Duration) -> Instant {
    let now = Utc::now();
    let until_end = end_date - now;
    let total = until_end + ChronoDuration::from_std(buffer).unwrap_or_default();
    let millis = total.num_milliseconds().max(0) as u64;
    Instant::now() + Duration::from_millis(millis)
}

async fn fetch_new_market(
    gamma: &GammaClient,
    old_slug: &str,
    max_backoff: Duration,
) -> Option<MarketMeta> {
    let mut delay = Duration::from_secs(1);
    let deadline = Instant::now() + max_backoff;
    loop {
        match gamma.pick_current_market(Utc::now()).await {
            Ok(Some(m)) if m.slug != old_slug => return Some((&m).into()),
            Ok(_) => {
                tracing::debug!(old = old_slug, "market: gamma still shows old/no market");
            }
            Err(e) => tracing::debug!(err = %e, "market: gamma error during rollover retry"),
        }
        if Instant::now() >= deadline {
            return None;
        }
        sleep(delay).await;
        delay = (delay * 2).min(Duration::from_secs(4));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[tokio::test]
    async fn tracker_set_and_read() {
        let t = MarketTracker::new();
        assert!(t.current().await.is_none());
        let meta = MarketMeta {
            slug: "btc-updown-5m-x".into(),
            end_date: Utc.with_ymd_and_hms(2026, 4, 16, 17, 5, 0).unwrap(),
            up_token_id: "1".into(),
            down_token_id: "2".into(),
        };
        t.set(meta.clone()).await;
        assert_eq!(t.current().await.unwrap().slug, "btc-updown-5m-x");
    }

    #[tokio::test]
    async fn tracker_broadcasts_rollover() {
        let t = MarketTracker::new();
        let mut rx = t.subscribe();
        let meta = MarketMeta {
            slug: "a".into(),
            end_date: Utc::now(),
            up_token_id: "1".into(),
            down_token_id: "2".into(),
        };
        t.set(meta.clone()).await;
        let recv = rx.recv().await.unwrap();
        assert_eq!(recv.slug, "a");
    }

    #[test]
    fn wake_instant_is_after_end_plus_buffer() {
        let end = Utc::now() + ChronoDuration::seconds(10);
        let wake = wake_instant_for(end, Duration::from_secs(2));
        assert!(wake > Instant::now() + Duration::from_secs(10));
    }
}
