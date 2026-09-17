#!/usr/bin/env bash
set -euo pipefail

workflow="${1:-.github/workflows/ci.yml}"

grep -Fq 'pull_request:' "$workflow"
grep -Fq 'branches:' "$workflow"
grep -Fq -- '- main' "$workflow"
grep -Fq 'cargo +"${{ steps.build-pins.outputs.rust_toolchain }}" fmt --all --check' "$workflow"
grep -Fq 'cargo +"${{ steps.build-pins.outputs.rust_toolchain }}" test --workspace' "$workflow"
grep -Fq 'cargo +"${{ steps.build-pins.outputs.rust_toolchain }}" install cargo-zigbuild' "$workflow"
grep -Fq 'tools/prs-t1-agent-build.sh build-and-verify' "$workflow"
grep -Fq 'version: "${{ steps.build-pins.outputs.zig_version }}"' "$workflow"
grep -Fq -- '--target "${{ steps.build-pins.outputs.target }}"' "$workflow"

if grep -Eq 'release-please|gh release|workflow_dispatch|release upload' "$workflow"; then
  printf '%s\n' 'CI workflow contains release operations' >&2
  exit 1
fi

printf '%s\n' 'CI workflow regression checks passed'
