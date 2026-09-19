#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -ne 1 || -z "$1" ]]; then
    cat >&2 <<'USAGE'
Inspect the deployed PRSync Worker's read-only schema readiness endpoint.

The expected schema requirement is read from the checked-in Worker release
declaration. The response must report that exact requirement.

Usage:
  tools/prs-cloudflare-readiness.sh <worker-url>
USAGE
    exit 2
fi

case "$1" in
    http://*|https://*)
        ;;
    *)
        echo "worker URL must start with http:// or https://" >&2
        exit 2
        ;;
esac

if ! command -v curl >/dev/null 2>&1; then
    echo "curl is required; install it before checking readiness" >&2
    exit 1
fi

if ! command -v python3 >/dev/null 2>&1; then
    echo "python3 is required; install it before checking readiness" >&2
    exit 1
fi

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
schema_source="$repo_root/crates/prs-cloudflare/src/schema.rs"
if [[ ! -f "$schema_source" ]]; then
    echo "Worker schema declaration is missing: $schema_source" >&2
    exit 1
fi

if ! expected_requirement=$(python3 - "$schema_source" <<'PY'
from pathlib import Path
import re
import sys

source = Path(sys.argv[1]).read_text()
declaration = re.search(
    r"pub const RELEASE_SCHEMA_REQUIREMENT: SchemaRequirement = SchemaRequirement\s*\{(?P<body>.*?)\};",
    source,
    re.DOTALL,
)
if declaration is None:
    raise SystemExit("release schema requirement declaration is missing")

body = declaration.group("body")
migration_id = re.search(r"migration_id:\s*(\d+)", body)
migration_name = re.search(r'migration_name:\s*"([^"]+)"', body)
if migration_id is None or migration_name is None:
    raise SystemExit("release schema requirement is incomplete")

print(f"{migration_id.group(1)}\t{migration_name.group(1)}")
PY
); then
    echo "could not read the release schema requirement from $schema_source" >&2
    exit 1
fi
IFS=$'\t' read -r expected_migration_id expected_migration_name <<< "$expected_requirement"

url="${1%/}/ready"
response_file=$(mktemp)
trap 'rm -f "$response_file"' EXIT

status=$(curl --silent --show-error --max-time 10 \
    --output "$response_file" --write-out '%{http_code}' "$url")
cat "$response_file"
printf '\n'

if [[ "$status" != "200" ]]; then
    echo "Worker schema readiness failed with HTTP $status" >&2
    exit 1
fi

if ! python3 - "$response_file" "$expected_migration_id" "$expected_migration_name" <<'PY'
import json
import sys

response_path, expected_id, expected_name = sys.argv[1:]
try:
    response = json.loads(open(response_path, encoding="utf-8").read())
except (OSError, json.JSONDecodeError) as error:
    raise SystemExit(f"Worker schema readiness returned invalid JSON: {error}") from error

expected_requirement = {
    "migration_id": int(expected_id),
    "migration_name": expected_name,
}
if response.get("status") != "ready":
    raise SystemExit(
        "Worker schema readiness returned status "
        f"{response.get('status')!r} (reason: {response.get('reason')!r})"
    )
if response.get("schema_requirement") != expected_requirement:
    raise SystemExit(
        "Worker schema requirement does not match this release: "
        f"expected {expected_requirement!r}, got {response.get('schema_requirement')!r}"
    )
PY
then
    exit 1
fi
