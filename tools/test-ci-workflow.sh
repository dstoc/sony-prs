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
grep -Fq 'run: tools/test-ci-workflow.sh' "$workflow"
grep -Fq 'run: tools/test-prs-t1-recovery-docs.sh' "$workflow"
grep -Fq 'tools/prsync-version-report.sh ci-artifacts/prsync-versions.txt' "$workflow"
grep -Fq 'name: Test PRSync protocol serialization' "$workflow"
grep -Fq 'cargo +"${{ steps.build-pins.outputs.rust_toolchain }}" test -p prs-sync-protocol --lib' "$workflow"
grep -Fq 'name: Test PRSync bundle validation' "$workflow"
grep -Fq 'cargo +"${{ steps.build-pins.outputs.rust_toolchain }}" test -p prs-sync-bundle --lib' "$workflow"
grep -Fq 'name: Test sender CLI' "$workflow"
grep -Fq 'cargo +"${{ steps.build-pins.outputs.rust_toolchain }}" test -p prs-send --test cli' "$workflow"
grep -Fq 'cargo +"${{ steps.build-pins.outputs.rust_toolchain }}" test --workspace' "$workflow"
grep -Fq 'WORKER_BUILD_VERSION: "0.8.6"' "$workflow"
grep -Fq 'WRANGLER_VERSION: "4.135.0"' "$workflow"
grep -Fq 'cargo +"${{ steps.build-pins.outputs.rust_toolchain }}" install worker-build --version "$WORKER_BUILD_VERSION" --locked' "$workflow"
grep -Fq 'npm install --global "wrangler@$WRANGLER_VERSION"' "$workflow"
grep -Fq 'python3 tools/test-prs-cloudflare-local.py' "$workflow"
grep -Fq 'python3 tools/test-prs-cloudflare-publishing.py' "$workflow"
grep -Fq 'python3 tools/test-prs-cloudflare-deployment-credentials.py' "$workflow"
grep -Fq 'cargo +"${{ steps.build-pins.outputs.rust_toolchain }}" install cargo-zigbuild' "$workflow"
grep -Fq 'tools/prs-t1-agent-build.sh build-and-verify' "$workflow"
grep -Fq 'version: "${{ steps.build-pins.outputs.zig_version }}"' "$workflow"
grep -Fq -- '--target "${{ steps.build-pins.outputs.target }}"' "$workflow"
grep -Fq 'uses: Swatinem/rust-cache@63fed3e2fecf6f7b51dc6f043341b79ef82a9ae7 # v2.9.2' "$workflow"
grep -Fq 'shared-key: prs-workspace-host' "$workflow"
grep -Fq 'name: PRS-T1 ARMv7 build' "$workflow"
grep -Fq 'shared-key: prs-t1-armv7' "$workflow"
grep -Fq 'add-job-id-key: false' "$workflow"
grep -Fq 'cache-targets: true' "$workflow"
grep -Fq 'cache-all-crates: true' "$workflow"
grep -Fq 'cache-bin: true' "$workflow"
grep -Fq 'printf '\''manifest_hash=%s\n'\'' "$(tools/prs-t1-agent-build.sh print-manifest-hash)" >> "$GITHUB_OUTPUT"' "$workflow"
grep -Fq "manifests-\${{ steps.build-pins.outputs.manifest_hash }}" "$workflow"
if grep -Fq 'hashFiles(' "$workflow"; then
  printf '%s\n' 'CI cache keys must not use hashFiles' >&2
  exit 1
fi
grep -Fq 'build-${{ steps.build-pins.outputs.build_config_hash }}' "$workflow"
grep -Fq 'if [[ ! -x "$cargo_zigbuild_bin" ]]' "$workflow"
grep -Fq 'uses: actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02 # v4.6.2' "$workflow"
grep -Fq 'name: prsync-versions' "$workflow"
grep -Fq 'path: ci-artifacts/prsync-versions.txt' "$workflow"

grep -Fq 'reader-web-wasm:' "$workflow"
grep -Fq 'name: Browser reader WASM build' "$workflow"
grep -Fq 'clean: true' "$workflow"
grep -Fq 'fetch-depth: 1' "$workflow"
grep -Fq 'cargo +"${{ steps.build-pins.outputs.rust_toolchain }}" test -p prs-markdown --lib' "$workflow"
grep -Fq 'cargo +"${{ steps.build-pins.outputs.rust_toolchain }}" build -p prs-markdown --target "${{ steps.build-pins.outputs.target }}" --release' "$workflow"
grep -Fq 'tools/reader-web-build.sh build' "$workflow"
grep -Fq 'tools/test-reader-web.sh target/reader-web' "$workflow"

for variable in CLOUDFLARE_API_TOKEN CLOUDFLARE_ACCOUNT_ID; do
  grep -Fq "$variable: \"\"" "$workflow"
done

if grep -Eq 'secrets\.(CLOUDFLARE|CF_)' "$workflow"; then
  printf '%s\n' 'CI workflow references a production Cloudflare secret' >&2
  exit 1
fi

version_report=$(mktemp)
trap 'rm -f "$version_report"' EXIT
tools/prsync-version-report.sh "$version_report"
grep -Eq '^protocol_version=[0-9]+\.[0-9]+$' "$version_report"
grep -Eq '^bundle_format_version=[0-9]+$' "$version_report"

if grep -Eq 'armv5te|arm926ej-s' "$workflow"; then
  printf '%s\n' 'CI workflow contains stale T1 ARMv5 settings' >&2
  exit 1
fi

if grep -vF 'tools/test-release-please-workflow.sh' "$workflow" |
  grep -Eq 'release-please|gh release|workflow_dispatch|release upload'; then
  printf '%s\n' 'CI workflow contains release operations' >&2
  exit 1
fi

printf '%s\n' 'CI workflow regression checks passed'
