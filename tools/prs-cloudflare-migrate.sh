#!/usr/bin/env bash
set -euo pipefail

if [[ "${1:-}" != "--production" || "$#" -ne 1 ]]; then
    cat >&2 <<'USAGE'
Apply checked-in PRSync D1 migrations with operator credentials.

This command is separate from Worker publishing. It targets the existing
production database and never creates a database or a bucket.

Run this command with operator credentials that can manage the production D1
database. Do not use the restricted Worker-publishing credential.

Usage:
  tools/prs-cloudflare-migrate.sh --production
USAGE
    exit 2
fi

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
worker_dir="$repo_root/crates/prs-cloudflare"
config="$worker_dir/wrangler.toml"
# shellcheck source=tools/prs-cloudflare-wrangler-version.sh
source "$repo_root/tools/prs-cloudflare-wrangler-version.sh"
require_prs_wrangler
lock_dir="${TMPDIR:-/tmp}/prs-cloudflare-migrations.lock"

if ! mkdir "$lock_dir" 2>/dev/null; then
    echo "another PRSync migration run is active: $lock_dir" >&2
    exit 1
fi
trap 'rmdir "$lock_dir"' EXIT

if grep -Fq 'REPLACE_WITH_PRODUCTION_D1_DATABASE_ID' "$config"; then
    echo "replace the production D1 ID before applying migrations" >&2
    exit 1
fi

cat <<'NOTICE'
Target: production D1 database prs-reader-db
Environment: production

Review the pending migration list and confirm the account, database, and
compatibility plan before continuing. The command records successful files in
Wrangler's D1 migration history.
NOTICE

cd "$worker_dir"
wrangler d1 migrations list prs-reader-db --remote --env production
wrangler d1 migrations apply prs-reader-db --remote --env production --no-x-provision
wrangler d1 migrations list prs-reader-db --remote --env production

cat <<'NEXT'

Migration verification is complete only when the final migration list reports
no pending migrations. If a migration failed, inspect the D1 state and
Wrangler history before retrying. Do not deploy the release until the list is
complete and the relevant foreign-key/schema checks have passed.
NEXT
