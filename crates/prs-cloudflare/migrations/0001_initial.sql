-- PRSync metadata only. Bundle bytes remain in the private R2 bucket.
--
-- This migration is deliberately idempotent. Wrangler records applied
-- migrations, but idempotence also makes local database resets and migration
-- regression tests safe to repeat.
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS bundles (
    bundle_id TEXT PRIMARY KEY,
    object_key TEXT NOT NULL UNIQUE,
    etag TEXT NOT NULL,
    manifest_json TEXT NOT NULL,
    size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0 AND size_bytes <= 16777216),
    created_at INTEGER NOT NULL CHECK (created_at >= 0)
);

CREATE TABLE IF NOT EXISTS sender_credentials (
    credential_id TEXT PRIMARY KEY,
    name TEXT NOT NULL CHECK (length(name) > 0),
    bearer_token_hash BLOB NOT NULL
        CHECK (typeof(bearer_token_hash) = 'blob' AND length(bearer_token_hash) = 32),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    last_used_at INTEGER CHECK (last_used_at IS NULL OR last_used_at >= created_at),
    revoked_at INTEGER CHECK (revoked_at IS NULL OR revoked_at >= created_at)
);

CREATE UNIQUE INDEX IF NOT EXISTS sender_credentials_active_name_idx
    ON sender_credentials (name)
    WHERE revoked_at IS NULL;

CREATE TABLE IF NOT EXISTS reader_sessions (
    session_id TEXT PRIMARY KEY,
    bearer_token_hash BLOB NOT NULL
        CHECK (typeof(bearer_token_hash) = 'blob' AND length(bearer_token_hash) = 32),
    issued_at INTEGER NOT NULL CHECK (issued_at >= 0),
    expires_at INTEGER NOT NULL CHECK (expires_at > issued_at),
    revoked_at INTEGER CHECK (revoked_at IS NULL OR revoked_at >= issued_at)
);

CREATE INDEX IF NOT EXISTS reader_sessions_expiry_idx
    ON reader_sessions (expires_at, revoked_at);

CREATE TABLE IF NOT EXISTS authorization_requests (
    request_id TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('sender', 'reader')),
    polling_secret_hash BLOB NOT NULL
        CHECK (typeof(polling_secret_hash) = 'blob' AND length(polling_secret_hash) = 32),
    approval_url TEXT NOT NULL CHECK (length(approval_url) > 0),
    state TEXT NOT NULL DEFAULT 'pending'
        CHECK (state IN ('pending', 'approved', 'denied', 'expired', 'consumed')),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    expires_at INTEGER NOT NULL CHECK (expires_at > created_at),
    approved_at INTEGER,
    denied_at INTEGER,
    expired_at INTEGER,
    consumed_at INTEGER,
    credential_id TEXT REFERENCES sender_credentials (credential_id),
    session_id TEXT REFERENCES reader_sessions (session_id),
    CHECK (
        (kind = 'sender' AND session_id IS NULL)
        OR (kind = 'reader' AND credential_id IS NULL)
    ),
    CHECK (
        (state = 'pending'
            AND approved_at IS NULL
            AND denied_at IS NULL
            AND expired_at IS NULL
            AND consumed_at IS NULL
            AND credential_id IS NULL
            AND session_id IS NULL)
        OR (state = 'approved'
            AND approved_at IS NOT NULL
            AND denied_at IS NULL
            AND expired_at IS NULL
            AND consumed_at IS NULL
            AND credential_id IS NULL
            AND session_id IS NULL)
        OR (state = 'denied'
            AND approved_at IS NULL
            AND denied_at IS NOT NULL
            AND expired_at IS NULL
            AND consumed_at IS NULL
            AND credential_id IS NULL
            AND session_id IS NULL)
        OR (state = 'expired'
            AND approved_at IS NULL
            AND denied_at IS NULL
            AND expired_at IS NOT NULL
            AND consumed_at IS NULL
            AND credential_id IS NULL
            AND session_id IS NULL)
        OR (state = 'consumed'
            AND approved_at IS NOT NULL
            AND denied_at IS NULL
            AND expired_at IS NULL
            AND consumed_at IS NOT NULL
            AND ((kind = 'sender' AND credential_id IS NOT NULL AND session_id IS NULL)
                OR (kind = 'reader' AND credential_id IS NULL AND session_id IS NOT NULL)))
    )
);

CREATE INDEX IF NOT EXISTS authorization_requests_expiry_idx
    ON authorization_requests (expires_at, state);

CREATE TABLE IF NOT EXISTS inbox (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    revision INTEGER NOT NULL DEFAULT 0 CHECK (revision >= 0),
    current_bundle_id TEXT REFERENCES bundles (bundle_id),
    updated_at INTEGER NOT NULL CHECK (updated_at >= 0)
);

INSERT OR IGNORE INTO inbox (singleton, revision, updated_at)
VALUES (1, 0, 0);

CREATE TABLE IF NOT EXISTS owner_identity (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    issuer TEXT NOT NULL CHECK (length(issuer) > 0),
    subject TEXT NOT NULL CHECK (length(subject) > 0),
    email TEXT,
    display_name TEXT,
    configured_at INTEGER NOT NULL CHECK (configured_at >= 0),
    updated_at INTEGER NOT NULL CHECK (updated_at >= configured_at),
    UNIQUE (issuer, subject)
);

-- Approval state is one-way. The Worker must perform the child-row insert and
-- the approved-to-consumed update in one D1 transaction.
CREATE TRIGGER IF NOT EXISTS authorization_requests_state_transition
BEFORE UPDATE OF state ON authorization_requests
FOR EACH ROW
WHEN NOT (
    NEW.state = OLD.state
    OR (OLD.state = 'pending' AND NEW.state IN ('approved', 'denied', 'expired'))
    OR (OLD.state = 'approved' AND NEW.state = 'consumed')
)
BEGIN
    SELECT RAISE(ABORT, 'invalid authorization request state transition');
END;
