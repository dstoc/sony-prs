# PRS-T1 native UI launcher

This is a deliberately small Android 2.2/API 8 launcher entry point. It is
not the native UI itself. The launcher Activity invokes `su` on tap, starts the
detached root handoff script, and exits before the Android framework is
stopped.

The handoff script expects the ARM binary at
`/data/local/tmp/prs-t1-agent`. It starts that binary after stopping zygote;
`prs-t1-agent` then owns the framebuffer and input devices. Reboot or hardware
reset is the supported return path to Android.

## Build

The host needs a JDK, Android `android.jar` for API 8, and modern Android
build-tools containing `aapt2`, `d8`, `zipalign`, and `apksigner`. The repository build
script defaults to:

```text
/home/agent/.cache/prs-t1-android-sdk/
```

Override it with `PRST1_ANDROID_SDK` if the SDK is elsewhere, then run:

```sh
./tools/prs-t1-launcher/build.sh
```

The signed APK is written under the ignored workspace `target/` directory.
The build uses the legacy v1 APK signature scheme for compatibility with
Android 2.2; newer v2/v3/v4 schemes are disabled.

## Stage for a manual test

First restore normal Android if the native test is currently running:

```sh
./crates/prs-t1-agent/tools/native-test.sh reboot
```

Then push the agent and handoff script, install the APK, and add the “Native
UI” application to the Home screen:

```sh
adb push target/armv5te-unknown-linux-musleabi/release/prs-t1-agent /data/local/tmp/prs-t1-agent
adb shell chmod 755 /data/local/tmp/prs-t1-agent
adb push tools/prs-t1-launcher/prs-t1-launch /data/local/tmp/prs-t1-launch
adb shell chmod 755 /data/local/tmp/prs-t1-launch
adb install -r target/prs-t1-launcher/prs-t1-native-launcher.apk
```

Tap the launcher icon once. The handoff log is:

```sh
adb shell 'cat /data/local/tmp/prs-t1-native-launch.log'
```

If the application list does not update immediately, reboot Android once or
use the launcher's application list and drag “Native UI” to a Home page.
