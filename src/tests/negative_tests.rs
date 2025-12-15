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

fn envelope(to: UserId, seq: u64, blob: String) -> RelayEnvelope {
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
async fn invalid_inputs_are_rejected() {
    let mut cfg = RelayConfig::default();
    cfg.max_envelopes_per_push = 1;
    let running = start(cfg).await.unwrap();
    let client = Client::new();
    let bad_pull = client
        .get(format!("{}/relay/pull/not-a-hex", running.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(bad_pull.status(), 400);
    let user = UserId::from_username("dave").unwrap();
    let envs = vec![
        envelope(user, 1, STANDARD.encode(b"one")),
        envelope(user, 2, STANDARD.encode(b"two")),
    ];
    let too_many = client
        .post(format!("{}/relay/push", running.base_url))
        .json(&RelayPushRequest { envelopes: envs })
        .send()
        .await
        .unwrap();
    assert_eq!(too_many.status(), 400);
    let invalid_blob = client
        .post(format!("{}/relay/push", running.base_url))
        .json(&RelayPushRequest {
            envelopes: vec![envelope(user, 3, "%%%".to_string())],
        })
        .send()
        .await
        .unwrap();
    assert_eq!(invalid_blob.status(), 400);
    let mut bulk_cfg = RelayConfig::default();
    bulk_cfg.max_envelopes_per_push = 300;
    let bulk_running = start(bulk_cfg).await.unwrap();
    let mut many = Vec::new();
    for i in 0..200u64 {
        many.push(envelope(
            user,
            i,
            STANDARD.encode(format!("msg-{i}").as_bytes()),
        ));
    }
    let push_ok = client
        .post(format!("{}/relay/push", bulk_running.base_url))
        .json(&RelayPushRequest { envelopes: many })
        .send()
        .await
        .unwrap();
    assert_eq!(push_ok.status(), 200);
    let query = PullQuery {
        limit: 10_000,
        cursor: None,
    };
    let pull = client
        .get(format!(
            "{}/relay/pull/{}",
            bulk_running.base_url,
            user.to_hex()
        ))
        .query(&query)
        .send()
        .await
        .unwrap();
    assert_eq!(pull.status(), 200);
    let body: RelayPullResponse = pull.json().await.unwrap();
    assert_eq!(body.envelopes.len(), 200);
    assert!(body.next_cursor.is_none());
    let _ = running.shutdown.send(());
    let _ = running.handle.await;
    let _ = bulk_running.shutdown.send(());
    let _ = bulk_running.handle.await;
}
