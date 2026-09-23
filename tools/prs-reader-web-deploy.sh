#!/usr/bin/env bash
set -euo pipefail

if [[ "${1:-}" != "--production" || "$#" -ne 1 ]]; then
    cat >&2 <<'USAGE'
Deploy the assembled browser reader to the existing prs-reader-web Worker.

The site must first be built and checked with:
  tools/reader-web-build.sh build
  tools/test-reader-web.sh target/reader-web

Usage:
  tools/prs-reader-web-deploy.sh --production
USAGE
    exit 2
fi

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
config="$repo_root/web/reader-web/wrangler.toml"
output_dir="$repo_root/target/reader-web"
# shellcheck source=tools/prs-cloudflare-wrangler-version.sh
source "$repo_root/tools/prs-cloudflare-wrangler-version.sh"
require_prs_wrangler

if [[ ! -s "$output_dir/index.html" || ! -s "$output_dir/main.js" || \
      ! -s "$output_dir/pkg/prs_reader_web_bg.wasm" ]]; then
    echo "browser reader assets are incomplete; build and validate target/reader-web first" >&2
    exit 1
fi

if [[ ! -f "$config" ]]; then
    echo "missing static Worker configuration: $config" >&2
    exit 1
fi

cd -- "$repo_root"
wrangler deploy --config "$config"
