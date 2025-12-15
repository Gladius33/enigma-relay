use std::collections::HashMap;
use std::ops::Bound;

use async_trait::async_trait;
use enigma_node_types::{RelayEnvelope, UserId};
use sled::Tree;
use uuid::Uuid;

use crate::config::RelayConfig;
use crate::error::{EnigmaRelayError, Result};
use crate::store::{decode_cursor, effective_expiry, encode_cursor, RelayStore};

pub struct SledStore {
    cfg: RelayConfig,
    by_user: Tree,
    id_index: Tree,
}

impl SledStore {
    pub fn new(path: &str, cfg: RelayConfig) -> Result<Self> {
        let db = sled::open(path)?;
        let by_user = db.open_tree("by_user")?;
        let id_index = db.open_tree("id_index")?;
        Ok(SledStore {
            cfg,
            by_user,
            id_index,
        })
    }
}

#[async_trait]
impl RelayStore for SledStore {
    async fn push(
        &self,
        envs: Vec<RelayEnvelope>,
        cfg: &RelayConfig,
        _now_ms: u64,
    ) -> Result<usize> {
        if envs.len() > cfg.max_envelopes_per_push {
            return Err(EnigmaRelayError::InvalidInput("too_many_envelopes"));
        }
        let mut inserted = 0;
        for env in &envs {
            if self.id_index.contains_key(env.id.as_bytes())? {
                return Err(EnigmaRelayError::Conflict);
            }
        }
        let mut per_user: HashMap<UserId, usize> = HashMap::new();
        for env in &envs {
            *per_user.entry(env.to).or_insert(0usize) += 1;
        }
        for (user, add) in per_user.iter() {
            let prefix = by_user_prefix(user);
            let mut count = 0usize;
            for entry in self.by_user.scan_prefix(prefix.clone()) {
                entry?;
                count += 1;
            }
            if count + add > cfg.max_queue_per_user {
                return Err(EnigmaRelayError::Conflict);
            }
        }
        for env in envs {
            let key = by_user_key(&env.to, env.created_at_ms, env.id);
            let value = serde_json::to_vec(&env)?;
            self.by_user.insert(key.clone(), value)?;
            self.id_index.insert(env.id.as_bytes(), key)?;
            inserted += 1;
        }
        self.by_user.flush_async().await?;
        self.id_index.flush_async().await?;
        Ok(inserted)
    }

    async fn pull(
        &self,
        to: UserId,
        cursor: Option<String>,
        limit: usize,
        now_ms: u64,
    ) -> Result<(Vec<RelayEnvelope>, Option<String>)> {
        let prefix = by_user_prefix(&to);
        let start_bound = if let Some(raw) = cursor {
            let (cursor_user, created_at_ms, id) = decode_cursor(&raw)?;
            if cursor_user != to {
                return Err(EnigmaRelayError::InvalidInput("invalid cursor"));
            }
            Bound::Excluded(by_user_key(&to, created_at_ms, id))
        } else {
            Bound::Included(prefix.clone())
        };
        let mut envelopes = Vec::new();
        let iter = self.by_user.range((start_bound, Bound::Unbounded));
        for entry in iter {
            let (key, value) = entry?;
            if !key.starts_with(&prefix) {
                break;
            }
            let env: RelayEnvelope = serde_json::from_slice(&value)?;
            if effective_expiry(&env, &self.cfg) <= now_ms {
                continue;
            }
            envelopes.push(env);
            if envelopes.len() > limit {
                break;
            }
        }
        let mut more = false;
        if envelopes.len() > limit {
            envelopes.pop();
            more = true;
        }
        let next_cursor = if more {
            envelopes
                .last()
                .map(|last| encode_cursor(&to, last.created_at_ms, last.id))
        } else {
            None
        };
        Ok((envelopes, next_cursor))
    }

    async fn ack(&self, ids: Vec<Uuid>) -> Result<usize> {
        let mut removed = 0;
        for id in ids {
            let id_bytes = id.as_bytes();
            if let Some(key_bytes) = self.id_index.get(id_bytes)? {
                self.by_user.remove(key_bytes.as_ref())?;
                self.id_index.remove(id_bytes)?;
                removed += 1;
            }
        }
        self.by_user.flush_async().await?;
        self.id_index.flush_async().await?;
        Ok(removed)
    }

    async fn purge_expired(&self, now_ms: u64) -> Result<usize> {
        let mut to_remove: Vec<(Vec<u8>, Uuid)> = Vec::new();
        for entry in self.by_user.iter() {
            let (key, value) = entry?;
            let env: RelayEnvelope = serde_json::from_slice(&value)?;
            if effective_expiry(&env, &self.cfg) <= now_ms {
                to_remove.push((key.to_vec(), env.id));
            }
        }
        let mut removed = 0;
        for (key, id) in to_remove {
            if self.by_user.remove(key.as_slice())?.is_some() {
                removed += 1;
            }
            self.id_index.remove(id.as_bytes())?;
        }
        self.by_user.flush_async().await?;
        self.id_index.flush_async().await?;
        Ok(removed)
    }
}

fn by_user_prefix(user: &UserId) -> Vec<u8> {
    let mut key = Vec::with_capacity(64 + 1);
    key.extend_from_slice(user.to_hex().as_bytes());
    key.push(b':');
    key
}

fn by_user_key(user: &UserId, created_at_ms: u64, id: Uuid) -> Vec<u8> {
    let mut key = by_user_prefix(user);
    key.extend_from_slice(&created_at_ms.to_be_bytes());
    key.push(b':');
    key.extend_from_slice(id.as_bytes());
    key
}
