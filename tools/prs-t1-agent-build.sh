#!/usr/bin/env bash
set -euo pipefail

# Keep the production build pins in one place. Workflows read these values via
# print-github-actions before installing their toolchains.
readonly RUST_TOOLCHAIN="1.98.1"
readonly ZIG_VERSION="0.16.0"
readonly CARGO_ZIGBUILD_VERSION="0.23.4"
readonly TARGET="armv7-unknown-linux-musleabi"
readonly MANIFEST="crates/prs-t1-agent/Cargo.toml"
readonly PACKAGE="prs-t1-agent"
readonly DIST_ARTIFACT="dist/prs-t1-agent-armv7"

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
repo_root=${PRS_T1_AGENT_REPO_ROOT:-$(cd -- "$script_dir/.." && pwd)}
cd -- "$repo_root"

artifact_path="target/$TARGET/release/$PACKAGE"

usage() {
  cat <<'EOF'
Usage: tools/prs-t1-agent-build.sh <command>

Commands:
  print <pin>                  Print one production build pin.
  print-github-actions         Print pins as GitHub Actions step outputs.
  print-manifest-hash          Print the tracked Cargo manifest hash.
  build                        Build the production PRS-T1 ARM executable.
  verify [artifact]             Validate and copy the production executable.
  build-and-verify              Build, validate, and copy the executable.
EOF
}

print_pin() {
  case "${1:-}" in
    rust-toolchain)
      printf '%s\n' "$RUST_TOOLCHAIN"
      ;;
    zig-version)
      printf '%s\n' "$ZIG_VERSION"
      ;;
    cargo-zigbuild-version)
      printf '%s\n' "$CARGO_ZIGBUILD_VERSION"
      ;;
    target)
      printf '%s\n' "$TARGET"
      ;;
    *)
      usage >&2
      exit 2
      ;;
  esac
}

print_github_actions() {
  local build_config_hash

  build_config_hash=$(sha256sum "$script_dir/prs-t1-agent-build.sh" | cut -d ' ' -f1)

  printf 'rust_toolchain=%s\n' "$RUST_TOOLCHAIN"
  printf 'zig_version=%s\n' "$ZIG_VERSION"
  printf 'cargo_zigbuild_version=%s\n' "$CARGO_ZIGBUILD_VERSION"
  printf 'target=%s\n' "$TARGET"
  printf 'build_config_hash=%s\n' "$build_config_hash"
}

print_manifest_hash() {
  (
    cd -- "$repo_root"
    git ls-files -z -- '*Cargo.toml' '*Cargo.lock' |
      sort -z |
      xargs -0 sha256sum |
      sha256sum |
      cut -d ' ' -f1
  )
}

check_installed_toolchain() {
  local rust_version zig_version

  rust_version=$(rustup run "$RUST_TOOLCHAIN" rustc --version)
  case "$rust_version" in
    "rustc $RUST_TOOLCHAIN "*) ;;
    *)
      printf 'unexpected Rust compiler: %s\n' "$rust_version" >&2
      exit 1
      ;;
  esac

  zig_version=$(zig version)
  if [[ "$zig_version" != "$ZIG_VERSION" ]]; then
    printf 'unexpected Zig version: %s\n' "$zig_version" >&2
    exit 1
  fi
}

build() {
  local build_log="target/prs-t1-agent-build.log"
  local build_status

  check_installed_toolchain
  mkdir -p "$(dirname -- "$build_log")"

  set +e
  # Force onig_sys to compile its bundled source instead of probing a host
  # Oniguruma installation; the production reader must remain self-contained.
  RUSTONIG_SYSTEM_LIBONIG=0 \
  RUSTFLAGS='-C target-cpu=cortex-a8 -C link-arg=-mcpu=cortex-a8' \
    cargo +"$RUST_TOOLCHAIN" zigbuild \
    --manifest-path "$MANIFEST" \
    --release \
    --target "$TARGET" 2>&1 | tee "$build_log"
  build_status=${PIPESTATUS[0]}
  set -e

  if (( build_status != 0 )); then
    while IFS= read -r line; do
      line=${line//'%'/'%25'}
      line=${line//$'\r'/'%0D'}
      line=${line//$'\n'/'%0A'}
      printf '::error file=tools/prs-t1-agent-build.sh::%s\n' "$line"
    done < <(tail -n 20 "$build_log")
    return "$build_status"
  fi
}

verify() {
  local artifact=${1:-$artifact_path}
  local file_output
  local inspection_dir="target/prs-t1-agent-elf"

  test -x "$artifact"

  file_output=$(file -b "$artifact")
  printf '%s\n' "$file_output"
  case "$file_output" in
    *"ELF 32-bit LSB"*"ARM"*"EABI5"*"statically linked"*) ;;
    *)
      printf 'unexpected executable format: %s\n' "$file_output" >&2
      exit 1
      ;;
  esac

  mkdir -p "$inspection_dir"
  readelf -h "$artifact" | tee "$inspection_dir/header.txt"
  readelf -l "$artifact" | tee "$inspection_dir/program-headers.txt"
  readelf -A "$artifact" | tee "$inspection_dir/attributes.txt"

  grep -Eq '^  Class:.*ELF32' "$inspection_dir/header.txt"
  grep -Eq '^  Machine:.*ARM' "$inspection_dir/header.txt"
  grep -Eq '^  Flags:.*Version5 EABI, soft-float ABI' "$inspection_dir/header.txt"
  grep -Eq '^  Tag_CPU_arch: v7' "$inspection_dir/attributes.txt"

  if grep -q 'INTERP' "$inspection_dir/program-headers.txt"; then
    printf '%s\n' 'the production executable has a dynamic loader' >&2
    exit 1
  fi

  if grep -Eq 'Tag_ABI_VFP_args: VFP registers|Tag_ABI_HardFP_use' \
    "$inspection_dir/attributes.txt"; then
    printf '%s\n' 'the production executable uses hard-float ABI attributes' >&2
    exit 1
  fi

  install -Dm755 "$artifact" "$DIST_ARTIFACT"
  sha256sum "$DIST_ARTIFACT"
}

case "${1:-}" in
  print)
    print_pin "${2:-}"
    ;;
  print-github-actions)
    print_github_actions
    ;;
  print-manifest-hash)
    print_manifest_hash
    ;;
  build)
    build
    ;;
  verify)
    verify "${2:-}"
    ;;
  build-and-verify)
    build
    verify
    ;;
  *)
    usage >&2
    exit 2
    ;;
esac
