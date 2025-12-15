enigma-relay
============

Offline store-and-forward relay for Enigma clients. Nodes push opaque end-to-end encrypted envelopes for recipients who may be offline; recipients pull pending envelopes later and acknowledge once processed. The relay never decrypts payloads and only sees opaque blobs plus routing metadata.

Features
- Axum HTTP server with push, pull (paged cursors), ack, and health endpoints
- Pluggable storage with in-memory and sled backends (sled enabled by default)
- Deterministic ordering by creation time + id with opaque base64 cursor tokens
- TTL and retention-based expiration with a background purger
- Canonical request/response and envelope types from enigma-node-types

Quickstart
- Run tests to see the service in action: `cargo test`
- Start an in-process relay from code:
  1. Create a `RelayConfig` (default binds to 127.0.0.1:0 and uses the sled backend when a storage path is provided)
  2. Call `enigma_relay::start(config).await?`
  3. Use the returned base URL for HTTP clients
- Default endpoints:
  - POST `/relay/push` with `RelayPushRequest`
  - GET `/relay/pull/{user_id_hex}?cursor=&limit=`
  - POST `/relay/ack` with `RelayAckRequest`
  - GET `/health`

Privacy and safety
- Payloads are opaque base64 blobs; the relay only enforces size and TTL policies
- Addressing is by `UserId` (hex) only; plaintext usernames are never accepted over HTTP
- Paging avoids loading full queues into memory and works deterministically across backends
