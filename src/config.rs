use std::fs;
use std::path::Path;

use serde::Deserialize;

use crate::error::{EnigmaRelayError, Result};

#[derive(Clone, Deserialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RelayMode {
    Http,
    Tls,
}

#[derive(Clone, Deserialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum StorageKind {
    Sled,
    Memory,
}

#[derive(Clone, Deserialize, Debug)]
pub struct TlsConfig {
    pub cert_pem_path: String,
    pub key_pem_path: String,
    #[serde(default)]
    pub client_ca_pem_path: Option<String>,
}

#[derive(Clone, Deserialize, Debug)]
pub struct StorageConfig {
    pub kind: StorageKind,
    pub path: String,
}

impl Default for StorageConfig {
    fn default() -> Self {
        StorageConfig {
            kind: StorageKind::Sled,
            path: "./relay_db".to_string(),
        }
    }
}

#[derive(Clone, Deserialize, Debug)]
pub struct RelayLimits {
    pub max_message_bytes: u64,
    pub max_queue_bytes_per_user: u64,
    pub max_messages_per_user: u64,
    pub message_ttl_seconds: u64,
    pub gc_interval_seconds: u64,
    pub pull_batch_max: u64,
}

impl Default for RelayLimits {
    fn default() -> Self {
        RelayLimits {
            max_message_bytes: 8_388_608,
            max_queue_bytes_per_user: 1_073_741_824,
            max_messages_per_user: 200_000,
            message_ttl_seconds: 14 * 24 * 3600,
            gc_interval_seconds: 60,
            pull_batch_max: 256,
        }
    }
}

#[derive(Clone, Deserialize, Debug)]
pub struct EndpointRateLimit {
    pub push_rps: u32,
    pub pull_rps: u32,
    pub ack_rps: u32,
}

impl Default for EndpointRateLimit {
    fn default() -> Self {
        EndpointRateLimit {
            push_rps: 10,
            pull_rps: 5,
            ack_rps: 10,
        }
    }
}

#[derive(Clone, Deserialize, Debug)]
pub struct RateLimitConfig {
    pub enabled: bool,
    pub per_ip_rps: u32,
    pub burst: u32,
    pub ban_seconds: u64,
    #[serde(default)]
    pub endpoints: EndpointRateLimit,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        RateLimitConfig {
            enabled: true,
            per_ip_rps: 20,
            burst: 40,
            ban_seconds: 300,
            endpoints: EndpointRateLimit::default(),
        }
    }
}

#[derive(Clone, Deserialize, Debug)]
pub struct RelayConfig {
    pub address: String,
    #[serde(default = "RelayConfig::default_mode")]
    pub mode: RelayMode,
    #[serde(default)]
    pub tls: Option<TlsConfig>,
    #[serde(default)]
    pub storage: StorageConfig,
    #[serde(default)]
    pub relay: RelayLimits,
    #[serde(default)]
    pub rate_limit: RateLimitConfig,
}

impl Default for RelayConfig {
    fn default() -> Self {
        RelayConfig {
            address: "127.0.0.1:0".to_string(),
            mode: RelayMode::Http,
            tls: None,
            storage: StorageConfig::default(),
            relay: RelayLimits::default(),
            rate_limit: RateLimitConfig::default(),
        }
    }
}

impl RelayConfig {
    fn default_mode() -> RelayMode {
        RelayMode::Http
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let raw = fs::read_to_string(&path).map_err(|e| EnigmaRelayError::Config(e.to_string()))?;
        let cfg: RelayConfig =
            toml::from_str(&raw).map_err(|e| EnigmaRelayError::Config(e.to_string()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn validate(&self) -> Result<()> {
        if self.address.trim().is_empty() {
            return Err(EnigmaRelayError::Config("address is required".to_string()));
        }
        match self.mode {
            RelayMode::Http => {}
            RelayMode::Tls => {
                if self.tls.is_none() {
                    return Err(EnigmaRelayError::Config(
                        "tls mode requires tls config".to_string(),
                    ));
                }
                #[cfg(not(feature = "tls"))]
                {
                    return Err(EnigmaRelayError::Disabled(
                        "tls feature not enabled".to_string(),
                    ));
                }
            }
        }
        if self.relay.max_message_bytes == 0 {
            return Err(EnigmaRelayError::Config(
                "max_message_bytes must be positive".to_string(),
            ));
        }
        if self.relay.max_queue_bytes_per_user == 0 {
            return Err(EnigmaRelayError::Config(
                "max_queue_bytes_per_user must be positive".to_string(),
            ));
        }
        if self.relay.max_messages_per_user == 0 {
            return Err(EnigmaRelayError::Config(
                "max_messages_per_user must be positive".to_string(),
            ));
        }
        if self.relay.pull_batch_max == 0 {
            return Err(EnigmaRelayError::Config(
                "pull_batch_max must be positive".to_string(),
            ));
        }
        if self.relay.gc_interval_seconds == 0 {
            return Err(EnigmaRelayError::Config(
                "gc_interval_seconds must be positive".to_string(),
            ));
        }
        if self.rate_limit.enabled {
            if self.rate_limit.per_ip_rps == 0 {
                return Err(EnigmaRelayError::Config(
                    "rate_limit.per_ip_rps must be positive".to_string(),
                ));
            }
            if self.rate_limit.burst == 0 {
                return Err(EnigmaRelayError::Config(
                    "rate_limit.burst must be positive".to_string(),
                ));
            }
            if self.rate_limit.endpoints.push_rps == 0
                || self.rate_limit.endpoints.pull_rps == 0
                || self.rate_limit.endpoints.ack_rps == 0
            {
                return Err(EnigmaRelayError::Config(
                    "endpoint rate limits must be positive".to_string(),
                ));
            }
        }
        match self.storage.kind {
            StorageKind::Sled => {
                #[cfg(not(feature = "persistence"))]
                {
                    return Err(EnigmaRelayError::Disabled(
                        "persistence feature not enabled".to_string(),
                    ));
                }
            }
            StorageKind::Memory => {}
        }
        Ok(())
    }
}
