#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
readiness="$repo_root/tools/prs-cloudflare-readiness.sh"
test_dir=$(mktemp -d)
trap 'rm -rf "$test_dir"' EXIT
mkdir "$test_dir/bin"

cat > "$test_dir/bin/curl" <<'CURL'
#!/usr/bin/env bash
set -euo pipefail

output=''
write_out=''
while (($# > 0)); do
    case "$1" in
        --output|--write-out)
            variable="$1"
            value="$2"
            if [[ "$variable" == '--output' ]]; then
                output="$value"
            else
                write_out="$value"
            fi
            shift 2
            ;;
        --*)
            shift
            ;;
        *)
            shift
            ;;
    esac
done

call_file="${FAKE_CURL_CALL_FILE:?}"
calls=0
if [[ -f "$call_file" ]]; then
    calls=$(<"$call_file")
fi
calls=$((calls + 1))
printf '%s' "$calls" > "$call_file"

if [[ "${FAKE_READINESS_SCENARIO:-single}" == "rollout" ]]; then
    case "$calls" in
        1)
            exit 28
            ;;
        2)
            printf '%s' 'Hello World!' > "$output"
            ;;
        3)
            printf '%s' 'not json yet' > "$output"
            ;;
        4)
            printf '%s' '{"status":"ready","schema_requirement":{"migration_id":7,"migration_name":"0007_remove_owner_identity.sql"},"release":{"version_id":"old-version"}}' > "$output"
            ;;
        *)
            printf '%s' '{"status":"ready","schema_requirement":{"migration_id":7,"migration_name":"0007_remove_owner_identity.sql"},"release":{"version_id":"new-version"}}' > "$output"
            ;;
    esac
else
    printf '%s' "${FAKE_READINESS_BODY:?}" > "$output"
fi
if [[ "$write_out" == '%{http_code}' ]]; then
    printf '%s' '200'
fi
CURL
chmod +x "$test_dir/bin/curl"

ready_response=$(cat <<'JSON'
{"status":"ready","schema_requirement":{"migration_id":7,"migration_name":"0007_remove_owner_identity.sql"},"release":{"version_id":"new-version"}}
JSON
)
if ! PATH="$test_dir/bin:$PATH" FAKE_CURL_CALL_FILE="$test_dir/ready.calls" \
    FAKE_READINESS_BODY="$ready_response" \
    "$readiness" https://prs-reader.dstoc.workers.dev >"$test_dir/ready.out"; then
    echo "the readiness probe rejected the current release requirement" >&2
    exit 1
fi

if ! PATH="$test_dir/bin:$PATH" FAKE_CURL_CALL_FILE="$test_dir/release.calls" \
    FAKE_READINESS_SCENARIO=rollout \
    PRS_READINESS_INITIAL_BACKOFF_SECONDS=0 \
    PRS_READINESS_TIMEOUT_SECONDS=2 \
    "$readiness" https://prs-reader.dstoc.workers.dev new-version >"$test_dir/rollout.out" \
    2>"$test_dir/rollout.err"; then
    echo "the readiness probe did not poll through Worker rollout" >&2
    exit 1
fi
if [[ "$(<"$test_dir/release.calls")" -ne 5 ]]; then
    echo "the readiness probe did not retry each rollout response" >&2
    exit 1
fi
grep -Fq 'Hello World!' "$test_dir/rollout.out"
grep -Fq 'not json yet' "$test_dir/rollout.out"
grep -Fq 'transport failure' "$test_dir/rollout.err"
grep -Fq "different release" "$test_dir/rollout.err"

old_response=$(cat <<'JSON'
{"status":"ready","schema_requirement":{"migration_id":6,"migration_name":"0006_authorization_maintenance.sql"}}
JSON
)
if PATH="$test_dir/bin:$PATH" FAKE_CURL_CALL_FILE="$test_dir/old.calls" \
    FAKE_READINESS_BODY="$old_response" \
    PRS_READINESS_TIMEOUT_SECONDS=0 \
    "$readiness" https://prs-reader.dstoc.workers.dev >"$test_dir/old.out" 2>"$test_dir/old.err"; then
    echo "the readiness probe accepted an older Worker response" >&2
    exit 1
fi
grep -Fq 'did not converge' "$test_dir/old.err"
grep -Fq 'schema requirement does not match this release' "$test_dir/old.err"

missing_release_response=$(cat <<'JSON'
{"status":"ready","schema_requirement":{"migration_id":7,"migration_name":"0007_remove_owner_identity.sql"}}
JSON
)
if PATH="$test_dir/bin:$PATH" FAKE_CURL_CALL_FILE="$test_dir/never.calls" \
    FAKE_READINESS_BODY="$missing_release_response" \
    PRS_READINESS_TIMEOUT_SECONDS=0 \
    "$readiness" https://prs-reader.dstoc.workers.dev new-version >"$test_dir/never.out" 2>"$test_dir/never.err"; then
    echo "the readiness probe accepted a response without the promoted release" >&2
    exit 1
fi
grep -Fq 'promoted release' "$test_dir/never.err"

echo "prs-cloudflare readiness probe checks passed"
