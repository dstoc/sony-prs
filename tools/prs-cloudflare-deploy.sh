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

Optional readiness check:
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

cd "$worker_dir"
wrangler deploy --env production --no-x-provision

if [[ -n "${PRS_READER_URL:-}" ]]; then
    "$repo_root/tools/prs-cloudflare-readiness.sh" "$PRS_READER_URL"
else
    cat <<'NEXT'

Deployment completed. Check the new release before serving traffic:
  tools/prs-cloudflare-readiness.sh <worker-url>
The response must report status "ready" and this release's schema requirement.
NEXT
fi
