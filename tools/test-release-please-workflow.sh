#!/usr/bin/env bash
set -euo pipefail

workflow="${1:-.github/workflows/release-please.yml}"
manifest="${2:-crates/prs-t1-agent/Cargo.toml}"
build_script="${3:-tools/prs-t1-agent-build.sh}"

grep -Fq 'workflow_dispatch:' "$workflow"
grep -Fq 'description: Existing prs-t1-agent release tag to build/upload' "$workflow"
grep -Fq 'required: true' "$workflow"
grep -Fq 'type: string' "$workflow"
grep -Fq 'rust-version = "1.98"' "$manifest"
grep -Fq "if: \${{ github.event_name == 'push' }}" "$workflow"
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

printf '%s\n' 'release-please workflow regression checks passed'
