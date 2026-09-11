#!/bin/sh
set -eu

usage() {
    echo "usage: $0 UPDATE_TOOLS_DIR SHADOW_FILE ARM_BINARY OUTPUT_DIR" >&2
    echo "  UPDATE_TOOLS_DIR must contain create_update.sh, update_test.sh, Info.img" >&2
    echo "  set PRS350_OPENSSL_WRAPPER_DIR if the host needs the legacy OpenSSL wrapper" >&2
}

if [ "$#" -ne 4 ]; then
    usage
    exit 2
fi

ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
UPDATE_TOOLS=$1
SHADOW=$2
ARM_BINARY=$3
OUTPUT=$4
WRAPPER_DIR=\${PRS350_OPENSSL_WRAPPER_DIR:-}

for file in create_update.sh update_test.sh Info.img; do
    if [ ! -f "$UPDATE_TOOLS/$file" ]; then
        echo "error: missing $UPDATE_TOOLS/$file" >&2
        exit 1
    fi
done
if [ ! -f "$SHADOW" ]; then
    echo "error: missing shadow file: $SHADOW" >&2
    exit 1
fi
if [ ! -x "$ARM_BINARY" ]; then
    echo "error: ARM binary is not executable: $ARM_BINARY" >&2
    exit 1
fi
if [ -e "$OUTPUT" ]; then
    echo "error: output directory already exists; choose a fresh directory" >&2
    exit 1
fi

mkdir -p "$OUTPUT"
cp "$ROOT/tools/prs350-serial-gadget.sh" "$OUTPUT/serial-gadget-protocol.sh"
cp "$ROOT/tools/prs350-serial-service.sh" "$OUTPUT/serial-service.sh"
cp "$ROOT/tools/prs350-update.sh" "$OUTPUT/update.sh"
cp "$ARM_BINARY" "$OUTPUT/prs350-agent"
cp "$SHADOW" "$OUTPUT/shadow"
chmod 755 "$OUTPUT/serial-gadget-protocol.sh" \
    "$OUTPUT/serial-service.sh" "$OUTPUT/update.sh" "$OUTPUT/prs350-agent"

cleanup() {
    if [ -f /tmp/sigKeyPriv.pem ]; then
        truncate -s 0 /tmp/sigKeyPriv.pem
    fi
}
trap cleanup EXIT HUP INT TERM

if [ -n "$WRAPPER_DIR" ]; then
    PATH="$WRAPPER_DIR:$PATH"
    export PATH
fi

(
    cd "$UPDATE_TOOLS"
    ./create_update.sh "$OUTPUT" >"$OUTPUT/create.log" 2>&1
    ./update_test.sh "$OUTPUT" >"$OUTPUT/update-test.log" 2>&1
)

sha256sum "$OUTPUT/PRS-350 Updater.package"
echo "package directory: $OUTPUT"
