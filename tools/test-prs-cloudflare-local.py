#!/usr/bin/env python3
"""Exercise the complete PRSync workflow against local Wrangler bindings."""

from __future__ import annotations

import io
import json
import os
import shutil
import signal
import socket
import subprocess
import sys
import tarfile
import tempfile
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from typing import Any
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen


PROTOCOL_VERSION = {"major": 1, "minor": 0}
MAX_BUNDLE_SIZE = 16 * 1024 * 1024
OWNER_HEADER = "https://team.cloudflareaccess.com|local-owner|owner@example.com"
WRANGLER_CONFIG = "wrangler.local.toml"


class HttpResponse:
    def __init__(self, status: int, headers: dict[str, str], body: bytes) -> None:
        self.status = status
        self.headers = headers
        self.body = body

    def json(self) -> Any:
        return json.loads(self.body)


def run_wrangler(
    wrangler: str,
    worker_dir: Path,
    state_dir: Path,
    *arguments: str,
) -> None:
    command = [
        wrangler,
        "--config",
        WRANGLER_CONFIG,
        *arguments,
        "--persist-to",
        str(state_dir),
    ]
    result = subprocess.run(
        command,
        cwd=worker_dir,
        check=False,
        text=True,
        capture_output=True,
    )
    if result.returncode != 0:
        raise RuntimeError(
            "Wrangler command failed:\n"
            f"$ {' '.join(command)}\n"
            f"{result.stdout}{result.stderr}"
        )


def request(
    base_url: str,
    method: str,
    path: str,
    *,
    body: bytes | None = None,
    headers: dict[str, str] | None = None,
) -> HttpResponse:
    request_headers = {"Accept": "application/json"}
    if headers:
        request_headers.update(headers)
    http_request = Request(
        f"{base_url}{path}",
        data=body,
        headers=request_headers,
        method=method,
    )
    try:
        with urlopen(http_request, timeout=10) as response:
            return HttpResponse(
                response.status,
                {key.lower(): value for key, value in response.headers.items()},
                response.read(),
            )
    except HTTPError as error:
        return HttpResponse(
            error.code,
            {key.lower(): value for key, value in error.headers.items()},
            error.read(),
        )


def json_request(
    base_url: str,
    method: str,
    path: str,
    value: Any | None = None,
    *,
    headers: dict[str, str] | None = None,
) -> HttpResponse:
    body = None if value is None else json.dumps(value, separators=(",", ":")).encode()
    request_headers = {"Content-Type": "application/json"} if body is not None else {}
    if headers:
        request_headers.update(headers)
    return request(base_url, method, path, body=body, headers=request_headers)


def assert_status(response: HttpResponse, expected: int, description: str) -> None:
    if response.status != expected:
        raise AssertionError(
            f"{description}: expected HTTP {expected}, got {response.status}: "
            f"{response.body.decode(errors='replace')}"
        )


def assert_api_error(response: HttpResponse, status: int, code: str, description: str) -> None:
    assert_status(response, status, description)
    error = response.json()["error"]
    if error["code"] != code:
        raise AssertionError(f"{description}: expected error code {code}, got {error['code']}")


def make_bundle(files: dict[str, bytes]) -> bytes:
    if "index.md" not in files:
        raise ValueError("bundle must contain index.md")
    manifest = {
        "protocol_version": PROTOCOL_VERSION,
        "bundle_format_version": 1,
        "entry_point": "index.md",
        "files": [
            {"path": path, "size": len(contents)}
            for path, contents in files.items()
        ],
    }
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode="w", format=tarfile.USTAR_FORMAT) as archive:
        append_tar_file(archive, "manifest.json", json.dumps(manifest, separators=(",", ":")).encode())
        for path, contents in files.items():
            append_tar_file(archive, path, contents)
    return output.getvalue()


def make_boundary_bundle() -> bytes:
    # Python's tarfile writer pads the archive to a 10 KiB record. Trim only
    # zero padding after the required tar end marker to reach the protocol
    # boundary without changing any archive entry.
    content_size = MAX_BUNDLE_SIZE - 4096
    contents = b"# boundary\n" + b"x" * (content_size - len(b"# boundary\n"))
    bundle = make_bundle({"index.md": contents})
    if len(bundle) < MAX_BUNDLE_SIZE or any(bundle[MAX_BUNDLE_SIZE:]):
        raise AssertionError("could not construct a boundary-size valid bundle")
    return bundle[:MAX_BUNDLE_SIZE]


def append_tar_file(archive: tarfile.TarFile, path: str, contents: bytes) -> None:
    info = tarfile.TarInfo(path)
    info.size = len(contents)
    info.mtime = 0
    info.mode = 0o644
    archive.addfile(info, io.BytesIO(contents))


def start_authorization(
    base_url: str,
    kind: str,
    credential_name: str | None = None,
) -> tuple[dict[str, Any], str]:
    body: dict[str, Any] = {"protocol_version": PROTOCOL_VERSION}
    if kind == "sender":
        body["credential_name"] = credential_name
    response = json_request(base_url, "POST", f"/api/v1/authorization/{kind}", body)
    assert_status(response, 200, f"create {kind} authorization")
    payload = response.json()
    return payload["request"], payload["polling_secret"]


def approve(base_url: str, request_id: str, kind: str) -> None:
    owner_headers = {"X-PRSync-Test-Owner": OWNER_HEADER}
    wrong_headers = {"X-PRSync-Test-Owner": OWNER_HEADER.replace("local-owner", "wrong-owner")}
    response = request(base_url, "GET", f"/a/{request_id}", headers=wrong_headers)
    assert_status(response, 403, "wrong owner approval request")
    response = request(base_url, "GET", f"/a/{request_id}", headers=owner_headers)
    assert_status(response, 200, f"show {kind} approval request")
    page = response.body.decode()
    if request_id not in page or "Approve request" not in page:
        raise AssertionError(f"{kind} approval page did not contain request context")
    if "polling_secret" in page or "bearer_token" in page:
        raise AssertionError(f"{kind} approval page exposed a secret")
    response = request(
        base_url,
        "POST",
        f"/a/{request_id}/approve",
        body=b"",
        headers={**owner_headers, "Content-Type": "application/x-www-form-urlencoded"},
    )
    assert_status(response, 200, f"approve {kind} authorization")
    if "approved" not in response.body.decode().lower():
        raise AssertionError(f"{kind} approval response did not show approved state")


def claim(
    base_url: str,
    request_id: str,
    polling_secret: str,
    kind: str,
) -> dict[str, Any]:
    response = json_request(
        base_url,
        "POST",
        "/api/v1/authorization/poll",
        {
            "protocol_version": PROTOCOL_VERSION,
            "request_id": request_id,
            "polling_secret": polling_secret,
        },
    )
    assert_status(response, 200, f"claim {kind} authorization")
    outcome = response.json()["outcome"]
    if outcome["kind"] != kind:
        raise AssertionError(f"expected {kind} claim, got {outcome['kind']}")
    return outcome


def verify_sender_flow(base_url: str) -> tuple[str, dict[str, Any]]:
    request_data, polling_secret = start_authorization(base_url, "sender", "ci-sender")
    request_id = request_data["request_id"]
    if request_data["kind"] != "sender" or request_data["credential_name"] != "ci-sender":
        raise AssertionError("sender authorization did not preserve its name and kind")
    response = json_request(
        base_url,
        "GET",
        f"/api/v1/authorization/{request_id}",
    )
    assert_status(response, 200, "read pending sender status")
    if response.json()["state"] != "pending":
        raise AssertionError("new sender authorization was not pending")
    approve(base_url, request_id, "sender")
    grant = claim(base_url, request_id, polling_secret, "sender")["credential"]
    token = grant["bearer_token"]
    if grant["metadata"]["name"] != "ci-sender":
        raise AssertionError("sender claim returned the wrong credential name")
    if grant["metadata"]["scope"]["capabilities"] != [
        "upload_bundle",
        "clear_inbox",
        "manage_credentials",
    ]:
        raise AssertionError("sender claim returned the wrong scope")
    replay = json_request(
        base_url,
        "POST",
        "/api/v1/authorization/poll",
        {
            "protocol_version": PROTOCOL_VERSION,
            "request_id": request_id,
            "polling_secret": polling_secret,
        },
    )
    assert_status(replay, 200, "replay sender claim")
    if replay.json()["outcome"]["kind"] != "already_claimed":
        raise AssertionError("sender claim was not one-time")
    return token, grant["metadata"]


def verify_reader_flow(base_url: str) -> str:
    request_data, polling_secret = start_authorization(base_url, "reader")
    request_id = request_data["request_id"]
    if request_data["kind"] != "reader" or "credential_name" in request_data:
        raise AssertionError("reader authorization contained sender-only fields")
    approve(base_url, request_id, "reader")
    session = claim(base_url, request_id, polling_secret, "reader")["session"]
    capabilities = session["scope"]["capabilities"]
    if capabilities != ["read_manifest", "download_bundle"]:
        raise AssertionError("reader claim returned mutation capabilities")
    return session["bearer_token"]


def verify_manifest(base_url: str, token: str, expected_entry: str) -> tuple[int, str]:
    response = request(
        base_url,
        "GET",
        "/api/v1/reader/manifest",
        headers={"Authorization": f"Bearer {token}"},
    )
    assert_status(response, 200, "read current manifest")
    state = response.json()["state"]
    if state["kind"] != "current" or state["manifest"]["entry_point"] != expected_entry:
        raise AssertionError("manifest did not describe the current bundle")
    revision = state["revision"]
    etag = state["etag"]

    response = request(
        base_url,
        "GET",
        "/api/v1/reader/manifest",
        headers={
            "Authorization": f"Bearer {token}",
            "If-Revision": str(revision),
        },
    )
    assert_status(response, 200, "read manifest with matching revision")
    if response.json()["state"]["kind"] != "not_modified":
        raise AssertionError("matching revision did not produce not_modified")

    response = request(
        base_url,
        "GET",
        "/api/v1/reader/manifest",
        headers={
            "Authorization": f"Bearer {token}",
            "If-None-Match": f'"{etag}"',
        },
    )
    assert_status(response, 200, "read manifest with matching ETag")
    if response.json()["state"]["kind"] != "not_modified":
        raise AssertionError("matching ETag did not produce not_modified")
    return revision, etag


def verify_capability_separation(base_url: str, sender_token: str, reader_token: str) -> None:
    for path in ("/api/v1/reader/manifest", "/api/v1/reader/bundle"):
        response = request(
            base_url,
            "GET",
            path,
            headers={"Authorization": f"Bearer {sender_token}"},
        )
        assert_api_error(response, 401, "unauthorized", f"sender readback at {path}")

    for method, path, body in (
        ("PUT", "/api/v1/sender/bundle", b"not a bundle"),
        ("DELETE", "/api/v1/sender/bundle", None),
        ("GET", "/api/v1/sender/credentials", None),
        ("DELETE", "/api/v1/sender/credentials/ci-sender", None),
    ):
        response = request(
            base_url,
            method,
            path,
            body=body,
            headers={"Authorization": f"Bearer {reader_token}"},
        )
        assert_api_error(response, 401, "unauthorized", f"reader mutation at {path}")


def verify_concurrent_mutation_results(base_url: str, sender_token: str) -> None:
    bundles = [
        make_bundle(
            {
                "index.md": f"# Concurrent bundle {index}\n".encode(),
                f"payload-{index}.txt": bytes([65 + index]) * (index + 1),
            }
        )
        for index in range(8)
    ]

    def push(bundle: bytes) -> HttpResponse:
        return request(
            base_url,
            "PUT",
            "/api/v1/sender/bundle",
            body=bundle,
            headers={
                "Authorization": f"Bearer {sender_token}",
                "Content-Type": "application/x-tar",
            },
        )

    with ThreadPoolExecutor(max_workers=len(bundles)) as executor:
        responses = list(executor.map(push, bundles))

    revisions: set[int] = set()
    for bundle, response in zip(bundles, responses):
        assert_status(response, 200, "concurrent push")
        payload = response.json()
        if payload["size_bytes"] != len(bundle):
            raise AssertionError("concurrent push returned another bundle's size")
        if not payload["etag"]:
            raise AssertionError("concurrent push returned an empty ETag")
        revision = payload["revision"]
        if revision in revisions:
            raise AssertionError("concurrent pushes returned a duplicate revision")
        revisions.add(revision)

    def clear(_: int) -> HttpResponse:
        return request(
            base_url,
            "DELETE",
            "/api/v1/sender/bundle",
            headers={"Authorization": f"Bearer {sender_token}"},
        )

    with ThreadPoolExecutor(max_workers=4) as executor:
        clear_responses = list(executor.map(clear, range(4)))

    clear_revisions: set[int] = set()
    for response in clear_responses:
        assert_status(response, 200, "concurrent clear")
        payload = response.json()
        if payload["state"] != "empty":
            raise AssertionError("concurrent clear returned a non-empty state")
        revision = payload["revision"]
        if revision in clear_revisions:
            raise AssertionError("concurrent clears returned a duplicate revision")
        clear_revisions.add(revision)


def verify_credentials(base_url: str, sender_token: str) -> None:
    response = request(
        base_url,
        "GET",
        "/api/v1/sender/credentials",
        headers={"Authorization": f"Bearer {sender_token}"},
    )
    assert_status(response, 200, "list sender credentials")
    credentials = response.json()["credentials"]
    if len(credentials) != 1 or credentials[0]["name"] != "ci-sender":
        raise AssertionError("sender credential list did not contain ci-sender")
    if "bearer_token" in json.dumps(credentials):
        raise AssertionError("sender credential list exposed a bearer token")

    response = json_request(
        base_url,
        "DELETE",
        "/api/v1/sender/credentials",
        {"protocol_version": PROTOCOL_VERSION, "name": "ci-sender"},
        headers={"Authorization": f"Bearer {sender_token}"},
    )
    assert_status(response, 200, "revoke sender credential")
    if not response.json()["revoked"]:
        raise AssertionError("sender credential was not revoked")

    response = request(
        base_url,
        "GET",
        "/api/v1/sender/credentials",
        headers={"Authorization": f"Bearer {sender_token}"},
    )
    assert_api_error(response, 401, "unauthorized", "use revoked sender credential")


def main() -> None:
    repository_root = Path(__file__).resolve().parents[1]
    worker_dir = repository_root / "crates" / "prs-cloudflare"
    wrangler = os.environ.get("WRANGLER") or shutil.which("wrangler")
    if not wrangler:
        raise RuntimeError("wrangler is required; install Wrangler before running this test")
    if not shutil.which("worker-build"):
        raise RuntimeError("worker-build is required; install worker-build before running this test")

    with tempfile.TemporaryDirectory(prefix="prs-cloudflare-local-") as temporary:
        state_dir = Path(temporary) / "state"
        run_wrangler(
            wrangler,
            worker_dir,
            state_dir,
            "d1",
            "migrations",
            "apply",
            "DB",
            "--local",
        )
        run_wrangler(
            wrangler,
            worker_dir,
            state_dir,
            "d1",
            "execute",
            "DB",
            "--local",
            "--yes",
            "--command",
            "INSERT INTO owner_identity (singleton, issuer, subject, email, display_name, configured_at, updated_at) VALUES (1, 'https://team.cloudflareaccess.com', 'local-owner', 'owner@example.com', 'Local owner', 0, 0)",
        )

        port = free_port()
        log_path = Path(temporary) / "wrangler.log"
        with log_path.open("w+") as log:
            process = subprocess.Popen(
                [
                    wrangler,
                    "--config",
                    WRANGLER_CONFIG,
                    "dev",
                    "--persist-to",
                    str(state_dir),
                    "--ip",
                    "127.0.0.1",
                    "--port",
                    str(port),
                    "--log-level",
                    "error",
                    "--show-interactive-dev-session=false",
                ],
                cwd=worker_dir,
                stdout=log,
                stderr=subprocess.STDOUT,
                start_new_session=True,
            )
            try:
                wait_for_health(process, port, log_path)
                run_workflow(f"http://127.0.0.1:{port}")
            except Exception as error:
                log.flush()
                log.seek(0)
                raise RuntimeError(
                    f"local Worker test failed: {error}\nWrangler log:\n{log.read()}"
                ) from error
            finally:
                stop_process(process)

        expected_shutdown_codes = {
            0,
            -signal.SIGTERM,
            -signal.SIGINT,
            128 + signal.SIGTERM,
            128 + signal.SIGINT,
        }
        if process.returncode not in expected_shutdown_codes:
            raise RuntimeError(f"Wrangler exited with status {process.returncode}; see {log_path}")

    print("prs-cloudflare local end-to-end workflow passed")


def run_workflow(base_url: str) -> None:
    response = request(base_url, "GET", "/health")
    assert_status(response, 200, "Worker health")

    oversized_authorization = request(
        base_url,
        "POST",
        "/api/v1/authorization/reader",
        body=json.dumps(
            {"protocol_version": PROTOCOL_VERSION, "padding": "x" * 4096},
            separators=(",", ":"),
        ).encode(),
        headers={
            "Content-Type": "application/json",
            "Transfer-Encoding": "chunked",
        },
    )
    assert_api_error(
        oversized_authorization,
        413,
        "payload_too_large",
        "oversized authorization body without Content-Length",
    )

    sender_token, _ = verify_sender_flow(base_url)
    oversized_credentials = request(
        base_url,
        "DELETE",
        "/api/v1/sender/credentials",
        body=json.dumps(
            {"protocol_version": PROTOCOL_VERSION, "name": "ci-sender", "padding": "x" * 4096},
            separators=(",", ":"),
        ).encode(),
        headers={
            "Authorization": f"Bearer {sender_token}",
            "Content-Type": "application/json",
            "Transfer-Encoding": "chunked",
        },
    )
    assert_api_error(
        oversized_credentials,
        413,
        "payload_too_large",
        "oversized credential-management body without Content-Length",
    )
    reader_token = verify_reader_flow(base_url)
    bundle_a = make_bundle({"index.md": b"# First bundle\n"})
    boundary_bundle = make_boundary_bundle()
    bundle_b = make_bundle(
        {
            "index.md": b"# Replacement bundle\n",
            "chapter.md": b"## Second file\n",
        }
    )

    response = request(
        base_url,
        "PUT",
        "/api/v1/sender/bundle",
        body=bundle_a,
        headers={
            "Authorization": f"Bearer {sender_token}",
            "Content-Type": "application/x-tar",
        },
    )
    assert_status(response, 200, "push first bundle")
    if "manifest" in response.body.decode().lower():
        raise AssertionError("sender push response exposed manifest content")
    verify_manifest(base_url, reader_token, "index.md")

    response = request(
        base_url,
        "PUT",
        "/api/v1/sender/bundle",
        body=boundary_bundle,
        headers={
            "Authorization": f"Bearer {sender_token}",
            "Content-Type": "application/x-tar",
        },
    )
    assert_status(response, 200, "push boundary-size bundle")
    if response.json()["size_bytes"] != MAX_BUNDLE_SIZE:
        raise AssertionError("boundary-size bundle was not accepted at the protocol limit")

    response = request(
        base_url,
        "PUT",
        "/api/v1/sender/bundle",
        body=bundle_b,
        headers={
            "Authorization": f"Bearer {sender_token}",
            "Content-Type": "application/x-tar",
        },
    )
    assert_status(response, 200, "replace current bundle")
    _, replacement_etag = verify_manifest(base_url, reader_token, "index.md")
    downloaded = request(
        base_url,
        "GET",
        "/api/v1/reader/bundle",
        headers={"Authorization": f"Bearer {reader_token}"},
    )
    assert_status(downloaded, 200, "download current bundle")
    if downloaded.headers.get("content-type") != "application/x-tar":
        raise AssertionError("reader bundle did not use the tar content type")
    if downloaded.headers.get("etag") != f'"{replacement_etag}"':
        raise AssertionError("reader bundle ETag did not match the current manifest")
    if downloaded.body != bundle_b:
        raise AssertionError("reader downloaded bytes from the wrong bundle")
    with tarfile.open(fileobj=io.BytesIO(downloaded.body), mode="r:") as archive:
        if archive.getnames() != ["manifest.json", "index.md", "chapter.md"]:
            raise AssertionError("downloaded bundle did not retain its archive entries")

    verify_concurrent_mutation_results(base_url, sender_token)

    unauthorized = request(
        base_url,
        "PUT",
        "/api/v1/sender/bundle",
        body=b"x" * (MAX_BUNDLE_SIZE + 1),
        headers={
            "Authorization": "Bearer invalid-token",
            "Content-Type": "application/x-tar",
        },
    )
    assert_api_error(unauthorized, 401, "unauthorized", "unauthorized oversized upload")
    verify_manifest(base_url, reader_token, "index.md")

    oversized = request(
        base_url,
        "PUT",
        "/api/v1/sender/bundle",
        body=b"x" * (MAX_BUNDLE_SIZE + 1),
        headers={
            "Authorization": f"Bearer {sender_token}",
            "Content-Type": "application/x-tar",
            "Transfer-Encoding": "chunked",
        },
    )
    assert_api_error(oversized, 413, "payload_too_large", "oversized bundle")
    empty = request(
        base_url,
        "GET",
        "/api/v1/reader/manifest",
        headers={"Authorization": f"Bearer {reader_token}"},
    )
    assert_status(empty, 200, "read inbox after oversized replacement")
    if empty.json()["state"]["kind"] != "empty":
        raise AssertionError("oversized replacement left a readable bundle in the inbox")

    invalid = request(
        base_url,
        "PUT",
        "/api/v1/sender/bundle",
        body=b"not a valid PRSync bundle",
        headers={
            "Authorization": f"Bearer {sender_token}",
            "Content-Type": "application/x-tar",
        },
    )
    assert_api_error(invalid, 422, "invalid_request", "replace with invalid bundle")
    empty = request(
        base_url,
        "GET",
        "/api/v1/reader/manifest",
        headers={"Authorization": f"Bearer {reader_token}"},
    )
    assert_status(empty, 200, "read inbox after failed replacement")
    if empty.json()["state"]["kind"] != "empty":
        raise AssertionError("failed replacement left a readable bundle in the inbox")

    response = request(
        base_url,
        "PUT",
        "/api/v1/sender/bundle",
        body=bundle_b,
        headers={
            "Authorization": f"Bearer {sender_token}",
            "Content-Type": "application/x-tar",
        },
    )
    assert_status(response, 200, "republish bundle after failed replacement")
    verify_capability_separation(base_url, sender_token, reader_token)

    response = request(
        base_url,
        "DELETE",
        "/api/v1/sender/bundle",
        headers={"Authorization": f"Bearer {sender_token}"},
    )
    assert_status(response, 200, "clear inbox")
    if response.json()["state"] != "empty":
        raise AssertionError("clear did not return an empty state")
    empty = request(
        base_url,
        "GET",
        "/api/v1/reader/manifest",
        headers={"Authorization": f"Bearer {reader_token}"},
    )
    assert_status(empty, 200, "read manifest after clear")
    if empty.json()["state"]["kind"] != "empty":
        raise AssertionError("clear left a current manifest")
    response = request(
        base_url,
        "GET",
        "/api/v1/reader/bundle",
        headers={"Authorization": f"Bearer {reader_token}"},
    )
    assert_api_error(response, 404, "not_found", "download bundle after clear")

    verify_credentials(base_url, sender_token)


def free_port() -> int:
    with socket.socket() as socket_handle:
        socket_handle.bind(("127.0.0.1", 0))
        return socket_handle.getsockname()[1]


def wait_for_health(process: subprocess.Popen[bytes], port: int, log_path: Path) -> None:
    deadline = time.monotonic() + 180
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(f"Wrangler exited with status {process.returncode}; see {log_path}")
        try:
            response = request(f"http://127.0.0.1:{port}", "GET", "/health")
            if response.status == 200:
                return
        except (OSError, URLError):
            pass
        time.sleep(1)
    raise TimeoutError(f"Worker did not become healthy within 180 seconds; see {log_path}")


def stop_process(process: subprocess.Popen[bytes]) -> None:
    if process.poll() is None:
        os.killpg(process.pid, signal.SIGTERM)
        try:
            process.wait(timeout=20)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        message = str(error).replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")
        print(f"::error title=Local Worker end-to-end test::{message}", file=sys.stderr)
        raise
