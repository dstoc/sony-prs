#!/usr/bin/env python3
"""Decide whether a tested commit can affect either production Worker."""

from __future__ import annotations

import argparse
from collections import deque
from pathlib import Path
import subprocess
import sys
import tomllib


WORKER_MANIFEST = Path("crates/prs-cloudflare/Cargo.toml")
READER_WEB_MANIFEST = Path("web/reader-web/Cargo.toml")
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
READER_WEB_RELEVANT_FILES = {
    ".github/workflows/ci.yml",
    ".github/workflows/prs-cloudflare-deploy.yml",
    "Cargo.lock",
    "Cargo.toml",
    "tools/prs-cloudflare-deploy-gate.py",
    "tools/reader-web-build.sh",
    "tools/test-reader-web.sh",
    "tools/prs-reader-web-deploy.sh",
    "tools/prs-reader-web-verify-production.sh",
    "web/reader-web/Cargo.toml",
    "web/reader-web/wrangler.toml",
    "web/reader-web/directory-library.js",
    "web/reader-web/fullscreen.mjs",
    "web/reader-web/index.html",
    "web/reader-web/input.mjs",
    "web/reader-web/main.js",
    "web/reader-web/prsync-client.mjs",
    "web/reader-web/style.css",
}
READER_WEB_RELEVANT_PREFIXES = ("web/reader-web/src/",)


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


def dependency_roots(repo_root: Path, manifest: Path, package_name: str) -> list[str]:
    """Return every local package directory reachable from a Cargo manifest."""

    initial_manifest = (repo_root / manifest).resolve()
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
                f"local {package_name} dependency is outside the repository: {package_root}"
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
                    f"local {package_name} dependency has no Cargo.toml: {dependency_manifest}"
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


def is_reader_web_relevant(path: str, dependency_roots: list[str]) -> bool:
    if path in READER_WEB_RELEVANT_FILES or any(
        path.startswith(prefix) for prefix in READER_WEB_RELEVANT_PREFIXES
    ):
        return True
    return any(
        root != READER_WEB_MANIFEST.parent.as_posix()
        and (path == root or path.startswith(f"{root}/"))
        for root in dependency_roots
    )


def write_output(path: str | None, deploy_api: bool, deploy_reader_web: bool) -> None:
    if path is None:
        return
    with open(path, "a", encoding="utf-8") as output:
        output.write(f"deploy_api={'true' if deploy_api else 'false'}\n")
        output.write(
            f"deploy_reader_web={'true' if deploy_reader_web else 'false'}\n"
        )
        output.write(f"deploy={'true' if deploy_api or deploy_reader_web else 'false'}\n")


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
    api_dependency_roots = dependency_roots(repo_root, WORKER_MANIFEST, "Worker")
    reader_web_dependency_roots = dependency_roots(
        repo_root, READER_WEB_MANIFEST, "browser reader"
    )

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

    api_relevant_paths = [
        path for path in paths if is_relevant(path, api_dependency_roots)
    ]
    reader_web_relevant_paths = [
        path for path in paths if is_reader_web_relevant(path, reader_web_dependency_roots)
    ]
    deploy_api = not reliable_base or bool(api_relevant_paths)
    deploy_reader_web = not reliable_base or bool(reader_web_relevant_paths)
    deploy = deploy_api or deploy_reader_web

    print(f"tested_sha={head_sha}")
    print(f"base_sha={base_sha or 'unknown'}")
    print("changed_paths:")
    if paths:
        for path in paths:
            print(f"  {path}")
    else:
        print("  (unavailable)" if not reliable_base else "  (none)")
    if api_relevant_paths:
        print("api_deploy_relevant_paths:")
        for path in api_relevant_paths:
            print(f"  {path}")
    if reader_web_relevant_paths:
        print("reader_web_deploy_relevant_paths:")
        for path in reader_web_relevant_paths:
            print(f"  {path}")
    if reason:
        print(f"decision_reason={reason}")
    print(f"deploy_api={'true' if deploy_api else 'false'}")
    print(f"deploy_reader_web={'true' if deploy_reader_web else 'false'}")
    print(f"deploy={'true' if deploy else 'false'}")

    if not reliable_base:
        summary_message = (
            f"Could not prove a safe base for `{head_sha}`; both protected "
            "production deployment paths will run conservatively."
        )
    else:
        summary_message = f"API Worker deploy: `{str(deploy_api).lower()}`; browser reader deploy: `{str(deploy_reader_web).lower()}`."
    write_output(arguments.github_output, deploy_api, deploy_reader_web)
    write_summary(
        arguments.summary,
        [
            "## Cloudflare deployment gate",
            f"- Tested SHA: `{head_sha}`",
            f"- Base SHA: `{base_sha or 'unknown'}`",
            f"- API Worker: `deploy={'true' if deploy_api else 'false'}`",
            f"- Browser reader: `deploy={'true' if deploy_reader_web else 'false'}`",
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
