# API

Base path: `/` (see quickstart in README for starting the server). All payloads are JSON using canonical types from `enigma-node-types`.

## POST /relay/push
- Body: `RelayPushRequest { envelopes: Vec<RelayEnvelope> }`
- Validation: envelope field validation, base64 decoding for blob size checks, and per-request envelope count caps
- Response: `RelayPushResponse { accepted: usize }`

## GET /relay/pull/{user_id_hex}
- Query: `cursor` (optional opaque string), `limit` (optional, clamped to 1..=512; defaults to config `page_size_default`)
- Response: `RelayPullResponse { envelopes: Vec<RelayEnvelope>, next_cursor: Option<String> }`
- Ordering: deterministic `(created_at_ms, id)`

## POST /relay/ack
- Body: `RelayAckRequest { ids: Vec<Uuid> }`
- Response: `RelayAckResponse { removed: usize }`

## GET /health
- Response: `{ "ok": true }`
