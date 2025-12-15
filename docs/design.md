# Design

Ordering and cursors
- Envelopes are ordered by `(created_at_ms, id)` and stored in that order in both backends
- Cursors are base64 of `user_hex:{created_at_ms}:{uuid}`, keeping them opaque and portable
- Paging returns at most `limit` envelopes and a `next_cursor` only when more data is available

Retention and expiration
- Each envelope may carry `expires_at_ms`; otherwise a retention window from config is applied
- Purging removes expired envelopes and also clears the id index used for ack lookups
- Pull operations skip expired envelopes; the TTL purger clears them proactively on an interval

Validation and limits
- Push requests enforce per-request envelope count, maximum blob size after base64 decoding, and per-user queue caps
- Pull limits are clamped, preventing oversized responses while still allowing bulk retrieval over multiple pages
