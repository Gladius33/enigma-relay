use thiserror::Error;

pub type Result<T> = std::result::Result<T, EnigmaRelayError>;

#[derive(Debug, Error, Clone)]
pub enum EnigmaRelayError {
    #[error("invalid input: {0}")]
    InvalidInput(&'static str),
    #[error("conflict")]
    Conflict,
    #[error("not found")]
    NotFound,
    #[error("json error")]
    JsonError,
    #[error("storage error")]
    StorageError,
    #[error("transport error")]
    Transport,
    #[error("internal error")]
    Internal,
}

impl From<serde_json::Error> for EnigmaRelayError {
    fn from(_: serde_json::Error) -> Self {
        EnigmaRelayError::JsonError
    }
}

#[cfg(feature = "sled-backend")]
impl From<sled::Error> for EnigmaRelayError {
    fn from(_: sled::Error) -> Self {
        EnigmaRelayError::StorageError
    }
}
