use std::sync::Arc;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use enigma_node_types::{
    OpaqueMessage, RelayEnvelope, RelayKind, RelayPullResponse, RelayPushRequest, UserId,
};
use reqwest::Client;
use uuid::Uuid;

use crate::server::now_millis;
use crate::start_with_store;
use crate::store::DynRelayStore;
use crate::store_mem::MemStore;
use crate::RelayConfig;

fn envelope(to: UserId, expires: Option<u64>, seq: u64) -> RelayEnvelope {
    let blob = STANDARD.encode(format!("ttl-{seq}").as_bytes());
    RelayEnvelope {
        id: Uuid::new_v4(),
        to,
        from: None,
        created_at_ms: now_millis(),
        expires_at_ms: expires,
        kind: RelayKind::OpaqueMessage(OpaqueMessage {
            blob_b64: blob,
            content_type: None,
        }),
    }
}

#[tokio::test]
async fn expired_envelopes_are_purged() {
    let cfg = RelayConfig::default();
    let store: DynRelayStore = Arc::new(MemStore::new(cfg.clone()));
    let running = start_with_store(store.clone(), cfg.clone()).await.unwrap();
    let client = Client::new();
    let to = UserId::from_username("carol").unwrap();
    let now = now_millis();
    let soon = envelope(to, Some(now + 10), 1);
    let later = envelope(to, Some(now + 10_000), 2);
    let push_resp = client
        .post(format!("{}/relay/push", running.base_url))
        .json(&RelayPushRequest {
            envelopes: vec![soon, later.clone()],
        })
        .send()
        .await
        .unwrap();
    assert_eq!(push_resp.status(), 200);
    tokio::time::sleep(Duration::from_millis(30)).await;
    let _ = store.purge_expired(now_millis()).await.unwrap();
    let pull_resp = client
        .get(format!("{}/relay/pull/{}", running.base_url, to.to_hex()))
        .send()
        .await
        .unwrap();
    assert_eq!(pull_resp.status(), 200);
    let pull_body: RelayPullResponse = pull_resp.json().await.unwrap();
    assert_eq!(pull_body.envelopes.len(), 1);
    assert_eq!(pull_body.envelopes[0].id, later.id);
    let _ = running.shutdown.send(());
    let _ = running.handle.await;
}
