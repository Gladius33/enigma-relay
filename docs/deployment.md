# Deployment

- Run the relay behind a TLS-terminating reverse proxy; the service itself does not manage certificates
- Apply rate limiting and quotas per `UserId` to contain abuse and storage pressure
- Configure a sled storage path for persistence; the in-memory backend is useful for testing only
- Monitor purge metrics and tune `retention_ms_default` and `purge_interval_secs` to match expected offline durations
- Place the relay on a private network segment; only HTTP traffic to the exposed endpoints is required
