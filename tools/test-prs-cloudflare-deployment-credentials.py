#!/usr/bin/env python3
"""Check GitHub workflow boundaries for production Cloudflare credentials."""

from __future__ import annotations

from pathlib import Path
import re


REPO_ROOT = Path(__file__).resolve().parents[1]
WORKFLOW_DIRECTORY = REPO_ROOT / ".github" / "workflows"
CI_WORKFLOW = WORKFLOW_DIRECTORY / "ci.yml"
RUNBOOK = REPO_ROOT / "docs" / "prs-cloudflare-github-deployment.md"

CLOUDFLARE_SECRET_REFERENCE = re.compile(
    r"secrets\.(CLOUDFLARE_ACCOUNT_ID|CLOUDFLARE_API_TOKEN)"
)
JOB_HEADER = re.compile(r"^  ([A-Za-z0-9_-]+):\s*$")


def workflow_jobs(source: str) -> dict[str, list[str]]:
    """Return the lines for each top-level job in the jobs mapping."""

    jobs: dict[str, list[str]] = {}
    in_jobs = False
    current_job: str | None = None

    for line in source.splitlines():
        if line == "jobs:":
            in_jobs = True
            continue
        if not in_jobs:
            continue

        match = JOB_HEADER.match(line)
        if match:
            current_job = match.group(1)
            jobs[current_job] = []
            continue

        if line and not line[0].isspace():
            in_jobs = False
            current_job = None
            continue

        if current_job is not None:
            jobs[current_job].append(line)

    return jobs


def has_production_environment(lines: list[str]) -> bool:
    for index, line in enumerate(lines):
        if re.match(r"^\s+environment:\s*['\"]?production['\"]?\s*$", line):
            return True
        if re.match(r"^\s+environment:\s*$", line):
            following = lines[index + 1] if index + 1 < len(lines) else ""
            if re.match(r"^\s+name:\s*['\"]?production['\"]?\s*$", following):
                return True
    return False


def workflow_has_pull_request_trigger(source: str) -> bool:
    return bool(re.search(r"^\s{2}pull_request:\s*$", source, re.MULTILINE))


def assert_workflow_boundaries() -> None:
    workflow_files = sorted(WORKFLOW_DIRECTORY.glob("*.yml")) + sorted(
        WORKFLOW_DIRECTORY.glob("*.yaml")
    )
    if not workflow_files:
        raise AssertionError("no GitHub Actions workflow files found")

    referenced_jobs: list[tuple[Path, str]] = []
    referenced_secrets: set[str] = set()
    for workflow_path in workflow_files:
        source = workflow_path.read_text(encoding="utf-8")
        jobs = workflow_jobs(source)
        secret_lines = [
            line for line in source.splitlines() if CLOUDFLARE_SECRET_REFERENCE.search(line)
        ]

        if workflow_has_pull_request_trigger(source):
            if secret_lines:
                raise AssertionError(
                    f"pull-request workflow receives a production Cloudflare secret: {workflow_path}"
                )
            if any(has_production_environment(lines) for lines in jobs.values()):
                raise AssertionError(
                    f"pull-request workflow uses the production environment: {workflow_path}"
                )

        secret_jobs = [
            job_name
            for job_name, lines in jobs.items()
            if any(CLOUDFLARE_SECRET_REFERENCE.search(line) for line in lines)
        ]

        if secret_lines and not secret_jobs:
            raise AssertionError(
                f"Cloudflare secret reference is outside a GitHub Actions job: {workflow_path}"
            )
        if secret_lines:
            if not re.search(r"^\s{2}push:\s*$", source, re.MULTILINE):
                raise AssertionError(
                    f"production Cloudflare secret workflow must run from a push: {workflow_path}"
                )
            if not re.search(r"^\s{6}- main\s*$", source, re.MULTILINE):
                raise AssertionError(
                    f"production Cloudflare secret workflow must be limited to main: {workflow_path}"
                )
            referenced_secrets.update(
                match.group(1)
                for match in CLOUDFLARE_SECRET_REFERENCE.finditer(source)
            )
        for job_name in secret_jobs:
            if not has_production_environment(jobs[job_name]):
                raise AssertionError(
                    f"Cloudflare secret job lacks environment: production: "
                    f"{workflow_path}:{job_name}"
                )
            referenced_jobs.append((workflow_path, job_name))

    if len(set(referenced_jobs)) > 1:
        raise AssertionError(
            "CLOUDFLARE_ACCOUNT_ID and CLOUDFLARE_API_TOKEN must be referenced by one protected job"
        )
    if referenced_secrets and referenced_secrets != {
        "CLOUDFLARE_ACCOUNT_ID",
        "CLOUDFLARE_API_TOKEN",
    }:
        raise AssertionError(
            "the protected deployment job must reference both required Cloudflare secrets"
        )


def assert_pull_request_ci_defaults() -> None:
    source = CI_WORKFLOW.read_text(encoding="utf-8")
    if "CLOUDFLARE_API_TOKEN: \"\"" not in source:
        raise AssertionError("pull-request CI must clear CLOUDFLARE_API_TOKEN")
    if "CLOUDFLARE_ACCOUNT_ID: \"\"" not in source:
        raise AssertionError("pull-request CI must clear CLOUDFLARE_ACCOUNT_ID")


def assert_runbook_contract() -> None:
    source = RUNBOOK.read_text(encoding="utf-8")
    required_phrases = (
        "Name: `production`",
        "Protected deployment branch: `main`",
        "Required reviewers",
        "CLOUDFLARE_ACCOUNT_ID",
        "CLOUDFLARE_API_TOKEN",
        "token name",
        "owner account",
        "permission categories",
        "resource restrictions",
        "creation date",
        "rotation owner",
        "D1 management",
        "R2 management",
        "pull-request jobs",
        "Revoke the old token",
        "never record the token value",
    )
    for phrase in required_phrases:
        if phrase not in source:
            raise AssertionError(f"deployment credential runbook is missing: {phrase}")


def main() -> None:
    assert_workflow_boundaries()
    assert_pull_request_ci_defaults()
    assert_runbook_contract()
    print("Cloudflare deployment credential boundary checks passed")


if __name__ == "__main__":
    main()
