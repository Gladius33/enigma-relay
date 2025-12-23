use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct MessageMeta {
    pub kind: String,
    pub total_len: u64,
    pub chunk_index: u32,
    pub chunk_count: u32,
    pub sent_ms: u64,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct PushRequest {
    pub recipient: String,
    pub message_id: Uuid,
    pub ciphertext_b64: String,
    pub meta: MessageMeta,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct PushResponse {
    pub stored: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub duplicate: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queue_len: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queue_bytes: Option<u64>,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct PullRequest {
    pub recipient: String,
    pub cursor: Option<String>,
    pub max: Option<u64>,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct DeliveryItem {
    pub recipient: String,
    pub message_id: Uuid,
    pub ciphertext_b64: String,
    pub meta: MessageMeta,
    pub arrival_ms: u64,
    pub deadline_ms: u64,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct PullResponse {
    pub items: Vec<DeliveryItem>,
    pub next_cursor: Option<String>,
    pub remaining_estimate: u64,
}

#[derive(Clone, Serialize, Deserialize, Debug, Eq, PartialEq, Hash)]
pub struct AckEntry {
    pub message_id: Uuid,
    pub chunk_index: u32,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct AckRequest {
    pub recipient: String,
    pub ack: Vec<AckEntry>,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct AckResponse {
    pub deleted: u64,
    pub missing: u64,
    pub remaining: u64,
}

fn is_false(value: &bool) -> bool {
    !*value
}
