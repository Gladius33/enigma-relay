# Testing

- `cargo test` runs in-process integration tests against a live Axum server
- Tests start the relay with the memory backend to avoid filesystem dependencies
- HTTP calls use `reqwest` to exercise push, pull, ack, paging, and TTL purge flows
- TTL tests trigger purge hooks directly on the store to verify expiration semantics
