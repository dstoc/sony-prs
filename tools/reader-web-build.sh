#!/usr/bin/env bash
set -euo pipefail

# Keep browser simulator pins in one place. CI and local instructions use the
# Rust toolchain selected by RUSTUP_TOOLCHAIN, when that variable is set.
readonly RUST_TOOLCHAIN="1.98.1"
readonly WASM_BINDGEN_VERSION="0.2.128"
readonly TARGET="wasm32-unknown-unknown"
readonly MANIFEST="web/reader-web/Cargo.toml"
readonly WASM_ARTIFACT="target/$TARGET/release/prs_reader_web.wasm"
readonly OUTPUT_DIR="target/reader-web"

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
repo_root=${PRS_READER_WEB_REPO_ROOT:-$(cd -- "$script_dir/.." && pwd)}
cd -- "$repo_root"

usage() {
  cat <<'EOF'
Usage: tools/reader-web-build.sh <command>

Commands:
  print-wasm-bindgen-version  Print the pinned wasm-bindgen-cli version.
  print-github-actions        Print pins as GitHub Actions step outputs.
  build                       Build and assemble target/reader-web/.
EOF
}

print_github_actions() {
  printf 'rust_toolchain=%s\n' "$RUST_TOOLCHAIN"
  printf 'wasm_bindgen_version=%s\n' "$WASM_BINDGEN_VERSION"
  printf 'target=%s\n' "$TARGET"
}

check_rust_toolchain() {
  local rust_version
  rust_version=$(rustc --version)
  case "$rust_version" in
    "rustc $RUST_TOOLCHAIN "*) ;;
    *)
      printf 'unexpected Rust compiler: %s\n' "$rust_version" >&2
      exit 1
      ;;
  esac
}

check_wasm_bindgen() {
  local wasm_bindgen_version
  if ! command -v wasm-bindgen >/dev/null; then
    printf 'wasm-bindgen %s is required; install it with cargo install wasm-bindgen-cli --version %s --locked\n' \
      "$WASM_BINDGEN_VERSION" "$WASM_BINDGEN_VERSION" >&2
    exit 1
  fi
  wasm_bindgen_version=$(wasm-bindgen --version)
  if [[ "$wasm_bindgen_version" != "wasm-bindgen $WASM_BINDGEN_VERSION" ]]; then
    printf 'unexpected wasm-bindgen version: %s\n' "$wasm_bindgen_version" >&2
    exit 1
  fi
}

build() {
  check_rust_toolchain
  check_wasm_bindgen

  cargo build \
    --manifest-path "$MANIFEST" \
    --target "$TARGET" \
    --release

  rm -rf -- "$OUTPUT_DIR"
  mkdir -p -- "$OUTPUT_DIR/pkg"

  wasm-bindgen \
    --target web \
    --no-typescript \
    --out-dir "$OUTPUT_DIR/pkg" \
    "$WASM_ARTIFACT"

  cp -- web/reader-web/directory-library.js web/reader-web/index.html web/reader-web/main.js \
    web/reader-web/input.mjs \
    web/reader-web/style.css "$OUTPUT_DIR/"
  test -s "$OUTPUT_DIR/index.html"
  test -s "$OUTPUT_DIR/directory-library.js"
  test -s "$OUTPUT_DIR/main.js"
  test -s "$OUTPUT_DIR/style.css"
  test -s "$OUTPUT_DIR/pkg/prs_reader_web.js"
  test -s "$OUTPUT_DIR/pkg/prs_reader_web_bg.wasm"
}

case "${1:-}" in
  print-wasm-bindgen-version)
    printf '%s\n' "$WASM_BINDGEN_VERSION"
    ;;
  print-github-actions)
    print_github_actions
    ;;
  build)
    build
    ;;
  *)
    usage >&2
    exit 2
    ;;
esac
