# PRS-T1 native UI launcher

This is a deliberately small Android 2.2/API 8 Home replacement entry point.
It is not the native UI itself. The Home Activity invokes a narrowly scoped
`su` request when selected from Android's Home-app resolver. The restored
Superuser companion approves that request; the root handoff script then
detaches from the Activity's streams and invokes the native agent's privileged
handoff mode. The agent creates a new session before stopping the Android
framework, then enters the native runtime.

The handoff script and ARM binary both live in `/data/local/tmp`; the `nosuid`
mount option on `/data` does not prevent an already-root `su` process from
executing them. The agent must create a new session before stopping zygote:
Android tears down the APK's inherited zygote process group, so a shell
background job is not sufficient. `prs-t1-agent` owns the framebuffer and
input devices after zygote stops. Reboot or hardware reset is the supported
return path to Android.

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
The signing keystore is kept outside that cleaned output directory at
`$PRST1_ANDROID_SDK/prs-t1-native-launcher.debug.keystore`; set
`PRST1_KEYSTORE` to use a different stable keystore. Keeping the same key
allows `adb install -r` upgrades.
The build uses the legacy v1 APK signature scheme for compatibility with
Android 2.2; newer v2/v3/v4 schemes are disabled.

## Stage for a manual test

First restore normal Android if the native test is currently running:

```sh
./crates/prs-t1-agent/tools/native-test.sh reboot
```

Then push the agent and handoff script, and install the APK. The
`com.noshufou.android.su` Superuser companion must already be installed and
enabled:

```sh
adb push target/armv5te-unknown-linux-musleabi/release/prs-t1-agent /data/local/tmp/prs-t1-agent
adb shell chmod 755 /data/local/tmp/prs-t1-agent
adb push tools/prs-t1-launcher/prs-t1-launch /data/local/tmp/prs-t1-launch
adb shell chmod 755 /data/local/tmp/prs-t1-launch
adb install -r target/prs-t1-launcher/prs-t1-native-launcher.apk
```

Press the Home button. Android should offer “Native UI” alongside the stock
Home choices. Selecting it starts the native runtime; do not mark it as the
permanent default until the handoff has been tested. Approve the Superuser
request. The handoff log is:

```sh
adb shell 'cat /data/local/tmp/prs-t1-native-launch.log'
```

If the Home resolver does not update immediately, reboot Android once and press
Home again.
