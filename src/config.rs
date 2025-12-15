#[derive(Clone)]
pub struct RelayConfig {
    pub bind_addr: String,
    pub request_timeout_ms: u64,
    pub max_envelopes_per_push: usize,
    pub max_envelope_blob_bytes: usize,
    pub page_size_default: usize,
    pub retention_ms_default: u64,
    pub purge_interval_secs: u64,
    pub max_queue_per_user: usize,
    pub storage_path: Option<String>,
}

impl Default for RelayConfig {
    fn default() -> Self {
        RelayConfig {
            bind_addr: "127.0.0.1:0".to_string(),
            request_timeout_ms: 3000,
            max_envelopes_per_push: 256,
            max_envelope_blob_bytes: 8 * 1024 * 1024,
            page_size_default: 64,
            retention_ms_default: 7 * 24 * 60 * 60 * 1000,
            purge_interval_secs: 60,
            max_queue_per_user: 50_000,
            storage_path: None,
        }
    }
}
