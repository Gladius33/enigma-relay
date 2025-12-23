# API

Base path: `/`. Payloads are JSON and responses include structured errors.

## POST /push
Body:
```json
{
  "recipient": "alice",
  "message_id": "uuid",
  "ciphertext_b64": "...",
  "meta": { "kind": "msg", "total_len": 123, "chunk_index": 0, "chunk_count": 1, "sent_ms": 0 }
}
```
Response: `{ "stored": true, "duplicate": false, "queue_len": 1, "queue_bytes": 1024 }` (duplicates return `stored: false, duplicate: true`).

## POST /pull
Body: `{ "recipient": "alice", "cursor": null, "max": 100 }`
Response: `{ "items": [ ... ], "next_cursor": "base64", "remaining_estimate": 5 }`

## POST /ack
Body: `{ "recipient": "alice", "ack": [ { "message_id": "uuid", "chunk_index": 0 } ] }`
Response: `{ "deleted": 1, "missing": 0, "remaining": 0 }`

## GET /health
Response: `{ "status": "ok" }`

## GET /stats
Enabled when the `metrics` feature is on; returns uptime and counters. Otherwise returns `{ "status": "ok", "uptime_ms": ... }`.
