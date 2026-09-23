#!/usr/bin/env python3
"""Check the static reader Worker configuration and production smoke probe."""

from __future__ import annotations

import os
from pathlib import Path
import subprocess
import tempfile
import tomllib


REPO_ROOT = Path(__file__).resolve().parents[1]
STATIC_CONFIG = REPO_ROOT / "web" / "reader-web" / "wrangler.toml"
API_CONFIG = REPO_ROOT / "crates" / "prs-cloudflare" / "wrangler.toml"
API_SOURCE = REPO_ROOT / "crates" / "prs-cloudflare" / "src" / "api.rs"
DEPLOY_SCRIPT = REPO_ROOT / "tools" / "prs-reader-web-deploy.sh"
VERIFY_SCRIPT = REPO_ROOT / "tools" / "prs-reader-web-verify-production.sh"
WORKFLOW = REPO_ROOT / ".github" / "workflows" / "prs-cloudflare-deploy.yml"
RUNBOOK = REPO_ROOT / "docs" / "prs-cloudflare-github-deployment.md"

FAKE_CURL = r'''#!/usr/bin/env python3
import os
from pathlib import Path
import sys
from urllib.parse import urlsplit

args = sys.argv[1:]
if "--location" in args or "-L" in args:
    raise SystemExit("the verifier must validate each redirect before making another request")
url = next(argument for argument in args if argument.startswith("https://"))
parsed_url = urlsplit(url)


def respond(status, content_type, headers=None):
    if "--dump-header" in args:
        header_path = Path(args[args.index("--dump-header") + 1])
        header_path.write_text(
            f"HTTP/2 {status}\r\n"
            + "".join(f"{header}\r\n" for header in (headers or []))
            + f"Content-Type: {content_type}\r\n\r\n",
            encoding="utf-8",
        )
    write_out = args[args.index("--write-out") + 1]
    if write_out == "%{http_code}":
        print(str(status), end="")
    else:
        print(f"{status}\n{content_type}\n{url}", end="")
    if status >= 400 and "--fail-with-body" in args:
        raise SystemExit(22)


if parsed_url.hostname == "prs-reader.dstoc.workers.dev":
    if "--request" in args and args[args.index("--request") + 1] == "OPTIONS":
        header_values = [
            args[index + 1].split(":", 1)[1].strip()
            for index, value in enumerate(args[:-1])
            if value == "--header" and args[index + 1].lower().startswith("origin:")
        ]
        origin = header_values[-1] if header_values else ""
        if origin == "https://prs-reader-web.dstoc.workers.dev" and not os.environ.get(
            "FAKE_REJECT_READER_WEB_ORIGIN"
        ):
            respond(
                204,
                "",
                [
                    "Access-Control-Allow-Origin: https://prs-reader-web.dstoc.workers.dev",
                    "Access-Control-Allow-Methods: POST",
                    "Access-Control-Allow-Headers: Content-Type",
                ],
            )
        else:
            respond(403, "text/plain")
        raise SystemExit(0)

    respond(403, "text/plain")
    raise SystemExit(0)

asset = parsed_url.path.lstrip("/")
if asset == "index.html" and os.environ.get("FAKE_INDEX_REDIRECT"):
    destinations = {
        "expected": "https://prs-reader-web.dstoc.workers.dev/",
        "relative_expected": "/",
        "cross_origin": "https://example.invalid/login",
        "access_login": "https://prs-reader-web.dstoc.workers.dev/cdn-cgi/access/login?token=do-not-leak",
        "unexpected_path": "https://prs-reader-web.dstoc.workers.dev/main.js",
    }
    respond(
        307,
        "text/html; charset=utf-8",
        [
            f"Location: {destinations[os.environ['FAKE_INDEX_REDIRECT']]}",
            "CF-Ray: fake-ray-176",
            "Set-Cookie: secret=must-not-be-logged",
        ],
    )
    raise SystemExit(0)

if asset == "" and os.environ.get("FAKE_CANONICAL_FINAL_STATUS"):
    respond(
        int(os.environ["FAKE_CANONICAL_FINAL_STATUS"]),
        os.environ.get("FAKE_CANONICAL_FINAL_CONTENT_TYPE", "text/html"),
        ["CF-Ray: fake-final-ray-176"],
    )
    raise SystemExit(0)

if os.environ.get("FAKE_MISSING_WASM") and asset.endswith("prs_reader_web_bg.wasm"):
    respond(404, "text/html")
    raise SystemExit(0)

asset_types = {
    "index.html": "text/html; charset=utf-8",
    "main.js": "text/javascript; charset=utf-8",
    "directory-library.js": "text/javascript; charset=utf-8",
    "input.mjs": "text/javascript; charset=utf-8",
    "fullscreen.mjs": "text/javascript; charset=utf-8",
    "prsync-client.mjs": "text/javascript; charset=utf-8",
    "style.css": "text/css; charset=utf-8",
    "pkg/prs_reader_web.js": "text/javascript; charset=utf-8",
    "pkg/prs_reader_web_bg.wasm": "application/wasm",
}
if asset == "":
    asset = "index.html"
if os.environ.get("FAKE_WRONG_WASM_MIME") and asset.endswith("prs_reader_web_bg.wasm"):
    respond(200, "application/wasm-invalid")
    raise SystemExit(0)
for suffix, content_type in asset_types.items():
    if asset.endswith(suffix):
        respond(200, content_type)
        raise SystemExit(0)
raise SystemExit(1)
'''


def toml(path: Path) -> dict:
    with path.open("rb") as config_file:
        return tomllib.load(config_file)


def assert_static_worker_config() -> None:
    config = toml(STATIC_CONFIG)
    assert config["name"] == "prs-reader-web"
    assert config["workers_dev"] is True
    assert config["compatibility_date"]
    assert "main" not in config
    assert not any(key in config for key in ("route", "routes", "custom_domains"))
    assets = config["assets"]
    assert (STATIC_CONFIG.parent / assets["directory"]).resolve() == (
        REPO_ROOT / "target" / "reader-web"
    ).resolve()
    assert assets["not_found_handling"] == "none"
    assert "binding" not in assets


def assert_api_origin_configuration() -> None:
    config = toml(API_CONFIG)
    production = config["env"]["production"]["vars"]
    local = config["env"]["local"]["vars"]
    assert production["PRS_READER_WEB_ORIGIN"] == (
        "https://prs-reader-web.dstoc.workers.dev"
    )
    assert local["PRS_READER_WEB_ORIGIN"] == "http://127.0.0.1:8000"

    source = API_SOURCE.read_text(encoding="utf-8")
    assert '.var("PRS_READER_WEB_ORIGIN")' in source
    assert '"/api/v1/authorization/reader"' in source
    assert '"/api/v1/authorization/poll"' in source
    assert '"/api/v1/reader/manifest"' in source
    assert '"/api/v1/reader/bundle"' in source
    assert "`/a/*`" in source


def assert_workflow_and_runbook() -> None:
    workflow = WORKFLOW.read_text(encoding="utf-8")
    required_workflow_text = (
        "workflow_run:",
        "ref: ${{ github.event.workflow_run.head_sha }}",
        "needs.change-gate.outputs.deploy_api",
        "needs.change-gate.outputs.deploy_reader_web",
        "environment:",
        "name: production",
        "secrets.CLOUDFLARE_ACCOUNT_ID",
        "secrets.CLOUDFLARE_API_TOKEN",
        "tools/reader-web-build.sh build",
        "tools/test-reader-web.sh target/reader-web",
        "tools/prs-reader-web-deploy.sh --production",
        "tools/prs-reader-web-verify-production.sh",
    )
    for expected in required_workflow_text:
        assert expected in workflow, f"missing deployment workflow contract: {expected}"

    deploy_job = workflow.split("  deploy:\n", 1)[1]
    job_environment = deploy_job.split("    steps:\n", 1)[0]
    assert "secrets.CLOUDFLARE_" not in job_environment, (
        "Cloudflare secrets must not be inherited by the post-deploy checks"
    )
    for step_name in (
        "Deploy browser reader to the existing prs-reader-web Worker",
        "Upload and promote production version, then verify schema readiness",
    ):
        step = workflow.split(f"      - name: {step_name}\n", 1)[1].split(
            "      - name:", 1
        )[0]
        assert "secrets.CLOUDFLARE_ACCOUNT_ID" in step
        assert "secrets.CLOUDFLARE_API_TOKEN" in step

    smoke_step = workflow.split(
        "      - name: Verify browser assets and exact-origin API CORS\n", 1
    )[1]
    assert "secrets.CLOUDFLARE_" not in smoke_step

    runbook = " ".join(RUNBOOK.read_text(encoding="utf-8").split())
    for expected in (
        "prs-reader-web",
        "PRS_READER_WEB_ORIGIN",
        "does not change the Worker runtime value",
        "wrangler rollback",
        "interactive browser session",
        "`/a/*` approval routes remain",
        "canonical 307",
        "CF-Ray",
        "bodies, cookies",
        "smoke tests execute without them",
    ):
        assert expected in runbook, f"runbook is missing: {expected}"


def assert_deploy_and_verify_scripts() -> None:
    deploy = DEPLOY_SCRIPT.read_text(encoding="utf-8")
    for expected in (
        "prs-cloudflare-wrangler-version.sh",
        "require_prs_wrangler",
        '"$repo_root/web/reader-web/wrangler.toml"',
        '"$repo_root/target/reader-web"',
        'wrangler deploy --config "$config"',
    ):
        assert expected in deploy, f"deploy script is missing: {expected}"

    verify = VERIFY_SCRIPT.read_text(encoding="utf-8")
    for expected in (
        "https://prs-reader-web.dstoc.workers.dev",
        "https://prs-reader.dstoc.workers.dev",
        "pkg/prs_reader_web_bg.wasm wasm",
        "application/wasm",
        "Access-Control-Allow-Origin",
        "https://example.invalid",
        "--disable",
        "--dump-header",
        "%{url_effective}",
        "following verified canonical redirect",
        "relevant response headers",
    ):
        assert expected in verify, f"production smoke probe is missing: {expected}"


def assert_production_smoke_probe() -> None:
    with tempfile.TemporaryDirectory(prefix="prs-reader-web-deployment-") as temp:
        root = Path(temp)
        fake_bin = root / "bin"
        fake_bin.mkdir()
        curl = fake_bin / "curl"
        curl.write_text(FAKE_CURL, encoding="utf-8")
        curl.chmod(0o755)

        environment = os.environ.copy()
        environment["PATH"] = f"{fake_bin}:{environment['PATH']}"
        result = subprocess.run(
            [str(VERIFY_SCRIPT)],
            cwd=REPO_ROOT,
            env=environment,
            capture_output=True,
            text=True,
            check=False,
        )
        assert result.returncode == 0, f"{result.stdout}\n{result.stderr}"
        assert "pkg/prs_reader_web_bg.wasm at https://prs-reader-web.dstoc.workers.dev/pkg/prs_reader_web_bg.wasm" in result.stdout
        assert "verified API reader authorization CORS" in result.stdout

        for destination in ("expected", "relative_expected"):
            environment["FAKE_INDEX_REDIRECT"] = destination
            canonical = subprocess.run(
                [str(VERIFY_SCRIPT)],
                cwd=REPO_ROOT,
                env=environment,
                capture_output=True,
                text=True,
                check=False,
            )
            assert canonical.returncode == 0, f"{canonical.stdout}\n{canonical.stderr}"
            assert "following verified canonical redirect" in canonical.stdout
            assert "verified index.html at https://prs-reader-web.dstoc.workers.dev/" in canonical.stdout

        for destination in ("cross_origin", "access_login", "unexpected_path"):
            environment["FAKE_INDEX_REDIRECT"] = destination
            rejected_redirect = subprocess.run(
                [str(VERIFY_SCRIPT)],
                cwd=REPO_ROOT,
                env=environment,
                capture_output=True,
                text=True,
                check=False,
            )
            assert rejected_redirect.returncode != 0, f"smoke probe accepted redirect: {destination}"
            assert "status: 307" in rejected_redirect.stderr
            assert "location:" in rejected_redirect.stderr
            assert "effective_url:" in rejected_redirect.stderr
            assert "content_type:" in rejected_redirect.stderr
            assert "CF-Ray: fake-ray-176" in rejected_redirect.stderr
            assert "must-not-be-logged" not in rejected_redirect.stderr
            assert "do-not-leak" not in rejected_redirect.stderr

        environment["FAKE_INDEX_REDIRECT"] = "expected"
        environment["FAKE_CANONICAL_FINAL_STATUS"] = "404"
        bad_final_status = subprocess.run(
            [str(VERIFY_SCRIPT)],
            cwd=REPO_ROOT,
            env=environment,
            capture_output=True,
            text=True,
            check=False,
        )
        assert bad_final_status.returncode != 0, "smoke probe accepted a non-200 canonical response"
        assert "status: 404" in bad_final_status.stderr

        environment.pop("FAKE_CANONICAL_FINAL_STATUS")
        environment.pop("FAKE_INDEX_REDIRECT")
        environment["FAKE_WRONG_WASM_MIME"] = "1"
        wrong_mime = subprocess.run(
            [str(VERIFY_SCRIPT)],
            cwd=REPO_ROOT,
            env=environment,
            capture_output=True,
            text=True,
            check=False,
        )
        assert wrong_mime.returncode != 0, "smoke probe accepted an incorrect WASM MIME type"
        assert "content_type: application/wasm-invalid" in wrong_mime.stderr

        environment.pop("FAKE_WRONG_WASM_MIME")
        environment["FAKE_REJECT_READER_WEB_ORIGIN"] = "1"
        rejected_origin = subprocess.run(
            [str(VERIFY_SCRIPT)],
            cwd=REPO_ROOT,
            env=environment,
            capture_output=True,
            text=True,
            check=False,
        )
        assert rejected_origin.returncode != 0, "smoke probe accepted a failed preflight"

        environment.pop("FAKE_REJECT_READER_WEB_ORIGIN")
        environment["FAKE_MISSING_WASM"] = "1"
        missing_asset = subprocess.run(
            [str(VERIFY_SCRIPT)],
            cwd=REPO_ROOT,
            env=environment,
            capture_output=True,
            text=True,
            check=False,
        )
        assert missing_asset.returncode != 0, "smoke probe accepted a missing WASM asset"


def main() -> None:
    assert_static_worker_config()
    assert_api_origin_configuration()
    assert_workflow_and_runbook()
    assert_deploy_and_verify_scripts()
    assert_production_smoke_probe()
    print("browser reader production deployment checks passed")


if __name__ == "__main__":
    main()
