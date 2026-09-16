#!/usr/bin/env bash
set -euo pipefail

workflow="${1:-.github/workflows/release-please.yml}"

grep -Fq 'workflow_dispatch:' "$workflow"
grep -Fq 'description: Existing prs-t1-agent release tag to build/upload' "$workflow"
grep -Fq 'required: true' "$workflow"
grep -Fq 'type: string' "$workflow"
grep -Fq "if: \${{ github.event_name == 'push' }}" "$workflow"
grep -Fq 'id: release-tag' "$workflow"
grep -Fq 'MANUAL_TAG: ${{ inputs.tag }}' "$workflow"
grep -Fq 'RELEASE_TAG: ${{ steps.release.outputs['"'"'crates/prs-t1-agent--tag_name'"'"'] }}' "$workflow"
grep -Fq 'tag=%s' "$workflow"
grep -Fq 'ref: ${{ steps.release-tag.outputs.tag }}' "$workflow"
grep -Fq '"${{ steps.release-tag.outputs.tag }}" \' "$workflow"

if grep -Fq 'cargo +"$RUST_TOOLCHAIN" zigbuild --version' "$workflow"; then
  printf '%s\n' 'unsupported cargo zigbuild version check is still present' >&2
  exit 1
fi

printf '%s\n' 'release-please workflow regression checks passed'
