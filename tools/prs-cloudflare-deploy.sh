#!/usr/bin/env bash
set -euo pipefail

if [[ "${1:-}" != "--production" || "$#" -ne 1 ]]; then
    cat >&2 <<'USAGE'
Upload and promote the production PRSync Worker version.

This command only publishes the Worker. It never applies migrations and never
creates a D1 database or R2 bucket. Use the operator migration command and
tools/prs-cloudflare-bootstrap.sh separately.

Run this command with a token that can publish the Worker only. The token must
not have D1 or R2 management permission.

Usage:
  tools/prs-cloudflare-deploy.sh --production

Required post-publish readiness check:
  PRS_READER_URL=https://reader.example.com tools/prs-cloudflare-deploy.sh --production
USAGE
    exit 2
fi

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
worker_dir="$repo_root/crates/prs-cloudflare"
config="$worker_dir/wrangler.toml"
# shellcheck source=tools/prs-cloudflare-wrangler-version.sh
source "$repo_root/tools/prs-cloudflare-wrangler-version.sh"
require_prs_wrangler

if grep -Fq 'REPLACE_WITH_PRODUCTION_D1_DATABASE_ID' "$config"; then
    echo "replace the production D1 ID before deploying" >&2
    exit 1
fi

if [[ -z "${PRS_READER_URL:-}" ]]; then
    echo "PRS_READER_URL is required so the published release can be verified" >&2
    exit 2
fi

release_tag=$(git -C "$repo_root" rev-parse --verify HEAD)

cd "$worker_dir"
if upload_output=$(
    wrangler versions upload \
        --env production \
        --tag "$release_tag" \
        --no-x-provision \
        2>&1
); then
    :
else
    upload_status=$?
    printf '%s\n' "$upload_output"
    echo "Worker version upload failed" >&2
    exit "$upload_status"
fi
printf '%s\n' "$upload_output"

uploaded_version_id=$(printf '%s\n' "$upload_output" | sed -nE \
    's/.*Worker Version ID:[[:space:]]*([[:xdigit:]]{8}-[[:xdigit:]]{4}-[[:xdigit:]]{4}-[[:xdigit:]]{4}-[[:xdigit:]]{12}).*/\1/p' | \
    tail -n 1)
if [[ -z "$uploaded_version_id" ]]; then
    echo "could not determine the uploaded Worker Version ID" >&2
    exit 1
fi

wrangler versions deploy \
    --env production \
    --version-id "$uploaded_version_id" \
    --percentage 100 \
    --yes \
    --no-x-provision
"$repo_root/tools/prs-cloudflare-readiness.sh" "$PRS_READER_URL"
