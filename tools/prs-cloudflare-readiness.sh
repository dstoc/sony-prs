#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -lt 1 || "$#" -gt 2 || -z "$1" ]]; then
    cat >&2 <<'USAGE'
Inspect the deployed PRSync Worker's read-only schema readiness endpoint.

The expected schema requirement is read from the checked-in Worker release
declaration. If a promoted Worker Version ID is supplied, the response must
also report that exact release. The probe polls for a bounded rollout window.

Usage:
  tools/prs-cloudflare-readiness.sh <worker-url> [promoted-version-id]
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

readonly READINESS_TIMEOUT_SECONDS="${PRS_READINESS_TIMEOUT_SECONDS:-120}"
readonly READINESS_INITIAL_BACKOFF_SECONDS="${PRS_READINESS_INITIAL_BACKOFF_SECONDS:-1}"
readonly READINESS_MAX_BACKOFF_SECONDS=10
readonly READINESS_CURL_TIMEOUT_SECONDS=10
if ! [[ "$READINESS_TIMEOUT_SECONDS" =~ ^[0-9]+$ ]] ||
    ! [[ "$READINESS_INITIAL_BACKOFF_SECONDS" =~ ^[0-9]+$ ]]; then
    echo "readiness timeout and backoff must be non-negative integers" >&2
    exit 2
fi

expected_version_id="${2:-}"
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

validate_response() {
    python3 - "$response_file" "$expected_migration_id" \
        "$expected_migration_name" "$expected_version_id" <<'PY'
import json
import sys

response_path, expected_id, expected_name, expected_version_id = sys.argv[1:]
try:
    with open(response_path, encoding="utf-8") as response_file:
        response = json.load(response_file)
except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
    raise SystemExit(f"invalid JSON: {error}") from error

if not isinstance(response, dict):
    raise SystemExit("readiness response is not a JSON object")

if response.get("status") != "ready":
    raise SystemExit(
        "readiness status is "
        f"{response.get('status')!r} (reason: {response.get('reason')!r})"
    )

expected_requirement = {
    "migration_id": int(expected_id),
    "migration_name": expected_name,
}
if response.get("schema_requirement") != expected_requirement:
    raise SystemExit(
        "schema requirement does not match this release: "
        f"expected {expected_requirement!r}, got {response.get('schema_requirement')!r}"
    )

def release_identifier(payload):
    release = payload.get("release")
    if isinstance(release, dict):
        for key in ("version_id", "release_id", "id"):
            value = release.get(key)
            if isinstance(value, str) and value:
                return value
    for key in ("version_id", "release_id", "version"):
        value = payload.get(key)
        if isinstance(value, str) and value:
            return value
    return None

if expected_version_id:
    actual_version_id = release_identifier(response)
    if actual_version_id is None:
        raise SystemExit(
            "readiness response does not expose a release identifier; "
            "the promoted version is not confirmed"
        )
    if actual_version_id != expected_version_id:
        raise SystemExit(
            "readiness response is from a different release: "
            f"expected version {expected_version_id!r}, got {actual_version_id!r}"
        )
PY
}

start_seconds=$SECONDS
attempt=0
backoff_seconds=$READINESS_INITIAL_BACKOFF_SECONDS
last_diagnostic="no response"
while :; do
    attempt=$((attempt + 1))
    : > "$response_file"
    status=''
    if status=$(curl --silent --show-error --max-time "$READINESS_CURL_TIMEOUT_SECONDS" \
        --output "$response_file" --write-out '%{http_code}' "$url"); then
        if [[ -s "$response_file" ]]; then
            cat "$response_file"
            printf '\n'
        fi
        if [[ "$status" != "200" ]]; then
            last_diagnostic="HTTP $status"
        elif diagnostic=$(validate_response 2>&1); then
            echo "Worker schema readiness confirmed after $attempt attempt(s)" >&2
            exit 0
        else
            last_diagnostic="$diagnostic"
        fi
    else
        curl_status=$?
        last_diagnostic="transport failure (curl exit $curl_status)"
    fi

    echo "readiness attempt $attempt did not confirm the promoted release: $last_diagnostic" >&2
    elapsed_seconds=$((SECONDS - start_seconds))
    if ((elapsed_seconds >= READINESS_TIMEOUT_SECONDS)); then
        break
    fi
    sleep "$backoff_seconds"
    if ((backoff_seconds < READINESS_MAX_BACKOFF_SECONDS)); then
        backoff_seconds=$((backoff_seconds * 2))
        if ((backoff_seconds > READINESS_MAX_BACKOFF_SECONDS)); then
            backoff_seconds=$READINESS_MAX_BACKOFF_SECONDS
        fi
    fi
done

echo "Worker schema readiness did not converge to the promoted release within " \
    "$READINESS_TIMEOUT_SECONDS seconds after $attempt attempt(s); last result: " \
    "$last_diagnostic" >&2
exit 1
