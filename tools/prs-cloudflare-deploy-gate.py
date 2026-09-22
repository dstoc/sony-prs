#!/usr/bin/env python3
"""Decide whether a tested commit can affect the production Worker."""

from __future__ import annotations

import argparse
from collections import deque
from pathlib import Path
import subprocess
import sys
import tomllib


WORKER_MANIFEST = Path("crates/prs-cloudflare/Cargo.toml")
STATIC_RELEVANT_FILES = {
    ".github/workflows/ci.yml",
    ".github/workflows/prs-cloudflare-deploy.yml",
    "Cargo.lock",
    "Cargo.toml",
    "tools/prs-t1-agent-build.sh",
}
STATIC_RELEVANT_PREFIXES = (
    "docs/prs-cloudflare-",
    "tools/prs-cloudflare-",
    "tools/test-prs-cloudflare-",
)


def run_git(repo_root: Path, *arguments: str, check: bool = True) -> str:
    result = subprocess.run(
        ["git", *arguments],
        cwd=repo_root,
        check=False,
        capture_output=True,
        text=True,
    )
    if check and result.returncode != 0:
        raise RuntimeError(result.stderr.strip() or "git command failed")
    return result.stdout.strip()


def load_manifest(path: Path) -> dict:
    with path.open("rb") as manifest:
        return tomllib.load(manifest)


def dependency_specs(manifest: dict):
    for section_name in ("dependencies", "dev-dependencies", "build-dependencies"):
        section = manifest.get(section_name, {})
        if isinstance(section, dict):
            yield from section.items()

    targets = manifest.get("target", {})
    if isinstance(targets, dict):
        for target in targets.values():
            if not isinstance(target, dict):
                continue
            for section_name in (
                "dependencies",
                "dev-dependencies",
                "build-dependencies",
            ):
                section = target.get(section_name, {})
                if isinstance(section, dict):
                    yield from section.items()


def workspace_root(manifest_path: Path) -> Path | None:
    for directory in (manifest_path.parent, *manifest_path.parents):
        candidate = directory / "Cargo.toml"
        if not candidate.is_file():
            continue
        if isinstance(load_manifest(candidate).get("workspace"), dict):
            return directory
    return None


def workspace_dependency_spec(manifest_path: Path, name: str):
    root = workspace_root(manifest_path)
    if root is None:
        return None
    workspace = load_manifest(root / "Cargo.toml").get("workspace", {})
    dependencies = workspace.get("dependencies", {}) if isinstance(workspace, dict) else {}
    if not isinstance(dependencies, dict):
        return None
    return dependencies.get(name)


def path_dependency(manifest_path: Path, name: str, spec) -> Path | None:
    if not isinstance(spec, dict):
        return None
    if spec.get("workspace") is True:
        spec = workspace_dependency_spec(manifest_path, name)
        if not isinstance(spec, dict):
            return None
    dependency_path = spec.get("path")
    if not isinstance(dependency_path, str):
        return None
    return (manifest_path.parent / dependency_path).resolve()


def worker_dependency_roots(repo_root: Path) -> list[str]:
    """Return every local package directory reachable from prs-cloudflare."""

    initial_manifest = (repo_root / WORKER_MANIFEST).resolve()
    queue = deque([initial_manifest])
    visited: set[Path] = set()
    roots: set[str] = set()

    while queue:
        manifest_path = queue.popleft()
        if manifest_path in visited:
            continue
        visited.add(manifest_path)

        package_root = manifest_path.parent.resolve()
        try:
            relative_root = package_root.relative_to(repo_root.resolve())
        except ValueError as error:
            raise RuntimeError(
                f"local Worker dependency is outside the repository: {package_root}"
            ) from error
        roots.add(relative_root.as_posix())

        manifest = load_manifest(manifest_path)
        for name, spec in dependency_specs(manifest):
            dependency_manifest = path_dependency(manifest_path, name, spec)
            if dependency_manifest is None:
                continue
            if dependency_manifest.is_dir():
                dependency_manifest /= "Cargo.toml"
            if not dependency_manifest.is_file():
                raise RuntimeError(
                    f"local Worker dependency has no Cargo.toml: {dependency_manifest}"
                )
            queue.append(dependency_manifest)

    return sorted(roots)


def changed_paths(repo_root: Path, base_sha: str, head_sha: str) -> list[str]:
    output = run_git(
        repo_root,
        "diff",
        "--name-only",
        "--no-renames",
        "--diff-filter=ACDMRTUXB",
        base_sha,
        head_sha,
        "--",
    )
    return [path for path in output.splitlines() if path]


def is_relevant(path: str, dependency_roots: list[str]) -> bool:
    if path in STATIC_RELEVANT_FILES:
        return True
    if any(path.startswith(prefix) for prefix in STATIC_RELEVANT_PREFIXES):
        return True
    return any(path == root or path.startswith(f"{root}/") for root in dependency_roots)


def write_output(path: str | None, deploy: bool) -> None:
    if path is None:
        return
    with open(path, "a", encoding="utf-8") as output:
        output.write(f"deploy={'true' if deploy else 'false'}\n")


def write_summary(path: str | None, lines: list[str]) -> None:
    if path is None:
        return
    with open(path, "a", encoding="utf-8") as summary:
        summary.write("\n".join(lines))
        summary.write("\n")


def resolve_commit(repo_root: Path, reference: str) -> str:
    return run_git(repo_root, "rev-parse", "--verify", f"{reference}^{{commit}}")


def parser() -> argparse.ArgumentParser:
    command = argparse.ArgumentParser(description=__doc__)
    command.add_argument("--repo-root", type=Path, default=Path(__file__).resolve().parents[1])
    command.add_argument("--head", required=True, help="tested commit SHA")
    command.add_argument(
        "--base",
        help="the previous successful main CI SHA; omit only when no safe base is known",
    )
    command.add_argument("--github-output", help="append the deploy output for GitHub Actions")
    command.add_argument("--summary", help="append the decision to the Actions summary")
    return command


def main() -> int:
    arguments = parser().parse_args()
    repo_root = arguments.repo_root.resolve()
    head_sha = resolve_commit(repo_root, arguments.head)
    dependency_roots = worker_dependency_roots(repo_root)

    reliable_base = arguments.base is not None
    base_sha = None
    paths: list[str] = []
    reason = ""
    if arguments.base is not None:
        base_sha = resolve_commit(repo_root, arguments.base)
        if subprocess.run(
            ["git", "merge-base", "--is-ancestor", base_sha, head_sha],
            cwd=repo_root,
            check=False,
        ).returncode != 0:
            reliable_base = False
            reason = "the supplied base is not an ancestor of the tested commit"
        else:
            paths = changed_paths(repo_root, base_sha, head_sha)
    else:
        reason = "no previous successful main CI commit was available"

    relevant_paths = [path for path in paths if is_relevant(path, dependency_roots)]
    deploy = not reliable_base or bool(relevant_paths)

    print(f"tested_sha={head_sha}")
    print(f"base_sha={base_sha or 'unknown'}")
    print("changed_paths:")
    if paths:
        for path in paths:
            print(f"  {path}")
    else:
        print("  (unavailable)" if not reliable_base else "  (none)")
    if relevant_paths:
        print("deploy_relevant_paths:")
        for path in relevant_paths:
            print(f"  {path}")
    if reason:
        print(f"decision_reason={reason}")
    print(f"deploy={'true' if deploy else 'false'}")

    if deploy:
        if reliable_base:
            summary_message = (
                f"Cloudflare deploy-relevant changes found for `{head_sha}`; "
                "the protected production deployment will run."
            )
        else:
            summary_message = (
                f"Could not prove a safe base for `{head_sha}`; "
                "the protected production deployment will run conservatively."
            )
    else:
        summary_message = (
            f"No Cloudflare deploy-relevant changes for `{head_sha}`; "
            "production deployment skipped."
        )
    write_output(arguments.github_output, deploy)
    write_summary(
        arguments.summary,
        [
            "## Cloudflare deployment gate",
            f"- Tested SHA: `{head_sha}`",
            f"- Base SHA: `{base_sha or 'unknown'}`",
            f"- Decision: `deploy={'true' if deploy else 'false'}`",
            f"- {summary_message}",
            "- Changed paths:",
            *(f"  - `{path}`" for path in paths),
        ],
    )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, RuntimeError, tomllib.TOMLDecodeError) as error:
        print(f"deployment gate failed: {error}", file=sys.stderr)
        raise SystemExit(1)
