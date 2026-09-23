#!/usr/bin/env python3
"""Test Cloudflare deployment change detection with temporary Git histories."""

from __future__ import annotations

import os
from pathlib import Path
import subprocess
import tempfile


REPO_ROOT = Path(__file__).resolve().parents[1]
GATE = REPO_ROOT / "tools" / "prs-cloudflare-deploy-gate.py"


def run_git(root: Path, *arguments: str) -> str:
    result = subprocess.run(
        ["git", *arguments],
        cwd=root,
        check=True,
        capture_output=True,
        text=True,
    )
    return result.stdout.strip()


def write(root: Path, relative_path: str, contents: str = "changed\n") -> None:
    path = root / relative_path
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(contents, encoding="utf-8")


def fixture() -> tuple[tempfile.TemporaryDirectory[str], Path, str]:
    temporary = tempfile.TemporaryDirectory(prefix="prs-cloudflare-gate-")
    root = Path(temporary.name)
    run_git(root, "init", "--quiet", "--initial-branch", "main")
    run_git(root, "config", "user.email", "test@example.invalid")
    run_git(root, "config", "user.name", "Cloudflare gate test")

    write(root, "Cargo.toml", "[workspace]\nmembers = [\"crates/prs-cloudflare\", \"crates/prs-sync-bundle\", \"crates/prs-sync-protocol\"]\n")
    write(root, "Cargo.lock", "# fixture lockfile\n")
    write(
        root,
        "crates/prs-cloudflare/Cargo.toml",
        """[package]
name = "prs-cloudflare"
version = "0.1.0"

[dependencies]
prs-sync-bundle = { path = "../prs-sync-bundle" }
""",
    )
    write(
        root,
        "crates/prs-sync-bundle/Cargo.toml",
        """[package]
name = "prs-sync-bundle"
version = "0.1.0"

[dependencies]
prs-sync-protocol = { path = "../prs-sync-protocol" }
""",
    )
    write(
        root,
        "crates/prs-sync-protocol/Cargo.toml",
        """[package]
name = "prs-sync-protocol"
version = "0.1.0"
""",
    )
    write(
        root,
        "crates/prs-markdown/Cargo.toml",
        """[package]
name = "prs-markdown"
version = "0.1.0"
""",
    )
    write(
        root,
        "web/reader-web/Cargo.toml",
        """[package]
name = "prs-reader-web"
version = "0.1.0"

[dependencies]
prs-markdown = { path = "../../crates/prs-markdown" }
prs-sync-bundle = { path = "../../crates/prs-sync-bundle" }
""",
    )
    for relative_path in (
        ".github/workflows/ci.yml",
        ".github/workflows/prs-cloudflare-deploy.yml",
        "tools/prs-t1-agent-build.sh",
        "tools/prs-cloudflare-deploy.sh",
        "tools/test-prs-cloudflare-config.sh",
        "docs/prs-cloudflare-github-deployment.md",
        "crates/prs-cloudflare/src/lib.rs",
        "crates/prs-cloudflare/migrations/0001_initial.sql",
        "crates/prs-cloudflare/wrangler.toml",
        "crates/prs-sync-bundle/src/lib.rs",
        "crates/prs-sync-protocol/src/lib.rs",
        "crates/prs-t1-agent/src/lib.rs",
        "web/reader-web/src/lib.rs",
    ):
        write(root, relative_path)
    run_git(root, "add", ".")
    run_git(root, "commit", "--quiet", "-m", "fixture")
    return temporary, root, run_git(root, "rev-parse", "HEAD")


def gate(root: Path, base: str | None, head: str) -> tuple[dict[str, bool], str]:
    arguments = [
        "python3",
        str(GATE),
        "--repo-root",
        str(root),
        "--head",
        head,
    ]
    if base is not None:
        arguments.extend(("--base", base))
    result = subprocess.run(
        arguments,
        cwd=REPO_ROOT,
        check=True,
        capture_output=True,
        text=True,
        env=os.environ.copy(),
    )
    decisions = {
        key: value == "true"
        for line in result.stdout.splitlines()
        if "=" in line
        for key, value in [line.split("=", 1)]
        if key in {"deploy_api", "deploy_reader_web", "deploy"}
    }
    return decisions, result.stdout


def single_change(
    relative_path: str, expected_api: bool, expected_reader_web: bool
) -> None:
    temporary, root, base = fixture()
    try:
        write(root, relative_path, "changed again\n")
        run_git(root, "add", ".")
        run_git(root, "commit", "--quiet", "-m", relative_path)
        actual, output = gate(root, base, run_git(root, "rev-parse", "HEAD"))
        assert actual["deploy_api"] is expected_api, output
        assert actual["deploy_reader_web"] is expected_reader_web, output
        assert actual["deploy"] is (expected_api or expected_reader_web), output
        assert f"  {relative_path}" in output, output
    finally:
        temporary.cleanup()


def test_cases() -> None:
    single_change("crates/prs-t1-agent/src/lib.rs", False, False)
    single_change("web/reader-web/src/lib.rs", False, True)
    single_change("web/reader-web/wrangler.toml", False, True)
    single_change("web/reader-web/README.md", False, False)
    single_change("web/reader-web/demo/assets/detail.png", False, False)
    single_change("tools/reader-web-build.sh", False, True)
    single_change("crates/prs-markdown/src/lib.rs", False, True)
    single_change("crates/prs-cloudflare/src/lib.rs", True, False)
    single_change("crates/prs-cloudflare/migrations/0002_new.sql", True, False)
    single_change("crates/prs-cloudflare/wrangler.toml", True, False)
    single_change("tools/prs-cloudflare-deploy.sh", True, False)
    single_change("crates/prs-sync-bundle/src/lib.rs", True, True)
    single_change("crates/prs-sync-protocol/src/lib.rs", True, True)
    single_change("Cargo.toml", True, True)
    single_change("Cargo.lock", True, True)


def test_multi_commit_range() -> None:
    temporary, root, base = fixture()
    try:
        write(root, "crates/prs-t1-agent/src/lib.rs", "first change\n")
        run_git(root, "add", ".")
        run_git(root, "commit", "--quiet", "-m", "unrelated first commit")
        write(root, "crates/prs-cloudflare/src/lib.rs", "second change\n")
        run_git(root, "add", ".")
        run_git(root, "commit", "--quiet", "-m", "Worker second commit")
        actual, output = gate(root, base, run_git(root, "rev-parse", "HEAD"))
        assert actual["deploy_api"] and not actual["deploy_reader_web"], output
        assert "  crates/prs-t1-agent/src/lib.rs" in output, output
        assert "  crates/prs-cloudflare/src/lib.rs" in output, output
    finally:
        temporary.cleanup()


def test_merge_commit_range() -> None:
    temporary, root, base = fixture()
    try:
        run_git(root, "switch", "--quiet", "-c", "feature")
        write(root, "web/reader-web/src/lib.rs", "browser change\n")
        run_git(root, "add", ".")
        run_git(root, "commit", "--quiet", "-m", "browser change")
        run_git(root, "switch", "--quiet", "main")
        write(root, "crates/prs-cloudflare/src/lib.rs", "Worker change\n")
        run_git(root, "add", ".")
        run_git(root, "commit", "--quiet", "-m", "Worker change")
        run_git(root, "merge", "--quiet", "--no-ff", "feature", "-m", "merge feature")
        actual, output = gate(root, base, run_git(root, "rev-parse", "HEAD"))
        assert actual["deploy_api"] and actual["deploy_reader_web"], output
        assert "  web/reader-web/src/lib.rs" in output, output
        assert "  crates/prs-cloudflare/src/lib.rs" in output, output
    finally:
        temporary.cleanup()


def test_unknown_base_deploys_both() -> None:
    temporary, root, _base = fixture()
    try:
        actual, output = gate(root, None, run_git(root, "rev-parse", "HEAD"))
        assert actual["deploy_api"] and actual["deploy_reader_web"], output
        assert "no previous successful main CI commit" in output, output
    finally:
        temporary.cleanup()


def main() -> None:
    test_cases()
    test_multi_commit_range()
    test_merge_commit_range()
    test_unknown_base_deploys_both()
    print("prs-cloudflare deployment gate checks passed")


if __name__ == "__main__":
    main()
