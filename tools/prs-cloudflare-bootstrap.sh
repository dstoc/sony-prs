#!/usr/bin/env bash
set -euo pipefail

if [[ "${1:-}" != "--confirm-production" || "$#" -ne 1 ]]; then
    cat >&2 <<'USAGE'
This command creates the production PRSync D1 database and R2 bucket.

It is a one-time bootstrap action. Ordinary Worker deploys do not create
Cloudflare resources.

Usage:
  tools/prs-cloudflare-bootstrap.sh --confirm-production
USAGE
    exit 2
fi

if ! command -v wrangler >/dev/null 2>&1; then
    echo "wrangler is required; install it before running the bootstrap" >&2
    exit 1
fi

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
