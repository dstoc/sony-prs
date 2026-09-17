#!/usr/bin/env bash
set -euo pipefail

workflow="${1:-.github/workflows/ci.yml}"

grep -Fq 'pull_request:' "$workflow"
grep -Fq 'tools/test-release-please-workflow.sh' "$workflow"
grep -Fq 'branches:' "$workflow"
grep -Fq -- '- main' "$workflow"
grep -Fq 'name: GitHub Actions workflow syntax' "$workflow"
grep -Fq 'uses: raven-actions/actionlint@3d39aea434753780c3b3d4a1a31c854b4dbf49d7 # v2.2.0' "$workflow"
grep -Fq 'version: 1.7.12' "$workflow"
grep -Fq 'cache: false' "$workflow"
grep -Fq 'shellcheck: false' "$workflow"
grep -Fq 'pyflakes: false' "$workflow"
grep -Fq 'cargo +"${{ steps.build-pins.outputs.rust_toolchain }}" fmt --all --check' "$workflow"
grep -Fq 'cargo +"${{ steps.build-pins.outputs.rust_toolchain }}" test --workspace' "$workflow"
grep -Fq 'cargo +"${{ steps.build-pins.outputs.rust_toolchain }}" install cargo-zigbuild' "$workflow"
grep -Fq 'tools/prs-t1-agent-build.sh build-and-verify' "$workflow"
grep -Fq 'version: "${{ steps.build-pins.outputs.zig_version }}"' "$workflow"
grep -Fq -- '--target "${{ steps.build-pins.outputs.target }}"' "$workflow"
grep -Fq 'uses: Swatinem/rust-cache@63fed3e2fecf6f7b51dc6f043341b79ef82a9ae7 # v2.9.2' "$workflow"
grep -Fq 'shared-key: prs-workspace-host' "$workflow"
grep -Fq 'shared-key: prs-t1-armv5te' "$workflow"
grep -Fq 'add-job-id-key: false' "$workflow"
grep -Fq 'cache-targets: true' "$workflow"
grep -Fq 'cache-all-crates: true' "$workflow"
grep -Fq 'cache-bin: true' "$workflow"
grep -Fq "manifests-\${{ hashFiles('**/Cargo.toml', '**/Cargo.lock') }}" "$workflow"
grep -Fq 'build-${{ steps.build-pins.outputs.build_config_hash }}' "$workflow"
grep -Fq 'if [[ ! -x "$cargo_zigbuild_bin" ]]' "$workflow"

if grep -vF 'tools/test-release-please-workflow.sh' "$workflow" |
  grep -Eq 'release-please|gh release|workflow_dispatch|release upload'; then
  printf '%s\n' 'CI workflow contains release operations' >&2
  exit 1
fi

printf '%s\n' 'CI workflow regression checks passed'
