use std::sync::Arc;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use enigma_relay::MessageMeta;
#[cfg(feature = "tls")]
use enigma_relay::TlsConfig;
use enigma_relay::{
    start, start_with_store, AckEntry, AckRequest, AckResponse, DeliveryItem, MemStore,
    PullRequest, PullResponse, PushRequest, PushResponse, RelayConfig, RelayMode, StorageKind,
};
use rand::Rng;
use reqwest::Client;
#[cfg(feature = "tls")]
use tempfile::TempDir;
use tokio::time::sleep;
use uuid::Uuid;

fn memory_config() -> RelayConfig {
    let mut cfg = RelayConfig::default();
    cfg.address = "127.0.0.1:0".to_string();
    cfg.storage.kind = StorageKind::Memory;
    cfg.mode = RelayMode::Http;
    cfg.rate_limit.enabled = false;
    cfg
}

fn push_body(recipient: &str, chunk_index: u32, chunk_count: u32, payload: &str) -> PushRequest {
    PushRequest {
        recipient: recipient.to_string(),
        message_id: Uuid::new_v4(),
        ciphertext_b64: STANDARD.encode(payload.as_bytes()),
        meta: MessageMeta {
            kind: "msg".to_string(),
            total_len: payload.len() as u64,
            chunk_index,
            chunk_count,
            sent_ms: 0,
        },
    }
}

async fn do_push(client: &Client, base: &str, body: &PushRequest) -> PushResponse {
    client
        .post(format!("{}/push", base))
        .json(body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

async fn do_pull(client: &Client, base: &str, body: &PullRequest) -> PullResponse {
    client
        .post(format!("{}/pull", base))
        .json(body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

async fn do_ack(client: &Client, base: &str, body: &AckRequest) -> AckResponse {
    client
        .post(format!("{}/ack", base))
        .json(body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

fn ids(items: &[DeliveryItem]) -> Vec<Uuid> {
    items.iter().map(|i| i.message_id).collect()
}

#[actix_rt::test]
async fn push_then_pull_returns_items_in_order() {
    let cfg = memory_config();
    let running = start(cfg).await.unwrap();
    let client = Client::new();
    let first = push_body("alice", 0, 1, "one");
    let second = push_body("alice", 0, 1, "two");
    let third = push_body("alice", 0, 1, "three");
    let _ = do_push(&client, &running.base_url, &first).await;
    let _ = do_push(&client, &running.base_url, &second).await;
    let _ = do_push(&client, &running.base_url, &third).await;
    let pull = do_pull(
        &client,
        &running.base_url,
        &PullRequest {
            recipient: "alice".to_string(),
            cursor: None,
            max: Some(10),
        },
    )
    .await;
    assert_eq!(pull.items.len(), 3);
    assert_eq!(
        ids(&pull.items),
        vec![first.message_id, second.message_id, third.message_id]
    );
    let ack = AckRequest {
        recipient: "alice".to_string(),
        ack: pull
            .items
            .iter()
            .map(|i| AckEntry {
                message_id: i.message_id,
                chunk_index: i.meta.chunk_index,
            })
            .collect(),
    };
    let ack_resp = do_ack(&client, &running.base_url, &ack).await;
    assert_eq!(ack_resp.deleted, 3);
    let pull_empty = do_pull(
        &client,
        &running.base_url,
        &PullRequest {
            recipient: "alice".to_string(),
            cursor: None,
            max: Some(10),
        },
    )
    .await;
    assert!(pull_empty.items.is_empty());
    let _ = running.shutdown.send(());
    let _ = running.handle.await;
}

#[actix_rt::test]
async fn duplicate_push_is_idempotent() {
    let cfg = memory_config();
    let running = start(cfg).await.unwrap();
    let client = Client::new();
    let body = push_body("bob", 0, 1, "payload");
    let first = do_push(&client, &running.base_url, &body).await;
    assert!(first.stored);
    assert!(!first.duplicate);
    let second = do_push(&client, &running.base_url, &body).await;
    assert!(!second.stored);
    assert!(second.duplicate);
    assert_eq!(second.queue_len, Some(1));
    let _ = running.shutdown.send(());
    let _ = running.handle.await;
}

#[actix_rt::test]
async fn ack_deletes_and_is_idempotent() {
    let cfg = memory_config();
    let running = start(cfg).await.unwrap();
    let client = Client::new();
    let body = push_body("carol", 0, 1, "payload");
    let _ = do_push(&client, &running.base_url, &body).await;
    let ack_body = AckRequest {
        recipient: "carol".to_string(),
        ack: vec![AckEntry {
            message_id: body.message_id,
            chunk_index: 0,
        }],
    };
    let ack_first = do_ack(&client, &running.base_url, &ack_body).await;
    assert_eq!(ack_first.deleted, 1);
    assert_eq!(ack_first.remaining, 0);
    let ack_second = do_ack(&client, &running.base_url, &ack_body).await;
    assert_eq!(ack_second.deleted, 0);
    assert_eq!(ack_second.missing, 1);
    let _ = running.shutdown.send(());
    let _ = running.handle.await;
}

#[actix_rt::test]
async fn quota_blocks_when_exceeded() {
    let mut cfg = memory_config();
    cfg.relay.max_messages_per_user = 1;
    cfg.relay.max_queue_bytes_per_user = 32;
    let running = start(cfg).await.unwrap();
    let client = Client::new();
    let ok = push_body("dave", 0, 1, "one");
    let _ = do_push(&client, &running.base_url, &ok).await;
    let too_many = push_body("dave", 0, 1, "two");
    let resp = client
        .post(format!("{}/push", running.base_url))
        .json(&too_many)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 409);
    let err_json: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(err_json["error"]["code"], "quota_exceeded");
    let pull = do_pull(
        &client,
        &running.base_url,
        &PullRequest {
            recipient: "dave".to_string(),
            cursor: None,
            max: Some(10),
        },
    )
    .await;
    assert_eq!(pull.items.len(), 1);
    let _ = running.shutdown.send(());
    let _ = running.handle.await;
}

#[actix_rt::test]
async fn ttl_gc_removes_expired() {
    let mut cfg = memory_config();
    cfg.relay.message_ttl_seconds = 1;
    cfg.relay.gc_interval_seconds = 1;
    let running = start(cfg).await.unwrap();
    let client = Client::new();
    let body = push_body("erin", 0, 1, "ttl");
    let _ = do_push(&client, &running.base_url, &body).await;
    sleep(Duration::from_millis(1500)).await;
    let pull = do_pull(
        &client,
        &running.base_url,
        &PullRequest {
            recipient: "erin".to_string(),
            cursor: None,
            max: Some(10),
        },
    )
    .await;
    assert!(pull.items.is_empty());
    let _ = running.shutdown.send(());
    let _ = running.handle.await;
}

#[actix_rt::test]
async fn rate_limit_trips_after_threshold() {
    let mut cfg = memory_config();
    cfg.rate_limit.enabled = true;
    cfg.rate_limit.per_ip_rps = 1;
    cfg.rate_limit.burst = 1;
    cfg.rate_limit.endpoints.push_rps = 1;
    cfg.rate_limit.endpoints.pull_rps = 1;
    cfg.rate_limit.endpoints.ack_rps = 1;
    cfg.rate_limit.ban_seconds = 1;
    let running = start(cfg).await.unwrap();
    let client = Client::new();
    let body = push_body("frank", 0, 1, "one");
    let first = client
        .post(format!("{}/push", running.base_url))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), 200);
    let second = client
        .post(format!("{}/push", running.base_url))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(second.status(), 429);
    let err_json: serde_json::Value = second.json().await.unwrap();
    assert_eq!(err_json["error"]["code"], "rate_limited");
    let _ = running.shutdown.send(());
    let _ = running.handle.await;
}

#[cfg(feature = "tls")]
#[actix_rt::test]
async fn tls_builder_smoke() {
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    let cert_pem = cert.cert.pem();
    let key_pem = cert.key_pair.serialize_pem();
    let dir = TempDir::new().unwrap();
    let cert_path = dir.path().join("cert.pem");
    let key_path = dir.path().join("key.pem");
    std::fs::write(&cert_path, cert_pem).unwrap();
    std::fs::write(&key_path, key_pem).unwrap();
    let mut cfg = RelayConfig::default();
    cfg.address = "127.0.0.1:0".to_string();
    cfg.storage.kind = StorageKind::Memory;
    cfg.mode = RelayMode::Tls;
    cfg.rate_limit.enabled = false;
    cfg.tls = Some(TlsConfig {
        cert_pem_path: cert_path.to_string_lossy().to_string(),
        key_pem_path: key_path.to_string_lossy().to_string(),
        client_ca_pem_path: None,
    });
    let running = start(cfg).await.unwrap();
    let client = Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .unwrap();
    let resp = client
        .get(format!("{}/health", running.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let _ = running.shutdown.send(());
    let _ = running.handle.await;
}

#[actix_rt::test]
async fn random_chunk_indices_preserve_arrival_order() {
    let store = Arc::new(MemStore::new());
    let mut cfg = memory_config();
    cfg.storage.kind = StorageKind::Memory;
    let running = start_with_store(store.clone(), cfg).await.unwrap();
    let client = Client::new();
    let mut expected = Vec::new();
    for i in 0..50 {
        let chunk_index = rand::thread_rng().gen_range(0..5);
        let body = push_body("gina", chunk_index, 5, &format!("chunk-{i}"));
        expected.push(body.message_id);
        let _ = do_push(&client, &running.base_url, &body).await;
    }
    let pull = do_pull(
        &client,
        &running.base_url,
        &PullRequest {
            recipient: "gina".to_string(),
            cursor: None,
            max: Some(100),
        },
    )
    .await;
    assert_eq!(pull.items.len(), 50);
    assert_eq!(ids(&pull.items), expected);
    let _ = running.shutdown.send(());
    let _ = running.handle.await;
}
