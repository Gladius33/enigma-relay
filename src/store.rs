use std::sync::Arc;

use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::RelayLimits;
use crate::error::{EnigmaRelayError, Result};
use crate::model::MessageMeta;

pub type DynRelayStore = Arc<dyn RelayStore + Send + Sync>;

#[derive(Clone, Debug)]
pub struct InboundMessage {
    pub recipient: String,
    pub message_id: Uuid,
    pub ciphertext_b64: String,
    pub meta: MessageMeta,
    pub payload_bytes: u64,
    pub arrival_ms: u64,
    pub deadline_ms: u64,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct QueueMessage {
    pub recipient: String,
    pub message_id: Uuid,
    pub ciphertext_b64: String,
    pub meta: MessageMeta,
    pub arrival_ms: u64,
    pub deadline_ms: u64,
    pub payload_bytes: u64,
    pub arrival_seq: u64,
}

#[derive(Clone, Debug)]
pub struct PushResult {
    pub stored: bool,
    pub duplicate: bool,
    pub queue_len: u64,
    pub queue_bytes: u64,
}

#[derive(Clone, Debug)]
pub struct PullBatch {
    pub items: Vec<QueueMessage>,
    pub next_cursor: Option<String>,
    pub remaining_estimate: u64,
}

#[derive(Clone, Debug, Default)]
pub struct AckOutcome {
    pub deleted: u64,
    pub missing: u64,
    pub remaining: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct AckItem {
    pub message_id: Uuid,
    pub chunk_index: u32,
}

#[async_trait]
pub trait RelayStore: Send + Sync {
    async fn push(&self, msg: InboundMessage, limits: &RelayLimits) -> Result<PushResult>;
    async fn pull(
        &self,
        recipient: &str,
        cursor: Option<String>,
        limit: u64,
        now_ms: u64,
    ) -> Result<PullBatch>;
    async fn ack(&self, recipient: &str, items: Vec<AckItem>) -> Result<AckOutcome>;
    async fn purge_expired(&self, now_ms: u64, max: usize) -> Result<usize>;
}

pub fn encode_cursor(recipient: &str, arrival_seq: u64) -> String {
    let raw = format!("{}:{:020}", recipient, arrival_seq);
    STANDARD.encode(raw.as_bytes())
}

pub fn decode_cursor(cursor: &str) -> Result<(String, u64)> {
    let decoded = STANDARD
        .decode(cursor.as_bytes())
        .map_err(|_| EnigmaRelayError::InvalidInput("invalid cursor".to_string()))?;
    let text = String::from_utf8(decoded)
        .map_err(|_| EnigmaRelayError::InvalidInput("invalid cursor".to_string()))?;
    let mut parts = text.splitn(2, ':');
    let recipient = parts
        .next()
        .ok_or_else(|| EnigmaRelayError::InvalidInput("invalid cursor".to_string()))?;
    let seq = parts
        .next()
        .ok_or_else(|| EnigmaRelayError::InvalidInput("invalid cursor".to_string()))?;
    let arrival_seq = seq
        .parse::<u64>()
        .map_err(|_| EnigmaRelayError::InvalidInput("invalid cursor".to_string()))?;
    Ok((recipient.to_string(), arrival_seq))
}
