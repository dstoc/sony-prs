#!/usr/bin/env python3
"""Test the restricted PRSync publishing command contract without Cloudflare."""

from __future__ import annotations

import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import tempfile
import tomllib


REPO_ROOT = Path(__file__).resolve().parents[1]
TOOLS = REPO_ROOT / "tools"
WORKER_SOURCE = REPO_ROOT / "crates" / "prs-cloudflare" / "src" / "api.rs"
SCHEMA_SOURCE = REPO_ROOT / "crates" / "prs-cloudflare" / "src" / "schema.rs"
RUNBOOK = REPO_ROOT / "docs" / "prs-cloudflare-restricted-publishing.md"
WRANGLER_CONFIG = REPO_ROOT / "crates" / "prs-cloudflare" / "wrangler.toml"
DEPLOYMENT_WORKFLOW = (
    REPO_ROOT / ".github" / "workflows" / "prs-cloudflare-deploy.yml"
)
DEPLOYMENT_GATE = TOOLS / "prs-cloudflare-deploy-gate.py"


FAKE_WRANGLER = r'''#!/usr/bin/env python3
import json
import os
from pathlib import Path
import sys

args = sys.argv[1:]
if args == ["--version"]:
    print("wrangler " + os.environ.get("FAKE_WRANGLER_VERSION", "4.135.0"))
    raise SystemExit(0)

log_path = Path(os.environ["WRANGLER_LOG"])
with log_path.open("a", encoding="utf-8") as log:
    log.write(json.dumps(args) + "\n")

if not os.environ.get("ALLOW_RESOURCE_COMMANDS") and any(
    argument in {"d1", "r2", "--x-provision"} for argument in args
):
    print("unexpected resource-management command", file=sys.stderr)
    raise SystemExit(97)

state_path = Path(os.environ.get("WRANGLER_STATE", str(log_path.with_suffix(".state"))))
if state_path.exists():
    state = json.loads(state_path.read_text(encoding="utf-8"))
else:
    state = {"uploads": 0, "promotions": 0}

if args[:2] == ["versions", "upload"]:
    state["uploads"] += 1
    state["last_version_id"] = (
        f"00000000-0000-4000-8000-{state['uploads']:012d}"
    )
    state_path.write_text(json.dumps(state), encoding="utf-8")
    print(f"Worker Version ID: {state['last_version_id']}")
elif args[:2] == ["versions", "deploy"]:
    state["promotions"] += 1
    state_path.write_text(json.dumps(state), encoding="utf-8")
    if os.environ.get("FAIL_FIRST_PROMOTION") and state["promotions"] == 1:
        print("simulated promotion failure", file=sys.stderr)
        raise SystemExit(19)
    version_id = args[args.index("--version-id") + 1]
    if version_id != state.get("last_version_id"):
        print("promotion used the wrong uploaded version", file=sys.stderr)
        raise SystemExit(98)
raise SystemExit(0)
'''

FAKE_CURL = r'''#!/usr/bin/env python3
import json
import os
from pathlib import Path
import sys

args = sys.argv[1:]
output = args[args.index("--output") + 1]
body = json.loads(os.environ["FAKE_READINESS_BODY"])
state_path = os.environ.get("WRANGLER_STATE") or str(
    Path(os.environ["WRANGLER_LOG"]).with_suffix(".state")
)
if state_path and Path(state_path).exists():
    state = json.loads(Path(state_path).read_text(encoding="utf-8"))
    body["release"] = {"version_id": state["last_version_id"]}
Path(output).write_text(json.dumps(body), encoding="utf-8")
print("200", end="")
'''


def write_executable(path: Path, body: str) -> None:
    path.write_text(body, encoding="utf-8")
    path.chmod(0o755)


def run_script(
    script: Path,
    arguments: list[str],
    fake_bin: Path,
    log_path: Path,
    *,
    allow_resource_commands: bool = False,
    extra_environment: dict[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    log_path.write_text("", encoding="utf-8")
    environment = os.environ.copy()
    environment.update(
        {
            "PATH": f"{fake_bin}:{environment['PATH']}",
            "TMPDIR": str(log_path.parent),
            "WRANGLER_LOG": str(log_path),
            "FAKE_READINESS_BODY": json.dumps(
                {
                    "status": "ready",
                    "schema_requirement": {
                        "migration_id": 7,
                        "migration_name": "0007_remove_owner_identity.sql",
                    },
                }
            ),
        }
    )
    if allow_resource_commands:
        environment["ALLOW_RESOURCE_COMMANDS"] = "1"
    else:
        environment.pop("ALLOW_RESOURCE_COMMANDS", None)
    if extra_environment:
        environment.update(extra_environment)
    return subprocess.run(
        [str(script), *arguments],
        cwd=REPO_ROOT,
        env=environment,
        capture_output=True,
        text=True,
        check=False,
    )


def logged_commands(log_path: Path) -> list[list[str]]:
    return [json.loads(line) for line in log_path.read_text().splitlines()]


def git_commit_sha() -> str:
    return subprocess.run(
        ["git", "rev-parse", "--verify", "HEAD"],
        cwd=REPO_ROOT,
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()


def command_lines(path: Path) -> list[list[str]]:
    return [
        shlex.split(line.strip().removesuffix("\\").rstrip())
        for line in path.read_text(encoding="utf-8").splitlines()
        if line.strip().startswith("wrangler ")
    ]


def rust_function_body(source: str, function_name: str) -> str:
    match = re.search(rf"(?:async )?fn {function_name}\b", source)
    if match is None:
        raise AssertionError(f"missing Rust function: {function_name}")
    opening = source.find("{", match.end())
    if opening < 0:
        raise AssertionError(f"missing function body: {function_name}")
    depth = 0
    for index in range(opening, len(source)):
        if source[index] == "{":
            depth += 1
        elif source[index] == "}":
            depth -= 1
            if depth == 0:
                return source[opening : index + 1]
    raise AssertionError(f"unterminated function body: {function_name}")


def assert_command_contracts() -> None:
    deploy = TOOLS / "prs-cloudflare-deploy.sh"
    migrate = TOOLS / "prs-cloudflare-migrate.sh"
    bootstrap = TOOLS / "prs-cloudflare-bootstrap.sh"

    assert command_lines(deploy) == [
        ["wrangler", "versions", "upload"],
        ["wrangler", "versions", "deploy"],
    ]
    assert command_lines(migrate) == [
        [
            "wrangler",
            "d1",
            "migrations",
            "list",
            "prs-reader-db",
            "--remote",
            "--env",
            "production",
        ],
        [
            "wrangler",
            "d1",
            "migrations",
            "apply",
            "prs-reader-db",
            "--remote",
            "--env",
            "production",
            "--no-x-provision",
        ],
        [
            "wrangler",
            "d1",
            "migrations",
            "list",
            "prs-reader-db",
            "--remote",
            "--env",
            "production",
        ],
    ]
    assert command_lines(bootstrap) == [
        ["wrangler", "d1", "create", "prs-reader-db"],
        ["wrangler", "r2", "bucket", "create", "prs-reader-documents"],
    ]

    deploy_source = deploy.read_text(encoding="utf-8")
    assert "CLOUDFLARE_API_TOKEN" not in deploy_source
    assert "wrangler d1" not in deploy_source
    assert "wrangler r2" not in deploy_source
    assert "wrangler deploy" not in deploy_source
    assert "wrangler triggers deploy" not in deploy_source
    assert "--remote" not in deploy_source
    assert "--x-provision" not in deploy_source.replace("--no-x-provision", "")
    assert "operator migration command" in deploy_source
    assert (
        '"$repo_root/tools/prs-cloudflare-readiness.sh" '
        '"$PRS_READER_URL" "$uploaded_version_id"'
    ) in deploy_source
    assert "prs-cloudflare-deploy.sh --production" not in migrate.read_text(encoding="utf-8")
    assert "prs-cloudflare-deploy.sh --production" not in bootstrap.read_text(encoding="utf-8")


def assert_production_topology() -> None:
    with WRANGLER_CONFIG.open("rb") as config_file:
        config = tomllib.load(config_file)

    production = config["env"]["production"]
    assert production["name"] == "prs-reader"
    assert production["workers_dev"] is True
    production_vars = production["vars"]
    assert production_vars["PRS_ENVIRONMENT"] == "production"
    assert production_vars["PRS_APPROVAL_BASE_URL"] == (
        "https://prs-reader.dstoc.workers.dev"
    )
    assert "reader.example.com" not in production_vars["PRS_APPROVAL_BASE_URL"]
    assert config["version_metadata"]["binding"] == "CF_VERSION_METADATA"
    for routing_key in ("route", "routes", "custom_domains"):
        assert routing_key not in production, (
            f"production topology must not configure {routing_key}"
        )
    assert config["triggers"]["crons"] == ["0 * * * *"]


def assert_fake_wrangler_boundaries() -> None:
    with tempfile.TemporaryDirectory(prefix="prs-cloudflare-publishing-") as directory:
        test_dir = Path(directory)
        fake_bin = test_dir / "bin"
        fake_bin.mkdir()
        write_executable(fake_bin / "wrangler", FAKE_WRANGLER)
        write_executable(fake_bin / "curl", FAKE_CURL)
        log_path = test_dir / "wrangler.log"

        deploy_result = run_script(
            TOOLS / "prs-cloudflare-deploy.sh",
            ["--production"],
            fake_bin,
            log_path,
            extra_environment={
                "PRS_READER_URL": "https://prs-reader.dstoc.workers.dev",
                "CLOUDFLARE_API_TOKEN": "test-only-placeholder",
            },
        )
        assert deploy_result.returncode == 0, deploy_result.stderr
        release_tag = git_commit_sha()
        assert logged_commands(log_path) == [
            [
                "versions",
                "upload",
                "--env",
                "production",
                "--tag",
                release_tag,
                "--no-x-provision",
            ],
            [
                "versions",
                "deploy",
                "--env",
                "production",
                "--version-id",
                "00000000-0000-4000-8000-000000000001",
                "--percentage",
                "100",
                "--yes",
                "--no-x-provision",
            ],
        ]

        retry_state = test_dir / "retry.state"
        first_attempt = run_script(
            TOOLS / "prs-cloudflare-deploy.sh",
            ["--production"],
            fake_bin,
            log_path,
            extra_environment={
                "FAIL_FIRST_PROMOTION": "1",
                "PRS_READER_URL": "https://prs-reader.dstoc.workers.dev",
                "WRANGLER_STATE": str(retry_state),
            },
        )
        assert first_attempt.returncode == 19, first_attempt.stderr
        assert logged_commands(log_path) == [
            [
                "versions",
                "upload",
                "--env",
                "production",
                "--tag",
                release_tag,
                "--no-x-provision",
            ],
            [
                "versions",
                "deploy",
                "--env",
                "production",
                "--version-id",
                "00000000-0000-4000-8000-000000000001",
                "--percentage",
                "100",
                "--yes",
                "--no-x-provision",
            ],
        ]

        second_attempt = run_script(
            TOOLS / "prs-cloudflare-deploy.sh",
            ["--production"],
            fake_bin,
            log_path,
            extra_environment={
                "FAIL_FIRST_PROMOTION": "1",
                "PRS_READER_URL": "https://prs-reader.dstoc.workers.dev",
                "WRANGLER_STATE": str(retry_state),
            },
        )
        assert second_attempt.returncode == 0, second_attempt.stderr
        assert logged_commands(log_path) == [
            [
                "versions",
                "upload",
                "--env",
                "production",
                "--tag",
                release_tag,
                "--no-x-provision",
            ],
            [
                "versions",
                "deploy",
                "--env",
                "production",
                "--version-id",
                "00000000-0000-4000-8000-000000000002",
                "--percentage",
                "100",
                "--yes",
                "--no-x-provision",
            ],
        ]

        missing_url = run_script(
            TOOLS / "prs-cloudflare-deploy.sh",
            ["--production"],
            fake_bin,
            log_path,
        )
        assert missing_url.returncode == 2
        assert logged_commands(log_path) == []
        assert "PRS_READER_URL is required" in missing_url.stderr

        wrong_wrangler = run_script(
            TOOLS / "prs-cloudflare-deploy.sh",
            ["--production"],
            fake_bin,
            log_path,
            extra_environment={
                "FAKE_WRANGLER_VERSION": "4.134.0",
                "PRS_READER_URL": "https://prs-reader.dstoc.workers.dev",
            },
        )
        assert wrong_wrangler.returncode != 0
        assert logged_commands(log_path) == []
        assert "Wrangler 4.135.0 is required" in wrong_wrangler.stderr

        migrate_result = run_script(
            TOOLS / "prs-cloudflare-migrate.sh",
            ["--production"],
            fake_bin,
            log_path,
            allow_resource_commands=True,
        )
        assert migrate_result.returncode == 0, migrate_result.stderr
        assert logged_commands(log_path) == [
            command[1:]
            for command in command_lines(TOOLS / "prs-cloudflare-migrate.sh")
        ]

        bootstrap_result = run_script(
            TOOLS / "prs-cloudflare-bootstrap.sh",
            ["--confirm-production"],
            fake_bin,
            log_path,
            allow_resource_commands=True,
        )
        assert bootstrap_result.returncode == 0, bootstrap_result.stderr
        assert logged_commands(log_path) == [
            command[1:]
            for command in command_lines(TOOLS / "prs-cloudflare-bootstrap.sh")
        ]


def assert_request_paths_are_migration_free() -> None:
    source = WORKER_SOURCE.read_text(encoding="utf-8")
    schema = SCHEMA_SOURCE.read_text(encoding="utf-8")
    for handler in (
        "root",
        "health",
        "create_sender",
        "create_reader",
        "poll_authorization",
        "authorization_status",
        "push_bundle",
        "clear_bundle",
        "list_credentials",
        "revoke_credentials",
        "revoke_named_credential",
        "reader_manifest",
        "reader_bundle",
    ):
        body = rust_function_body(source, handler)
        assert "schema::" not in body, f"{handler} can invoke schema readiness"
        assert "d1_migrations" not in body, f"{handler} can access migration history"
        assert "wrangler" not in body, f"{handler} can invoke Wrangler"

    readiness = rust_function_body(source, "readiness")
    assert "schema::inspect" in readiness
    assert not re.search(r"\.(?:execute|batch|run)\s*\(", readiness)
    inspect = rust_function_body(schema, "inspect")
    assert 'SELECT id, name FROM d1_migrations ORDER BY id ASC' in inspect
    assert not re.search(r"\.(?:execute|batch|run)\s*\(", inspect)
    assert not re.search(r"\b(?:ALTER|CREATE|DELETE|INSERT|UPDATE)\b", inspect)


def assert_runbook_is_reproducible() -> None:
    text = RUNBOOK.read_text(encoding="utf-8")
    for required in (
        "sony-prs/114",
        "wrangler@4.135.0",
        "CLOUDFLARE_API_TOKEN",
        "wrangler d1 migrations apply prs-reader-db --remote --env production --no-x-provision",
        "export PRS_READER_URL='https://prs-reader.dstoc.workers.dev'",
        "tools/prs-cloudflare-deploy.sh --production",
        "tools/prs-cloudflare-readiness.sh https://prs-reader.dstoc.workers.dev",
        "wrangler versions upload --env production --tag <commit-sha> --no-x-provision",
        "wrangler versions deploy --env production --version-id <version-id> --percentage 100 --yes --no-x-provision",
        "https://prs-reader.dstoc.workers.dev",
        "workers_dev = true",
        "Workers Routes Write",
        "wrangler triggers deploy",
        "does not change the existing Cron Trigger",
        "no D1 or R2\nmanagement permissions",
        "Workers Scripts",
        "binding",
        "Do not record token values",
        "code-only",
        "interrupted",
        "incompatible",
        "expected result",
    ):
        assert required in text, f"runbook is missing: {required}"


def assert_ci_invokes_contract_test() -> None:
    workflow = (REPO_ROOT / ".github" / "workflows" / "ci.yml").read_text(
        encoding="utf-8"
    )
    assert "python3 tools/test-prs-cloudflare-publishing.py" in workflow


def workflow_step(source: str, name: str) -> str:
    match = re.search(
        rf"(?ms)^      - name: {re.escape(name)}\n(.*?)(?=^      - name:|\Z)",
        source,
    )
    if match is None:
        raise AssertionError(f"deployment workflow is missing step: {name}")
    return match.group(0)


def assert_deployment_debug_contract() -> None:
    workflow = DEPLOYMENT_WORKFLOW.read_text(encoding="utf-8")
    debug_step = workflow_step(workflow, "Enable sanitized Wrangler debug logging")
    deploy_step = workflow_step(
        workflow,
        "Upload and promote production version, then verify schema readiness",
    )
    health_step = workflow_step(workflow, "Verify deployed Worker health")

    assert "if: ${{ runner.debug == '1' }}" in debug_step
    assert "echo 'WRANGLER_LOG=debug' >> \"$GITHUB_ENV\"" in debug_step
    assert "WRANGLER_LOG_SANITIZE: \"true\"" in deploy_step
    assert "WRANGLER_LOG_SANITIZE=false" not in workflow
    assert workflow.count("WRANGLER_LOG=debug") == 1
    assert "WRANGLER_LOG: debug" not in workflow
    assert workflow.index(debug_step) < workflow.index(deploy_step)
    assert workflow.index(deploy_step) < workflow.index(health_step)


def assert_deployment_workflow_contract() -> None:
    workflow = DEPLOYMENT_WORKFLOW.read_text(encoding="utf-8")
    required_phrases = (
        "workflow_run:",
        "actions: read",
        "- CI",
        "- completed",
        "github.event.workflow_run.conclusion == 'success'",
        "github.event.workflow_run.event == 'push'",
        "github.event.workflow_run.head_branch == 'main'",
        "github.event.workflow_run.head_repository.full_name == github.repository",
        "ref: ${{ github.event.workflow_run.head_sha }}",
        "fetch-depth: 0",
        "change-gate:",
        "python3 tools/prs-cloudflare-deploy-gate.py",
        "needs: change-gate",
        "needs.change-gate.outputs.deploy == 'true'",
        "gh api --paginate",
        "actions/workflows/ci.yml/runs?branch=main&status=completed",
        "environment:",
        "name: production",
        "CLOUDFLARE_ACCOUNT_ID: ${{ secrets.CLOUDFLARE_ACCOUNT_ID }}",
        "CLOUDFLARE_API_TOKEN: ${{ secrets.CLOUDFLARE_API_TOKEN }}",
        "PRS_READER_URL: ${{ vars.PRS_READER_URL }}",
        'WRANGLER_VERSION: "4.135.0"',
        "cargo +\"${{ steps.build-pins.outputs.rust_toolchain }}\" install worker-build --version \"$WORKER_BUILD_VERSION\" --locked",
        'npm install --global "wrangler@$WRANGLER_VERSION"',
        "worker-build --release",
        "tools/prs-cloudflare-deploy.sh --production",
        '"${PRS_READER_URL%/}/health"',
        "Upload and promote production version, then verify schema readiness",
    )
    for required in required_phrases:
        assert required in workflow, f"deployment workflow is missing: {required}"
    assert workflow.count("ref: ${{ github.event.workflow_run.head_sha }}") == 2

    gate = DEPLOYMENT_GATE.read_text(encoding="utf-8")
    assert "No Cloudflare deploy-relevant changes" in gate
    assert "deploy={'true' if deploy else 'false'}" in gate

    assert "pull_request" not in workflow
    assert "wrangler d1" not in workflow
    assert "wrangler r2" not in workflow
    assert "migrations apply" not in workflow
    assert "wrangler deploy" not in workflow
    assert "wrangler triggers deploy" not in workflow
    assert "--x-provision" not in workflow


def main() -> None:
    assert_command_contracts()
    assert_production_topology()
    assert_fake_wrangler_boundaries()
    assert_request_paths_are_migration_free()
    assert_runbook_is_reproducible()
    assert_ci_invokes_contract_test()
    assert_deployment_debug_contract()
    assert_deployment_workflow_contract()
    print("prs-cloudflare publishing contract checks passed")


if __name__ == "__main__":
    main()
