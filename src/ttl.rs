use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures::future::Shared;
use tokio::sync::oneshot;

use crate::config::RelayLimits;
#[cfg(feature = "metrics")]
use crate::metrics::RelayMetrics;
use crate::store::DynRelayStore;
#[cfg(feature = "metrics")]
use std::sync::Arc;

pub type ShutdownSignal = Shared<oneshot::Receiver<()>>;

pub async fn run_gc(
    store: DynRelayStore,
    limits: RelayLimits,
    shutdown: ShutdownSignal,
    #[cfg(feature = "metrics")] metrics: Option<Arc<RelayMetrics>>,
) {
    let mut ticker = tokio::time::interval(Duration::from_secs(limits.gc_interval_seconds));
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let now = now_millis();
                let result = store.purge_expired(now, 1024).await;
                if let Ok(removed) = result {
                    if removed > 0 {
                        #[cfg(feature = "metrics")]
                        if let Some(m) = metrics.as_ref() {
                            m.gc_removed.fetch_add(removed as u64, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                }
            }
            _ = shutdown.clone() => {
                break;
            }
        }
    }
}

pub fn now_millis() -> u64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_millis() as u64,
        Err(_) => 0,
    }
}
