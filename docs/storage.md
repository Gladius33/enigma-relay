# Storage

In-memory backend
- Per-user queues live in `BTreeMap<(created_at_ms, Uuid), RelayEnvelope>` for deterministic iteration
- An id index accelerates ack deletions by mapping `Uuid` to `(user, key)`

Sled backend
- Trees: `by_user` and `id_index`
- `by_user` key: `{user_hex}:{created_at_ms_be}:{uuid_bytes}` with values as JSON-encoded `RelayEnvelope`
- `id_index` key: `uuid_bytes`, value: raw `by_user` key
- Pull uses range scans starting from the cursor key and stops when the prefix no longer matches the requested user
- Purge walks `by_user`, deletes expired envelopes, and removes corresponding id index entries

Durability
- Inserts and deletions flush both trees asynchronously to persist changes
- Cursor tokens encode logical ordering only, so they stay valid across backends as long as ordering rules are unchanged
