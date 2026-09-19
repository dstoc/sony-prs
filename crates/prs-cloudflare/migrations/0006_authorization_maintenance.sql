-- Index bearer-token lookups and add the bounded state used by public
-- authorization rate limits.
CREATE INDEX sender_credentials_bearer_token_idx
    ON sender_credentials (bearer_token_hash);

CREATE INDEX reader_sessions_bearer_token_idx
    ON reader_sessions (bearer_token_hash);

CREATE INDEX authorization_requests_terminal_cleanup_idx
    ON authorization_requests (state, expires_at, consumed_at, denied_at, expired_at);

CREATE TABLE authorization_rate_limits (
    bucket_key TEXT PRIMARY KEY CHECK (length(bucket_key) > 0),
    window_started_at INTEGER NOT NULL CHECK (window_started_at >= 0),
    request_count INTEGER NOT NULL CHECK (request_count >= 0)
);

CREATE INDEX authorization_rate_limits_window_idx
    ON authorization_rate_limits (window_started_at);
