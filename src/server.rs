use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use actix_web::middleware::Compress;
use actix_web::web::{self, Data, Json};
use actix_web::{App, HttpRequest, HttpResponse, HttpServer};
use base64::Engine;
use futures::FutureExt;
use tokio::sync::oneshot;

use crate::config::{RelayConfig, RelayLimits, RelayMode, StorageKind};
use crate::error::{EnigmaRelayError, Result};
#[cfg(feature = "metrics")]
use crate::metrics::RelayMetrics;
use crate::model::{
    AckRequest, AckResponse, DeliveryItem, PullRequest, PullResponse, PushRequest, PushResponse,
};
use crate::store::{AckItem, DynRelayStore, InboundMessage, QueueMessage};
use crate::store_mem::MemStore;
#[cfg(feature = "persistence")]
use crate::store_sled::SledStore;
use crate::ttl::{now_millis, run_gc, ShutdownSignal};

pub struct RunningRelay {
    pub base_url: String,
    pub shutdown: oneshot::Sender<()>,
    pub handle: tokio::task::JoinHandle<Result<()>>,
}

#[derive(Clone)]
struct AppState {
    store: DynRelayStore,
    config: RelayConfig,
    rate_limiter: RateLimiter,
    started: Instant,
    #[cfg(feature = "metrics")]
    metrics: Arc<RelayMetrics>,
}

#[derive(Clone, Eq, PartialEq, Hash)]
struct RateKey {
    ip: IpAddr,
    label: &'static str,
}

#[derive(Clone)]
struct Bucket {
    tokens: f64,
    last: Instant,
    burst: f64,
    rate: f64,
    ban_until: Option<Instant>,
}

#[derive(Clone)]
struct RateLimiter {
    enabled: bool,
    ban_seconds: u64,
    buckets: Arc<parking_lot::Mutex<HashMap<RateKey, Bucket>>>,
}

impl RateLimiter {
    fn new(enabled: bool, ban_seconds: u64) -> Self {
        RateLimiter {
            enabled,
            ban_seconds,
            buckets: Arc::new(parking_lot::Mutex::new(HashMap::new())),
        }
    }

    fn allow(&self, ip: IpAddr, label: &'static str, rate: f64, burst: f64) -> bool {
        if !self.enabled {
            return true;
        }
        let now = Instant::now();
        let mut guard = self.buckets.lock();
        let key = RateKey { ip, label };
        let entry = guard.entry(key).or_insert(Bucket {
            tokens: burst,
            last: now,
            burst,
            rate,
            ban_until: None,
        });
        if let Some(until) = entry.ban_until {
            if until > now {
                return false;
            }
            entry.ban_until = None;
        }
        let elapsed = now.saturating_duration_since(entry.last).as_secs_f64();
        entry.tokens = (entry.tokens + elapsed * entry.rate).min(entry.burst);
        entry.last = now;
        if entry.tokens >= 1.0 {
            entry.tokens -= 1.0;
            true
        } else {
            entry.ban_until = Some(now + Duration::from_secs(self.ban_seconds));
            false
        }
    }
}

pub async fn start(cfg: RelayConfig) -> Result<RunningRelay> {
    let store: DynRelayStore = match cfg.storage.kind {
        StorageKind::Memory => Arc::new(MemStore::new()),
        StorageKind::Sled => {
            #[cfg(feature = "persistence")]
            {
                Arc::new(SledStore::new(&cfg.storage.path)?)
            }
            #[cfg(not(feature = "persistence"))]
            {
                return Err(EnigmaRelayError::Disabled(
                    "persistence feature not enabled".to_string(),
                ));
            }
        }
    };
    start_with_store(store, cfg).await
}

pub async fn start_with_store(store: DynRelayStore, cfg: RelayConfig) -> Result<RunningRelay> {
    cfg.validate()?;
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let shutdown_signal: ShutdownSignal = shutdown_rx.shared();
    let rate_limiter = RateLimiter::new(cfg.rate_limit.enabled, cfg.rate_limit.ban_seconds);
    #[cfg(feature = "metrics")]
    let metrics = Arc::new(RelayMetrics::default());
    let state = AppState {
        store: store.clone(),
        config: cfg.clone(),
        rate_limiter: rate_limiter.clone(),
        started: Instant::now(),
        #[cfg(feature = "metrics")]
        metrics: metrics.clone(),
    };
    let gc_store = store.clone();
    let gc_limits = cfg.relay.clone();
    let gc_signal = shutdown_signal.clone();
    #[cfg(feature = "metrics")]
    let gc_task = {
        let metrics = metrics.clone();
        tokio::spawn(async move { run_gc(gc_store, gc_limits, gc_signal, Some(metrics)).await })
    };
    #[cfg(not(feature = "metrics"))]
    let gc_task = tokio::spawn(async move { run_gc(gc_store, gc_limits, gc_signal).await });
    let (server, addr) = build_server(state, &cfg)?;
    let handle = server.handle();
    let server_task = actix_web::rt::spawn(server);
    let joined = tokio::spawn(async move {
        let _ = shutdown_signal.await;
        handle.stop(true).await;
        let _ = gc_task.await;
        let srv = server_task
            .await
            .map_err(|e| EnigmaRelayError::Internal(e.to_string()))?;
        srv.map_err(|e: std::io::Error| EnigmaRelayError::Internal(e.to_string()))
    });
    let scheme = match cfg.mode {
        RelayMode::Http => "http",
        RelayMode::Tls => "https",
    };
    Ok(RunningRelay {
        base_url: format!("{}://{}", scheme, addr),
        shutdown: shutdown_tx,
        handle: joined,
    })
}

fn build_server(
    state: AppState,
    cfg: &RelayConfig,
) -> Result<(actix_web::dev::Server, SocketAddr)> {
    let state_data = state.clone();
    let builder = HttpServer::new(move || {
        App::new()
            .app_data(Data::new(state_data.clone()))
            .wrap(Compress::default())
            .route("/push", web::post().to(push))
            .route("/pull", web::post().to(pull))
            .route("/ack", web::post().to(ack))
            .route("/health", web::get().to(health))
            .route("/stats", web::get().to(stats))
    });
    match cfg.mode {
        RelayMode::Http => {
            let listener = std::net::TcpListener::bind(&cfg.address)
                .map_err(|e| EnigmaRelayError::Internal(e.to_string()))?;
            listener
                .set_nonblocking(true)
                .map_err(|e| EnigmaRelayError::Internal(e.to_string()))?;
            let addr = listener
                .local_addr()
                .map_err(|e| EnigmaRelayError::Internal(e.to_string()))?;
            let server = builder
                .listen(listener)
                .map_err(|e| EnigmaRelayError::Internal(e.to_string()))?
                .disable_signals()
                .run();
            Ok((server, addr))
        }
        RelayMode::Tls => {
            #[cfg(feature = "tls")]
            {
                let tls_cfg = cfg
                    .tls
                    .clone()
                    .ok_or_else(|| EnigmaRelayError::Config("missing tls config".to_string()))?;
                let server_config = build_rustls_config(tls_cfg)?;
                let listener = std::net::TcpListener::bind(&cfg.address)
                    .map_err(|e| EnigmaRelayError::Internal(e.to_string()))?;
                listener
                    .set_nonblocking(true)
                    .map_err(|e| EnigmaRelayError::Internal(e.to_string()))?;
                let addr = listener
                    .local_addr()
                    .map_err(|e| EnigmaRelayError::Internal(e.to_string()))?;
                let server = builder
                    .listen_rustls_0_23(listener, server_config)
                    .map_err(|e| EnigmaRelayError::Internal(e.to_string()))?
                    .disable_signals()
                    .run();
                Ok((server, addr))
            }
            #[cfg(not(feature = "tls"))]
            {
                Err(EnigmaRelayError::Disabled(
                    "tls feature not enabled".to_string(),
                ))
            }
        }
    }
}

async fn push(
    req: HttpRequest,
    state: Data<AppState>,
    body: Json<PushRequest>,
) -> std::result::Result<HttpResponse, EnigmaRelayError> {
    enforce_rate(&state, &req, "push")?;
    #[cfg(feature = "metrics")]
    state
        .metrics
        .push_total
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let msg = build_inbound_message(body.into_inner(), &state.config.relay)?;
    let result = state.store.push(msg, &state.config.relay).await?;
    #[cfg(feature = "metrics")]
    {
        if result.stored {
            state
                .metrics
                .push_stored
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        if result.duplicate {
            state
                .metrics
                .duplicates
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }
    let response = PushResponse {
        stored: result.stored,
        duplicate: result.duplicate,
        queue_len: Some(result.queue_len),
        queue_bytes: Some(result.queue_bytes),
    };
    Ok(HttpResponse::Ok().json(response))
}

async fn pull(
    req: HttpRequest,
    state: Data<AppState>,
    body: Json<PullRequest>,
) -> std::result::Result<HttpResponse, EnigmaRelayError> {
    enforce_rate(&state, &req, "pull")?;
    #[cfg(feature = "metrics")]
    state
        .metrics
        .pull_total
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let pull = body.into_inner();
    if pull.recipient.trim().is_empty() {
        return Err(EnigmaRelayError::InvalidInput(
            "recipient required".to_string(),
        ));
    }
    let max = pull.max.unwrap_or(state.config.relay.pull_batch_max);
    if max == 0 {
        return Err(EnigmaRelayError::InvalidInput(
            "max must be positive".to_string(),
        ));
    }
    let clamped = max.min(state.config.relay.pull_batch_max);
    let batch = state
        .store
        .pull(
            pull.recipient.as_str(),
            pull.cursor.clone(),
            clamped,
            now_millis(),
        )
        .await?;
    let items: Vec<DeliveryItem> = batch.items.into_iter().map(to_delivery).collect();
    let response = PullResponse {
        items,
        next_cursor: batch.next_cursor,
        remaining_estimate: batch.remaining_estimate,
    };
    Ok(HttpResponse::Ok().json(response))
}

async fn ack(
    req: HttpRequest,
    state: Data<AppState>,
    body: Json<AckRequest>,
) -> std::result::Result<HttpResponse, EnigmaRelayError> {
    enforce_rate(&state, &req, "ack")?;
    #[cfg(feature = "metrics")]
    state
        .metrics
        .ack_total
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let ack = body.into_inner();
    if ack.recipient.trim().is_empty() {
        return Err(EnigmaRelayError::InvalidInput(
            "recipient required".to_string(),
        ));
    }
    let mut items = Vec::new();
    for entry in ack.ack {
        items.push(AckItem {
            message_id: entry.message_id,
            chunk_index: entry.chunk_index,
        });
    }
    let outcome = state.store.ack(ack.recipient.as_str(), items).await?;
    #[cfg(feature = "metrics")]
    state
        .metrics
        .ack_deleted
        .fetch_add(outcome.deleted, std::sync::atomic::Ordering::Relaxed);
    let response = AckResponse {
        deleted: outcome.deleted,
        missing: outcome.missing,
        remaining: outcome.remaining,
    };
    Ok(HttpResponse::Ok().json(response))
}

async fn health() -> std::result::Result<HttpResponse, EnigmaRelayError> {
    Ok(HttpResponse::Ok().json(serde_json::json!({"status": "ok"})))
}

async fn stats(state: Data<AppState>) -> std::result::Result<HttpResponse, EnigmaRelayError> {
    #[cfg(feature = "metrics")]
    {
        let snapshot = state.metrics.snapshot();
        let uptime_ms = state.started.elapsed().as_millis() as u64;
        return Ok(HttpResponse::Ok().json(serde_json::json!({
            "status": "ok",
            "uptime_ms": uptime_ms,
            "metrics": snapshot
        })));
    }
    #[cfg(not(feature = "metrics"))]
    {
        let uptime_ms = state.started.elapsed().as_millis() as u64;
        Ok(HttpResponse::Ok().json(serde_json::json!({
            "status": "ok",
            "uptime_ms": uptime_ms
        })))
    }
}

fn enforce_rate(state: &AppState, req: &HttpRequest, label: &'static str) -> Result<()> {
    if !state.rate_limiter.enabled {
        return Ok(());
    }
    let cfg = &state.config.rate_limit;
    let ip = peer_ip(req);
    let burst = cfg.burst as f64;
    let global_ok = state
        .rate_limiter
        .allow(ip, "global", cfg.per_ip_rps as f64, burst);
    let endpoint_rps = match label {
        "push" => cfg.endpoints.push_rps,
        "pull" => cfg.endpoints.pull_rps,
        "ack" => cfg.endpoints.ack_rps,
        _ => cfg.per_ip_rps,
    };
    let endpoint_ok = state
        .rate_limiter
        .allow(ip, label, endpoint_rps as f64, burst);
    if global_ok && endpoint_ok {
        return Ok(());
    }
    #[cfg(feature = "metrics")]
    state
        .metrics
        .rate_limited
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    Err(EnigmaRelayError::RateLimited)
}

fn peer_ip(req: &HttpRequest) -> IpAddr {
    req.peer_addr()
        .map(|s| s.ip())
        .unwrap_or(Ipv4Addr::LOCALHOST.into())
}

fn build_inbound_message(body: PushRequest, limits: &RelayLimits) -> Result<InboundMessage> {
    if body.recipient.trim().is_empty() {
        return Err(EnigmaRelayError::InvalidInput(
            "recipient required".to_string(),
        ));
    }
    if body.meta.chunk_count == 0 {
        return Err(EnigmaRelayError::InvalidInput(
            "chunk_count must be positive".to_string(),
        ));
    }
    if body.meta.chunk_index >= body.meta.chunk_count {
        return Err(EnigmaRelayError::InvalidInput(
            "chunk_index out of range".to_string(),
        ));
    }
    if body.meta.kind.trim().is_empty() {
        return Err(EnigmaRelayError::InvalidInput("kind required".to_string()));
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(body.ciphertext_b64.as_bytes())
        .map_err(|_| EnigmaRelayError::InvalidInput("invalid base64".to_string()))?;
    let payload_bytes = decoded.len() as u64;
    if payload_bytes > limits.max_message_bytes {
        return Err(EnigmaRelayError::InvalidInput(
            "message too large".to_string(),
        ));
    }
    let arrival_ms = now_millis();
    let ttl_ms = limits.message_ttl_seconds.saturating_mul(1000);
    let deadline_ms = arrival_ms.saturating_add(ttl_ms);
    Ok(InboundMessage {
        recipient: body.recipient,
        message_id: body.message_id,
        ciphertext_b64: body.ciphertext_b64,
        meta: body.meta,
        payload_bytes,
        arrival_ms,
        deadline_ms,
    })
}

fn to_delivery(msg: QueueMessage) -> DeliveryItem {
    DeliveryItem {
        recipient: msg.recipient,
        message_id: msg.message_id,
        ciphertext_b64: msg.ciphertext_b64,
        meta: msg.meta,
        arrival_ms: msg.arrival_ms,
        deadline_ms: msg.deadline_ms,
    }
}

#[cfg(feature = "tls")]
fn build_rustls_config(tls: crate::config::TlsConfig) -> Result<rustls::ServerConfig> {
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
    #[cfg(feature = "mtls")]
    use rustls::RootCertStore;
    use rustls_pemfile::{certs, pkcs8_private_keys};
    use std::fs::File;
    use std::io::BufReader;

    let mut cert_reader = BufReader::new(
        File::open(&tls.cert_pem_path).map_err(|e| EnigmaRelayError::Tls(e.to_string()))?,
    );
    let mut key_reader = BufReader::new(
        File::open(&tls.key_pem_path).map_err(|e| EnigmaRelayError::Tls(e.to_string()))?,
    );
    let cert_chain: Vec<CertificateDer<'static>> = certs(&mut cert_reader)
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| EnigmaRelayError::Tls(e.to_string()))?;
    let mut keys = pkcs8_private_keys(&mut key_reader)
        .collect::<std::result::Result<Vec<PrivatePkcs8KeyDer<'static>>, _>>()
        .map_err(|e| EnigmaRelayError::Tls(e.to_string()))?;
    let key_bytes = keys
        .pop()
        .ok_or_else(|| EnigmaRelayError::Tls("no private key found".to_string()))?;
    let key = PrivateKeyDer::Pkcs8(key_bytes);
    let builder = rustls::ServerConfig::builder();
    let cfg = {
        #[cfg(feature = "mtls")]
        {
            if let Some(ca_path) = tls.client_ca_pem_path.clone() {
                let mut ca_reader = BufReader::new(
                    File::open(ca_path).map_err(|e| EnigmaRelayError::Tls(e.to_string()))?,
                );
                let mut store = RootCertStore::empty();
                let cas = certs(&mut ca_reader)
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(|e| EnigmaRelayError::Tls(e.to_string()))?;
                for ca in cas {
                    store
                        .add(ca.into())
                        .map_err(|e| EnigmaRelayError::Tls(e.to_string()))?;
                }
                let verifier = rustls::server::WebPkiClientVerifier::builder(store.into())
                    .build()
                    .map_err(|e| EnigmaRelayError::Tls(e.to_string()))?;
                builder
                    .with_client_cert_verifier(verifier)
                    .with_single_cert(cert_chain, key)
                    .map_err(|e| EnigmaRelayError::Tls(e.to_string()))?
            } else {
                builder
                    .with_no_client_auth()
                    .with_single_cert(cert_chain, key)
                    .map_err(|e| EnigmaRelayError::Tls(e.to_string()))?
            }
        }
        #[cfg(not(feature = "mtls"))]
        {
            if tls.client_ca_pem_path.is_some() {
                return Err(EnigmaRelayError::Disabled(
                    "mtls feature not enabled".to_string(),
                ));
            }
            builder
                .with_no_client_auth()
                .with_single_cert(cert_chain, key)
                .map_err(|e| EnigmaRelayError::Tls(e.to_string()))?
        }
    };
    Ok(cfg)
}
