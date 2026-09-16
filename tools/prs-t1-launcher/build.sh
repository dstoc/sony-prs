#!/bin/sh

set -eu

PROJECT_ROOT=$(CDPATH= cd -- "$(dirname "$0")" && pwd)
SDK_ROOT=${PRST1_ANDROID_SDK:-/home/agent/.cache/prs-t1-android-sdk}
BUILD_TOOLS_VERSION=${PRST1_BUILD_TOOLS_VERSION:-35.0.0}
BUILD_TOOLS="$SDK_ROOT/build-tools/$BUILD_TOOLS_VERSION"
ANDROID_JAR="$SDK_ROOT/platforms/android-8/android.jar"
OUT_DIR=${PRST1_LAUNCHER_OUT:-$PROJECT_ROOT/../../target/prs-t1-launcher}

JAVAC=${JAVAC:-javac}
JAR=${JAR:-jar}
AAPT2=${AAPT2:-$BUILD_TOOLS/aapt2}
D8=${D8:-$BUILD_TOOLS/d8}
ZIPALIGN=${ZIPALIGN:-$BUILD_TOOLS/zipalign}
APKSIGNER=${APKSIGNER:-$BUILD_TOOLS/apksigner}
KEYTOOL=${KEYTOOL:-keytool}

require_file() {
    if [ ! -f "$1" ]; then
        echo "required file not found: $1" >&2
        exit 1
    fi
}

require_executable() {
    if [ ! -x "$1" ]; then
        echo "required executable not found: $1" >&2
        exit 1
    fi
}

require_file "$ANDROID_JAR"
require_executable "$AAPT2"
require_executable "$D8"
require_executable "$ZIPALIGN"
require_executable "$APKSIGNER"

rm -rf "$OUT_DIR"
mkdir -p "$OUT_DIR/classes" "$OUT_DIR/dex"

"$JAVAC" --release 8 -classpath "$ANDROID_JAR" \
    -d "$OUT_DIR/classes" \
    "$PROJECT_ROOT/src/org/prs/t1/nativeui/LauncherActivity.java"

"$JAR" cf "$OUT_DIR/classes.jar" -C "$OUT_DIR/classes" .
"$D8" --lib "$ANDROID_JAR" --min-api 8 --output "$OUT_DIR/dex" "$OUT_DIR/classes.jar"

"$AAPT2" link \
    --manifest "$PROJECT_ROOT/AndroidManifest.xml" \
    -I "$ANDROID_JAR" \
    -o "$OUT_DIR/unsigned.apk"

"$JAR" uf "$OUT_DIR/unsigned.apk" -C "$OUT_DIR/dex" classes.dex

if [ ! -f "$OUT_DIR/debug.keystore" ]; then
    "$KEYTOOL" -genkeypair \
        -keystore "$OUT_DIR/debug.keystore" \
        -storepass android \
        -keypass android \
        -alias androiddebugkey \
        -keyalg RSA \
        -keysize 2048 \
        -validity 10000 \
        -dname 'CN=Android Debug,O=Android,C=US'
fi

"$ZIPALIGN" -f 4 "$OUT_DIR/unsigned.apk" "$OUT_DIR/aligned.apk"

"$APKSIGNER" sign \
    --ks "$OUT_DIR/debug.keystore" \
    --ks-key-alias androiddebugkey \
    --ks-pass pass:android \
    --key-pass pass:android \
    --min-sdk-version 8 \
    --v1-signing-enabled true \
    --v2-signing-enabled false \
    --v3-signing-enabled false \
    --v4-signing-enabled false \
    --out "$OUT_DIR/prs-t1-native-launcher.apk" \
    "$OUT_DIR/aligned.apk"

"$ZIPALIGN" -c -v 4 "$OUT_DIR/prs-t1-native-launcher.apk"
"$APKSIGNER" verify --verbose "$OUT_DIR/prs-t1-native-launcher.apk"
echo "built $OUT_DIR/prs-t1-native-launcher.apk"
