-- Upgrade the metadata schema created by 0001_initial.sql.
-- Bundle bytes remain in the private R2 bucket.
--
-- D1 applies each numbered migration once. The shadow tables below preserve
-- the existing rows while SQLite replaces tables whose checks and foreign keys
-- must change.
PRAGMA foreign_keys = OFF;

DROP INDEX IF EXISTS authorization_requests_expiry_idx;
DROP INDEX IF EXISTS reader_sessions_expiry_idx;
DROP INDEX IF EXISTS sender_credentials_active_name_idx;

CREATE TABLE IF NOT EXISTS bundles (
    bundle_id TEXT PRIMARY KEY,
    object_key TEXT NOT NULL UNIQUE,
    etag TEXT NOT NULL,
    manifest_json TEXT NOT NULL,
    size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0 AND size_bytes <= 16777216),
    created_at INTEGER NOT NULL CHECK (created_at >= 0)
);

-- Reject a partially written legacy metadata row instead of silently dropping
-- one of its values during the table replacement.
CREATE TABLE prs_upgrade_guard (value INTEGER);

CREATE TRIGGER prs_upgrade_validate_inbox
BEFORE INSERT ON prs_upgrade_guard
FOR EACH ROW
WHEN EXISTS (
    SELECT 1
    FROM inbox
    WHERE ((object_key IS NULL) + (etag IS NULL) + (manifest_json IS NULL)) NOT IN (0, 3)
)
BEGIN
    SELECT RAISE(ABORT, 'legacy inbox metadata is incomplete');
END;

INSERT INTO prs_upgrade_guard (value) VALUES (1);
DROP TRIGGER prs_upgrade_validate_inbox;
DROP TABLE prs_upgrade_guard;

-- The original inbox stored these three values together. Keep a stable
-- reference for that legacy row because 0001 did not assign a bundle ID.
INSERT OR IGNORE INTO bundles
    (bundle_id, object_key, etag, manifest_json, size_bytes, created_at)
SELECT
    'legacy-inbox',
    object_key,
    etag,
    manifest_json,
    0,
    updated_at
FROM inbox
WHERE object_key IS NOT NULL
  AND etag IS NOT NULL
  AND manifest_json IS NOT NULL;

CREATE TABLE sender_credentials_v2 (
    credential_id TEXT PRIMARY KEY,
    name TEXT NOT NULL CHECK (length(name) > 0),
    bearer_token_hash BLOB NOT NULL
        CHECK (typeof(bearer_token_hash) = 'blob' AND length(bearer_token_hash) = 32),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    last_used_at INTEGER CHECK (last_used_at IS NULL OR last_used_at >= created_at),
    revoked_at INTEGER CHECK (revoked_at IS NULL OR revoked_at >= created_at)
);

INSERT INTO sender_credentials_v2
    (credential_id, name, bearer_token_hash, created_at, last_used_at, revoked_at)
SELECT credential_id, name, bearer_token_hash, created_at, last_used_at, revoked_at
FROM sender_credentials;

CREATE TABLE reader_sessions_v2 (
    session_id TEXT PRIMARY KEY,
    bearer_token_hash BLOB NOT NULL
        CHECK (typeof(bearer_token_hash) = 'blob' AND length(bearer_token_hash) = 32),
    issued_at INTEGER NOT NULL CHECK (issued_at >= 0),
    expires_at INTEGER NOT NULL CHECK (expires_at > issued_at),
    revoked_at INTEGER CHECK (revoked_at IS NULL OR revoked_at >= issued_at)
);

-- 0001 allowed a NULL expiry. Expire such legacy sessions immediately after
-- issuance instead of creating a session with an unlimited lifetime.
INSERT INTO reader_sessions_v2
    (session_id, bearer_token_hash, issued_at, expires_at, revoked_at)
SELECT
    session_id,
    bearer_token_hash,
    issued_at,
    CASE
        WHEN expires_at IS NULL OR expires_at <= issued_at THEN issued_at + 1
        ELSE expires_at
    END,
    CASE
        WHEN revoked_at IS NULL OR revoked_at >= issued_at THEN revoked_at
        ELSE issued_at
    END
FROM reader_sessions;

CREATE TABLE inbox_v2 (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    revision INTEGER NOT NULL DEFAULT 0 CHECK (revision >= 0),
    current_bundle_id TEXT REFERENCES bundles (bundle_id),
    updated_at INTEGER NOT NULL CHECK (updated_at >= 0)
);

INSERT INTO inbox_v2 (singleton, revision, current_bundle_id, updated_at)
SELECT
    singleton,
    revision,
    CASE
        WHEN object_key IS NOT NULL
         AND etag IS NOT NULL
         AND manifest_json IS NOT NULL
        THEN 'legacy-inbox'
        ELSE NULL
    END,
    updated_at
FROM inbox;

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

CREATE TABLE authorization_requests_v2 (
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
    legacy_claimed_at INTEGER,
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
            AND session_id IS NULL
            AND legacy_claimed_at IS NULL)
        OR (state = 'approved'
            AND approved_at IS NOT NULL
            AND denied_at IS NULL
            AND expired_at IS NULL
            AND consumed_at IS NULL
            AND credential_id IS NULL
            AND session_id IS NULL
            AND legacy_claimed_at IS NULL)
        OR (state = 'denied'
            AND approved_at IS NULL
            AND denied_at IS NOT NULL
            AND expired_at IS NULL
            AND consumed_at IS NULL
            AND credential_id IS NULL
            AND session_id IS NULL
            AND legacy_claimed_at IS NULL)
        OR (state = 'expired'
            AND approved_at IS NULL
            AND denied_at IS NULL
            AND expired_at IS NOT NULL
            AND consumed_at IS NULL
            AND credential_id IS NULL
            AND session_id IS NULL
            AND legacy_claimed_at IS NULL)
        OR (state = 'consumed'
            AND approved_at IS NOT NULL
            AND denied_at IS NULL
            AND expired_at IS NULL
            AND consumed_at IS NOT NULL
            AND (
                (kind = 'sender' AND credential_id IS NOT NULL AND session_id IS NULL)
                OR (kind = 'reader' AND credential_id IS NULL AND session_id IS NOT NULL)
                OR (legacy_claimed_at IS NOT NULL AND credential_id IS NULL AND session_id IS NULL)
            ))
    )
);

-- 0001 used claimed/claimed_at without a child-row reference. Preserve that
-- historical state as consumed and retain its timestamp as a compatibility
-- marker until the authorization endpoint replaces the record.
INSERT INTO authorization_requests_v2
    (request_id, kind, polling_secret_hash, approval_url, state, created_at,
     expires_at, approved_at, denied_at, expired_at, consumed_at,
     legacy_claimed_at)
SELECT
    request_id,
    kind,
    polling_secret_hash,
    approval_url,
    CASE WHEN state = 'claimed' THEN 'consumed' ELSE state END,
    created_at,
    expires_at,
    CASE WHEN state IN ('approved', 'claimed') THEN created_at ELSE NULL END,
    CASE WHEN state = 'denied' THEN created_at ELSE NULL END,
    CASE WHEN state = 'expired' THEN expires_at ELSE NULL END,
    CASE WHEN state = 'claimed' THEN COALESCE(claimed_at, created_at) ELSE NULL END,
    CASE WHEN state = 'claimed' THEN COALESCE(claimed_at, created_at) ELSE NULL END
FROM authorization_requests;

DROP TABLE authorization_requests;
ALTER TABLE authorization_requests_v2 RENAME TO authorization_requests;

DROP TABLE reader_sessions;
ALTER TABLE reader_sessions_v2 RENAME TO reader_sessions;

DROP TABLE sender_credentials;
ALTER TABLE sender_credentials_v2 RENAME TO sender_credentials;

DROP TABLE inbox;
ALTER TABLE inbox_v2 RENAME TO inbox;

CREATE UNIQUE INDEX sender_credentials_active_name_idx
    ON sender_credentials (name)
    WHERE revoked_at IS NULL;

CREATE INDEX reader_sessions_expiry_idx
    ON reader_sessions (expires_at, revoked_at);

CREATE INDEX authorization_requests_expiry_idx
    ON authorization_requests (expires_at, state);

-- Approval state is one-way. The Worker must perform the child-row insert and
-- the approved-to-consumed update in one D1 transaction.
CREATE TRIGGER authorization_requests_state_transition
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

PRAGMA foreign_keys = ON;
