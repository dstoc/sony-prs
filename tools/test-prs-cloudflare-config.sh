#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
config="$repo_root/crates/prs-cloudflare/wrangler.toml"
local_test_config="$repo_root/crates/prs-cloudflare/wrangler.local.toml"
migrations_directory="$repo_root/crates/prs-cloudflare/migrations"
bootstrap="$repo_root/tools/prs-cloudflare-bootstrap.sh"
migrate="$repo_root/tools/prs-cloudflare-migrate.sh"
deploy="$repo_root/tools/prs-cloudflare-deploy.sh"
readiness="$repo_root/tools/prs-cloudflare-readiness.sh"

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

python3 - "$config" "$local_test_config" <<'PY'
from pathlib import Path
import sys
import tomllib

for config_path in map(Path, sys.argv[1:]):
    try:
        with config_path.open("rb") as config_file:
            tomllib.load(config_file)
    except tomllib.TOMLDecodeError as error:
        raise SystemExit(f"invalid TOML in {config_path}: {error}") from error
PY

for expected in \
    'name = "prs-reader-local-e2e"' \
    'command = "worker-build --release --features local-test"' \
    'PRS_ENVIRONMENT = "local"' \
    'PRS_APPROVAL_BASE_URL = "http://127.0.0.1"' \
    'PRS_CSRF_SECRET = "local-development-csrf-secret-change-me-32-bytes"' \
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

for expected in \
    'RELEASE_SCHEMA_REQUIREMENT' \
    'migration_id: 7' \
    'migration_name: "0007_remove_owner_identity.sql"' \
    'SELECT id, name FROM d1_migrations ORDER BY id ASC' \
    'get_async("/ready", readiness)'; do
    if ! grep -R -Fq "$expected" \
        "$repo_root/crates/prs-cloudflare/src" "$repo_root/crates/prs-cloudflare/README.md"; then
        echo "missing schema readiness contract: $expected" >&2
        exit 1
    fi
done

for expected in \
    '--confirm-production' \
    'wrangler d1 create prs-reader-db' \
    'wrangler r2 bucket create prs-reader-documents' \
    'tools/prs-cloudflare-migrate.sh --production'; do
    if ! grep -Fq -- "$expected" "$bootstrap"; then
        echo "missing bootstrap guard or resource creation: $expected" >&2
        exit 1
    fi
done
if grep -Fq -- 'tools/prs-cloudflare-deploy.sh --production' "$bootstrap"; then
    echo "bootstrap must not direct operators to publish before migration" >&2
    exit 1
fi

for expected in '--production' 'wrangler deploy --env production --no-x-provision'; do
    if ! grep -Fq -- "$expected" "$deploy"; then
        echo "missing safe deployment guard or command: $expected" >&2
        exit 1
    fi
done
if grep -Fq 'wrangler d1' "$deploy"; then
    echo "Worker deployment must not run D1 management commands" >&2
    exit 1
fi

for expected in \
    '--production' \
    'wrangler d1 migrations list prs-reader-db --remote --env production' \
    'wrangler d1 migrations apply prs-reader-db --remote --env production --no-x-provision' \
    'prs-cloudflare-migrations.lock'; do
    if ! grep -Fq -- "$expected" "$migrate"; then
        echo "missing operator migration guard or command: $expected" >&2
        exit 1
    fi
done

for expected in \
    '/ready' \
    'curl --silent --show-error --max-time 10' \
    'status" != "200"' \
    'RELEASE_SCHEMA_REQUIREMENT' \
    'schema_requirement' \
    'does not match this release'; do
    if ! grep -Fq -- "$expected" "$readiness"; then
        echo "missing read-only readiness check: $expected" >&2
        exit 1
    fi
done
if ! grep -Fq 'prs-cloudflare-readiness.sh" "$PRS_READER_URL"' "$deploy"; then
    echo "production deploy must run the release-aware readiness check" >&2
    exit 1
fi
if ! grep -Fq 'PRS_READER_URL is required' "$deploy"; then
    echo "production deploy must require a readiness URL" >&2
    exit 1
fi

for expected in \
    'COMPATIBLE_FUTURE_MIGRATIONS' \
    'is_declared_compatible_future_migration' \
    '0008_additive_column.sql' \
    '0008_remove_required_column.sql'; do
    if ! grep -Fq -- "$expected" "$repo_root/crates/prs-cloudflare/src/schema.rs"; then
        echo "missing explicit migration compatibility regression: $expected" >&2
        exit 1
    fi
done

echo "prs-cloudflare configuration checks passed"
