#!/usr/bin/env bash
set -euo pipefail

if [[ "${1:-}" != "--confirm-production" || "$#" -ne 1 ]]; then
    cat >&2 <<'USAGE'
This command creates the production PRSync D1 database and R2 bucket.

It is a one-time bootstrap action. Ordinary Worker deploys do not create
Cloudflare resources.

Run this command with operator credentials that can create D1 databases and R2
buckets. Do not use the restricted Worker-publishing credential.

Usage:
  tools/prs-cloudflare-bootstrap.sh --confirm-production
USAGE
    exit 2
fi

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
# shellcheck source=tools/prs-cloudflare-wrangler-version.sh
source "$repo_root/tools/prs-cloudflare-wrangler-version.sh"
require_prs_wrangler

cat >&2 <<'NOTICE'
You are about to create or reserve the production PRSync resources:
  D1 database: prs-reader-db
  R2 bucket:   prs-reader-documents

The command uses the active Wrangler account. Check that account before you
continue. The generated D1 ID must be copied into the production section of
crates/prs-cloudflare/wrangler.toml.
NOTICE

wrangler d1 create prs-reader-db
wrangler r2 bucket create prs-reader-documents

cat <<'NEXT'

Bootstrap finished. Copy the D1 database ID from the command output into:

  crates/prs-cloudflare/wrangler.toml

Then review the production ID and apply the checked-in schema with the
operator migration command:

  tools/prs-cloudflare-migrate.sh --production

The production R2 bucket remains private because wrangler.toml declares it as
a Worker binding and does not configure public access.
NEXT
