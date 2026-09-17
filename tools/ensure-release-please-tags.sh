#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: tools/ensure-release-please-tags.sh [--push <remote>]

Find merged Release Please version applications for prs-markdown that are
reachable from HEAD, create any missing component boundary tags, and optionally
push those tags to a remote.
EOF
}

push_remote=''
while (($# > 0)); do
  case "$1" in
    --push)
      (($# >= 2)) || {
        printf '%s\n' '--push requires a remote name' >&2
        exit 2
      }
      push_remote=$2
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      printf 'unknown argument: %s\n' "$1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

repo_root=$(git rev-parse --show-toplevel)
cd "$repo_root"

readonly package_path='crates/prs-markdown'
readonly cargo_manifest="$package_path/Cargo.toml"
readonly release_manifest='.release-please-manifest.json'
readonly component='prs-markdown'

manifest_version_at() {
  local commit=$1
  git show "$commit:$release_manifest" 2>/dev/null |
    jq -er --arg path "$package_path" '.[$path] // empty'
}

cargo_version_at() {
  local commit=$1
  git show "$commit:$cargo_manifest" 2>/dev/null |
    awk '
      /^\[package\]$/ { in_package = 1; next }
      in_package && /^\[/ { exit }
      in_package && /^version = / {
        sub(/^version = "/, "")
        sub(/".*$/, "")
        print
        exit
      }
    '
}

created=0
checked=0
while IFS= read -r commit; do
  if ! parent=$(git rev-parse "$commit^" 2>/dev/null); then
    continue
  fi

  version=$(manifest_version_at "$commit" || true)
  parent_version=$(manifest_version_at "$parent" || true)
  cargo_version=$(cargo_version_at "$commit" || true)
  parent_cargo_version=$(cargo_version_at "$parent" || true)

  # A Release Please application changes both the package manifest and the
  # release manifest. This identifies the release commit itself rather than
  # tagging the latest unrelated source commit.
  [[ -n "$version" && "$version" == "$cargo_version" ]] || continue
  [[ -n "$parent_version" && "$version" != "$parent_version" ]] || continue
  [[ -n "$parent_cargo_version" && "$version" != "$parent_cargo_version" ]] || continue

  checked=$((checked + 1))
  tag="$component-v$version"
  if git show-ref --tags --verify --quiet "refs/tags/$tag"; then
    actual=$(git rev-list -n 1 "$tag")
    if [[ "$actual" != "$commit" ]]; then
      printf 'existing %s points to %s, expected %s\n' "$tag" "$actual" "$commit" >&2
      exit 1
    fi
    continue
  fi

  git tag "$tag" "$commit"
  if [[ -n "$push_remote" ]]; then
    git push "$push_remote" "refs/tags/$tag"
  fi
  printf 'created %s at %s\n' "$tag" "$commit"
  created=$((created + 1))
done < <(git log --reverse --format=%H -- "$release_manifest" "$cargo_manifest")

if ((created == 0)); then
  printf 'internal release boundaries verified (%d release commits checked)\n' "$checked"
fi
