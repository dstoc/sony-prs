#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -ne 1 || -z "$1" ]]; then
    cat >&2 <<'USAGE'
Inspect the deployed PRSync Worker's read-only schema readiness endpoint.

Usage:
  tools/prs-cloudflare-readiness.sh <worker-url>
USAGE
    exit 2
fi

case "$1" in
    http://*|https://*)
        ;;
    *)
        echo "worker URL must start with http:// or https://" >&2
        exit 2
        ;;
esac

if ! command -v curl >/dev/null 2>&1; then
    echo "curl is required; install it before checking readiness" >&2
    exit 1
fi

url="${1%/}/ready"
response_file=$(mktemp)
trap 'rm -f "$response_file"' EXIT

status=$(curl --silent --show-error --max-time 10 \
    --output "$response_file" --write-out '%{http_code}' "$url")
cat "$response_file"
printf '\n'

if [[ "$status" != "200" ]]; then
    echo "Worker schema readiness failed with HTTP $status" >&2
    exit 1
fi
