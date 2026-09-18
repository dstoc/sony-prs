-- PRSync metadata only. Bundle bytes remain in the private R2 bucket.
PRAGMA foreign_keys = ON;

CREATE TABLE inbox (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    revision INTEGER NOT NULL DEFAULT 0,
    object_key TEXT,
    etag TEXT,
    manifest_json TEXT,
    updated_at INTEGER NOT NULL
);

INSERT INTO inbox (singleton, updated_at) VALUES (1, 0);

CREATE TABLE authorization_requests (
    request_id TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('sender', 'reader')),
    polling_secret_hash BLOB NOT NULL,
    approval_url TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('pending', 'approved', 'denied', 'expired', 'claimed')),
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    claimed_at INTEGER
);

CREATE INDEX authorization_requests_expiry_idx
    ON authorization_requests (expires_at, state);

CREATE TABLE sender_credentials (
    credential_id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    bearer_token_hash BLOB NOT NULL,
    created_at INTEGER NOT NULL,
    last_used_at INTEGER,
    revoked_at INTEGER
);

CREATE UNIQUE INDEX sender_credentials_active_name_idx
    ON sender_credentials (name)
    WHERE revoked_at IS NULL;

CREATE TABLE reader_sessions (
    session_id TEXT PRIMARY KEY,
    bearer_token_hash BLOB NOT NULL,
    issued_at INTEGER NOT NULL,
    expires_at INTEGER,
    revoked_at INTEGER
);

CREATE INDEX reader_sessions_expiry_idx
    ON reader_sessions (expires_at, revoked_at);
