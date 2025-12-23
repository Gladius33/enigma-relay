use std::collections::{HashMap, HashSet};

use async_trait::async_trait;
use sled::transaction::Transactional;
use sled::{transaction::ConflictableTransactionError, Tree};
use uuid::Uuid;

use crate::config::RelayLimits;
use crate::error::{EnigmaRelayError, Result};
use crate::store::{
    decode_cursor, encode_cursor, AckItem, AckOutcome, InboundMessage, PullBatch, PushResult,
    QueueMessage, RelayStore,
};

const GC_BATCH_LIMIT: usize = 1024;

#[derive(Clone, serde::Serialize, serde::Deserialize, Default)]
struct QuotaValue {
    bytes: u64,
    count: u64,
}

pub struct SledStore {
    messages: Tree,
    index: Tree,
    quota: Tree,
    seq: Tree,
}

impl SledStore {
    pub fn new(path: &str) -> Result<Self> {
        let db = sled::open(path)?;
        let messages = db.open_tree("messages")?;
        let index = db.open_tree("index")?;
        let quota = db.open_tree("quota")?;
        let seq = db.open_tree("seq")?;
        Ok(SledStore {
            messages,
            index,
            quota,
            seq,
        })
    }
}

#[async_trait]
impl RelayStore for SledStore {
    async fn push(&self, msg: InboundMessage, limits: &RelayLimits) -> Result<PushResult> {
        let idx_key = index_key(&msg.recipient, msg.message_id, msg.meta.chunk_index);
        let quota_key = quota_key(&msg.recipient);
        let seq_key = seq_key(&msg.recipient);
        let res: sled::transaction::TransactionResult<PushResult, EnigmaRelayError> = (
            &self.messages,
            &self.index,
            &self.quota,
            &self.seq,
        )
            .transaction(|(messages, index, quota, seq)| {
                if let Some(_) = index.get(&idx_key)? {
                    let quota_value = read_quota_tx(quota, &quota_key)?;
                    return Ok(PushResult {
                        stored: false,
                        duplicate: true,
                        queue_len: quota_value.count,
                        queue_bytes: quota_value.bytes,
                    });
                }
                let mut quota_value = read_quota_tx(quota, &quota_key)?;
                let next_count = quota_value.count.saturating_add(1);
                if next_count > limits.max_messages_per_user {
                    return Err(ConflictableTransactionError::Abort(
                        EnigmaRelayError::QuotaExceeded,
                    ));
                }
                let next_bytes = quota_value.bytes.saturating_add(msg.payload_bytes);
                if next_bytes > limits.max_queue_bytes_per_user {
                    return Err(ConflictableTransactionError::Abort(
                        EnigmaRelayError::QuotaExceeded,
                    ));
                }
                let current_seq = seq
                    .get(&seq_key)?
                    .map(|v| decode_u64(v.as_ref()))
                    .transpose()
                    .map_err(ConflictableTransactionError::Abort)?
                    .unwrap_or(0);
                let arrival_seq = current_seq;
                let stored = QueueMessage {
                    arrival_seq,
                    recipient: msg.recipient.clone(),
                    message_id: msg.message_id,
                    ciphertext_b64: msg.ciphertext_b64.clone(),
                    meta: msg.meta.clone(),
                    arrival_ms: msg.arrival_ms,
                    deadline_ms: msg.deadline_ms,
                    payload_bytes: msg.payload_bytes,
                };
                let msg_key = message_key(
                    &msg.recipient,
                    arrival_seq,
                    msg.message_id,
                    msg.meta.chunk_index,
                );
                let data = serde_json::to_vec(&stored).map_err(|e| {
                    ConflictableTransactionError::Abort(EnigmaRelayError::Internal(e.to_string()))
                })?;
                messages.insert(msg_key, data)?;
                index.insert(idx_key.clone(), encode_u64(arrival_seq).to_vec())?;
                quota_value.count = next_count;
                quota_value.bytes = next_bytes;
                let quota_data = serde_json::to_vec(&quota_value).map_err(|e| {
                    ConflictableTransactionError::Abort(EnigmaRelayError::Internal(e.to_string()))
                })?;
                quota.insert(quota_key.clone(), quota_data)?;
                let next_seq = arrival_seq.saturating_add(1);
                seq.insert(seq_key.clone(), encode_u64(next_seq).to_vec())?;
                Ok(PushResult {
                    stored: true,
                    duplicate: false,
                    queue_len: quota_value.count,
                    queue_bytes: quota_value.bytes,
                })
            });
        match res {
            Ok(outcome) => {
                self.messages.flush_async().await?;
                self.index.flush_async().await?;
                self.quota.flush_async().await?;
                self.seq.flush_async().await?;
                Ok(outcome)
            }
            Err(sled::transaction::TransactionError::Abort(err)) => Err(err),
            Err(sled::transaction::TransactionError::Storage(err)) => Err(err.into()),
        }
    }

    async fn pull(
        &self,
        recipient: &str,
        cursor: Option<String>,
        limit: u64,
        now_ms: u64,
    ) -> Result<PullBatch> {
        let start_seq = if let Some(raw) = cursor {
            let decoded = decode_cursor(&raw)?;
            if decoded.0 != recipient {
                return Err(EnigmaRelayError::InvalidInput(
                    "cursor recipient mismatch".to_string(),
                ));
            }
            Some(decoded.1)
        } else {
            None
        };
        let mut items = Vec::new();
        let mut more = false;
        let prefix = message_prefix(recipient);
        let limit_usize = limit.min(u64::from(u32::MAX)) as usize;
        for entry in self.messages.scan_prefix(prefix) {
            let (key, value) = entry?;
            if let Some(seq) = parse_seq_from_key(&key)? {
                if let Some(start) = start_seq {
                    if seq <= start {
                        continue;
                    }
                }
                let msg: QueueMessage = serde_json::from_slice(&value)?;
                if msg.deadline_ms <= now_ms {
                    continue;
                }
                if items.len() < limit_usize {
                    items.push(msg);
                } else {
                    more = true;
                    break;
                }
            }
        }
        let next_cursor = if more {
            items
                .last()
                .map(|item| encode_cursor(recipient, item.arrival_seq))
        } else {
            None
        };
        let quota_value = self.read_quota_latest(recipient)?;
        let remaining_estimate = quota_value.count.saturating_sub(items.len() as u64);
        Ok(PullBatch {
            items,
            next_cursor,
            remaining_estimate,
        })
    }

    async fn ack(&self, recipient: &str, items: Vec<AckItem>) -> Result<AckOutcome> {
        let idx_keys: HashSet<AckItem> = items.into_iter().collect();
        let quota_key = quota_key(recipient);
        let res: sled::transaction::TransactionResult<AckOutcome, EnigmaRelayError> =
            (&self.messages, &self.index, &self.quota).transaction(|(messages, index, quota)| {
                let mut quota_value = read_quota_tx(quota, &quota_key)?;
                let mut outcome = AckOutcome::default();
                for ack_item in idx_keys.iter() {
                    let idx_key = index_key(recipient, ack_item.message_id, ack_item.chunk_index);
                    if let Some(seq_bytes) = index.get(idx_key.clone())? {
                        let seq = decode_u64(seq_bytes.as_ref())
                            .map_err(ConflictableTransactionError::Abort)?;
                        let msg_key =
                            message_key(recipient, seq, ack_item.message_id, ack_item.chunk_index);
                        if let Some(val) = messages.get(msg_key.clone())? {
                            let msg: QueueMessage = serde_json::from_slice(&val).map_err(|e| {
                                ConflictableTransactionError::Abort(EnigmaRelayError::Internal(
                                    e.to_string(),
                                ))
                            })?;
                            messages.remove(msg_key.clone())?;
                            index.remove(idx_key.clone())?;
                            quota_value.count = quota_value.count.saturating_sub(1);
                            quota_value.bytes = quota_value.bytes.saturating_sub(msg.payload_bytes);
                            outcome.deleted = outcome.deleted.saturating_add(1);
                        } else {
                            index.remove(idx_key.clone())?;
                            outcome.missing = outcome.missing.saturating_add(1);
                        }
                    } else {
                        outcome.missing = outcome.missing.saturating_add(1);
                    }
                }
                let quota_data = serde_json::to_vec(&quota_value).map_err(|e| {
                    ConflictableTransactionError::Abort(EnigmaRelayError::Internal(e.to_string()))
                })?;
                quota.insert(quota_key.clone(), quota_data)?;
                outcome.remaining = quota_value.count;
                Ok(outcome)
            });
        match res {
            Ok(outcome) => {
                self.messages.flush_async().await?;
                self.index.flush_async().await?;
                self.quota.flush_async().await?;
                Ok(outcome)
            }
            Err(sled::transaction::TransactionError::Abort(err)) => Err(err),
            Err(sled::transaction::TransactionError::Storage(err)) => Err(err.into()),
        }
    }

    async fn purge_expired(&self, now_ms: u64, max: usize) -> Result<usize> {
        let mut removed = 0usize;
        let target = max.min(GC_BATCH_LIMIT);
        let mut quotas: HashMap<String, QuotaValue> = HashMap::new();
        for entry in self.messages.iter() {
            if removed >= target {
                break;
            }
            let (key, value) = entry?;
            let msg: QueueMessage = serde_json::from_slice(&value)?;
            if msg.deadline_ms > now_ms {
                continue;
            }
            let idx_key = index_key(&msg.recipient, msg.message_id, msg.meta.chunk_index);
            self.messages.remove(key)?;
            self.index.remove(idx_key)?;
            let quota_entry = match quotas.entry(msg.recipient.clone()) {
                std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(self.read_quota_latest(&msg.recipient)?)
                }
            };
            quota_entry.count = quota_entry.count.saturating_sub(1);
            quota_entry.bytes = quota_entry.bytes.saturating_sub(msg.payload_bytes);
            removed += 1;
        }
        for (recipient, quota_value) in quotas {
            let key = quota_key(&recipient);
            let data = serde_json::to_vec(&quota_value)?;
            self.quota.insert(key, data)?;
        }
        self.messages.flush_async().await?;
        self.index.flush_async().await?;
        self.quota.flush_async().await?;
        Ok(removed)
    }
}

impl SledStore {
    fn read_quota_latest(&self, recipient: &str) -> Result<QuotaValue> {
        let key = quota_key(recipient);
        read_quota_tree(&self.quota, &key)
    }
}

fn read_quota_tree(quota: &Tree, key: &[u8]) -> Result<QuotaValue> {
    if let Some(val) = quota.get(key)? {
        let quota: QuotaValue = serde_json::from_slice(val.as_ref())
            .map_err(|e| EnigmaRelayError::Internal(e.to_string()))?;
        Ok(quota)
    } else {
        Ok(QuotaValue::default())
    }
}

fn read_quota_tx(
    quota: &sled::transaction::TransactionalTree,
    key: &[u8],
) -> std::result::Result<QuotaValue, ConflictableTransactionError<EnigmaRelayError>> {
    if let Some(val) = quota.get(key)? {
        serde_json::from_slice(val.as_ref()).map_err(|e| {
            ConflictableTransactionError::Abort(EnigmaRelayError::Internal(e.to_string()))
        })
    } else {
        Ok(QuotaValue::default())
    }
}

fn message_prefix(recipient: &str) -> Vec<u8> {
    let mut key = Vec::with_capacity(4 + recipient.len() + 1);
    key.extend_from_slice(b"msg:");
    key.extend_from_slice(recipient.as_bytes());
    key.push(b':');
    key
}

fn message_key(recipient: &str, arrival_seq: u64, message_id: Uuid, chunk_index: u32) -> Vec<u8> {
    let mut key = message_prefix(recipient);
    key.extend_from_slice(format!("{:020}", arrival_seq).as_bytes());
    key.push(b':');
    key.extend_from_slice(message_id.as_hyphenated().to_string().as_bytes());
    key.push(b':');
    key.extend_from_slice(format!("{:010}", chunk_index).as_bytes());
    key
}

fn index_key(recipient: &str, message_id: Uuid, chunk_index: u32) -> Vec<u8> {
    let mut key = Vec::with_capacity(4 + recipient.len() + 1 + 36 + 1 + 10);
    key.extend_from_slice(b"idx:");
    key.extend_from_slice(recipient.as_bytes());
    key.push(b':');
    key.extend_from_slice(message_id.as_hyphenated().to_string().as_bytes());
    key.push(b':');
    key.extend_from_slice(format!("{:010}", chunk_index).as_bytes());
    key
}

fn quota_key(recipient: &str) -> Vec<u8> {
    let mut key = Vec::with_capacity(6 + recipient.len());
    key.extend_from_slice(b"quota:");
    key.extend_from_slice(recipient.as_bytes());
    key
}

fn seq_key(recipient: &str) -> Vec<u8> {
    let mut key = Vec::with_capacity(4 + recipient.len());
    key.extend_from_slice(b"seq:");
    key.extend_from_slice(recipient.as_bytes());
    key
}

fn parse_seq_from_key(key: &[u8]) -> Result<Option<u64>> {
    let text = String::from_utf8_lossy(key);
    let mut parts = text.split(':');
    let kind = parts.next();
    if kind != Some("msg") {
        return Ok(None);
    }
    let _recipient = parts.next();
    let seq_part = parts.next();
    if let Some(seq_txt) = seq_part {
        let seq = seq_txt
            .parse::<u64>()
            .map_err(|_| EnigmaRelayError::Internal("invalid key".to_string()))?;
        Ok(Some(seq))
    } else {
        Ok(None)
    }
}

fn encode_u64(value: u64) -> [u8; 8] {
    value.to_be_bytes()
}

fn decode_u64(bytes: &[u8]) -> Result<u64> {
    if bytes.len() != 8 {
        return Err(EnigmaRelayError::Internal("invalid integer".to_string()));
    }
    let mut arr = [0u8; 8];
    arr.copy_from_slice(bytes);
    Ok(u64::from_be_bytes(arr))
}
