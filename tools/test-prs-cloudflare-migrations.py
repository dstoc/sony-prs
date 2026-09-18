#!/usr/bin/env python3
"""Exercise the D1-compatible SQL migrations with Python's SQLite library."""

from pathlib import Path
import sqlite3


REPO_ROOT = Path(__file__).resolve().parents[1]
MIGRATIONS_DIRECTORY = REPO_ROOT / "crates" / "prs-cloudflare" / "migrations"
MIGRATIONS = sorted(MIGRATIONS_DIRECTORY.glob("*.sql"))
INITIAL_MIGRATION = MIGRATIONS_DIRECTORY / "0001_initial.sql"
UPGRADE_MIGRATION = MIGRATIONS_DIRECTORY / "0002_metadata_schema_upgrade.sql"
AUTHORIZATION_NAME_MIGRATION = MIGRATIONS_DIRECTORY / "0003_authorization_credential_name.sql"


def database() -> sqlite3.Connection:
    connection = sqlite3.connect(":memory:", isolation_level=None)
    connection.row_factory = sqlite3.Row
    connection.execute("PRAGMA foreign_keys = ON")
    return connection


def apply_migration(connection: sqlite3.Connection, migration: Path) -> None:
    connection.executescript(migration.read_text())


def apply_all_migrations(
    connection: sqlite3.Connection, applied: set[str] | None = None
) -> set[str]:
    applied = set() if applied is None else applied
    for migration in MIGRATIONS:
        if migration.name in applied:
            continue
        apply_migration(connection, migration)
        applied.add(migration.name)
    return applied


def schema(connection: sqlite3.Connection) -> list[tuple[str, str, str]]:
    return connection.execute(
        """
        SELECT type, name, sql
        FROM sqlite_master
        WHERE name NOT LIKE 'sqlite_%'
        ORDER BY type, name
        """
    ).fetchall()


def columns(connection: sqlite3.Connection, table: str) -> set[str]:
    return {
        row[1]
        for row in connection.execute(f"PRAGMA table_info({table})").fetchall()
    }


def digest(byte: int) -> bytes:
    return bytes([byte]) * 32


def add_request(
    connection: sqlite3.Connection, request_id: str, kind: str
) -> None:
    credential_name = request_id if kind == "sender" else None
    connection.execute(
        """
        INSERT INTO authorization_requests
            (request_id, kind, polling_secret_hash, approval_url, created_at,
             expires_at, credential_name)
        VALUES (?, ?, ?, ?, ?, ?, ?)
        """,
        (
            request_id,
            kind,
            digest(ord(request_id[0])),
            f"https://example.test/{request_id}",
            100,
            200,
            credential_name,
        ),
    )


def seed_legacy_database(connection: sqlite3.Connection) -> None:
    """Create representative rows under the applied 0001 schema."""

    connection.execute(
        """
        UPDATE inbox
        SET revision = ?, object_key = ?, etag = ?, manifest_json = ?, updated_at = ?
        WHERE singleton = 1
        """,
        (7, "bundles/legacy.tar", "etag-legacy", '{"entry_point":"index.md"}', 100),
    )
    connection.execute(
        """
        INSERT INTO sender_credentials
            (credential_id, name, bearer_token_hash, created_at)
        VALUES (?, ?, ?, ?)
        """,
        ("legacy-credential", "legacy-laptop", digest(3), 10),
    )
    connection.execute(
        """
        INSERT INTO reader_sessions
            (session_id, bearer_token_hash, issued_at, expires_at)
        VALUES (?, ?, ?, ?)
        """,
        ("legacy-session", digest(4), 20, None),
    )
    connection.execute(
        """
        INSERT INTO authorization_requests
            (request_id, kind, polling_secret_hash, approval_url, state,
             created_at, expires_at, claimed_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        """,
        (
            "legacy-claimed",
            "sender",
            digest(5),
            "https://example.test/legacy-claimed",
            "claimed",
            30,
            40,
            35,
        ),
    )


def assert_fresh_schema_is_deterministic() -> None:
    first = database()
    second = database()
    apply_all_migrations(first)
    apply_all_migrations(second)
    assert schema(first) == schema(second)


def assert_d1_reapplication_is_a_noop() -> None:
    connection = database()
    applied = apply_all_migrations(connection)
    initial_schema = schema(connection)

    # Wrangler records applied migration filenames. A second migration apply
    # therefore skips the same files and leaves the schema unchanged.
    assert apply_all_migrations(connection, applied) == applied
    assert schema(connection) == initial_schema


def verify_upgrade_path() -> None:
    connection = database()
    apply_migration(connection, INITIAL_MIGRATION)
    seed_legacy_database(connection)
    apply_migration(connection, UPGRADE_MIGRATION)
    apply_migration(connection, AUTHORIZATION_NAME_MIGRATION)

    assert columns(connection, "inbox") == {
        "singleton",
        "revision",
        "current_bundle_id",
        "updated_at",
    }
    assert "credential_name" in columns(connection, "authorization_requests")
    assert dict(
        connection.execute(
            "SELECT revision, current_bundle_id FROM inbox"
        ).fetchone()
    ) == {"revision": 7, "current_bundle_id": "legacy-inbox"}
    assert tuple(
        connection.execute(
            "SELECT object_key, etag, manifest_json, size_bytes FROM bundles"
        ).fetchone()
    ) == ("bundles/legacy.tar", "etag-legacy", '{"entry_point":"index.md"}', 0)

    # The old nullable expiry is converted to a short, finite lifetime.
    assert tuple(
        connection.execute(
            "SELECT expires_at FROM reader_sessions WHERE session_id = 'legacy-session'"
        ).fetchone()
    ) == (21,)

    # The old claimed state has no child reference. Preserve it as consumed
    # with a compatibility timestamp instead of inventing a credential.
    assert tuple(
        connection.execute(
            """
            SELECT state, approved_at, consumed_at, legacy_claimed_at
            FROM authorization_requests
            WHERE request_id = 'legacy-claimed'
            """
        ).fetchone()
    ) == ("consumed", 30, 35, 35)
    assert tuple(
        connection.execute(
            """
            SELECT credential_name
            FROM authorization_requests
            WHERE request_id = 'legacy-claimed'
            """
        ).fetchone()
    ) == ("legacy-legacy-claimed",)


def verify_current_schema_behavior() -> None:
    connection = database()
    apply_all_migrations(connection)

    assert dict(
        connection.execute(
            "SELECT singleton, revision, current_bundle_id FROM inbox"
        ).fetchone()
    ) == {"singleton": 1, "revision": 0, "current_bundle_id": None}

    # The inbox stores only a reference and metadata. Bundle bytes are not a
    # D1 column, and the reference/revision update is atomic.
    connection.execute(
        """
        INSERT INTO bundles
            (bundle_id, object_key, etag, manifest_json, size_bytes, created_at)
        VALUES (?, ?, ?, ?, ?, ?)
        """,
        ("bundle-1", "bundles/bundle-1.tar", "etag-1", '{"entry_point":"index.md"}', 42, 10),
    )
    connection.execute("BEGIN IMMEDIATE")
    connection.execute(
        "UPDATE inbox SET current_bundle_id = ?, revision = ?, updated_at = ? WHERE singleton = 1",
        ("bundle-1", 1, 10),
    )
    connection.commit()
    assert dict(
        connection.execute(
            "SELECT revision, current_bundle_id FROM inbox"
        ).fetchone()
    ) == {"revision": 1, "current_bundle_id": "bundle-1"}

    connection.execute(
        """
        INSERT INTO owner_identity
            (singleton, issuer, subject, email, display_name, configured_at, updated_at)
        VALUES (?, ?, ?, ?, ?, ?, ?)
        """,
        (1, "https://access.example.test", "owner-1", "owner@example.test", "Owner", 10, 10),
    )
    assert tuple(
        connection.execute("SELECT issuer, subject FROM owner_identity").fetchone()
    ) == ("https://access.example.test", "owner-1")

    # Pending, approved, denied, and expired are explicit persisted states.
    add_request(connection, "pending", "reader")
    add_request(connection, "approved", "reader")
    connection.execute(
        "UPDATE authorization_requests SET state = 'approved', approved_at = ? WHERE request_id = ?",
        (120, "approved"),
    )
    add_request(connection, "denied", "reader")
    connection.execute(
        "UPDATE authorization_requests SET state = 'denied', denied_at = ? WHERE request_id = ?",
        (121, "denied"),
    )
    add_request(connection, "expired", "reader")
    connection.execute(
        "UPDATE authorization_requests SET state = 'expired', expired_at = ? WHERE request_id = ?",
        (202, "expired"),
    )
    assert [
        tuple(row)
        for row in connection.execute(
            "SELECT request_id, state FROM authorization_requests ORDER BY request_id"
        ).fetchall()
    ] == [
        ("approved", "approved"),
        ("denied", "denied"),
        ("expired", "expired"),
        ("pending", "pending"),
    ]

    # A sender claim inserts its credential and consumes the request as one
    # transaction. The trigger rejects a replay or state rewind.
    add_request(connection, "consumed", "sender")
    connection.execute(
        "UPDATE authorization_requests SET state = 'approved', approved_at = ? WHERE request_id = ?",
        (130, "consumed"),
    )
    connection.execute("BEGIN IMMEDIATE")
    connection.execute(
        """
        INSERT INTO sender_credentials
            (credential_id, name, bearer_token_hash, created_at)
        VALUES (?, ?, ?, ?)
        """,
        ("credential-1", "laptop", digest(9), 130),
    )
    connection.execute(
        """
        UPDATE authorization_requests
        SET state = 'consumed', consumed_at = ?, credential_id = ?
        WHERE request_id = ?
        """,
        (131, "credential-1", "consumed"),
    )
    connection.commit()
    assert tuple(
        connection.execute(
            "SELECT state FROM authorization_requests WHERE request_id = 'consumed'"
        ).fetchone()
    ) == ("consumed",)

    try:
        connection.execute(
            "UPDATE authorization_requests SET state = 'denied', denied_at = ? WHERE request_id = ?",
            (132, "consumed"),
        )
    except sqlite3.IntegrityError as error:
        assert "invalid authorization request state transition" in str(error)
    else:
        raise AssertionError("consumed request accepted a state rewind")

    connection.execute(
        """
        INSERT INTO reader_sessions
            (session_id, bearer_token_hash, issued_at, expires_at)
        VALUES (?, ?, ?, ?)
        """,
        ("session-1", digest(11), 150, 250),
    )
    add_request(connection, "reader-consumed", "reader")
    connection.execute(
        "UPDATE authorization_requests SET state = 'approved', approved_at = ? WHERE request_id = ?",
        (160, "reader-consumed"),
    )
    connection.execute("BEGIN IMMEDIATE")
    connection.execute(
        """
        UPDATE authorization_requests
        SET state = 'consumed', consumed_at = ?, session_id = ?
        WHERE request_id = ?
        """,
        (161, "session-1", "reader-consumed"),
    )
    connection.commit()
    assert tuple(
        connection.execute(
            "SELECT state, session_id FROM authorization_requests WHERE request_id = 'reader-consumed'"
        ).fetchone()
    ) == ("consumed", "session-1")

    assert tuple(
        connection.execute(
            "SELECT credential_name FROM authorization_requests WHERE request_id = 'consumed'"
        ).fetchone()
    ) == ("consumed",)

    # A reader request cannot smuggle a sender credential name into the
    # approval flow, and a sender request must retain one for claim.
    try:
        connection.execute(
            """
            INSERT INTO authorization_requests
                (request_id, kind, polling_secret_hash, approval_url,
                 created_at, expires_at, credential_name)
            VALUES (?, ?, ?, ?, ?, ?, ?)
            """,
            ("reader-named", "reader", digest(12), "https://example.test/reader-named", 180, 280, "wrong"),
        )
    except sqlite3.IntegrityError as error:
        assert "invalid authorization credential name" in str(error)
    else:
        raise AssertionError("reader request accepted a sender credential name")

    # Active names are unique, but a revoked name can be registered again.
    try:
        connection.execute(
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
    connection.execute(
        "UPDATE sender_credentials SET revoked_at = ? WHERE credential_id = ?",
        (140, "credential-1"),
    )
    connection.execute(
        """
        INSERT INTO sender_credentials
            (credential_id, name, bearer_token_hash, created_at)
        VALUES (?, ?, ?, ?)
        """,
        ("credential-2", "laptop", digest(10), 141),
    )

    # Hash columns reject plaintext values.
    try:
        connection.execute(
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
        connection.execute(
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
        connection.execute(
            "SELECT typeof(bearer_token_hash) FROM sender_credentials WHERE credential_id = 'credential-2'"
        ).fetchone()
    ) == ("blob",)


def main() -> None:
    if not MIGRATIONS:
        raise AssertionError("at least one D1 migration is required")
    assert INITIAL_MIGRATION in MIGRATIONS
    assert UPGRADE_MIGRATION in MIGRATIONS
    assert AUTHORIZATION_NAME_MIGRATION in MIGRATIONS

    assert_fresh_schema_is_deterministic()
    assert_d1_reapplication_is_a_noop()
    verify_upgrade_path()
    verify_current_schema_behavior()

    print(
        "prs-cloudflare migration checks passed ("
        + ", ".join(migration.name for migration in MIGRATIONS)
        + ")"
    )


if __name__ == "__main__":
    main()
