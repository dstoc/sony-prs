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
        WHERE request_id = ? AND state = 'pending' AND expires_at <= ?
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
    if row["state"] == "pending" and now >= row["expires_at"]:
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
    with connection:
        if expected_kind == "sender":
            connection.execute(
                """
                INSERT INTO sender_credentials
                    (credential_id, name, bearer_token_hash, created_at)
                SELECT ?, ?, ?, ?
                WHERE EXISTS (
                    SELECT 1 FROM authorization_requests
                    WHERE request_id = ? AND kind = 'sender' AND state = 'approved'
                )
                """,
                (
                    child_id,
                    row["credential_name"],
                    digest(bearer_token),
                    now,
                    request.request_id,
                ),
            )
            cursor = connection.execute(
                """
                UPDATE authorization_requests
                SET state = 'consumed', consumed_at = ?, credential_id = ?
                WHERE request_id = ? AND kind = 'sender' AND state = 'approved'
                """,
                (now, child_id, request.request_id),
            )
        else:
            connection.execute(
                """
                INSERT INTO reader_sessions
                    (session_id, bearer_token_hash, issued_at, expires_at)
                SELECT ?, ?, ?, ?
                WHERE EXISTS (
                    SELECT 1 FROM authorization_requests
                    WHERE request_id = ? AND kind = 'reader' AND state = 'approved'
                )
                """,
                (child_id, digest(bearer_token), now, now + 3600, request.request_id),
            )
            cursor = connection.execute(
                """
                UPDATE authorization_requests
                SET state = 'consumed', consumed_at = ?, session_id = ?
                WHERE request_id = ? AND kind = 'reader' AND state = 'approved'
                """,
                (now, child_id, request.request_id),
            )
    return "claimed" if cursor.rowcount == 1 else "already_claimed"


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
    assert authenticate_reader(connection, "reader-token-reader-request", 111)
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
    print("prs-cloudflare authorization checks passed")


if __name__ == "__main__":
    main()
