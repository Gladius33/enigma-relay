use std::collections::{BTreeMap, HashMap};
use std::ops::Bound;

use async_trait::async_trait;
use enigma_node_types::{RelayEnvelope, UserId};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::config::RelayConfig;
use crate::error::{EnigmaRelayError, Result};
use crate::store::{decode_cursor, effective_expiry, encode_cursor, RelayStore};

pub struct MemStore {
    cfg: RelayConfig,
    by_user: RwLock<HashMap<UserId, BTreeMap<(u64, Uuid), RelayEnvelope>>>,
    id_index: RwLock<HashMap<Uuid, (UserId, (u64, Uuid))>>,
}

impl MemStore {
    pub fn new(cfg: RelayConfig) -> Self {
        MemStore {
            cfg,
            by_user: RwLock::new(HashMap::new()),
            id_index: RwLock::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl RelayStore for MemStore {
    async fn push(
        &self,
        envs: Vec<RelayEnvelope>,
        cfg: &RelayConfig,
        _now_ms: u64,
    ) -> Result<usize> {
        if envs.len() > cfg.max_envelopes_per_push {
            return Err(EnigmaRelayError::InvalidInput("too_many_envelopes"));
        }
        let mut by_user = self.by_user.write().await;
        let mut id_index = self.id_index.write().await;
        let mut per_user = HashMap::new();
        for env in &envs {
            *per_user.entry(env.to).or_insert(0usize) += 1;
        }
        for (user, add) in per_user {
            let current = by_user.get(&user).map(|q| q.len()).unwrap_or(0);
            if current + add > cfg.max_queue_per_user {
                return Err(EnigmaRelayError::Conflict);
            }
        }
        let mut inserted = 0;
        for env in envs {
            if id_index.contains_key(&env.id) {
                return Err(EnigmaRelayError::Conflict);
            }
            let key = (env.created_at_ms, env.id);
            by_user.entry(env.to).or_default().insert(key, env.clone());
            id_index.insert(env.id, (env.to, key));
            inserted += 1;
        }
        Ok(inserted)
    }

    async fn pull(
        &self,
        to: UserId,
        cursor: Option<String>,
        limit: usize,
        now_ms: u64,
    ) -> Result<(Vec<RelayEnvelope>, Option<String>)> {
        let start_key = if let Some(raw) = cursor {
            let (cursor_user, created_at_ms, id) = decode_cursor(&raw)?;
            if cursor_user != to {
                return Err(EnigmaRelayError::InvalidInput("invalid cursor"));
            }
            Some((created_at_ms, id))
        } else {
            None
        };
        let by_user = self.by_user.read().await;
        let mut envelopes = Vec::new();
        if let Some(queue) = by_user.get(&to) {
            let range = match start_key {
                Some(start) => queue.range((Bound::Excluded(start), Bound::Unbounded)),
                None => queue.range(..),
            };
            for ((_ts, _id), env) in range {
                if effective_expiry(env, &self.cfg) <= now_ms {
                    continue;
                }
                envelopes.push(env.clone());
                if envelopes.len() > limit {
                    break;
                }
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
        let mut by_user = self.by_user.write().await;
        let mut id_index = self.id_index.write().await;
        let mut removed = 0;
        for id in ids {
            if let Some((user, key)) = id_index.remove(&id) {
                if let Some(queue) = by_user.get_mut(&user) {
                    if queue.remove(&key).is_some() {
                        removed += 1;
                    }
                }
            }
        }
        Ok(removed)
    }

    async fn purge_expired(&self, now_ms: u64) -> Result<usize> {
        let mut by_user = self.by_user.write().await;
        let mut id_index = self.id_index.write().await;
        let mut removed = 0;
        let mut to_remove: Vec<(UserId, (u64, Uuid), Uuid)> = Vec::new();
        for (user, queue) in by_user.iter() {
            for (key, env) in queue.iter() {
                if effective_expiry(env, &self.cfg) <= now_ms {
                    to_remove.push((*user, *key, env.id));
                }
            }
        }
        for (user, key, id) in to_remove {
            if let Some(queue) = by_user.get_mut(&user) {
                if queue.remove(&key).is_some() {
                    removed += 1;
                }
            }
            id_index.remove(&id);
        }
        Ok(removed)
    }
}
