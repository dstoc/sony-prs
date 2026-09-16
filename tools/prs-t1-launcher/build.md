# Build and deploy `prs-t1-launcher`

`prs-t1-launcher` is the small Android 2.2/API 8 Home replacement APK. It
requests root through the installed Superuser companion and launches the
native `prs-t1-agent`; it does not contain the native framebuffer UI.

## Host toolchain

The build uses the Android SDK command-line tools directly. Install or make
these commands available:

- A JDK providing `javac`, `jar`, and `keytool`.
- Android SDK platform API 8, specifically `platforms/android-8/android.jar`.
- Android build-tools containing `aapt2`, `d8`, `zipalign`, and `apksigner`.

The build script defaults to:

```text
SDK:         /home/agent/.cache/prs-t1-android-sdk/
Build tools: 35.0.0
```

No Gradle, Android Studio, Android NDK, or Rust toolchain is needed to build
the APK itself. Verify the required SDK files and host tools before building:

```sh
command -v javac jar keytool
test -f /home/agent/.cache/prs-t1-android-sdk/platforms/android-8/android.jar
test -x /home/agent/.cache/prs-t1-android-sdk/build-tools/35.0.0/aapt2
test -x /home/agent/.cache/prs-t1-android-sdk/build-tools/35.0.0/d8
test -x /home/agent/.cache/prs-t1-android-sdk/build-tools/35.0.0/zipalign
test -x /home/agent/.cache/prs-t1-android-sdk/build-tools/35.0.0/apksigner
```

If the SDK is installed elsewhere, set `PRST1_ANDROID_SDK`. A different
build-tools version can be selected with `PRST1_BUILD_TOOLS_VERSION`:

```sh
PRST1_ANDROID_SDK=/path/to/android-sdk \
PRST1_BUILD_TOOLS_VERSION=35.0.0 \
  ./tools/prs-t1-launcher/build.sh
```

## Build

From the repository root, run:

```sh
./tools/prs-t1-launcher/build.sh
```

The script compiles the Java activity against API 8, creates the dex file,
packages the manifest, aligns the APK, and signs it with a legacy v1
signature. v2/v3/v4 signatures are disabled for Android 2.2 compatibility.

The output is:

```text
target/prs-t1-launcher/prs-t1-native-launcher.apk
```

The output directory is regenerated on each build. The default debug keystore
is kept outside that directory at:

```text
/home/agent/.cache/prs-t1-android-sdk/prs-t1-native-launcher.debug.keystore
```

Set `PRST1_KEYSTORE` to use another stable keystore. Keeping the same key
allows `adb install -r` to upgrade an existing installation.

## Stage and install

Build the ARM agent first using [the agent build guide](../../crates/prs-t1-agent/build.md),
then restore Android if the native runtime is currently active:

```sh
./crates/prs-t1-agent/tools/native-test.sh reboot
adb wait-for-device
```

Copy the agent and its privileged handoff script, then install the APK:

```sh
adb push target/armv5te-unknown-linux-musleabi/release/prs-t1-agent \
  /data/local/tmp/prs-t1-agent
adb shell chmod 755 /data/local/tmp/prs-t1-agent

adb push tools/prs-t1-launcher/prs-t1-launch \
  /data/local/tmp/prs-t1-launch
adb shell chmod 755 /data/local/tmp/prs-t1-launch

adb install -r target/prs-t1-launcher/prs-t1-native-launcher.apk
```

The reader must have the rooted `com.noshufou.android.su` Superuser
companion installed and enabled. Press Home, choose **Native UI**, and approve
the Superuser request. The handoff log is available with:

```sh
adb shell 'cat /data/local/tmp/prs-t1-native-launch.log'
```

Do not make Native UI the permanent Home choice until the handoff has been
tested. Reboot or use the hardware reset button to return to Android after a
native-mode test. See [README.md](README.md) for the ownership model and
recovery notes.
