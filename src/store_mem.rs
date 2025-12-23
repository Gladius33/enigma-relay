use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Bound;

use async_trait::async_trait;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::config::RelayLimits;
use crate::error::{EnigmaRelayError, Result};
use crate::store::{
    decode_cursor, encode_cursor, AckItem, AckOutcome, InboundMessage, PullBatch, PushResult,
    QueueMessage, RelayStore,
};

const GC_BATCH_LIMIT: usize = 1024;

#[derive(Default)]
struct RecipientState {
    seq: u64,
    queue: BTreeMap<u64, QueueMessage>,
    idx: HashMap<(Uuid, u32), u64>,
    quota_bytes: u64,
    quota_count: u64,
}

pub struct MemStore {
    state: Mutex<HashMap<String, RecipientState>>,
}

impl MemStore {
    pub fn new() -> Self {
        MemStore {
            state: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl RelayStore for MemStore {
    async fn push(&self, msg: InboundMessage, limits: &RelayLimits) -> Result<PushResult> {
        let mut state = self.state.lock().await;
        let entry = state.entry(msg.recipient.clone()).or_default();
        let dedup_key = (msg.message_id, msg.meta.chunk_index);
        if entry.idx.contains_key(&dedup_key) {
            return Ok(PushResult {
                stored: false,
                duplicate: true,
                queue_len: entry.quota_count,
                queue_bytes: entry.quota_bytes,
            });
        }
        let next_count = entry.quota_count.saturating_add(1);
        if next_count > limits.max_messages_per_user {
            return Err(EnigmaRelayError::QuotaExceeded);
        }
        let next_bytes = entry.quota_bytes.saturating_add(msg.payload_bytes);
        if next_bytes > limits.max_queue_bytes_per_user {
            return Err(EnigmaRelayError::QuotaExceeded);
        }
        let arrival_seq = entry.seq;
        entry.seq = entry.seq.saturating_add(1);
        let stored = QueueMessage {
            arrival_seq,
            recipient: msg.recipient.clone(),
            message_id: msg.message_id,
            ciphertext_b64: msg.ciphertext_b64,
            meta: msg.meta,
            arrival_ms: msg.arrival_ms,
            deadline_ms: msg.deadline_ms,
            payload_bytes: msg.payload_bytes,
        };
        entry.queue.insert(arrival_seq, stored);
        entry.idx.insert(dedup_key, arrival_seq);
        entry.quota_count = next_count;
        entry.quota_bytes = next_bytes;
        Ok(PushResult {
            stored: true,
            duplicate: false,
            queue_len: entry.quota_count,
            queue_bytes: entry.quota_bytes,
        })
    }

    async fn pull(
        &self,
        recipient: &str,
        cursor: Option<String>,
        limit: u64,
        now_ms: u64,
    ) -> Result<PullBatch> {
        let mut state = self.state.lock().await;
        let entry = match state.get_mut(recipient) {
            Some(entry) => entry,
            None => {
                return Ok(PullBatch {
                    items: Vec::new(),
                    next_cursor: None,
                    remaining_estimate: 0,
                })
            }
        };
        let mut start_seq = None;
        if let Some(raw) = cursor {
            let (recip, seq) = decode_cursor(&raw)?;
            if recip != recipient {
                return Err(EnigmaRelayError::InvalidInput(
                    "cursor recipient mismatch".to_string(),
                ));
            }
            start_seq = Some(seq);
        }
        let limit_usize = limit.min(u64::from(u32::MAX)) as usize;
        let mut collected = Vec::new();
        let mut expired = Vec::new();
        let mut more = false;
        let mut entries: Vec<(u64, QueueMessage)> = Vec::new();
        if let Some(seq) = start_seq {
            for (k, v) in entry.queue.range((Bound::Excluded(seq), Bound::Unbounded)) {
                entries.push((*k, v.clone()));
            }
        } else {
            for (k, v) in entry.queue.iter() {
                entries.push((*k, v.clone()));
            }
        }
        for (seq, msg) in entries {
            if msg.deadline_ms <= now_ms {
                expired.push(seq);
                continue;
            }
            if collected.len() < limit_usize {
                collected.push(msg.clone());
            } else {
                more = true;
                break;
            }
        }
        for seq in expired {
            if let Some(removed) = entry.queue.remove(&seq) {
                entry.quota_count = entry.quota_count.saturating_sub(1);
                entry.quota_bytes = entry.quota_bytes.saturating_sub(removed.payload_bytes);
                entry
                    .idx
                    .remove(&(removed.message_id, removed.meta.chunk_index));
            }
        }
        let next_cursor = if more {
            collected
                .last()
                .map(|item| encode_cursor(&item.recipient, item.arrival_seq))
        } else {
            None
        };
        let remaining_estimate = entry.quota_count.saturating_sub(collected.len() as u64);
        Ok(PullBatch {
            items: collected,
            next_cursor,
            remaining_estimate,
        })
    }

    async fn ack(&self, recipient: &str, items: Vec<AckItem>) -> Result<AckOutcome> {
        let mut state = self.state.lock().await;
        let entry = match state.get_mut(recipient) {
            Some(entry) => entry,
            None => {
                return Ok(AckOutcome {
                    deleted: 0,
                    missing: items.len() as u64,
                    remaining: 0,
                })
            }
        };
        let mut unique: HashSet<AckItem> = HashSet::new();
        for item in items {
            unique.insert(item);
        }
        let mut outcome = AckOutcome::default();
        for ack_item in unique {
            let key = (ack_item.message_id, ack_item.chunk_index);
            if let Some(seq) = entry.idx.remove(&key) {
                if let Some(removed) = entry.queue.remove(&seq) {
                    entry.quota_count = entry.quota_count.saturating_sub(1);
                    entry.quota_bytes = entry.quota_bytes.saturating_sub(removed.payload_bytes);
                    outcome.deleted = outcome.deleted.saturating_add(1);
                }
            } else {
                outcome.missing = outcome.missing.saturating_add(1);
            }
        }
        outcome.remaining = entry.quota_count;
        Ok(outcome)
    }

    async fn purge_expired(&self, now_ms: u64, max: usize) -> Result<usize> {
        let mut state = self.state.lock().await;
        let mut removed = 0usize;
        let target = max.min(GC_BATCH_LIMIT);
        let mut recipients: Vec<String> = state.keys().cloned().collect();
        recipients.sort();
        for recip in recipients {
            if removed >= target {
                break;
            }
            if let Some(entry) = state.get_mut(&recip) {
                let mut to_drop = Vec::new();
                for (seq, msg) in entry.queue.iter() {
                    if msg.deadline_ms <= now_ms {
                        to_drop.push(*seq);
                        if removed + to_drop.len() >= target {
                            break;
                        }
                    } else {
                        break;
                    }
                }
                for seq in to_drop {
                    if let Some(removed_msg) = entry.queue.remove(&seq) {
                        entry
                            .idx
                            .remove(&(removed_msg.message_id, removed_msg.meta.chunk_index));
                        entry.quota_count = entry.quota_count.saturating_sub(1);
                        entry.quota_bytes =
                            entry.quota_bytes.saturating_sub(removed_msg.payload_bytes);
                        removed += 1;
                    }
                    if removed >= target {
                        break;
                    }
                }
            }
        }
        Ok(removed)
    }
}
