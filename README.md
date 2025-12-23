enigma-relay
============

Production-ready offline store-and-forward relay for Enigma nodes with durable queues, rate limiting, TTL/GC, idempotent acknowledgements, and optional TLS/mTLS.

Features
- HTTP/HTTPS API compatible with enigma-core outbox/pull/ack
- Durable sled backend (default) with atomic quotas; in-memory backend for testing
- Per-recipient quotas by count and bytes, per-endpoint/IP rate limiting, and structured JSON errors
- TTL with background GC, cursor-based paging, and idempotent push/ack flows
- Optional metrics endpoint and TLS/mTLS using rustls

Quickstart
- Run locally with defaults: `cargo run --features http` (binds 127.0.0.1:0, sled at ./relay_db)
- Embed from code:
  ```rust
  let mut cfg = enigma_relay::RelayConfig::default();
  cfg.address = "127.0.0.1:3000".into();
  cfg.storage.kind = enigma_relay::StorageKind::Sled;
  let running = enigma_relay::start(cfg).await?;
  ```
- Stop the server by sending on `running.shutdown` and awaiting `running.handle`.

Configuration (TOML)
```toml
address = "0.0.0.0:7000"
mode = "http" # or "tls"

[tls]
cert_pem_path = "./cert.pem"
key_pem_path = "./key.pem"
# client_ca_pem_path = "./ca.pem" # enables mTLS when the mtls feature is on

[storage]
kind = "sled" # or "memory"
path = "./relay_db"

[relay]
max_message_bytes = 8_388_608
max_queue_bytes_per_user = 1_073_741_824
max_messages_per_user = 200_000
message_ttl_seconds = 1_209_600
gc_interval_seconds = 60
pull_batch_max = 256

[rate_limit]
enabled = true
per_ip_rps = 20
burst = 40
ban_seconds = 300

  [rate_limit.endpoints]
  push_rps = 10
  pull_rps = 5
  ack_rps = 10
```

HTTP API
- `POST /push` — body `{ recipient, message_id, ciphertext_b64, meta: { kind, total_len, chunk_index, chunk_count, sent_ms } }`; enforces size, quotas, idempotency; response `{ stored, duplicate?, queue_len?, queue_bytes? }`.
- `POST /pull` — body `{ recipient, cursor?, max? }`; returns `{ items: [...], next_cursor, remaining_estimate }` ordered by arrival.
- `POST /ack` — body `{ recipient, ack: [ { message_id, chunk_index } ] }`; idempotent delete with `{ deleted, missing, remaining }`.
- `GET /health` — liveness.
- `GET /stats` — uptime and metrics (only when the `metrics` feature is enabled; otherwise minimal JSON).
- All errors are JSON `{ "error": { "code": "...", "message": "...", "details": ... } }`.

Persistence and GC
- Sled backend keys: `msg:{recipient}:{arrival_seq}:{message_id}:{chunk_index}`, `idx:{recipient}:{message_id}:{chunk_index}`, `quota:{recipient}`, `seq:{recipient}`.
- Push/ack updates are atomic and keep quotas consistent. GC runs on `gc_interval_seconds` and removes expired items while updating quotas.

TLS and mTLS
- Enable the `tls` feature and set `mode = "tls"` with PEM paths. Provide `client_ca_pem_path` and the `mtls` feature to require client certificates.

Testing
- Default suite: `cargo test`
- TLS/persistence suite: `cargo test --features tls,persistence`
- Release build: `cargo build --release --features tls,persistence`
