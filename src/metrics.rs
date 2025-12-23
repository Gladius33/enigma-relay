use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;

#[derive(Default)]
pub struct RelayMetrics {
    pub push_total: AtomicU64,
    pub push_stored: AtomicU64,
    pub duplicates: AtomicU64,
    pub pull_total: AtomicU64,
    pub ack_total: AtomicU64,
    pub ack_deleted: AtomicU64,
    pub rate_limited: AtomicU64,
    pub gc_removed: AtomicU64,
}

#[derive(Serialize, Default, Clone)]
pub struct MetricsSnapshot {
    pub push_total: u64,
    pub push_stored: u64,
    pub duplicates: u64,
    pub pull_total: u64,
    pub ack_total: u64,
    pub ack_deleted: u64,
    pub rate_limited: u64,
    pub gc_removed: u64,
}

impl RelayMetrics {
    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            push_total: self.push_total.load(Ordering::Relaxed),
            push_stored: self.push_stored.load(Ordering::Relaxed),
            duplicates: self.duplicates.load(Ordering::Relaxed),
            pull_total: self.pull_total.load(Ordering::Relaxed),
            ack_total: self.ack_total.load(Ordering::Relaxed),
            ack_deleted: self.ack_deleted.load(Ordering::Relaxed),
            rate_limited: self.rate_limited.load(Ordering::Relaxed),
            gc_removed: self.gc_removed.load(Ordering::Relaxed),
        }
    }
}
