#!/usr/bin/env bash
set -euo pipefail

workflow="${1:-.github/workflows/release-please.yml}"
manifest="${2:-crates/prs-t1-agent/Cargo.toml}"
build_script="${3:-tools/prs-t1-agent-build.sh}"
release_config="${4:-release-please-config.json}"
release_manifest="${5:-.release-please-manifest.json}"
boundary_script="${6:-tools/ensure-release-please-tags.sh}"

grep -Fq 'workflow_dispatch:' "$workflow"
grep -Fq 'description: Existing prs-t1-agent release tag to build/upload' "$workflow"
grep -Fq 'required: true' "$workflow"
grep -Fq 'type: string' "$workflow"
grep -Fq 'rust-version = "1.98"' "$manifest"
grep -Fq "if: \${{ github.event_name == 'push' }}" "$workflow"
grep -Fq 'fetch-depth: 0' "$workflow"
grep -Fq 'Ensure internal prs-markdown release boundaries' "$workflow"
grep -Fq 'tools/ensure-release-please-tags.sh --push origin' "$workflow"
grep -Fq 'id: release-tag' "$workflow"
grep -Fq 'MANUAL_TAG: ${{ inputs.tag }}' "$workflow"
grep -Fq 'RELEASE_TAG: ${{ steps.release.outputs['"'"'crates/prs-t1-agent--tag_name'"'"'] }}' "$workflow"
grep -Fq 'tag=%s' "$workflow"
grep -Fq 'ref: ${{ steps.release-tag.outputs.tag }}' "$workflow"
grep -Fq '"${{ steps.release-tag.outputs.tag }}" \' "$workflow"
grep -Fq 'tools/prs-t1-agent-build.sh print-github-actions' "$workflow"
grep -Fq 'cp tools/prs-t1-agent-build.sh "$RUNNER_TEMP/prs-t1-agent-build.sh"' "$workflow"
grep -Fq '"$RUNNER_TEMP/prs-t1-agent-build.sh" build-and-verify' "$workflow"
if ! awk '
  /name: Build and verify the released PRS-T1 binary/ { in_step = 1; next }
  in_step && /- name:/ { in_step = 0 }
  in_step && /run: \|/ { found = 1 }
  END { exit !found }
' "$workflow"; then
  printf '%s\n' 'released binary build must use a block scalar run command' >&2
  exit 1
fi
grep -Fq 'version: "${{ steps.build-pins.outputs.zig_version }}"' "$workflow"
grep -Fq 'uses: Swatinem/rust-cache@63fed3e2fecf6f7b51dc6f043341b79ef82a9ae7 # v2.9.2' "$workflow"
grep -Fq 'shared-key: prs-t1-armv5te' "$workflow"
grep -Fq 'add-job-id-key: false' "$workflow"
grep -Fq 'cache-targets: true' "$workflow"
grep -Fq 'cache-all-crates: true' "$workflow"
grep -Fq 'cache-bin: true' "$workflow"
grep -Fq "manifests-\${{ hashFiles('**/Cargo.toml', '**/Cargo.lock') }}" "$workflow"
grep -Fq 'build-${{ steps.build-pins.outputs.build_config_hash }}' "$workflow"
grep -Fq 'if [[ ! -x "$cargo_zigbuild_bin" ]]' "$workflow"
grep -Fq 'build_config_hash=' "$build_script"
grep -Fq 'readonly RUST_TOOLCHAIN="1.98.1"' "$build_script"
grep -Fq 'readonly ZIG_VERSION="0.16.0"' "$build_script"
grep -Fq 'readonly CARGO_ZIGBUILD_VERSION="0.23.4"' "$build_script"
grep -Fq 'readonly TARGET="armv5te-unknown-linux-musleabi"' "$build_script"
grep -Fq 'cargo +"$RUST_TOOLCHAIN" zigbuild' "$build_script"
grep -Fq 'readelf -h "$artifact"' "$build_script"
grep -Fq "grep -q 'INTERP'" "$build_script"
grep -Fq 'Tag_ABI_VFP_args: VFP registers' "$build_script"

if grep -Fq 'cargo +"$RUST_TOOLCHAIN" zigbuild' "$workflow"; then
  printf '%s\n' 'cross-build command is duplicated in the release workflow' >&2
  exit 1
fi

if grep -Fq 'readelf -h "$artifact"' "$workflow"; then
  printf '%s\n' 'ELF validation is duplicated in the release workflow' >&2
  exit 1
fi

test "$("$build_script" print rust-toolchain)" = '1.98.1'
test "$("$build_script" print zig-version)" = '0.16.0'
test "$("$build_script" print cargo-zigbuild-version)" = '0.23.4'
test "$("$build_script" print target)" = 'armv5te-unknown-linux-musleabi'
test "$("$build_script" print-github-actions | sed -n '/^build_config_hash=/p')" = \
  "build_config_hash=$(sha256sum "$build_script" | cut -d ' ' -f1)"

jq -e '
  any(.plugins[]?; .type == "cargo-workspace")
  and (.packages["crates/prs-t1-agent"]["release-type"] == "rust")
  and (.packages["crates/prs-t1-agent"].component == "prs-t1-agent")
  and (.packages["crates/prs-t1-agent"]["include-component-in-tag"] == true)
  and (.packages["crates/prs-markdown"]["release-type"] == "rust")
  and (.packages["crates/prs-markdown"].component == "prs-markdown")
  and (.packages["crates/prs-markdown"]["include-component-in-tag"] == true)
  and (.packages["crates/prs-markdown"]["skip-github-release"] == true)
  and (.packages["crates/prs-markdown"]["skip-changelog"] == true)
' "$release_config" >/dev/null

agent_version="$(cargo metadata --no-deps --format-version 1 |
  jq -r '.packages[] | select(.name == "prs-t1-agent") | .version')"
markdown_version="$(cargo metadata --no-deps --format-version 1 |
  jq -r '.packages[] | select(.name == "prs-markdown") | .version')"
test "$(jq -r '."crates/prs-t1-agent"' "$release_manifest")" = "$agent_version"
test "$(jq -r '."crates/prs-markdown"' "$release_manifest")" = "$markdown_version"
grep -Fq 'prs-markdown = { path = "../prs-markdown" }' "$manifest"
test -x "$boundary_script"

boundary_step_line="$(grep -nF 'Ensure internal prs-markdown release boundaries' "$workflow" | cut -d: -f1)"
release_step_line="$(grep -nF 'name: Run Release Please' "$workflow" | cut -d: -f1)"
test "$boundary_step_line" -lt "$release_step_line"

# Keep a local markdown-only Conventional Commit fixture here so changes to
# this regression check cannot accidentally stop exercising path collection.
fixture_repo="$(mktemp -d)"
trap 'rm -rf "$fixture_repo"' EXIT
git -C "$fixture_repo" init -q
git -C "$fixture_repo" config user.email release-test@example.invalid
git -C "$fixture_repo" config user.name release-test
mkdir -p "$fixture_repo/crates/prs-markdown/src" "$fixture_repo/crates/scsi-transport/src"
printf '%s\n' 'initial markdown source' > "$fixture_repo/crates/prs-markdown/src/lib.rs"
printf '%s\n' 'initial unrelated source' > "$fixture_repo/crates/scsi-transport/src/lib.rs"
git -C "$fixture_repo" add .
git -C "$fixture_repo" commit -q -m 'chore: seed release scope fixture'
printf '%s\n' 'markdown-only release candidate' > "$fixture_repo/crates/prs-markdown/src/lib.rs"
git -C "$fixture_repo" add crates/prs-markdown/src/lib.rs
git -C "$fixture_repo" commit -q -m 'fix(markdown): exercise workspace release scope'
markdown_commit_paths="$(git -C "$fixture_repo" diff-tree --no-commit-id --name-only -r HEAD)"
test "$markdown_commit_paths" = 'crates/prs-markdown/src/lib.rs'
printf '%s\n' 'unrelated workspace change' > "$fixture_repo/crates/scsi-transport/src/lib.rs"
git -C "$fixture_repo" add crates/scsi-transport/src/lib.rs
git -C "$fixture_repo" commit -q -m 'fix(scsi): remain outside application release scope'
unrelated_commit_paths="$(git -C "$fixture_repo" diff-tree --no-commit-id --name-only -r HEAD)"
test "$unrelated_commit_paths" = 'crates/scsi-transport/src/lib.rs'
if jq -e --arg path "$unrelated_commit_paths" '
  .packages
  | keys
  | any(.[]; . as $package |
      ($path == $package or ($path | startswith($package + "/"))))
' "$release_config" >/dev/null; then
  printf '%s\n' 'unrelated workspace paths must not be configured release package roots' >&2
  exit 1
fi

# Exercise the release boundary itself. This is deliberately a local git
# fixture so it can run without GitHub credentials or an npm download while
# still modeling the Release Please sequence: a tagged released version, a
# conventional source commit, the applied release commit, and the next
# calculation from the new component tag.
release_fixture="$(mktemp -d)"
trap 'rm -rf "$fixture_repo" "$release_fixture"' EXIT
git -C "$release_fixture" init -q
git -C "$release_fixture" config user.email release-test@example.invalid
git -C "$release_fixture" config user.name release-test
mkdir -p "$release_fixture/crates/prs-markdown/src" \
  "$release_fixture/crates/prs-t1-agent/src" "$release_fixture/tools"
cp "$boundary_script" "$release_fixture/tools/ensure-release-please-tags.sh"
chmod +x "$release_fixture/tools/ensure-release-please-tags.sh"
printf '%s\n' \
  '{' \
  '  "crates/prs-t1-agent": "0.1.0",' \
  '  "crates/prs-markdown": "0.1.0"' \
  '}' > "$release_fixture/.release-please-manifest.json"
printf '%s\n' \
  '[package]' \
  'name = "prs-markdown"' \
  'version = "0.1.0"' > "$release_fixture/crates/prs-markdown/Cargo.toml"
printf '%s\n' \
  '[package]' \
  'name = "prs-t1-agent"' \
  'version = "0.1.0"' \
  '' \
  '[dependencies]' \
  'prs-markdown = { path = "../prs-markdown" }' > \
  "$release_fixture/crates/prs-t1-agent/Cargo.toml"
printf '%s\n' '# Changelog' > "$release_fixture/crates/prs-markdown/CHANGELOG.md"
printf '%s\n' '# Changelog' > "$release_fixture/crates/prs-t1-agent/CHANGELOG.md"
printf '%s\n' 'initial markdown source' > "$release_fixture/crates/prs-markdown/src/lib.rs"
printf '%s\n' 'initial agent source' > "$release_fixture/crates/prs-t1-agent/src/lib.rs"
git -C "$release_fixture" add .
git -C "$release_fixture" commit -q -m 'chore: seed release fixture'
git -C "$release_fixture" tag prs-markdown-v0.1.0

fixture_candidate_count() {
  local tag
  tag="$(git -C "$release_fixture" tag --list 'prs-markdown-v*' --sort=-version:refname | head -1)"
  git -C "$release_fixture" log --format='%s' "$tag..HEAD" -- crates/prs-markdown |
    awk '/^(feat|fix|perf)(\([^)]*\))?!?:/ { count += 1 } END { print count + 0 }'
}

fixture_agent_direct_candidate_count() {
  local tag
  tag="$(git -C "$release_fixture" tag --list 'prs-markdown-v*' --sort=-version:refname | head -1)"
  git -C "$release_fixture" log --format='%s' "$tag..HEAD" -- crates/prs-t1-agent |
    awk '/^(feat|fix|perf)(\([^)]*\))?!?:/ { count += 1 } END { print count + 0 }'
}

fixture_application_candidate_count() {
  if [[ "$(fixture_candidate_count)" -gt 0 && "$(fixture_agent_direct_candidate_count)" = 0 ]]; then
    printf '%s\n' 1
  else
    printf '%s\n' 0
  fi
}

apply_fixture_release() {
  local markdown_version=$1
  local agent_version=$2
  sed -i -E "s/^version = \"[0-9]+\.[0-9]+\.[0-9]+\"$/version = \"$markdown_version\"/" \
    "$release_fixture/crates/prs-markdown/Cargo.toml"
  sed -i -E "s/^(  \"crates\/prs-markdown\": )\"[0-9]+\.[0-9]+\.[0-9]+\"/\1\"$markdown_version\"/" \
    "$release_fixture/.release-please-manifest.json"
  sed -i -E "s/^version = \"[0-9]+\.[0-9]+\.[0-9]+\"$/version = \"$agent_version\"/" \
    "$release_fixture/crates/prs-t1-agent/Cargo.toml"
  sed -i -E "s/^(  \"crates\/prs-t1-agent\": )\"[0-9]+\.[0-9]+\.[0-9]+\"/\1\"$agent_version\"/" \
    "$release_fixture/.release-please-manifest.json"
  git -C "$release_fixture" add .
  git -C "$release_fixture" commit -q -m 'chore: release main'
  (cd "$release_fixture" && tools/ensure-release-please-tags.sh)
}

printf '%s\n' 'first markdown fix' > "$release_fixture/crates/prs-markdown/src/lib.rs"
git -C "$release_fixture" add crates/prs-markdown/src/lib.rs
git -C "$release_fixture" commit -q -m 'fix(markdown): first released fix'
test "$(fixture_candidate_count)" = 1
test "$(fixture_application_candidate_count)" = 1
apply_fixture_release 0.1.1 0.1.1
test "$(git -C "$release_fixture" show -s --format='%H' prs-markdown-v0.1.1)" = \
  "$(git -C "$release_fixture" rev-parse HEAD)"
test "$(fixture_candidate_count)" = 0
test "$(fixture_agent_direct_candidate_count)" = 0
test "$(fixture_application_candidate_count)" = 0

printf '%s\n' 'second markdown fix' > "$release_fixture/crates/prs-markdown/src/lib.rs"
git -C "$release_fixture" add crates/prs-markdown/src/lib.rs
git -C "$release_fixture" commit -q -m 'fix(markdown): second released fix'
test "$(fixture_candidate_count)" = 1
test "$(fixture_agent_direct_candidate_count)" = 0
test "$(fixture_application_candidate_count)" = 1
apply_fixture_release 0.1.2 0.1.2
test "$(git -C "$release_fixture" show -s --format='%H' prs-markdown-v0.1.2)" = \
  "$(git -C "$release_fixture" rev-parse HEAD)"
test "$(fixture_candidate_count)" = 0
test "$(jq -r '."crates/prs-t1-agent"' "$release_fixture/.release-please-manifest.json")" = 0.1.2

printf '%s\n' 'release-please workspace and workflow regression checks passed'
