#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
config="$repo_root/crates/prs-cloudflare/wrangler.toml"
local_test_config="$repo_root/crates/prs-cloudflare/wrangler.local.toml"
migrations_directory="$repo_root/crates/prs-cloudflare/migrations"
bootstrap="$repo_root/tools/prs-cloudflare-bootstrap.sh"
deploy="$repo_root/tools/prs-cloudflare-deploy.sh"

required_config=(
    'name = "prs-reader-local"'
    'name = "prs-reader"'
    'binding = "DB"'
    'binding = "BUNDLES"'
    'bucket_name = "prs-reader-local"'
    'bucket_name = "prs-reader-documents"'
    'crons = ["0 * * * *"]'
    'command = "worker-build --release"'
)
for expected in "${required_config[@]}"; do
    if ! grep -Fq "$expected" "$config"; then
        echo "missing Worker configuration: $expected" >&2
        exit 1
    fi
done

for expected in \
    'name = "prs-reader-local-e2e"' \
    'command = "worker-build --release --features local-test"' \
    'PRS_ENVIRONMENT = "local"' \
    'PRS_APPROVAL_BASE_URL = "http://127.0.0.1"' \
    'aud = "prs-reader-local-approval"' \
    'email = "owner@example.com"' \
    'binding = "DB"' \
    'binding = "BUNDLES"'; do
    if ! grep -Fq "$expected" "$local_test_config"; then
        echo "missing local end-to-end Worker configuration: $expected" >&2
        exit 1
    fi
done

local_environment=$(awk '
    /^\[env\.local\]/ { in_local = 1 }
    /^\[env\.production\]/ { in_local = 0 }
    in_local { print }
' "$config")
if ! grep -Fq 'database_id = "00000000-0000-0000-0000-000000000000"' <<<"$local_environment"; then
    echo "local Worker configuration must use its non-production D1 ID" >&2
    exit 1
fi
if grep -Fq 'REPLACE_WITH_PRODUCTION_D1_DATABASE_ID' <<<"$local_environment"; then
    echo "local Worker configuration must not use the production D1 placeholder" >&2
    exit 1
fi

production_environment=$(awk '
    /^\[env\.production\]/ { in_production = 1 }
    in_production { print }
' "$config")
if grep -Fq 'REPLACE_WITH_PRODUCTION_D1_DATABASE_ID' <<<"$production_environment"; then
    echo "production Worker configuration must contain the bootstrapped D1 ID" >&2
    exit 1
fi
if ! grep -Eq 'database_id = "[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}"' <<<"$production_environment"; then
    echo "production Worker configuration must contain a UUID-shaped D1 ID" >&2
    exit 1
fi

if grep -Eni 'public|allow_public' "$config"; then
    echo "production R2 configuration must remain private" >&2
    exit 1
fi

for expected in \
    'CREATE TABLE IF NOT EXISTS bundles' \
    'CREATE TABLE inbox' \
    'CREATE TABLE authorization_requests' \
    'CREATE TABLE sender_credentials' \
    'CREATE TABLE reader_sessions' \
    'CREATE TABLE IF NOT EXISTS owner_identity' \
    'CREATE TABLE bundle_lifecycle' \
    "state IN ('uploading', 'published', 'cleanup_claimed')" \
    'CREATE TRIGGER bundles_require_uploading_lifecycle' \
    'DROP TABLE IF EXISTS owner_identity' \
    "state IN ('pending', 'approved', 'denied', 'expired', 'consumed')" \
    'CREATE TRIGGER authorization_requests_state_transition'; do
    if ! grep -R -Fq "$expected" "$migrations_directory"; then
        echo "missing D1 migration statement: $expected" >&2
        exit 1
    fi
done

for expected in '--confirm-production' 'wrangler d1 create prs-reader-db' 'wrangler r2 bucket create prs-reader-documents'; do
    if ! grep -Fq -- "$expected" "$bootstrap"; then
        echo "missing bootstrap guard or resource creation: $expected" >&2
        exit 1
    fi
done

for expected in '--production' 'wrangler d1 migrations apply DB --remote --env production --no-x-provision' 'wrangler deploy --env production --no-x-provision'; do
    if ! grep -Fq -- "$expected" "$deploy"; then
        echo "missing safe deployment guard or command: $expected" >&2
        exit 1
    fi
done

echo "prs-cloudflare configuration checks passed"
