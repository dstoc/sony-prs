#!/usr/bin/env python3
"""Check GitHub workflow boundaries for production Cloudflare credentials."""

from __future__ import annotations

from pathlib import Path
import re


REPO_ROOT = Path(__file__).resolve().parents[1]
WORKFLOW_DIRECTORY = REPO_ROOT / ".github" / "workflows"
CI_WORKFLOW = WORKFLOW_DIRECTORY / "ci.yml"
RUNBOOK = REPO_ROOT / "docs" / "prs-cloudflare-github-deployment.md"
REGRESSION_FIXTURE_DIRECTORY = (
    REPO_ROOT / "tools" / "testdata" / "prs-cloudflare-deployment-credentials"
)

CLOUDFLARE_SECRET_REFERENCE = re.compile(
    r"""
    \bsecrets\s*(?:
        \.\s*(?P<dot>CLOUDFLARE_ACCOUNT_ID|CLOUDFLARE_API_TOKEN)
        |
        \[\s*(?P<quote>[\"'])(?P<bracket>CLOUDFLARE_ACCOUNT_ID|CLOUDFLARE_API_TOKEN)
        (?P=quote)\s*\]
    )
    """,
    re.VERBOSE,
)
JOB_HEADER = re.compile(r"^  ([A-Za-z0-9_-]+):\s*$")
TOP_LEVEL_ON = re.compile(r"^(?:on|['\"]on['\"])\s*:")
PULL_REQUEST_TRIGGER = re.compile(
    r"(?<![A-Za-z0-9_-])(?:pull_request(?:_target)?|['\"]pull_request(?:_target)['\"])(?![A-Za-z0-9_-])"
)


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


def workflow_job_spans(source: str) -> dict[str, tuple[int, int]]:
    """Return the line span for each top-level job in the jobs mapping."""

    spans: dict[str, tuple[int, int]] = {}
    in_jobs = False
    current_job: str | None = None
    current_start: int | None = None

    for index, line in enumerate(source.splitlines()):
        if not in_jobs:
            if line == "jobs:":
                in_jobs = True
            continue

        match = JOB_HEADER.match(line)
        if match:
            if current_job is not None and current_start is not None:
                spans[current_job] = (current_start, index)
            current_job = match.group(1)
            current_start = index
            continue

        if line and not line[0].isspace():
            if current_job is not None and current_start is not None:
                spans[current_job] = (current_start, index)
            current_job = None
            current_start = None
            break

    if current_job is not None and current_start is not None:
        spans[current_job] = (current_start, len(source.splitlines()))

    return spans


def secret_name(match: re.Match[str]) -> str:
    return match.group("dot") or match.group("bracket")


def strip_yaml_comment(line: str) -> str:
    """Remove an unquoted YAML comment from one line."""

    quote: str | None = None
    escaped = False
    for index, character in enumerate(line):
        if quote is not None:
            if quote == '"' and escaped:
                escaped = False
            elif quote == '"' and character == "\\":
                escaped = True
            elif character == quote:
                quote = None
            continue
        if character in {'"', "'"}:
            quote = character
        elif character == "#" and (index == 0 or line[index - 1].isspace()):
            return line[:index]
    return line


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
    """Return whether the top-level on declaration includes a pull request event."""

    in_on = False
    for raw_line in source.splitlines():
        line = strip_yaml_comment(raw_line)
        if not in_on:
            if TOP_LEVEL_ON.match(line):
                in_on = True
                if PULL_REQUEST_TRIGGER.search(line.split(":", 1)[1]):
                    return True
            continue

        if line.strip() and not line[0].isspace():
            break
        if PULL_REQUEST_TRIGGER.search(line):
            return True

    return False


def job_for_line(
    job_spans: dict[str, tuple[int, int]], line_number: int
) -> str | None:
    for job_name, (start, end) in job_spans.items():
        if start <= line_number < end:
            return job_name
    return None


def assert_workflow_boundary(workflow_path: Path, source: str) -> None:
    jobs = workflow_jobs(source)
    job_spans = workflow_job_spans(source)
    secret_references = [
        (line_number, secret_name(match), job_for_line(job_spans, line_number))
        for line_number, line in enumerate(source.splitlines())
        for match in CLOUDFLARE_SECRET_REFERENCE.finditer(line)
    ]

    if workflow_has_pull_request_trigger(source):
        if secret_references:
            raise AssertionError(
                f"pull-request workflow receives a production Cloudflare secret: {workflow_path}"
            )
        if any(has_production_environment(lines) for lines in jobs.values()):
            raise AssertionError(
                f"pull-request workflow uses the production environment: {workflow_path}"
            )

    if not secret_references:
        return

    outside_job = [
        (line_number, name)
        for line_number, name, job_name in secret_references
        if job_name is None
    ]
    if outside_job:
        line_number, name = outside_job[0]
        raise AssertionError(
            f"Cloudflare secret {name} is outside a GitHub Actions job: "
            f"{workflow_path}:{line_number + 1}"
        )

    secret_jobs = {job_name for _, _, job_name in secret_references}
    if len(secret_jobs) != 1:
        raise AssertionError(
            "CLOUDFLARE_ACCOUNT_ID and CLOUDFLARE_API_TOKEN must be referenced by one protected job"
        )

    protected_job = next(iter(secret_jobs))
    if not has_production_environment(jobs[protected_job]):
        raise AssertionError(
            f"Cloudflare secret job lacks environment: production: "
            f"{workflow_path}:{protected_job}"
        )

    if not re.search(r"^\s{2}push:\s*$", source, re.MULTILINE):
        raise AssertionError(
            f"production Cloudflare secret workflow must run from a push: {workflow_path}"
        )
    if not re.search(r"^\s{6}- main\s*$", source, re.MULTILINE):
        raise AssertionError(
            f"production Cloudflare secret workflow must be limited to main: {workflow_path}"
        )

    referenced_secrets = {name for _, name, _ in secret_references}
    if referenced_secrets != {
        "CLOUDFLARE_ACCOUNT_ID",
        "CLOUDFLARE_API_TOKEN",
    }:
        raise AssertionError(
            "the protected deployment job must reference both required Cloudflare secrets"
        )


def assert_workflow_boundaries() -> None:
    workflow_files = sorted(WORKFLOW_DIRECTORY.glob("*.yml")) + sorted(
        WORKFLOW_DIRECTORY.glob("*.yaml")
    )
    if not workflow_files:
        raise AssertionError("no GitHub Actions workflow files found")

    for workflow_path in workflow_files:
        source = workflow_path.read_text(encoding="utf-8")
        assert_workflow_boundary(workflow_path, source)


def assert_regression_fixtures() -> None:
    expected_failures = {
        "workflow-level-secret-env.yml",
        "workflow-level-secret-env-after-jobs.yml",
        "pull-request-target.yml",
        "pull-request-inline-empty.yml",
    }
    fixture_paths = sorted(REGRESSION_FIXTURE_DIRECTORY.glob("*.yml"))
    if {path.name for path in fixture_paths} != expected_failures | {"bracket-secrets.yml"}:
        raise AssertionError("deployment credential regression fixtures are incomplete")

    for fixture_path in fixture_paths:
        source = fixture_path.read_text(encoding="utf-8")
        try:
            assert_workflow_boundary(fixture_path, source)
        except AssertionError:
            if fixture_path.name not in expected_failures:
                raise
        else:
            if fixture_path.name in expected_failures:
                raise AssertionError(
                    f"regression fixture unexpectedly passed: {fixture_path.name}"
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
    assert_regression_fixtures()
    assert_workflow_boundaries()
    assert_pull_request_ci_defaults()
    assert_runbook_contract()
    print("Cloudflare deployment credential boundary checks passed")


if __name__ == "__main__":
    main()
