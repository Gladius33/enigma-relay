use serde::Serialize;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, EnigmaRelayError>;

#[derive(Debug, Serialize, Clone)]
pub struct ErrorBody {
    pub code: &'static str,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, Clone)]
pub struct ErrorResponse {
    pub error: ErrorBody,
}

#[derive(Debug, Error, Clone)]
pub enum EnigmaRelayError {
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("rate limited")]
    RateLimited,
    #[error("duplicate")]
    Duplicate,
    #[error("quota exceeded")]
    QuotaExceeded,
    #[error("not found")]
    NotFound,
    #[error("storage error: {0}")]
    StorageError(String),
    #[error("config error: {0}")]
    Config(String),
    #[error("tls error: {0}")]
    Tls(String),
    #[error("feature disabled: {0}")]
    Disabled(String),
    #[error("internal error: {0}")]
    Internal(String),
}

impl EnigmaRelayError {
    pub fn into_response(self) -> ErrorResponse {
        match self.clone() {
            EnigmaRelayError::InvalidInput(msg) => ErrorResponse {
                error: ErrorBody {
                    code: "invalid_input",
                    message: msg,
                    details: None,
                },
            },
            EnigmaRelayError::RateLimited => ErrorResponse {
                error: ErrorBody {
                    code: "rate_limited",
                    message: "rate limit exceeded".to_string(),
                    details: None,
                },
            },
            EnigmaRelayError::Duplicate => ErrorResponse {
                error: ErrorBody {
                    code: "duplicate",
                    message: "duplicate message".to_string(),
                    details: None,
                },
            },
            EnigmaRelayError::QuotaExceeded => ErrorResponse {
                error: ErrorBody {
                    code: "quota_exceeded",
                    message: "recipient quota exceeded".to_string(),
                    details: None,
                },
            },
            EnigmaRelayError::NotFound => ErrorResponse {
                error: ErrorBody {
                    code: "not_found",
                    message: "not found".to_string(),
                    details: None,
                },
            },
            EnigmaRelayError::StorageError(msg) => ErrorResponse {
                error: ErrorBody {
                    code: "storage_error",
                    message: msg,
                    details: None,
                },
            },
            EnigmaRelayError::Config(msg) => ErrorResponse {
                error: ErrorBody {
                    code: "config_error",
                    message: msg,
                    details: None,
                },
            },
            EnigmaRelayError::Tls(msg) => ErrorResponse {
                error: ErrorBody {
                    code: "tls_error",
                    message: msg,
                    details: None,
                },
            },
            EnigmaRelayError::Disabled(msg) => ErrorResponse {
                error: ErrorBody {
                    code: "feature_disabled",
                    message: msg,
                    details: None,
                },
            },
            EnigmaRelayError::Internal(msg) => ErrorResponse {
                error: ErrorBody {
                    code: "internal_error",
                    message: msg,
                    details: None,
                },
            },
        }
    }
}

impl From<serde_json::Error> for EnigmaRelayError {
    fn from(err: serde_json::Error) -> Self {
        EnigmaRelayError::Internal(err.to_string())
    }
}

impl From<std::io::Error> for EnigmaRelayError {
    fn from(err: std::io::Error) -> Self {
        EnigmaRelayError::Internal(err.to_string())
    }
}

#[cfg(feature = "persistence")]
impl From<sled::Error> for EnigmaRelayError {
    fn from(err: sled::Error) -> Self {
        EnigmaRelayError::StorageError(err.to_string())
    }
}

#[cfg(feature = "http")]
impl actix_web::ResponseError for EnigmaRelayError {
    fn status_code(&self) -> actix_web::http::StatusCode {
        match self {
            EnigmaRelayError::InvalidInput(_) => actix_web::http::StatusCode::BAD_REQUEST,
            EnigmaRelayError::RateLimited => actix_web::http::StatusCode::TOO_MANY_REQUESTS,
            EnigmaRelayError::Duplicate => actix_web::http::StatusCode::OK,
            EnigmaRelayError::QuotaExceeded => actix_web::http::StatusCode::CONFLICT,
            EnigmaRelayError::NotFound => actix_web::http::StatusCode::NOT_FOUND,
            EnigmaRelayError::StorageError(_) => actix_web::http::StatusCode::SERVICE_UNAVAILABLE,
            EnigmaRelayError::Config(_) => actix_web::http::StatusCode::BAD_REQUEST,
            EnigmaRelayError::Tls(_) => actix_web::http::StatusCode::INTERNAL_SERVER_ERROR,
            EnigmaRelayError::Disabled(_) => actix_web::http::StatusCode::SERVICE_UNAVAILABLE,
            EnigmaRelayError::Internal(_) => actix_web::http::StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn error_response(&self) -> actix_web::HttpResponse {
        let body = self.clone().into_response();
        actix_web::HttpResponse::build(self.status_code()).json(body)
    }
}
