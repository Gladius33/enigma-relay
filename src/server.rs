use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Router;
use futures::FutureExt;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::config::RelayConfig;
use crate::error::{EnigmaRelayError, Result};
use crate::routes::router;
use crate::store::DynRelayStore;
use crate::store_mem::MemStore;
#[cfg(feature = "sled-backend")]
use crate::store_sled::SledStore;
use crate::ttl::{run_purger, ShutdownSignal};

pub struct RunningRelay {
    pub base_url: String,
    pub shutdown: oneshot::Sender<()>,
    pub handle: JoinHandle<Result<()>>,
}

pub async fn start(cfg: RelayConfig) -> Result<RunningRelay> {
    let store: DynRelayStore = if let Some(path) = cfg.storage_path.as_ref() {
        #[cfg(feature = "sled-backend")]
        {
            Arc::new(SledStore::new(path, cfg.clone())?)
        }
        #[cfg(not(feature = "sled-backend"))]
        {
            return Err(EnigmaRelayError::InvalidInput("sled backend not available"));
        }
    } else {
        Arc::new(MemStore::new(cfg.clone()))
    };
    start_with_store(store, cfg).await
}

pub async fn start_with_store(store: DynRelayStore, cfg: RelayConfig) -> Result<RunningRelay> {
    let app: Router = router(store.clone(), cfg.clone());
    let listener = tokio::net::TcpListener::bind(&cfg.bind_addr)
        .await
        .map_err(|_| EnigmaRelayError::Transport)?;
    let addr = listener
        .local_addr()
        .map_err(|_| EnigmaRelayError::Transport)?;
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let shared_shutdown: ShutdownSignal = shutdown_rx.shared();
    let purger_store = store.clone();
    let purger_cfg = cfg.clone();
    let purger_signal = shared_shutdown.clone();
    let purger_task = tokio::spawn(async move {
        run_purger(purger_store, purger_cfg, purger_signal).await;
    });
    let server = axum::serve(listener, app).with_graceful_shutdown(async move {
        let _ = shared_shutdown.await;
    });
    let handle = tokio::spawn(async move {
        let server_result = server.await.map_err(|_| EnigmaRelayError::Transport);
        let _ = purger_task.await;
        server_result
    });
    Ok(RunningRelay {
        base_url: format!("http://{}", format_socket(addr)),
        shutdown: shutdown_tx,
        handle,
    })
}

fn format_socket(addr: SocketAddr) -> String {
    addr.to_string()
}

pub fn now_millis() -> u64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_millis() as u64,
        Err(_) => 0,
    }
}
