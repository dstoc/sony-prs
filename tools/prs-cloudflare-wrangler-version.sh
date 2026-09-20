#!/usr/bin/env bash

# Keep the resource-management and Worker-publishing commands on the Wrangler
# version whose --no-x-provision behavior is covered by CI.
PRS_WRANGLER_VERSION="4.135.0"

require_prs_wrangler() {
    if ! command -v wrangler >/dev/null 2>&1; then
        echo "wrangler $PRS_WRANGLER_VERSION is required; install it before continuing" >&2
        return 1
    fi

    local version_output
    version_output=$(wrangler --version 2>&1) || {
        echo "could not read the installed Wrangler version" >&2
        return 1
    }
    local version_pattern
    version_pattern="(^|[^0-9])${PRS_WRANGLER_VERSION//./\\.}([^0-9]|$)"
    if ! grep -Eq "$version_pattern" <<<"$version_output"; then
        echo "Wrangler $PRS_WRANGLER_VERSION is required; found: $version_output" >&2
        return 1
    fi
}
