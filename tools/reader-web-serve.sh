#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
output_dir=${PRS_READER_WEB_OUTPUT_DIR:-"$repo_root/target/reader-web"}
port=${1:-8000}

if [[ ! "$port" =~ ^[0-9]+$ ]] || ((port < 1 || port > 65535)); then
  printf 'port must be an integer between 1 and 65535: %s\n' "$port" >&2
  exit 2
fi

if [[ ! -s "$output_dir/index.html" || ! -s "$output_dir/pkg/prs_reader_web.js" ]]; then
  printf 'assembled reader site not found at %s; run tools/reader-web-build.sh build first\n' \
    "$output_dir" >&2
  exit 1
fi

printf 'Serving %s at http://127.0.0.1:%s/\n' "$output_dir" "$port"
exec python3 -m http.server "$port" --bind 127.0.0.1 --directory "$output_dir"
