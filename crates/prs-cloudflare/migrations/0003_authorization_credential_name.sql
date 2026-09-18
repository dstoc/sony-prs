-- Persist the sender credential name with its authorization request so the
-- polling claim can create the named credential without returning a bearer
-- token to the approval browser.
ALTER TABLE authorization_requests ADD COLUMN credential_name TEXT;

-- Historical sender rows predate this field. Give them a stable compatibility
-- name; a consumed legacy row has no credential child but must remain readable.
UPDATE authorization_requests
SET credential_name = 'legacy-' || request_id
WHERE kind = 'sender' AND credential_name IS NULL;

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
