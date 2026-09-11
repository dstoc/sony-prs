#!/bin/sh
set -eu

usage() {
    echo "usage: $0 PACKAGE_DIR MOUNTPOINT" >&2
    echo "  MOUNTPOINT must already be the mounted PRS-350 READER volume" >&2
}

if [ "$#" -ne 2 ]; then
    usage
    exit 2
fi

PACKAGE_DIR=$1
MOUNTPOINT=$2
MODEL_MARKER=VsKg2WclV00Ohtwe25PhUgcyAn8K4F0h.eiy1Bm5I4HxPW04WdksJQ5DtYMFfetrB
SOURCE="$PACKAGE_DIR/PRS-350 Updater.package"
TARGET="$MOUNTPOINT/PRS-350 SP Updater.package"

if [ ! -f "$SOURCE" ]; then
    echo "error: missing package: $SOURCE" >&2
    exit 1
fi
if [ ! -d "$MOUNTPOINT" ]; then
    echo "error: mountpoint does not exist: $MOUNTPOINT" >&2
    exit 1
fi

cp "$SOURCE" "$TARGET"
touch "$MOUNTPOINT/$MODEL_MARKER"
sync

echo "staged: $TARGET"
sha256sum "$SOURCE" "$TARGET"
echo "marker: $MOUNTPOINT/$MODEL_MARKER"
