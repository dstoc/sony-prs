#!/usr/bin/env bash
set -euo pipefail

if [[ "${1:-}" != "--production" || "$#" -ne 1 ]]; then
    cat >&2 <<'USAGE'
Deploy the existing production PRSync resources.

This command only publishes the Worker. It never applies migrations and never
creates a D1 database or R2 bucket. Use the operator migration command and
tools/prs-cloudflare-bootstrap.sh separately.

Usage:
  tools/prs-cloudflare-deploy.sh --production

Required post-publish readiness check:
  PRS_READER_URL=https://reader.example.com tools/prs-cloudflare-deploy.sh --production
USAGE
    exit 2
fi

if ! command -v wrangler >/dev/null 2>&1; then
    echo "wrangler is required; install it before deploying" >&2
    exit 1
fi

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
worker_dir="$repo_root/crates/prs-cloudflare"
config="$worker_dir/wrangler.toml"

if grep -Fq 'REPLACE_WITH_PRODUCTION_D1_DATABASE_ID' "$config"; then
    echo "replace the production D1 ID before deploying" >&2
    exit 1
fi

if [[ -z "${PRS_READER_URL:-}" ]]; then
    echo "PRS_READER_URL is required so the published release can be verified" >&2
    exit 2
fi

cd "$worker_dir"
wrangler deploy --env production --no-x-provision
"$repo_root/tools/prs-cloudflare-readiness.sh" "$PRS_READER_URL"
