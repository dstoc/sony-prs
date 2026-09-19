-- Approved requests use the same absolute expiry deadline as pending requests.
-- Preserve approved_at when an approved request expires so the approval event
-- remains auditable.
DROP TRIGGER IF EXISTS authorization_requests_state_transition;
DROP TRIGGER IF EXISTS authorization_requests_credential_name_insert;
DROP TRIGGER IF EXISTS authorization_requests_credential_name_update;

CREATE TABLE authorization_requests_v4 (
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
    credential_name TEXT,
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
    ),
    CHECK (
        (kind = 'sender'
            AND credential_name IS NOT NULL
            AND length(credential_name) BETWEEN 1 AND 64)
        OR (kind = 'reader' AND credential_name IS NULL)
    )
);

INSERT INTO authorization_requests_v4
    (request_id, kind, polling_secret_hash, approval_url, state, created_at,
     expires_at, approved_at, denied_at, expired_at, consumed_at,
     credential_id, session_id, legacy_claimed_at, credential_name)
SELECT request_id, kind, polling_secret_hash, approval_url, state, created_at,
       expires_at, approved_at, denied_at, expired_at, consumed_at,
       credential_id, session_id, legacy_claimed_at, credential_name
FROM authorization_requests;

DROP TABLE authorization_requests;
ALTER TABLE authorization_requests_v4 RENAME TO authorization_requests;

CREATE INDEX authorization_requests_expiry_idx
    ON authorization_requests (expires_at, state);

CREATE TRIGGER authorization_requests_state_transition
BEFORE UPDATE OF state ON authorization_requests
FOR EACH ROW
WHEN NOT (
    NEW.state = OLD.state
    OR (OLD.state = 'pending' AND NEW.state IN ('approved', 'denied', 'expired'))
    OR (OLD.state = 'approved' AND NEW.state IN ('consumed', 'expired'))
)
BEGIN
    SELECT RAISE(ABORT, 'invalid authorization request state transition');
END;

CREATE TRIGGER authorization_requests_credential_name_insert
BEFORE INSERT ON authorization_requests
FOR EACH ROW
WHEN (NEW.kind = 'sender'
        AND (NEW.credential_name IS NULL OR length(NEW.credential_name) = 0
             OR length(NEW.credential_name) > 64))
   OR (NEW.kind = 'reader' AND NEW.credential_name IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT, 'invalid authorization credential name');
END;

CREATE TRIGGER authorization_requests_credential_name_update
BEFORE UPDATE OF kind, credential_name ON authorization_requests
FOR EACH ROW
WHEN (NEW.kind = 'sender'
        AND (NEW.credential_name IS NULL OR length(NEW.credential_name) = 0
             OR length(NEW.credential_name) > 64))
   OR (NEW.kind = 'reader' AND NEW.credential_name IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT, 'invalid authorization credential name');
END;
