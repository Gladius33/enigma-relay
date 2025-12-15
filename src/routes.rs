use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use enigma_node_types::{
    EnigmaNodeTypesError, RelayAckRequest, RelayAckResponse, RelayEnvelope, RelayKind,
    RelayPullResponse, RelayPushRequest, RelayPushResponse, UserId,
};
use serde::Deserialize;
use tower::ServiceBuilder;
use tower_http::timeout::TimeoutLayer;

use crate::config::RelayConfig;
use crate::error::EnigmaRelayError;
use crate::store::DynRelayStore;

#[derive(Clone)]
pub struct AppState {
    pub store: DynRelayStore,
    pub cfg: RelayConfig,
}

#[derive(Deserialize)]
pub struct PullParams {
    pub cursor: Option<String>,
    pub limit: Option<usize>,
}

type HandlerResult<T> = std::result::Result<T, (StatusCode, String)>;

pub fn router(store: DynRelayStore, cfg: RelayConfig) -> Router {
    let timeout = Duration::from_millis(cfg.request_timeout_ms);
    let timeout_layer = TimeoutLayer::with_status_code(StatusCode::REQUEST_TIMEOUT, timeout);
    let state = AppState { store, cfg };
    Router::new()
        .route("/relay/push", post(push))
        .route("/relay/pull/:user_id_hex", get(pull))
        .route("/relay/ack", post(ack))
        .route("/health", get(health))
        .with_state(state)
        .layer(ServiceBuilder::new().layer(timeout_layer))
}

async fn push(
    State(state): State<AppState>,
    Json(body): Json<RelayPushRequest>,
) -> HandlerResult<Json<RelayPushResponse>> {
    if let Err(err) = body.validate() {
        return Err(map_validation_error(err));
    }
    if body.envelopes.len() > state.cfg.max_envelopes_per_push {
        return Err((StatusCode::BAD_REQUEST, "too_many_envelopes".to_string()));
    }
    for env in &body.envelopes {
        validate_blob_size(env, state.cfg.max_envelope_blob_bytes)?;
    }
    let now = current_millis();
    let accepted = state
        .store
        .push(body.envelopes, &state.cfg, now)
        .await
        .map_err(map_error)?;
    Ok(Json(RelayPushResponse { accepted }))
}

async fn pull(
    Path(user_id_hex): Path<String>,
    Query(params): Query<PullParams>,
    State(state): State<AppState>,
) -> HandlerResult<Json<RelayPullResponse>> {
    let user = UserId::from_hex(&user_id_hex)
        .map_err(|_| (StatusCode::BAD_REQUEST, "invalid_user".to_string()))?;
    let limit_raw = params.limit.unwrap_or(state.cfg.page_size_default);
    let limit = limit_raw.clamp(1, 512);
    let now = current_millis();
    let (envelopes, next_cursor) = state
        .store
        .pull(user, params.cursor, limit, now)
        .await
        .map_err(map_error)?;
    Ok(Json(RelayPullResponse {
        envelopes,
        next_cursor,
    }))
}

async fn ack(
    State(state): State<AppState>,
    Json(body): Json<RelayAckRequest>,
) -> HandlerResult<Json<RelayAckResponse>> {
    let removed = state.store.ack(body.ids).await.map_err(map_error)?;
    Ok(Json(RelayAckResponse { removed }))
}

async fn health() -> HandlerResult<Json<serde_json::Value>> {
    Ok(Json(serde_json::json!({"ok": true})))
}

fn validate_blob_size(env: &RelayEnvelope, max_bytes: usize) -> Result<(), (StatusCode, String)> {
    let blob = match &env.kind {
        RelayKind::OpaqueMessage(m) => m.blob_b64.as_str(),
        RelayKind::OpaqueSignaling(s) => s.blob_b64.as_str(),
        RelayKind::OpaqueAttachmentChunk(c) => c.blob_b64.as_str(),
    };
    let decoded = STANDARD
        .decode(blob.as_bytes())
        .map_err(|_| (StatusCode::BAD_REQUEST, "invalid_base64".to_string()))?;
    if decoded.len() > max_bytes {
        return Err((StatusCode::BAD_REQUEST, "blob_too_large".to_string()));
    }
    Ok(())
}

fn map_validation_error(err: EnigmaNodeTypesError) -> (StatusCode, String) {
    match err {
        EnigmaNodeTypesError::InvalidField(_) => {
            (StatusCode::BAD_REQUEST, "invalid_field".to_string())
        }
        EnigmaNodeTypesError::InvalidBase64 => {
            (StatusCode::BAD_REQUEST, "invalid_base64".to_string())
        }
        EnigmaNodeTypesError::InvalidHex => (StatusCode::BAD_REQUEST, "invalid_user".to_string()),
        EnigmaNodeTypesError::InvalidUsername => {
            (StatusCode::BAD_REQUEST, "invalid_user".to_string())
        }
        EnigmaNodeTypesError::JsonError => (StatusCode::BAD_REQUEST, "invalid_json".to_string()),
        EnigmaNodeTypesError::Utf8Error => (StatusCode::BAD_REQUEST, "invalid_field".to_string()),
    }
}

fn map_error(err: EnigmaRelayError) -> (StatusCode, String) {
    match err {
        EnigmaRelayError::InvalidInput(_) => (StatusCode::BAD_REQUEST, "invalid_input".to_string()),
        EnigmaRelayError::Conflict => (StatusCode::CONFLICT, "conflict".to_string()),
        EnigmaRelayError::NotFound => (StatusCode::NOT_FOUND, "not_found".to_string()),
        EnigmaRelayError::JsonError => {
            (StatusCode::INTERNAL_SERVER_ERROR, "json_error".to_string())
        }
        EnigmaRelayError::StorageError => {
            (StatusCode::SERVICE_UNAVAILABLE, "storage_error".to_string())
        }
        EnigmaRelayError::Transport => (
            StatusCode::SERVICE_UNAVAILABLE,
            "transport_error".to_string(),
        ),
        EnigmaRelayError::Internal => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error".to_string(),
        ),
    }
}

fn current_millis() -> u64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_millis() as u64,
        Err(_) => 0,
    }
}
