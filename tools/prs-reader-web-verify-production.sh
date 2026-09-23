#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd -- "$repo_root"

readonly reader_web_origin="https://prs-reader-web.dstoc.workers.dev"
api_origin=$(node --input-type=module <<'NODE'
const configuredBase = process.env.PRS_READER_WEB_API_BASE?.trim()
  || "https://prs-reader.dstoc.workers.dev";
let apiBase;
try {
  apiBase = new URL(configuredBase);
} catch {
  process.stderr.write("PRS_READER_WEB_API_BASE must be an absolute HTTPS origin.\n");
  process.exit(1);
}
if (apiBase.protocol !== "https:" || apiBase.hostname === "api"
  || apiBase.username || apiBase.password
  || (apiBase.pathname !== "/" && apiBase.pathname !== "")
  || apiBase.search || apiBase.hash) {
  process.stderr.write("PRS_READER_WEB_API_BASE must be an HTTPS origin without credentials, a path, query, or fragment.\n");
  process.exit(1);
}
process.stdout.write(apiBase.origin);
NODE
)
readonly api_origin
readonly reader_web_api_path="/api/v1/authorization/reader"
headers=$(mktemp)
readonly deployed_index=$(mktemp)
trap 'rm -f -- "$headers" "$deployed_index"' EXIT

header_value() {
    local header_name=$1
    awk -F: -v wanted="$header_name" '
        tolower($1) == tolower(wanted) {
            sub(/^[^:]*:[[:space:]]*/, "")
            sub(/\r$/, "")
            print
        }
    ' "$headers" | tail -n 1
}

safe_location() {
    printf '%s\n' "$1" | awk '
        function redact_authority_userinfo(value, authority, prefix_length, separator) {
            if (match(value, /^([^/?#]+:)?\/\/[^/?#]*@/)) {
                prefix_length = RLENGTH
                authority = substr(value, 1, prefix_length)
                separator = index(authority, "//")
                return substr(authority, 1, separator + 1) "<redacted>@" \
                    substr(value, prefix_length + 1)
            }
            return value
        }
        {
            sub(/\r$/, "")
            sub(/[?#].*$/, "")
            print redact_authority_userinfo($0)
        }
    '
}

print_relevant_headers() {
    awk '
        function redact_authority_userinfo(value, authority, prefix_length, separator) {
            if (match(value, /^([^/?#]+:)?\/\/[^/?#]*@/)) {
                prefix_length = RLENGTH
                authority = substr(value, 1, prefix_length)
                separator = index(authority, "//")
                return substr(authority, 1, separator + 1) "<redacted>@" \
                    substr(value, prefix_length + 1)
            }
            return value
        }
        /^HTTP\// { print; next }
        {
            name = $1
            sub(/:$/, "", name)
            name = tolower(name)
            if (name == "location") {
                value = $0
                sub(/^[^:]*:[[:space:]]*/, "", value)
                sub(/[?#].*$/, "", value)
                print "Location: " redact_authority_userinfo(value)
            } else if (name == "content-type" ||
                name == "date" || name == "server" || name == "age" ||
                name == "cache-control" || name == "cf-ray" ||
                name == "cf-cache-status" || name == "cf-mitigated" ||
                name == "via" || name == "x-cache" ||
                name == "x-content-type-options") {
                print
            }
        }
    ' "$1"
}

report_asset_failure() {
    local asset_path=$1
    local request_url=$2
    local status=$3
    local location=$4
    local effective_url=$5
    local content_type=$6
    local curl_exit=$7
    local stage=$8
    local original_url=$9
    local initial_location=${10}
    local initial_headers=${11}

    printf 'browser asset check failed: %s (%s)\n' "$asset_path" "$stage" >&2
    printf '  request_url: %s\n' "$request_url" >&2
    printf '  status: %s\n' "$status" >&2
    printf '  location: %s\n' "$(safe_location "${location:-<none>}")" >&2
    printf '  effective_url: %s\n' "${effective_url:-<none>}" >&2
    printf '  content_type: %s\n' "${content_type:-<none>}" >&2
    printf '  curl_exit: %s\n' "$curl_exit" >&2
    printf '  relevant response headers:\n' >&2
    print_relevant_headers "$headers" >&2
    if [[ -n "$initial_location" ]]; then
        printf '  prior canonical redirect: %s -> %s\n' \
            "$original_url" "$(safe_location "$initial_location")" >&2
        printf '  prior redirect response headers:\n' >&2
        printf '%s\n' "$initial_headers" | print_relevant_headers /dev/stdin >&2
    fi
}

check_asset() {
    local asset_path=$1
    local expected_type=$2
    local output_file=${3:-/dev/null}
    local requested_url="$reader_web_origin/$asset_path"
    local original_url=$requested_url
    local status content_type media_type effective_url curl_exit location metadata initial_location=''
    local initial_headers=''
    local stage='initial response'
    local -a metadata_lines=()

    : > "$headers"
    curl_exit=0
    metadata=$(curl --disable --fail-with-body --silent --show-error --max-time 20 \
        --dump-header "$headers" --output "$output_file" \
        --write-out '%{http_code}\n%{content_type}\n%{url_effective}\n' \
        "$requested_url") || curl_exit=$?
    mapfile -t metadata_lines <<<"$metadata"
    status=${metadata_lines[0]:-000}
    content_type=${metadata_lines[1]:-}
    effective_url=${metadata_lines[2]:-}
    location=$(header_value 'Location')

    if ((curl_exit != 0)); then
        report_asset_failure "$asset_path" "$requested_url" "$status" \
            "$location" "$effective_url" "$content_type" "$curl_exit" \
            "$stage" "$original_url" "$initial_location" "$initial_headers"
        return 1
    fi

    if [[ "$status" == "307" ]]; then
        if [[ "$asset_path" != "index.html" || \
              ( "$location" != "/" && "$location" != "$reader_web_origin/" ) ]]; then
            report_asset_failure "$asset_path" "$requested_url" "$status" \
                "$location" "$effective_url" "$content_type" "$curl_exit" \
                "$stage" "$original_url" "$initial_location" "$initial_headers"
            return 1
        fi

        initial_location=$location
        initial_headers=$(cat "$headers")
        requested_url="$reader_web_origin/"
        stage='canonical destination response'
        printf 'following verified canonical redirect: %s -> %s\n' \
            "$original_url" "$requested_url"

        : > "$headers"
        curl_exit=0
        metadata=$(curl --disable --fail-with-body --silent --show-error --max-time 20 \
            --dump-header "$headers" --output "$output_file" \
            --write-out '%{http_code}\n%{content_type}\n%{url_effective}\n' \
            "$requested_url") || curl_exit=$?
        mapfile -t metadata_lines <<<"$metadata"
        status=${metadata_lines[0]:-000}
        content_type=${metadata_lines[1]:-}
        effective_url=${metadata_lines[2]:-}
        location=$(header_value 'Location')
    fi

    if ((curl_exit != 0)) || [[ "$status" != "200" || "$effective_url" != "$requested_url" ]]; then
        report_asset_failure "$asset_path" "$requested_url" "$status" \
            "$location" "$effective_url" "$content_type" "$curl_exit" \
            "$stage" "$original_url" "$initial_location" "$initial_headers"
        return 1
    fi
    media_type=${content_type%%;*}
    media_type=${media_type//[[:space:]]/}
    media_type=${media_type,,}
    case "$expected_type:$media_type" in
        html:text/html|css:text/css|wasm:application/wasm|js:application/javascript|js:text/javascript) ;;
        *)
            report_asset_failure "$asset_path" "$requested_url" "$status" \
                "$location" "$effective_url" "$content_type" "$curl_exit" \
                "$stage" "$original_url" "$initial_location" "$initial_headers"
            return 1
            ;;
    esac
    printf 'verified %s at %s (%s)\n' "$asset_path" "$effective_url" "$content_type"
}

check_asset index.html html "$deployed_index"
node "$repo_root/tools/test-reader-web-api-base.mjs" "$deployed_index" "$api_origin"
check_asset main.js js
check_asset directory-library.js js
check_asset input.mjs js
check_asset fullscreen.mjs js
check_asset prsync-client.mjs js
check_asset style.css css
check_asset pkg/prs_reader_web.js js
check_asset pkg/prs_reader_web_bg.wasm wasm

verify_reader_preflight() {
    local path=$1
    local method=$2
    local requested_headers=$3
    local status allow_origin allow_methods allow_headers allow_credentials

    : > "$headers"
    if ! status=$(curl --disable --silent --show-error --max-time 20 --request OPTIONS \
        --dump-header "$headers" --output /dev/null --write-out '%{http_code}' \
        "$api_origin$path" \
        --header "Origin: $reader_web_origin" \
        --header "Access-Control-Request-Method: $method" \
        --header "Access-Control-Request-Headers: $requested_headers"); then
        echo "reader API preflight request failed at $api_origin for $path" >&2
        return 1
    fi
    if [[ "$status" != "204" ]]; then
        echo "reader API preflight returned HTTP $status for $path and $reader_web_origin" >&2
        return 1
    fi

    allow_origin=$(header_value 'Access-Control-Allow-Origin')
    allow_methods=$(header_value 'Access-Control-Allow-Methods')
    allow_headers=$(header_value 'Access-Control-Allow-Headers')
    allow_credentials=$(header_value 'Access-Control-Allow-Credentials')
    if [[ "$allow_origin" != "$reader_web_origin" || "$allow_methods" != "$method" || \
          -n "$allow_credentials" ]]; then
        echo "reader API preflight did not return the exact-origin contract for $path" >&2
        return 1
    fi
    local requested_header normalized_allow_headers
    normalized_allow_headers=${allow_headers,,}
    normalized_allow_headers=${normalized_allow_headers//[[:space:]]/}
    normalized_allow_headers=",$normalized_allow_headers,"
    IFS=',' read -r -a requested_header_names <<<"$requested_headers"
    for requested_header in "${requested_header_names[@]}"; do
        requested_header=${requested_header//[[:space:]]/}
        if [[ "$normalized_allow_headers" != *",${requested_header,,},"* ]]; then
            echo "reader API preflight omitted $requested_header for $path" >&2
            return 1
        fi
    done
    printf 'verified API CORS %s %s at %s for %s\n' \
        "$method" "$path" "$api_origin" "$reader_web_origin"
}

verify_reader_preflight /api/v1/authorization/reader POST content-type
verify_reader_preflight /api/v1/authorization/poll POST content-type
verify_reader_preflight /api/v1/reader/manifest GET 'authorization, if-revision'
verify_reader_preflight /api/v1/reader/bundle GET authorization

status=$(curl --disable --silent --show-error --max-time 20 --request OPTIONS \
    --output /dev/null --write-out '%{http_code}' \
    "$api_origin$reader_web_api_path" \
    --header 'Origin: https://example.invalid' \
    --header 'Access-Control-Request-Method: POST' \
    --header 'Access-Control-Request-Headers: content-type')
if [[ "$status" != "403" ]]; then
    echo "reader API accepted an unexpected browser origin (HTTP $status)" >&2
    exit 1
fi
printf 'verified reader API rejects an unconfigured origin\n'
