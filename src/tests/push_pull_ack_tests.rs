use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use enigma_node_types::{
    OpaqueMessage, RelayAckRequest, RelayAckResponse, RelayEnvelope, RelayKind, RelayPullResponse,
    RelayPushRequest, RelayPushResponse, UserId,
};
use reqwest::Client;
use uuid::Uuid;

use crate::server::now_millis;
use crate::start;
use crate::RelayConfig;

fn envelope(to: UserId, seed: u64) -> RelayEnvelope {
    let blob = STANDARD.encode(format!("payload-{seed}").as_bytes());
    RelayEnvelope {
        id: Uuid::new_v4(),
        to,
        from: None,
        created_at_ms: now_millis(),
        expires_at_ms: None,
        kind: RelayKind::OpaqueMessage(OpaqueMessage {
            blob_b64: blob,
            content_type: None,
        }),
    }
}

#[tokio::test]
async fn push_pull_ack_flow() {
    let running = start(RelayConfig::default()).await.unwrap();
    let client = Client::new();
    let to = UserId::from_username("alice").unwrap();
    let envs = vec![envelope(to, 1), envelope(to, 2), envelope(to, 3)];
    let push_resp = client
        .post(format!("{}/relay/push", running.base_url))
        .json(&RelayPushRequest {
            envelopes: envs.clone(),
        })
        .send()
        .await
        .unwrap();
    assert_eq!(push_resp.status(), 200);
    let push_body: RelayPushResponse = push_resp.json().await.unwrap();
    assert_eq!(push_body.accepted, 3);
    let pull_resp = client
        .get(format!("{}/relay/pull/{}", running.base_url, to.to_hex()))
        .query(&[("limit", 10usize)])
        .send()
        .await
        .unwrap();
    assert_eq!(pull_resp.status(), 200);
    let pull_body: RelayPullResponse = pull_resp.json().await.unwrap();
    assert_eq!(pull_body.envelopes.len(), 3);
    let ids: Vec<Uuid> = pull_body.envelopes.iter().map(|e| e.id).collect();
    let ack_resp = client
        .post(format!("{}/relay/ack", running.base_url))
        .json(&RelayAckRequest { ids: ids.clone() })
        .send()
        .await
        .unwrap();
    assert_eq!(ack_resp.status(), 200);
    let ack_body: RelayAckResponse = ack_resp.json().await.unwrap();
    assert_eq!(ack_body.removed, 3);
    let pull_resp = client
        .get(format!("{}/relay/pull/{}", running.base_url, to.to_hex()))
        .send()
        .await
        .unwrap();
    assert_eq!(pull_resp.status(), 200);
    let pull_body: RelayPullResponse = pull_resp.json().await.unwrap();
    assert_eq!(pull_body.envelopes.len(), 0);
    let _ = running.shutdown.send(());
    let _ = running.handle.await;
}
