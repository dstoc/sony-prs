#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
protocol_source="$repo_root/crates/prs-sync-protocol/src/lib.rs"
protocol_manifest="$repo_root/crates/prs-sync-protocol/Cargo.toml"
bundle_manifest="$repo_root/crates/prs-sync-bundle/Cargo.toml"

protocol_version=$(sed -nE \
  's/^pub const CURRENT_PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion::new\(([0-9]+),[[:space:]]*([0-9]+)\);$/\1.\2/p' \
  "$protocol_source")
bundle_format_version=$(sed -nE \
  's/^pub const CURRENT_BUNDLE_FORMAT_VERSION: u16 = ([0-9]+);$/\1/p' \
  "$protocol_source")
protocol_crate_version=$(sed -nE 's/^version = "([^"]+)"$/\1/p' "$protocol_manifest" | head -n 1)
bundle_crate_version=$(sed -nE 's/^version = "([^"]+)"$/\1/p' "$bundle_manifest" | head -n 1)

for value in \
  "$protocol_version" \
  "$bundle_format_version" \
  "$protocol_crate_version" \
  "$bundle_crate_version"; do
  if [[ -z "$value" ]]; then
    printf '%s\n' 'could not read a PRSync version from the repository' >&2
    exit 1
  fi
done

write_report() {
  cat <<EOF
protocol_version=$protocol_version
bundle_format_version=$bundle_format_version
protocol_crate_version=$protocol_crate_version
bundle_crate_version=$bundle_crate_version
EOF
}

case "$#" in
  0)
    write_report
    ;;
  1)
    write_report > "$1"
    ;;
  *)
    printf 'Usage: %s [output-file]\n' "$0" >&2
    exit 2
    ;;
esac
