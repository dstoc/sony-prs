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

printf '%s' "${FAKE_READINESS_BODY:?}" > "$output"
if [[ "$write_out" == '%{http_code}' ]]; then
    printf '%s' '200'
fi
CURL
chmod +x "$test_dir/bin/curl"

ready_response=$(cat <<'JSON'
{"status":"ready","schema_requirement":{"migration_id":7,"migration_name":"0007_remove_owner_identity.sql"}}
JSON
)
if ! PATH="$test_dir/bin:$PATH" FAKE_READINESS_BODY="$ready_response" \
    "$readiness" https://reader.example.com >"$test_dir/ready.out"; then
    echo "the readiness probe rejected the current release requirement" >&2
    exit 1
fi

old_response=$(cat <<'JSON'
{"status":"ready","schema_requirement":{"migration_id":6,"migration_name":"0006_authorization_maintenance.sql"}}
JSON
)
if PATH="$test_dir/bin:$PATH" FAKE_READINESS_BODY="$old_response" \
    "$readiness" https://reader.example.com >"$test_dir/old.out" 2>"$test_dir/old.err"; then
    echo "the readiness probe accepted an older Worker response" >&2
    exit 1
fi
grep -Fq 'does not match this release' "$test_dir/old.err"

echo "prs-cloudflare readiness probe checks passed"
