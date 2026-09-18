#!/usr/bin/env bash
set -euo pipefail

if [[ "${1:-}" != "--production" || "$#" -ne 1 ]]; then
    cat >&2 <<'USAGE'
Deploy the existing production PRSync resources.

This command never creates a D1 database or R2 bucket. Use
tools/prs-cloudflare-bootstrap.sh for the one-time resource bootstrap.

Usage:
  tools/prs-cloudflare-deploy.sh --production
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

cd "$worker_dir"
wrangler d1 migrations apply DB --remote --env production --no-x-provision
wrangler deploy --env production --no-x-provision
