use std::sync::Arc;

use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use enigma_node_types::{RelayEnvelope, UserId};
use uuid::Uuid;

use crate::config::RelayConfig;
use crate::error::{EnigmaRelayError, Result};

pub type DynRelayStore = Arc<dyn RelayStore + Send + Sync>;

#[async_trait]
pub trait RelayStore: Send + Sync {
    async fn push(&self, envs: Vec<RelayEnvelope>, cfg: &RelayConfig, now_ms: u64)
        -> Result<usize>;
    async fn pull(
        &self,
        to: UserId,
        cursor: Option<String>,
        limit: usize,
        now_ms: u64,
    ) -> Result<(Vec<RelayEnvelope>, Option<String>)>;
    async fn ack(&self, ids: Vec<Uuid>) -> Result<usize>;
    async fn purge_expired(&self, now_ms: u64) -> Result<usize>;
}

pub fn encode_cursor(user: &UserId, created_at_ms: u64, id: Uuid) -> String {
    let sort_key = format!("{:020}:{}", created_at_ms, id.as_hyphenated());
    let raw = format!("{}:{}", user.to_hex(), sort_key);
    STANDARD.encode(raw.as_bytes())
}

pub fn decode_cursor(cursor: &str) -> Result<(UserId, u64, Uuid)> {
    let decoded = STANDARD
        .decode(cursor.as_bytes())
        .map_err(|_| EnigmaRelayError::InvalidInput("invalid cursor"))?;
    let text =
        String::from_utf8(decoded).map_err(|_| EnigmaRelayError::InvalidInput("invalid cursor"))?;
    let mut parts = text.splitn(2, ':');
    let user_hex = parts
        .next()
        .ok_or(EnigmaRelayError::InvalidInput("invalid cursor"))?;
    let sort = parts
        .next()
        .ok_or(EnigmaRelayError::InvalidInput("invalid cursor"))?;
    let user =
        UserId::from_hex(user_hex).map_err(|_| EnigmaRelayError::InvalidInput("invalid cursor"))?;
    let (created_at_ms, id) = parse_sort_key(sort)?;
    Ok((user, created_at_ms, id))
}

pub fn effective_expiry(envelope: &RelayEnvelope, cfg: &RelayConfig) -> u64 {
    match envelope.expires_at_ms {
        Some(value) => value,
        None => envelope
            .created_at_ms
            .saturating_add(cfg.retention_ms_default),
    }
}

fn parse_sort_key(sort: &str) -> Result<(u64, Uuid)> {
    let mut parts = sort.splitn(2, ':');
    let ts = parts
        .next()
        .ok_or(EnigmaRelayError::InvalidInput("invalid cursor"))?;
    let id_text = parts
        .next()
        .ok_or(EnigmaRelayError::InvalidInput("invalid cursor"))?;
    let created_at_ms = ts
        .parse::<u64>()
        .map_err(|_| EnigmaRelayError::InvalidInput("invalid cursor"))?;
    let id =
        Uuid::parse_str(id_text).map_err(|_| EnigmaRelayError::InvalidInput("invalid cursor"))?;
    Ok((created_at_ms, id))
}
