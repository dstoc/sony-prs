# PRS-T1 native UI agent

`prs-t1-agent` is the active native UI prototype for a rooted Sony PRS-T1.
It is a Rust ARM binary that talks directly to the T1 framebuffer, EPDC
refresh interface, Linux evdev devices, and legacy Android power interfaces.
The Android framework is not a runtime dependency of the native UI.

This crate has moved beyond hardware discovery: it can render and interact
with a small document-oriented shell after Android relinquishes display
ownership. It remains a development build that is manually launched through
ADB or the optional Android Home entry point. It does not install a boot hook
or replace the stock Android UI permanently.

## Current status

The current tested T1 path is:

| Area | State |
| --- | --- |
| Framebuffer discovery and capture | Implemented; `/dev/graphics/fb0` is queried using its visible geometry, stride, offsets, and RGB565 format. |
| EPDC refresh | Implemented for the recovered T1 ioctl ABI, including `DU`, `GC4`, `GC16`, and `A2` waveform probes. |
| Status and diagnostics | Implemented; battery, power, USB, Wi-Fi, ADB, Android processes, framebuffer, storage, and input state are displayed. |
| Native shell | Implemented as an opt-in standalone runtime with a status bar, paginated Markdown reading surface, details page, touch navigation, menu full refresh, and power actions. |
| Pixel damage | Implemented; complete logical frames are diffed and only the smallest changed rectangle is submitted. |
| Sleep and wake | Native EINK standby and wake-side power handoff have been exercised; USB/ADB rebind is deferred until the cable is detected after wake. |
| Android handoff | Implemented for manual ADB use and the tap-to-launch APK; no automatic startup integration. |
| Product-ready ownership | Not complete; the long-term display ownership boundary and recovery UX still need a deliberate design. |

The detailed device observations, command output, timings, and test history
are kept in [`docs/prs-t1/analysis.md`](../../docs/prs-t1/analysis.md). They
describe one rooted PRS-T1 and should not be generalized to every firmware
revision.

## Design goal and boundary

The near-term goal is a small native process that can own the visible T1 UI
for development and provide a document-reading surface without depending on
Android Java services. The current shell opens one configured development
Markdown document while making device state and recovery actions visible;
document browsing and broader asset support remain future work.

The reusable Markdown reader boundary is in
[`crates/prs-markdown`](../prs-markdown/) and its canonical design is in
[`docs/prs-t1/markdown-reader.md`](../../docs/prs-t1/markdown-reader.md).
The T1 agent supplies the reader's viewport and `embedded-graphics` target,
translates physical events into reader operations, and chooses when changed
pixels are sent to the EPDC. The home/reading page is rendered by the shared
`prs-markdown::EmbeddedGraphicsRenderer` into the existing packed RGB565
`DisplayCanvas`; there is no T1-only Markdown renderer. The T1 adapter owns
font loading, the development resource root, the content viewport below the
status bar, and the mapping from taps to link/page actions. It must not move
framebuffer, evdev, suspend, or refresh-policy code into the reusable crate.

The current control path is root ADB. Deploy test binaries to
`/data/local/tmp`, use read-only commands while Android is active, and use the
standalone path only as an explicit ownership experiment. The agent does not
modify boot or partition images.

## Commands

The binary accepts these commands. The default framebuffer is
`/dev/graphics/fb0`; pass another path as the first argument when inspecting a
different device.

| Command | Access | Description |
| --- | --- | --- |
| `probe [FRAMEBUFFER]` | Read-only | Prints framebuffer, Android-process, evdev, and status inventories. |
| `status` | Read-only | Prints battery, power, USB, Wi-Fi, ADB, uptime, screen, storage, and process state. Missing values are `unknown`. |
| `input` | Read-only | Queries evdev capabilities without opening an event stream, grabbing a device, or injecting events. |
| `events [EVENT_DEVICE] [SECONDS]` | Read-only | Logs a finite raw evdev stream. It defaults to `/dev/input/event1` for 10 seconds and never calls `EVIOCGRAB`. |
| `capture [FRAMEBUFFER]` | Read-only | Maps the visible RGB565 framebuffer with read access and emits an 8-bit grayscale PGM to stdout. |
| `render-test [FRAMEBUFFER] [SECONDS] [WAVEFORM] [WAIT\|NOWAIT]` | Writes framebuffer | Draws a centered 200x120 RGB565 marker, requests a bounded EPDC update, captures the mapping, waits, and restores the original rectangle. Defaults to 3 seconds, `GC16`, and `WAIT`. |
| `standalone-test [FRAMEBUFFER] [standby\|mem]` | Owns framebuffer/input | Runs the long-lived native shell after `zygote` and `system_server` have stopped. The default suspend mode is T1 EINK `standby`. |
| `launch-standalone [FRAMEBUFFER] [standby\|mem]` | Stops Android framework | Root `su` entry point. It detaches into a new session, stops zygote, waits for the framework to exit, and enters `standalone-test`. |

Only `render-test`, `standalone-test`, and `launch-standalone` mutate device
state. `render-test` is bounded and restores the bytes it changes, but it still
requires a reader-side recovery route and physical observation of the panel.

## Build and deploy

The T1 expects an ARMv5TE soft-float executable. The release build is statically
linked against musl so it does not depend on Android's old dynamic linker.
Follow [`build.md`](build.md) for the host toolchain, cross-build, artifact
checks, ADB copy, and detached test helper.

After building, the basic deployment is:

```sh
adb wait-for-device
adb push target/armv5te-unknown-linux-musleabi/release/prs-t1-agent \
  /data/local/tmp/prs-t1-agent
adb shell chmod 755 /data/local/tmp/prs-t1-agent
```

Exercise the read-only path while Android remains active:

```sh
adb shell /data/local/tmp/prs-t1-agent probe
adb shell /data/local/tmp/prs-t1-agent status
adb shell /data/local/tmp/prs-t1-agent capture > /data/local/tmp/t1-screen.pgm
adb pull /data/local/tmp/t1-screen.pgm ./t1-screen.pgm
```

The old T1 ADB daemon closes the `exec-out` channel and `adb shell` can
translate binary output through a PTY. Redirect the PGM on the reader and pull
it as shown above.

For a manual native ownership test, the helper pushes the binary, stops the
framework, launches a detached process, reports status, and provides the
reboot recovery command:

```sh
crates/prs-t1-agent/tools/native-test.sh start
crates/prs-t1-agent/tools/native-test.sh status
crates/prs-t1-agent/tools/native-test.sh reboot
```

`start` defaults to the release artifact above. Set `PRS_T1_AGENT_BINARY` for
another binary, `PRS_T1_FRAMEBUFFER` for another framebuffer path, or
`PRS_T1_SUSPEND_MODE=mem` to compare Android's normal early-suspend path. The
script is intentionally not an automatic startup mechanism.

## Native shell

The shell renders a 48-pixel black status bar with white 20x20 binary sprites
and compact text. It reports battery level, active USB/Wi-Fi/ADB indicators,
exceptional mode such as sleeping or rebooting, and the local time. Below the
bar, the home page renders the current paginated Markdown document. The page
viewport starts at y=76, leaving a small separation below the bar and a bottom
margin for the reader.

### Development Markdown document

The first integration deliberately opens one configured file rather than
providing a document browser. Stage the checked-in smoke-test document before
starting the native runtime:

```sh
adb push docs/prs-t1/development.md /mnt/sdcard/index.md
```

The default document is `/mnt/sdcard/index.md`, which avoids requiring a
separate staging directory on the development device. The document root and
file can still be overridden for another layout.

The default configuration is:

| Environment variable | Default | Purpose |
| --- | --- | --- |
| `PRS_T1_DOCUMENT_ROOT` | `/mnt/sdcard` | Root for Markdown and relative resources. |
| `PRS_T1_DOCUMENT` | `index.md` | Root-relative document opened at startup. |
| `PRS_T1_FONT` | `/system/fonts/DroidSans.ttf` | Required regular TrueType face. |
| `PRS_T1_FONT_BOLD` | regular face | Optional bold face. |
| `PRS_T1_FONT_ITALIC` | regular face | Optional italic face. |
| `PRS_T1_FONT_BOLD_ITALIC` | regular face | Optional bold-italic face. |
| `PRS_T1_FONT_MONOSPACE` | `/system/fonts/DroidSansMono.ttf` | Optional code face; falls back to regular if absent. |

The four optional proportional faces fall back to the regular font bytes when
their configured files are unavailable. To read another staged document from
the helper workflow, set `PRS_T1_DOCUMENT_ROOT` and `PRS_T1_DOCUMENT` in the
environment used to launch the agent. Relative links are resolved by the
shared `FileSystemResourceProvider` inside that root.

The reader handles link activation through `prs-markdown` first. A tap on an
otherwise empty page area advances on the right half and goes back on the left
half. The page coordinate is translated from whole-screen input by removing the
76-pixel status/chrome offset before the shared reader performs hit testing.
Internal links support anchors and root-relative Markdown files, including
`#anchor`, `other.md`, and `other.md#anchor`. External URLs are reported by the
shared reader and remain an application concern: the T1 runtime displays the
activated URL in a bottom-of-screen notice and does not launch a browser.

On the Home reading surface, the hardware left and right keys (`KEY_LEFT` code
105 and `KEY_RIGHT` code 106) perform one previous/next reader-page operation
per physical press. Repeat events are ignored. A short physical menu press
performs the reader's Back operation, restoring the previous document/anchor and
page after an internal link. A menu hold of at least one second retains its
existing full GC16 redraw behavior. Page controls and reader Back are ignored
while Details / Settings is open, so they cannot trigger document navigation
from the power/settings UI.

Tap the status bar to open **Details / Settings**. The details page groups the
live snapshot under:

- **Power** — battery state, temperature, voltage, AC, USB, and supported power states.
- **Connectivity** — Wi-Fi interface/link/supplicant state, USB gadget state, and ADB.
- **System** — uptime, framebuffer state/rotation, Android process state, and wake lock.
- **Storage** — available space on `/data` and `/mnt/sdcard`.
- **Input** — the latest touch, key, and power events and their coordinates/counts.

The page also provides full-width **Reboot**, **Power off**, and **Back to
reading** targets. Touch release is accepted from the event shapes observed on
the T1: `BTN_TOUCH=0`, `ABS_MT_TRACKING_ID=-1`, or
`ABS_MT_TOUCH_MAJOR=0`, committed by `SYN_REPORT`. The legacy `ABS_X/Y` path is
normalized from the panel's advertised 800x600 axes before hit testing.

The physical menu button is event0 code 357 (`Unknown` in the old kernel). A
hold of at least one second requests a full GC16 redraw with the EPDC's
`UPDATE_MODE_FULL` flag. Short menu presses only update diagnostics. A short
power press sleeps; a press of at least two seconds requests reboot.

## Display and refresh model

The T1 exposes a 600x800 visible RGB565 framebuffer with a 1216-byte stride
and a larger virtual buffer. The runtime therefore never assumes that the
visible image is tightly packed in the mapped framebuffer.

Each logical screen is first rendered into an owned, tightly packed RGB565
frame. After a completed full-screen update, the runtime keeps a shadow frame,
compares both bytes of every visible pixel, and submits the smallest enclosing
changed rectangle. Identical frames are skipped. A semantic region remains a
waveform hint and a fallback; a change outside that hint is promoted to GC16
rather than being silently omitted. One update marker is tracked so an
asynchronous transient update completes before the next mapped-frame write.

Transient touch, key, and power updates use the fast `DU` waveform and are
submitted asynchronously. Initial, status, and full redraws use `GC16` and
wait for completion. The T1's EPDC alignment quantum has not been measured, so
damage defaults to exact one-pixel alignment. Repeated DU ghosting and the
right cadence for GC16 cleanup still need physical visual characterization.

## Power and Android ownership

`standalone-test` fails closed if either `zygote` or `system_server` is still
running. This is important because the current framebuffer and all five input
devices were observed under `system_server`; stopping zygote is a broad
framework shutdown, not a precise display-owner switch. The native loop then:

1. acquires the legacy kernel wake lock;
2. renders the native screen and reads event0/event1/event2/event4;
3. on a short power press, renders a standby screen and requests sleep;
4. waits for `/sys/power/wait_for_fb_wake`, handles the wake-side power event,
   reacquires the lock, refreshes framebuffer metadata, and redraws; and
5. on a long power press, calls `/system/bin/reboot`.

The default `standby` mode uses the T1 EINK path. `mem` is retained as a
comparison mode for Android's normal early-suspend behavior. The native EINK
sleep/wake sequence has been exercised with USB disconnected; the old USB ADB
transport can require a reconnect after wake. When persistent ADB is enabled,
the runtime defers `adbd` restart until the USB power node reports the cable is
back.

The T1 vendor library exports Sony's `set_screen_state(int)` function. When
`/data/local/tmp/prs-t1-power-state` is present, the runtime uses it for
`standby`, `mem`, and the wake-side `on` handoff; otherwise it falls back to
`/sys/power/state`. The Android 2.2 compatibility helper is built and staged
with:

```sh
adb pull /system/lib/libdl.so /tmp/prs-t1-libdl.so
./crates/prs-t1-agent/tools/build-power-state-helper.sh \
  /tmp/prs-t1-libdl.so target/prs-t1-power-state
adb push target/prs-t1-power-state /data/local/tmp/prs-t1-power-state
adb shell chmod 755 /data/local/tmp/prs-t1-power-state
```

The optional [T1 launcher](../../tools/prs-t1-launcher/README.md) adds a
small Android 2.2/API 8 Home activity labelled **Native UI**. It invokes the
root handoff when selected from the Home resolver; it does not contain the
native renderer and does not make the native UI persistent.

## Safety and recovery

Run `probe`, `status`, `input`, `events`, and `capture` first. Keep the stock
Android recovery path available before using a write-capable command. Do not
stop zygote casually: Android services, input dispatch, Wi-Fi management, and
the normal settings UI can disappear. Do not make **Native UI** the permanent
Home choice until the handoff has been tested.

After a zygote-isolated test, use:

```sh
crates/prs-t1-agent/tools/native-test.sh reboot
```

If ADB or the native process does not return, use the reader's hardware reset
button. A normal reboot is the supported way to restore zygote,
`system_server`, `dispd`, and Android's normal UI.

## Next goals

The next work should stay focused on making the custom UI useful while keeping
the recovery path explicit:

1. Decide how the native UI should obtain durable display ownership without
   depending on a broad zygote shutdown.
2. Characterize repeated DU updates on the physical panel and choose a GC16
   cleanup policy.
3. Improve the standby image and USB/ADB recovery behavior across suspend.
4. Expand the configured reading surface with document browsing and image
   content after the ownership and refresh policy are reliable.
5. Only then evaluate a persistent startup integration.

The older discovery procedure and full evidence ledger remain available in
[`docs/prs-t1/analysis.md`](../../docs/prs-t1/analysis.md); this README is the
operational description of the current crate rather than a staged plan.
