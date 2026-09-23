#!/usr/bin/env bash
set -euo pipefail

readonly reader_web_origin="https://prs-reader-web.dstoc.workers.dev"
readonly api_origin="https://prs-reader.dstoc.workers.dev"
readonly reader_web_api_path="/api/v1/authorization/reader"

check_asset() {
    local asset_path=$1
    local expected_type=$2
    local metadata status content_type

    if ! metadata=$(curl --fail-with-body --silent --show-error --max-time 20 \
        --output /dev/null --write-out '%{http_code} %{content_type}' \
        "$reader_web_origin/$asset_path"); then
        echo "browser asset request failed: $reader_web_origin/$asset_path" >&2
        return 1
    fi
    read -r status content_type <<<"$metadata"
    if [[ "$status" != "200" ]]; then
        echo "browser asset returned HTTP $status: $asset_path" >&2
        return 1
    fi
    case "$expected_type:$content_type" in
        html:text/html*|css:text/css*|wasm:application/wasm*|js:application/javascript*|js:text/javascript*) ;;
        *)
            echo "browser asset has unexpected Content-Type '$content_type': $asset_path" >&2
            return 1
            ;;
    esac
    printf 'verified %s (%s)\n' "$asset_path" "$content_type"
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

headers=$(mktemp)
trap 'rm -f -- "$headers"' EXIT
status=$(curl --silent --show-error --max-time 20 --request OPTIONS \
    --dump-header "$headers" --output /dev/null --write-out '%{http_code}' \
    "$api_origin$reader_web_api_path" \
    --header "Origin: $reader_web_origin" \
    --header 'Access-Control-Request-Method: POST' \
    --header 'Access-Control-Request-Headers: content-type')
if [[ "$status" != "204" ]]; then
    echo "reader API preflight returned HTTP $status for $reader_web_origin" >&2
    exit 1
fi

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

status=$(curl --silent --show-error --max-time 20 --request OPTIONS \
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
