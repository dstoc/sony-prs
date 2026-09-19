#!/usr/bin/env python3
"""Exercise the authorization service's D1-compatible state transitions."""

from __future__ import annotations

from dataclasses import dataclass
import hashlib
from pathlib import Path
import sqlite3


REPO_ROOT = Path(__file__).resolve().parents[1]
MIGRATIONS = sorted(
    (REPO_ROOT / "crates" / "prs-cloudflare" / "migrations").glob("*.sql")
)


class AuthorizationFailure(Exception):
    """A deterministic equivalent of the Worker authorization failure class."""


@dataclass(frozen=True)
class Request:
    request_id: str
    kind: str
    polling_secret: str
    credential_name: str | None
    expires_at: int


def database() -> sqlite3.Connection:
    connection = sqlite3.connect(":memory:")
    connection.row_factory = sqlite3.Row
    connection.execute("PRAGMA foreign_keys = ON")
    for migration in MIGRATIONS:
        connection.executescript(migration.read_text())
    return connection


def digest(secret: str) -> bytes:
    return hashlib.sha256(secret.encode()).digest()


def create_request(
    connection: sqlite3.Connection,
    request_id: str,
    kind: str,
    polling_secret: str,
    now: int,
    ttl: int = 60,
    credential_name: str | None = None,
) -> Request:
    if kind == "sender" and credential_name is None:
        raise AssertionError("sender requests require a credential name")
    if kind == "reader" and credential_name is not None:
        raise AssertionError("reader requests cannot carry a credential name")
    expires_at = now + ttl
    approval_url = f"https://reader.example/a/{request_id}"
    connection.execute(
        """
        INSERT INTO authorization_requests
            (request_id, kind, polling_secret_hash, approval_url, state,
             created_at, expires_at, credential_name)
        VALUES (?, ?, ?, ?, 'pending', ?, ?, ?)
        """,
        (
            request_id,
            kind,
            digest(polling_secret),
            approval_url,
            now,
            expires_at,
            credential_name,
        ),
    )
    return Request(request_id, kind, polling_secret, credential_name, expires_at)


def expire_if_needed(connection: sqlite3.Connection, request_id: str, now: int) -> bool:
    cursor = connection.execute(
        """
        UPDATE authorization_requests
        SET state = 'expired', expired_at = ?
        WHERE request_id = ? AND state IN ('pending', 'approved') AND expires_at <= ?
        """,
        (now, request_id, now),
    )
    return cursor.rowcount == 1


def status(connection: sqlite3.Connection, request_id: str, now: int) -> str:
    expire_if_needed(connection, request_id, now)
    row = connection.execute(
        "SELECT state FROM authorization_requests WHERE request_id = ?",
        (request_id,),
    ).fetchone()
    if row is None:
        raise AuthorizationFailure("not found")
    return row["state"]


def transition(
    connection: sqlite3.Connection,
    request_id: str,
    expected_kind: str,
    approved: bool,
    now: int,
) -> None:
    state = "approved" if approved else "denied"
    timestamp_column = "approved_at" if approved else "denied_at"
    cursor = connection.execute(
        f"""
        UPDATE authorization_requests
        SET state = ?, {timestamp_column} = ?
        WHERE request_id = ? AND kind = ?
          AND state = 'pending' AND expires_at > ?
        """,
        (state, now, request_id, expected_kind, now),
    )
    if cursor.rowcount == 1:
        return

    row = connection.execute(
        "SELECT kind, state, expires_at FROM authorization_requests WHERE request_id = ?",
        (request_id,),
    ).fetchone()
    if row is None:
        raise AuthorizationFailure("not found")
    if row["kind"] != expected_kind:
        raise AuthorizationFailure("wrong authorization kind")
    if row["state"] == "pending" and row["expires_at"] <= now:
        expire_if_needed(connection, request_id, now)
        raise AuthorizationFailure("expired")
    if row["state"] == "denied":
        raise AuthorizationFailure("denied")
    if row["state"] == "expired":
        raise AuthorizationFailure("expired")
    if row["state"] == "consumed":
        raise AuthorizationFailure("already claimed")
    raise AuthorizationFailure("invalid state")


def claim(
    connection: sqlite3.Connection,
    request: Request,
    expected_kind: str,
    polling_secret: str,
    now: int,
    before_batch=None,
) -> str:
    row = connection.execute(
        """
        SELECT kind, polling_secret_hash, state, expires_at, credential_name
        FROM authorization_requests WHERE request_id = ?
        """,
        (request.request_id,),
    ).fetchone()
    if row is None:
        raise AuthorizationFailure("not found")
    if row["kind"] != expected_kind:
        raise AuthorizationFailure("wrong authorization kind")
    if row["polling_secret_hash"] != digest(polling_secret):
        raise AuthorizationFailure("invalid polling secret")
    if row["state"] in {"pending", "approved"} and now >= row["expires_at"]:
        expire_if_needed(connection, request.request_id, now)
        return "expired"
    if row["state"] == "pending":
        return "pending"
    if row["state"] == "denied":
        return "denied"
    if row["state"] == "expired":
        return "expired"
    if row["state"] == "consumed":
        return "already_claimed"

    child_id = f"{expected_kind}-child-{request.request_id}"
    bearer_token = f"{expected_kind}-token-{request.request_id}"
    if before_batch is not None:
        before_batch(connection)
    with connection:
        if expected_kind == "sender":
            connection.execute(
                """
                INSERT OR IGNORE INTO sender_credentials
                    (credential_id, name, bearer_token_hash, created_at)
                SELECT ?, ?, ?, ?
                WHERE EXISTS (
                    SELECT 1 FROM authorization_requests
                    WHERE request_id = ? AND kind = 'sender'
                      AND state = 'approved' AND expires_at > ?
                )
                """,
                (
                    child_id,
                    row["credential_name"],
                    digest(bearer_token),
                    now,
                    request.request_id,
                    now,
                ),
            )
            cursor = connection.execute(
                """
                UPDATE authorization_requests
                SET state = 'consumed', consumed_at = ?, credential_id = ?
                WHERE request_id = ? AND kind = 'sender' AND state = 'approved'
                  AND expires_at > ?
                  AND EXISTS (
                      SELECT 1 FROM sender_credentials WHERE credential_id = ?
                  )
                """,
                (now, child_id, request.request_id, now, child_id),
            )
        else:
            connection.execute(
                """
                INSERT INTO reader_sessions
                    (session_id, bearer_token_hash, issued_at, expires_at)
                SELECT ?, ?, ?, ?
                WHERE EXISTS (
                    SELECT 1 FROM authorization_requests
                    WHERE request_id = ? AND kind = 'reader'
                      AND state = 'approved' AND expires_at > ?
                )
                """,
                (child_id, digest(bearer_token), now, now + 3600, request.request_id, now),
            )
            cursor = connection.execute(
                """
                UPDATE authorization_requests
                SET state = 'consumed', consumed_at = ?, session_id = ?
                WHERE request_id = ? AND kind = 'reader' AND state = 'approved'
                  AND expires_at > ?
                  AND EXISTS (
                      SELECT 1 FROM reader_sessions WHERE session_id = ?
                  )
                """,
                (now, child_id, request.request_id, now, child_id),
            )
    if cursor.rowcount == 1:
        return "claimed"
    current = connection.execute(
        "SELECT state FROM authorization_requests WHERE request_id = ?",
        (request.request_id,),
    ).fetchone()
    if current["state"] == "expired":
        return "expired"
    if current["state"] == "approved" and now >= request.expires_at:
        expire_if_needed(connection, request.request_id, now)
        return "expired"
    if current["state"] != "consumed":
        name = row["credential_name"]
        if name is not None and connection.execute(
            "SELECT 1 FROM sender_credentials WHERE name = ? AND revoked_at IS NULL",
            (name,),
        ).fetchone():
            raise AuthorizationFailure("credential name already exists")
    return "already_claimed"


def authenticate_sender(connection: sqlite3.Connection, token: str) -> bool:
    row = connection.execute(
        """
        SELECT credential_id FROM sender_credentials
        WHERE bearer_token_hash = ? AND revoked_at IS NULL
        """,
        (digest(token),),
    ).fetchone()
    return row is not None


def authenticate_reader(connection: sqlite3.Connection, token: str, now: int) -> bool:
    row = connection.execute(
        """
        SELECT session_id FROM reader_sessions
        WHERE bearer_token_hash = ? AND revoked_at IS NULL AND expires_at > ?
        """,
        (digest(token), now),
    ).fetchone()
    return row is not None


def check_rate_limit(
    connection: sqlite3.Connection,
    bucket_key: str,
    limit: int,
    now: int,
    window: int = 60,
) -> int | None:
    row = connection.execute(
        """
        INSERT INTO authorization_rate_limits
            (bucket_key, window_started_at, request_count)
        VALUES (?, ?, 1)
        ON CONFLICT(bucket_key) DO UPDATE SET
            window_started_at = CASE
                WHEN authorization_rate_limits.window_started_at + ? <= excluded.window_started_at
                THEN excluded.window_started_at
                ELSE authorization_rate_limits.window_started_at
            END,
            request_count = CASE
                WHEN authorization_rate_limits.window_started_at + ? <= excluded.window_started_at
                THEN 1
                ELSE MIN(authorization_rate_limits.request_count + 1, 2147483647)
            END
        RETURNING window_started_at, request_count
        """,
        (bucket_key, now, window, window),
    ).fetchone()
    if row["request_count"] <= limit:
        return None
    return max(1, row["window_started_at"] + window - now)


def run_maintenance(connection: sqlite3.Connection, now: int) -> dict[str, int]:
    cutoff = max(0, now - 24 * 60 * 60)
    expired = connection.execute(
        """
        UPDATE authorization_requests
        SET state = 'expired', expired_at = expires_at
        WHERE rowid IN (
            SELECT rowid FROM authorization_requests
            WHERE state IN ('pending', 'approved') AND expires_at <= ?
            ORDER BY expires_at ASC
            LIMIT 100
        )
        """,
        (now,),
    ).rowcount
    sessions = connection.execute(
        """
        DELETE FROM reader_sessions
        WHERE rowid IN (
            SELECT rowid FROM reader_sessions
            WHERE expires_at <= ?
              AND NOT EXISTS (
                  SELECT 1 FROM authorization_requests
                  WHERE authorization_requests.session_id = reader_sessions.session_id
              )
            ORDER BY expires_at ASC
            LIMIT 100
        )
        """,
        (cutoff,),
    ).rowcount
    requests = connection.execute(
        """
        DELETE FROM authorization_requests
        WHERE rowid IN (
            SELECT rowid FROM authorization_requests
            WHERE state IN ('denied', 'expired', 'consumed')
              AND COALESCE(consumed_at, denied_at, expired_at) <= ?
            ORDER BY COALESCE(consumed_at, denied_at, expired_at) ASC
            LIMIT 100
        )
        """,
        (cutoff,),
    ).rowcount
    buckets = connection.execute(
        """
        DELETE FROM authorization_rate_limits
        WHERE rowid IN (
            SELECT rowid FROM authorization_rate_limits
            WHERE window_started_at + ? <= ?
            ORDER BY window_started_at ASC
            LIMIT 100
        )
        """,
        (60, now),
    ).rowcount
    return {
        "expired_requests": expired,
        "deleted_reader_sessions": sessions,
        "deleted_terminal_requests": requests,
        "deleted_rate_limit_buckets": buckets,
    }


def verify_sender_lifecycle() -> None:
    connection = database()
    request = create_request(
        connection,
        "sender-request",
        "sender",
        "sender-secret",
        100,
        credential_name="laptop",
    )
    public = dict(
        connection.execute(
            """
            SELECT request_id, kind, approval_url, created_at, expires_at
            FROM authorization_requests WHERE request_id = ?
            """,
            (request.request_id,),
        ).fetchone()
    )
    assert set(public) == {"request_id", "kind", "approval_url", "created_at", "expires_at"}
    assert "sender-secret" not in public["approval_url"]
    assert status(connection, request.request_id, 110) == "pending"
    assert claim(connection, request, "sender", "sender-secret", 110) == "pending"

    expect_failure(
        lambda: claim(connection, request, "reader", "sender-secret", 110),
        "wrong authorization kind",
    )
    expect_failure(
        lambda: claim(connection, request, "sender", "wrong-secret", 110),
        "invalid polling secret",
    )
    transition(connection, request.request_id, "sender", True, 120)
    expect_failure(
        lambda: transition(connection, request.request_id, "reader", True, 121),
        "wrong authorization kind",
    )
    assert claim(connection, request, "sender", "sender-secret", 121) == "claimed"
    assert status(connection, request.request_id, 121) == "consumed"
    assert connection.execute("SELECT count(*) FROM sender_credentials").fetchone()[0] == 1
    assert connection.execute("SELECT count(*) FROM reader_sessions").fetchone()[0] == 0
    assert claim(connection, request, "sender", "sender-secret", 122) == "already_claimed"
    assert connection.execute("SELECT count(*) FROM sender_credentials").fetchone()[0] == 1

    assert authenticate_sender(connection, "sender-token-sender-request")
    assert not authenticate_reader(connection, "sender-token-sender-request", 122)


def verify_reader_lifecycle() -> None:
    connection = database()
    request = create_request(connection, "reader-request", "reader", "reader-secret", 100)
    transition(connection, request.request_id, "reader", True, 110)
    assert claim(connection, request, "reader", "reader-secret", 111) == "claimed"
    assert connection.execute("SELECT count(*) FROM reader_sessions").fetchone()[0] == 1
    assert connection.execute("SELECT count(*) FROM sender_credentials").fetchone()[0] == 0
    assert authenticate_reader(connection, "reader-token-reader-request", 3710)
    assert not authenticate_reader(connection, "reader-token-reader-request", 3711)
    assert not authenticate_sender(connection, "reader-token-reader-request")
    assert claim(connection, request, "reader", "reader-secret", 112) == "already_claimed"


def verify_denial_and_expiration() -> None:
    connection = database()
    denied = create_request(connection, "denied-request", "reader", "denied-secret", 100)
    transition(connection, denied.request_id, "reader", False, 110)
    assert status(connection, denied.request_id, 111) == "denied"
    assert claim(connection, denied, "reader", "denied-secret", 111) == "denied"
    expect_failure(
        lambda: transition(connection, denied.request_id, "reader", True, 112),
        "denied",
    )

    expired = create_request(
        connection,
        "expired-request",
        "sender",
        "expired-secret",
        100,
        ttl=10,
        credential_name="expired",
    )
    assert status(connection, expired.request_id, 110) == "expired"
    assert claim(connection, expired, "sender", "expired-secret", 110) == "expired"
    expect_failure(
        lambda: transition(connection, expired.request_id, "sender", True, 111),
        "expired",
    )
    assert connection.execute("SELECT count(*) FROM sender_credentials").fetchone()[0] == 0

    approved = create_request(
        connection,
        "approved-expiry-request",
        "reader",
        "approved-expiry-secret",
        100,
        ttl=10,
    )
    transition(connection, approved.request_id, "reader", True, 105)
    assert status(connection, approved.request_id, 109) == "approved"
    assert status(connection, approved.request_id, 110) == "expired"
    assert claim(connection, approved, "reader", "approved-expiry-secret", 110) == "expired"
    assert tuple(
        connection.execute(
            "SELECT approved_at, expired_at FROM authorization_requests WHERE request_id = ?",
            (approved.request_id,),
        ).fetchone()
    ) == (105, 110)


def verify_same_name_claim_conflict() -> None:
    connection = database()
    first = create_request(
        connection,
        "same-name-first",
        "sender",
        "same-name-first-secret",
        100,
        credential_name="shared",
    )
    second = create_request(
        connection,
        "same-name-second",
        "sender",
        "same-name-second-secret",
        100,
        credential_name="shared",
    )
    transition(connection, first.request_id, "sender", True, 110)
    transition(connection, second.request_id, "sender", True, 110)
    assert claim(connection, first, "sender", "same-name-first-secret", 111) == "claimed"
    expect_failure(
        lambda: claim(connection, second, "sender", "same-name-second-secret", 111),
        "credential name already exists",
    )
    assert connection.execute(
        "SELECT state FROM authorization_requests WHERE request_id = ?",
        (second.request_id,),
    ).fetchone()[0] == "approved"
    assert connection.execute(
        "SELECT count(*) FROM sender_credentials WHERE name = 'shared'",
    ).fetchone()[0] == 1


def verify_claim_expiry_race() -> None:
    connection = database()
    sender = create_request(
        connection,
        "sender-expiry-race",
        "sender",
        "sender-expiry-race-secret",
        100,
        ttl=10,
        credential_name="expiry-race",
    )
    transition(connection, sender.request_id, "sender", True, 105)
    assert (
        claim(
            connection,
            sender,
            "sender",
            "sender-expiry-race-secret",
            105,
            before_batch=lambda db: expire_if_needed(db, sender.request_id, 110),
        )
        == "expired"
    )
    assert connection.execute(
        "SELECT count(*) FROM sender_credentials",
    ).fetchone()[0] == 0

    reader = create_request(
        connection,
        "reader-expiry-race",
        "reader",
        "reader-expiry-race-secret",
        100,
        ttl=10,
    )
    transition(connection, reader.request_id, "reader", True, 105)
    assert (
        claim(
            connection,
            reader,
            "reader",
            "reader-expiry-race-secret",
            105,
            before_batch=lambda db: expire_if_needed(db, reader.request_id, 110),
        )
        == "expired"
    )
    assert connection.execute(
        "SELECT count(*) FROM reader_sessions",
    ).fetchone()[0] == 0


def verify_rate_limits() -> None:
    connection = database()
    for _ in range(10):
        assert check_rate_limit(connection, "create:198.51.100.20", 10, 100) is None
    assert check_rate_limit(connection, "create:198.51.100.20", 10, 100) == 60
    assert check_rate_limit(connection, "create:198.51.100.21", 10, 100) is None
    assert check_rate_limit(connection, "create:198.51.100.20", 10, 159) == 1
    assert check_rate_limit(connection, "create:198.51.100.20", 10, 160) is None

    for _ in range(60):
        assert check_rate_limit(connection, "poll:198.51.100.20", 60, 200) is None
    assert check_rate_limit(connection, "poll:198.51.100.20", 60, 200) == 60


def verify_bounded_maintenance() -> None:
    connection = database()
    now = 100_000
    cutoff = now - 24 * 60 * 60
    connection.execute(
        "INSERT INTO sender_credentials (credential_id, name, bearer_token_hash, created_at) VALUES (?, ?, ?, ?)",
        ("active-credential", "active", digest("active"), 1),
    )
    connection.execute(
        "INSERT INTO reader_sessions (session_id, bearer_token_hash, issued_at, expires_at) VALUES (?, ?, ?, ?)",
        ("referenced-session", digest("referenced"), 1, cutoff),
    )
    connection.execute(
        """
        INSERT INTO authorization_requests
            (request_id, kind, polling_secret_hash, approval_url, state,
             created_at, expires_at, approved_at, consumed_at, credential_id, credential_name)
        VALUES (?, 'sender', ?, ?, 'consumed', 1, 2, 1, 2, ?, 'active')
        """,
        ("old-consumed-sender", digest("old-sender"), "https://example/old-sender", "active-credential"),
    )
    connection.execute(
        """
        INSERT INTO authorization_requests
            (request_id, kind, polling_secret_hash, approval_url, state,
             created_at, expires_at, approved_at, consumed_at, session_id)
        VALUES (?, 'reader', ?, ?, 'consumed', 1, 2, 1, 2, ?)
        """,
        ("old-consumed-reader", digest("old-reader"), "https://example/old-reader", "referenced-session"),
    )
    connection.execute(
        """
        INSERT INTO authorization_requests
            (request_id, kind, polling_secret_hash, approval_url, state,
             created_at, expires_at, denied_at, credential_name)
        VALUES (?, 'sender', ?, ?, 'denied', 1, 2, 2, 'denied')
        """,
        ("old-denied", digest("old-denied"), "https://example/old-denied"),
    )
    create_request(connection, "old-pending", "reader", "old-pending-secret", 1)
    connection.execute(
        "INSERT INTO authorization_rate_limits (bucket_key, window_started_at, request_count) VALUES (?, ?, ?)",
        ("old-bucket", 1, 100),
    )
    connection.execute(
        "INSERT INTO authorization_rate_limits (bucket_key, window_started_at, request_count) VALUES (?, ?, ?)",
        ("current-bucket", now - 1, 1),
    )

    report = run_maintenance(connection, now)
    assert report["expired_requests"] == 1
    assert report["deleted_terminal_requests"] == 4
    assert report["deleted_reader_sessions"] == 0
    assert report["deleted_rate_limit_buckets"] == 1
    assert connection.execute(
        "SELECT count(*) FROM sender_credentials WHERE credential_id = 'active-credential'"
    ).fetchone()[0] == 1
    assert connection.execute(
        "SELECT count(*) FROM reader_sessions WHERE session_id = 'referenced-session'"
    ).fetchone()[0] == 1
    assert connection.execute(
        "SELECT count(*) FROM authorization_requests WHERE request_id = 'old-pending'"
    ).fetchone()[0] == 0
    assert connection.execute(
        "SELECT count(*) FROM authorization_rate_limits WHERE bucket_key = 'current-bucket'"
    ).fetchone()[0] == 1

    second_report = run_maintenance(connection, now)
    assert second_report["deleted_reader_sessions"] == 1
    assert connection.execute(
        "SELECT count(*) FROM reader_sessions WHERE session_id = 'referenced-session'"
    ).fetchone()[0] == 0

    bounded = database()
    for index in range(101):
        bounded.execute(
            """
            INSERT INTO authorization_requests
                (request_id, kind, polling_secret_hash, approval_url, state,
                 created_at, expires_at, denied_at, credential_name)
            VALUES (?, 'sender', ?, ?, 'denied', 1, 2, 2, ?)
            """,
            (f"bounded-{index}", digest(str(index)), f"https://example/{index}", f"name-{index}"),
        )
    assert run_maintenance(bounded, now)["deleted_terminal_requests"] == 100
    assert bounded.execute(
        "SELECT count(*) FROM authorization_requests"
    ).fetchone()[0] == 1


def expect_failure(action, message: str) -> None:
    try:
        action()
    except AuthorizationFailure as error:
        assert str(error) == message
    else:
        raise AssertionError(f"expected authorization failure: {message}")


def main() -> None:
    verify_sender_lifecycle()
    verify_reader_lifecycle()
    verify_denial_and_expiration()
    verify_same_name_claim_conflict()
    verify_claim_expiry_race()
    verify_rate_limits()
    verify_bounded_maintenance()
    print("prs-cloudflare authorization checks passed")


if __name__ == "__main__":
    main()
