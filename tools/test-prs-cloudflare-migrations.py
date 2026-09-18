#!/usr/bin/env python3
"""Exercise the D1-compatible SQL migrations with Python's SQLite library."""

from pathlib import Path
import sqlite3


REPO_ROOT = Path(__file__).resolve().parents[1]
MIGRATIONS_DIRECTORY = REPO_ROOT / "crates" / "prs-cloudflare" / "migrations"
MIGRATIONS = sorted(MIGRATIONS_DIRECTORY.glob("*.sql"))


def apply_migrations(database: sqlite3.Connection) -> None:
    for migration in MIGRATIONS:
        database.executescript(migration.read_text())


def schema(database: sqlite3.Connection) -> list[tuple[str, str, str]]:
    return database.execute(
        """
        SELECT type, name, sql
        FROM sqlite_master
        WHERE name NOT LIKE 'sqlite_%'
        ORDER BY type, name
        """
    ).fetchall()


def digest(byte: int) -> bytes:
    return bytes([byte]) * 32


def add_request(
    database: sqlite3.Connection, request_id: str, kind: str
) -> None:
    database.execute(
        """
        INSERT INTO authorization_requests
            (request_id, kind, polling_secret_hash, approval_url, created_at, expires_at)
        VALUES (?, ?, ?, ?, ?, ?)
        """,
        (
            request_id,
            kind,
            digest(ord(request_id[0])),
            f"https://example.test/{request_id}",
            100,
            200,
        ),
    )


def main() -> None:
    if not MIGRATIONS:
        raise AssertionError("at least one D1 migration is required")

    database = sqlite3.connect(":memory:", isolation_level=None)
    database.row_factory = sqlite3.Row
    database.execute("PRAGMA foreign_keys = ON")
    apply_migrations(database)
    initial_schema = schema(database)

    # Applying the migration set again must not fail or create a different schema.
    apply_migrations(database)
    assert schema(database) == initial_schema
    assert dict(
        database.execute(
            "SELECT singleton, revision, current_bundle_id FROM inbox"
        ).fetchone()
    ) == {"singleton": 1, "revision": 0, "current_bundle_id": None}

    # The inbox stores only a reference and metadata. Bundle bytes are not a D1
    # column, and the reference/revision update can be committed atomically.
    database.execute(
        """
        INSERT INTO bundles
            (bundle_id, object_key, etag, manifest_json, size_bytes, created_at)
        VALUES (?, ?, ?, ?, ?, ?)
        """,
        ("bundle-1", "bundles/bundle-1.tar", "etag-1", '{"entry_point":"index.md"}', 42, 10),
    )
    database.execute("BEGIN IMMEDIATE")
    database.execute(
        "UPDATE inbox SET current_bundle_id = ?, revision = ?, updated_at = ? WHERE singleton = 1",
        ("bundle-1", 1, 10),
    )
    database.commit()
    assert dict(
        database.execute("SELECT revision, current_bundle_id FROM inbox").fetchone()
    ) == {"revision": 1, "current_bundle_id": "bundle-1"}
    database.execute(
        """
        INSERT INTO owner_identity
            (singleton, issuer, subject, email, display_name, configured_at, updated_at)
        VALUES (?, ?, ?, ?, ?, ?, ?)
        """,
        (1, "https://access.example.test", "owner-1", "owner@example.test", "Owner", 10, 10),
    )
    assert tuple(
        database.execute("SELECT issuer, subject FROM owner_identity").fetchone()
    ) == ("https://access.example.test", "owner-1")

    # Pending, approved, denied, and expired are all explicit persisted states.
    add_request(database, "pending", "reader")
    add_request(database, "approved", "reader")
    database.execute(
        "UPDATE authorization_requests SET state = 'approved', approved_at = ? WHERE request_id = ?",
        (120, "approved"),
    )
    add_request(database, "denied", "reader")
    database.execute(
        "UPDATE authorization_requests SET state = 'denied', denied_at = ? WHERE request_id = ?",
        (121, "denied"),
    )
    add_request(database, "expired", "reader")
    database.execute(
        "UPDATE authorization_requests SET state = 'expired', expired_at = ? WHERE request_id = ?",
        (202, "expired"),
    )
    assert [
        tuple(row)
        for row in database.execute(
            "SELECT request_id, state FROM authorization_requests ORDER BY request_id"
        ).fetchall()
    ] == [
        ("approved", "approved"),
        ("denied", "denied"),
        ("expired", "expired"),
        ("pending", "pending"),
    ]

    # A sender claim inserts its credential and consumes the request as one
    # transaction. The transition trigger rejects a replay or state rewind.
    add_request(database, "consumed", "sender")
    database.execute(
        "UPDATE authorization_requests SET state = 'approved', approved_at = ? WHERE request_id = ?",
        (130, "consumed"),
    )
    database.execute("BEGIN IMMEDIATE")
    database.execute(
        """
        INSERT INTO sender_credentials
            (credential_id, name, bearer_token_hash, created_at)
        VALUES (?, ?, ?, ?)
        """,
        ("credential-1", "laptop", digest(9), 130),
    )
    database.execute(
        """
        UPDATE authorization_requests
        SET state = 'consumed', consumed_at = ?, credential_id = ?
        WHERE request_id = ?
        """,
        (131, "credential-1", "consumed"),
    )
    database.commit()
    assert tuple(
        database.execute(
            "SELECT state FROM authorization_requests WHERE request_id = 'consumed'"
        ).fetchone()
    ) == ("consumed",)

    try:
        database.execute(
            "UPDATE authorization_requests SET state = 'denied', denied_at = ? WHERE request_id = ?",
            (132, "consumed"),
        )
    except sqlite3.IntegrityError as error:
        assert "invalid authorization request state transition" in str(error)
    else:
        raise AssertionError("consumed request accepted a state rewind")

    database.execute(
        """
        INSERT INTO reader_sessions
            (session_id, bearer_token_hash, issued_at, expires_at)
        VALUES (?, ?, ?, ?)
        """,
        ("session-1", digest(11), 150, 250),
    )
    add_request(database, "reader-consumed", "reader")
    database.execute(
        "UPDATE authorization_requests SET state = 'approved', approved_at = ? WHERE request_id = ?",
        (160, "reader-consumed"),
    )
    database.execute("BEGIN IMMEDIATE")
    database.execute(
        """
        UPDATE authorization_requests
        SET state = 'consumed', consumed_at = ?, session_id = ?
        WHERE request_id = ?
        """,
        (161, "session-1", "reader-consumed"),
    )
    database.commit()
    assert tuple(
        database.execute(
            "SELECT state, session_id FROM authorization_requests WHERE request_id = 'reader-consumed'"
        ).fetchone()
    ) == ("consumed", "session-1")

    # Active names are unique, but a revoked name can be registered again.
    try:
        database.execute(
            """
            INSERT INTO sender_credentials
                (credential_id, name, bearer_token_hash, created_at)
            VALUES (?, ?, ?, ?)
            """,
            ("credential-2", "laptop", digest(10), 140),
        )
    except sqlite3.IntegrityError as error:
        assert "UNIQUE constraint failed" in str(error)
    else:
        raise AssertionError("active sender credential names are not unique")
    database.execute(
        "UPDATE sender_credentials SET revoked_at = ? WHERE credential_id = ?",
        (140, "credential-1"),
    )
    database.execute(
        """
        INSERT INTO sender_credentials
            (credential_id, name, bearer_token_hash, created_at)
        VALUES (?, ?, ?, ?)
        """,
        ("credential-2", "laptop", digest(10), 141),
    )

    # Hash columns accept binary digests and reject plaintext strings.
    try:
        database.execute(
            """
            INSERT INTO authorization_requests
                (request_id, kind, polling_secret_hash, approval_url, created_at, expires_at)
            VALUES (?, ?, ?, ?, ?, ?)
            """,
            ("polling-plaintext", "reader", "polling-secret", "https://example.test/plain", 170, 270),
        )
    except sqlite3.IntegrityError as error:
        assert "CHECK constraint failed" in str(error)
    else:
        raise AssertionError("authorization request accepted a plaintext polling secret")
    try:
        database.execute(
            """
            INSERT INTO reader_sessions
                (session_id, bearer_token_hash, issued_at, expires_at)
            VALUES (?, ?, ?, ?)
            """,
            ("session-plaintext", "reader-token", 150, 250),
        )
    except sqlite3.IntegrityError as error:
        assert "CHECK constraint failed" in str(error)
    else:
        raise AssertionError("reader session accepted a plaintext bearer token")
    assert tuple(
        database.execute(
            "SELECT typeof(bearer_token_hash) FROM sender_credentials WHERE credential_id = 'credential-2'"
        ).fetchone()
    ) == ("blob",)

    print(
        "prs-cloudflare migration checks passed ("
        + ", ".join(migration.name for migration in MIGRATIONS)
        + ")"
    )


if __name__ == "__main__":
    main()
