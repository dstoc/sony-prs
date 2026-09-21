#!/usr/bin/env bash
set -euo pipefail

if [[ "${1:-}" != "--production" || "$#" -ne 1 ]]; then
    cat >&2 <<'USAGE'
Configure the production approval CSRF secret.

This command generates a new 32-byte secret and sends it directly to
Wrangler. The secret is not stored in the repository or printed by this
command. Running it rotates the secret and invalidates existing approval-page
forms; it does not change polling secrets or bearer credentials.

Run this command with operator credentials that can write Worker secrets. Do
not use the restricted Worker-publishing credential.

Usage:
  tools/prs-cloudflare-configure-approval.sh --production
USAGE
    exit 2
fi

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
# shellcheck source=tools/prs-cloudflare-wrangler-version.sh
source "$repo_root/tools/prs-cloudflare-wrangler-version.sh"
require_prs_wrangler

if ! command -v openssl >/dev/null 2>&1; then
    echo "openssl is required to generate the approval secret" >&2
    exit 1
fi

cat >&2 <<'NOTICE'
Configuring the production approval CSRF secret for Worker prs-reader.
The generated value is sent to Wrangler through standard input and is not
written to a file or displayed.
NOTICE

openssl rand -hex 32 | wrangler secret put PRS_CSRF_SECRET --env production

cat <<'NEXT'

The production approval CSRF secret is configured. Publish the Worker and
wait for the release-aware readiness check before exercising approval URLs.
NEXT
