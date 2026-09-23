#!/usr/bin/env bash
set -euo pipefail

readonly reader_web_origin="https://prs-reader-web.dstoc.workers.dev"
readonly api_origin="https://prs-reader.dstoc.workers.dev"
readonly reader_web_api_path="/api/v1/authorization/reader"
headers=$(mktemp)
trap 'rm -f -- "$headers"' EXIT

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
        {
            sub(/\r$/, "")
            sub(/[?#].*$/, "")
            sub(/:\/\/[^/]*@/, "://<redacted>@")
            print
        }
    '
}

print_relevant_headers() {
    awk '
        /^HTTP\// { print; next }
        {
            name = $1
            sub(/:$/, "", name)
            name = tolower(name)
            if (name == "location") {
                value = $0
                sub(/^[^:]*:[[:space:]]*/, "", value)
                sub(/[?#].*$/, "", value)
                sub(/:\/\/[^/]*@/, "://<redacted>@", value)
                print "Location: " value
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
    local requested_url="$reader_web_origin/$asset_path"
    local original_url=$requested_url
    local status content_type media_type effective_url curl_exit location metadata initial_location=''
    local initial_headers=''
    local stage='initial response'
    local -a metadata_lines=()

    : > "$headers"
    curl_exit=0
    metadata=$(curl --disable --fail-with-body --silent --show-error --max-time 20 \
        --dump-header "$headers" --output /dev/null \
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
            --dump-header "$headers" --output /dev/null \
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

check_asset index.html html
check_asset main.js js
check_asset directory-library.js js
check_asset input.mjs js
check_asset fullscreen.mjs js
check_asset prsync-client.mjs js
check_asset style.css css
check_asset pkg/prs_reader_web.js js
check_asset pkg/prs_reader_web_bg.wasm wasm

: > "$headers"
status=$(curl --disable --silent --show-error --max-time 20 --request OPTIONS \
    --dump-header "$headers" --output /dev/null --write-out '%{http_code}' \
    "$api_origin$reader_web_api_path" \
    --header "Origin: $reader_web_origin" \
    --header 'Access-Control-Request-Method: POST' \
    --header 'Access-Control-Request-Headers: content-type')
if [[ "$status" != "204" ]]; then
    echo "reader API preflight returned HTTP $status for $reader_web_origin" >&2
    exit 1
fi

allow_origin=$(header_value 'Access-Control-Allow-Origin')
allow_methods=$(header_value 'Access-Control-Allow-Methods')
allow_headers=$(header_value 'Access-Control-Allow-Headers')
allow_credentials=$(header_value 'Access-Control-Allow-Credentials')
if [[ "$allow_origin" != "$reader_web_origin" || "$allow_methods" != "POST" || \
      "${allow_headers,,}" != *content-type* || -n "$allow_credentials" ]]; then
    echo "reader API preflight did not return the exact-origin reader CORS contract" >&2
    exit 1
fi
printf 'verified API reader authorization CORS for %s\n' "$reader_web_origin"

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
