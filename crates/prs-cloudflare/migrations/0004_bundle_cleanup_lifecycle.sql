-- Track every bundle object from the start of an upload through cleanup.
--
-- A lifecycle row is created before the Worker writes to R2. Cleanup can
-- claim only an expired lifecycle row, and publication can insert metadata
-- only while that row is still uploading. This closes the gap between the R2
-- put and the D1 publication batch.
PRAGMA foreign_keys = OFF;

CREATE TABLE bundle_lifecycle (
    lifecycle_id TEXT PRIMARY KEY,
    object_key TEXT NOT NULL UNIQUE,
    state TEXT NOT NULL CHECK (state IN ('uploading', 'published', 'cleanup_claimed')),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    updated_at INTEGER NOT NULL CHECK (updated_at >= created_at),
    cleanup_after INTEGER NOT NULL CHECK (cleanup_after >= created_at)
);

-- Existing rows predate lifecycle tracking. Keep them publishable and apply
-- the same retention policy from their original creation time.
INSERT INTO bundle_lifecycle
    (lifecycle_id, object_key, state, created_at, updated_at, cleanup_after)
SELECT
    bundle_id,
    object_key,
    'published',
    created_at,
    created_at,
    created_at + 86400000
FROM bundles;

ALTER TABLE bundles
    ADD COLUMN lifecycle_id TEXT REFERENCES bundle_lifecycle (lifecycle_id);

UPDATE bundles
SET lifecycle_id = bundle_id;

CREATE UNIQUE INDEX bundles_lifecycle_id_idx
    ON bundles (lifecycle_id)
    WHERE lifecycle_id IS NOT NULL;

CREATE INDEX bundle_lifecycle_cleanup_idx
    ON bundle_lifecycle (cleanup_after, state);

-- New metadata must be tied to a lifecycle row that is still in the
-- publication phase. The publication batch changes that row to published.
CREATE TRIGGER bundles_require_uploading_lifecycle
BEFORE INSERT ON bundles
FOR EACH ROW
WHEN NEW.lifecycle_id IS NULL
    OR NOT EXISTS (
        SELECT 1
        FROM bundle_lifecycle
        WHERE lifecycle_id = NEW.lifecycle_id
          AND state = 'uploading'
    )
BEGIN
    SELECT RAISE(ABORT, 'bundle metadata is missing an active lifecycle');
END;

PRAGMA foreign_keys = ON;
