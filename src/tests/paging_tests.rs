use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use enigma_node_types::{
    OpaqueMessage, RelayEnvelope, RelayKind, RelayPullResponse, RelayPushRequest, UserId,
};
use reqwest::Client;
use serde::Serialize;
use uuid::Uuid;

use crate::server::now_millis;
use crate::start;
use crate::RelayConfig;

fn envelope(to: UserId, seq: u64) -> RelayEnvelope {
    let blob = STANDARD.encode(format!("msg-{seq}").as_bytes());
    RelayEnvelope {
        id: Uuid::new_v4(),
        to,
        from: None,
        created_at_ms: now_millis().saturating_add(seq),
        expires_at_ms: None,
        kind: RelayKind::OpaqueMessage(OpaqueMessage {
            blob_b64: blob,
            content_type: None,
        }),
    }
}

#[derive(Serialize)]
struct PullQuery {
    limit: usize,
    cursor: Option<String>,
}

#[tokio::test]
async fn paging_walks_all_envelopes() {
    let running = start(RelayConfig::default()).await.unwrap();
    let client = Client::new();
    let to = UserId::from_username("bob").unwrap();
    let mut envs = Vec::new();
    for i in 0..200u64 {
        envs.push(envelope(to, i));
    }
    let push_resp = client
        .post(format!("{}/relay/push", running.base_url))
        .json(&RelayPushRequest {
            envelopes: envs.clone(),
        })
        .send()
        .await
        .unwrap();
    assert_eq!(push_resp.status(), 200);
    let mut cursor: Option<String> = None;
    let mut received: Vec<RelayEnvelope> = Vec::new();
    loop {
        let query = PullQuery {
            limit: 50,
            cursor: cursor.clone(),
        };
        let resp = client
            .get(format!("{}/relay/pull/{}", running.base_url, to.to_hex()))
            .query(&query)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body: RelayPullResponse = resp.json().await.unwrap();
        received.extend(body.envelopes.clone());
        cursor = body.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(received.len(), 200);
    let mut expected = envs.clone();
    expected.sort_by_key(|e| (e.created_at_ms, e.id));
    let mut actual = received.clone();
    actual.sort_by_key(|e| (e.created_at_ms, e.id));
    let expected_ids: Vec<Uuid> = expected.iter().map(|e| e.id).collect();
    let actual_ids: Vec<Uuid> = actual.iter().map(|e| e.id).collect();
    assert_eq!(expected_ids, actual_ids);
    let _ = running.shutdown.send(());
    let _ = running.handle.await;
}
