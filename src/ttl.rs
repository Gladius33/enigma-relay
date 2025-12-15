use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures::future::Shared;
use tokio::sync::oneshot;

use crate::config::RelayConfig;
use crate::store::DynRelayStore;

pub type ShutdownSignal = Shared<oneshot::Receiver<()>>;

pub async fn run_purger(store: DynRelayStore, cfg: RelayConfig, shutdown: ShutdownSignal) {
    let mut ticker = tokio::time::interval(Duration::from_secs(cfg.purge_interval_secs));
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let now = current_millis();
                let _ = store.purge_expired(now).await;
            }
            _ = shutdown.clone() => {
                break;
            }
        }
    }
}

fn current_millis() -> u64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_millis() as u64,
        Err(_) => 0,
    }
}
