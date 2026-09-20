# PRS-T1 native UI agent

`prs-t1-agent` is the development native application for a rooted Sony PRS-T1.
It is a Rust ARM binary that talks directly to the T1 framebuffer, EPDC
refresh interface, Linux evdev devices, and legacy Android power interfaces.
The Android framework is not a runtime dependency of the native UI.

This crate is the device-side integration for the shared `prs-markdown`
reader. After Android relinquishes display ownership, it renders and interacts
with a paginated Markdown document, status bar, details/settings page, touch
links, hardware page controls, and power actions. It remains a development
build that is manually launched through ADB or the optional Android Home entry
point. It does not install a boot hook or replace the stock Android UI.

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
| Persistent/product ownership | Not implemented; the standalone path is a manual development handoff that stops Android framework services and retains an explicit reboot recovery path. |

The detailed device observations, command output, timings, and test history
are kept in [`docs/prs-t1/analysis.md`](../../docs/prs-t1/analysis.md). They
describe one rooted PRS-T1 and should not be generalized to every firmware
revision.

## Integration boundary

The current native process owns the visible T1 UI for development without
depending on Android Java services. It opens one configured root-relative
Markdown document at startup; the shared reader can then follow staged local
Markdown documents and anchors, while a document browser is not part of this
application. Device state and recovery actions remain visible alongside the
reading surface.

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
| `network-probe HOSTNAME_OR_HTTPS_URL [--invalid-hostname]` | Read-only | Resolves the supplied host and performs a bounded HTTPS `GET /health` with rustls certificate and hostname validation. The optional negative test must fail hostname validation. |
| `wifi-up` | Controls Wi-Fi | Loads the legacy driver, starts the supplicant, waits for WPA `COMPLETED`, starts DHCP, and waits for `dhcp.wlan0.result=BOUND`. |
| `wifi-down` | Controls Wi-Fi | Stops `dhcpcd`, stops the supplicant, and unloads the Wi-Fi driver in that order. |
| `wifi-probe HOSTNAME_OR_HTTPS_URL [--invalid-hostname]` | Controls Wi-Fi and network | Runs `wifi-up`, runs the HTTPS network probe, and always attempts `wifi-down`. |
| `render-test [FRAMEBUFFER] [SECONDS] [WAVEFORM] [WAIT\|NOWAIT]` | Writes framebuffer | Draws a centered 200x120 RGB565 marker, requests a bounded EPDC update, captures the mapping, waits, and restores the original rectangle. Defaults to 3 seconds, `GC16`, and `WAIT`. |
| `display-test [FRAMEBUFFER] [SECONDS] [WAVEFORM]` | Writes framebuffer | Draws a full-screen grayscale calibration pattern with fill/text swatches, gradients, a grayscale ramp, and 1-, 2-, 4-, and 8-pixel lines. Defaults to 60 seconds and `GC16`; it leaves the pattern visible and does not restore the previous framebuffer. |
| `standalone-test [FRAMEBUFFER] [standby\|mem]` | Owns framebuffer/input | Runs the long-lived native shell after `zygote` and `system_server` have stopped. The default suspend mode is T1 EINK `standby`. |
| `launch-standalone [FRAMEBUFFER] [standby\|mem]` | Stops Android framework | Root `su` entry point. It detaches into a new session, stops zygote, waits for the framework to exit, and enters `standalone-test`. |

Only `render-test`, `display-test`, `standalone-test`, `launch-standalone`,
`wifi-up`, `wifi-down`, and `wifi-probe` mutate device state. `render-test` is bounded and restores the bytes it changes,
but it still requires a reader-side recovery route and physical observation of
the panel. `display-test` is bounded but leaves its full-screen pattern visible
when it exits. Use `capture` during its wait to record the framebuffer and use a
physical camera to compare the panel output. The command does not stop Android
display services, so another display owner can repaint the screen.

## Build and deploy

The T1 expects an ARMv7-A/Cortex-A8 soft-float executable. The release build is statically
linked against musl so it does not depend on Android's old dynamic linker.
Follow [`build.md`](build.md) for the host toolchain, cross-build, artifact
checks, ADB copy, and detached test helper.

After building, the basic deployment is:

```sh
adb wait-for-device
adb push target/armv7-unknown-linux-musleabi/release/prs-t1-agent \
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

### Network and TLS capability probe

`network-probe` is the read-only artifact for physical validation before the
full PRSync client exists. It does not enable Wi-Fi, read Wi-Fi credentials,
send authorization data, or persist any data.

### Wi-Fi lifecycle probe

The lifecycle commands use the T1's existing
`/data/misc/wifi/wpa_supplicant.conf`. They do not create, read, print, or
persist Wi-Fi credentials. They require the separately built
`/data/local/tmp/prs-t1-wifi-helper` Bionic shim described in
[`build.md`](build.md).

`wifi-up` prints structured snapshots before startup and after DHCP reaches
`BOUND`. During startup it prints each lifecycle stage, the WPA association
state, the DHCP result, and the explicit 60-second association and 30-second
DHCP limits. `wifi-down` is explicit and runs the required shutdown sequence:
stop `dhcpcd`, stop the supplicant, then unload the driver. It attempts all
three steps even when one step fails.

Use `wifi-probe` for the complete physical sequence. It brings Wi-Fi up,
performs the existing HTTPS `/health` probe, and shuts Wi-Fi down after both
successful and failed bring-up or network operations:

```sh
PROBE_HOST='your-production-host.example'
adb shell /data/local/tmp/prs-t1-agent wifi-probe "$PROBE_HOST"
adb shell /data/local/tmp/prs-t1-agent status
```

The command reports `wifi.snapshot=before`, `wifi.snapshot=ready`, and
`wifi.snapshot=after` fields for the interface, carrier, WPA association, and
DHCP state. It is an explicit command. Waking the reader does not start
Wi-Fi.

The probe resolves the supplied hostname, pins the request to the resolved
addresses, and performs one HTTPS `GET /health`. It uses reqwest's async
client with rustls,
bundles Mozilla public roots for the static T1 target, rejects redirects, and
keeps certificate-chain and hostname validation enabled. DNS lookup uses
Hickory's async resolver on a current-thread Tokio runtime. The lookup has a
10-second result limit and adds no worker thread to the T1 binary. The probe
uses a 10-second connect limit, a 20-second request and response-read limit,
and a 64 KiB response-body limit. A DNS timeout reports
`failure_stage=dns` and `failure_kind=resolution_timeout`; it does not
continue to the HTTPS request.
The command prints one-line fields such as `dns_addresses`,
`tls_validation`, `http_status`, and `result`.

### TLS entropy prerequisite

The probe selects rustls' `ring` provider and checks a 32-byte secure entropy
request before it performs DNS or HTTPS work. The checked-in
`armv7-unknown-linux-musleabi` build uses ring's operating-system random source.
On Linux-musl, its `getrandom` implementation uses the `getrandom` system call
when available. On older kernels, it waits for `/dev/random` to report that the
kernel pool is initialized, then reads cryptographic bytes from `/dev/urandom`.
It does not use a fixed, time-based, process-based, or device-identity seed.

The rooted PRS-T1 must expose readable `/dev/random` and `/dev/urandom` nodes.
Run the probe after normal Android boot has completed. If secure entropy is not
available, it fails before DNS with
`failure_stage=tls failure_kind=entropy_unavailable` and a non-zero exit status.
Do not replace this failure with a deterministic seed or with bytes from a
predictable device property. Preserve the complete output for the hardware
record and retry only after the device's kernel random source is ready.

Set the production protocol hostname selected for #99. A hostname is enough;
the command adds `https://` and requests `/health`.

```sh
PROBE_HOST='your-production-host.example'
adb shell /data/local/tmp/prs-t1-agent network-probe "$PROBE_HOST"
```

The command exits with status 0 only after DNS, TLS validation, an HTTP 2xx
response, and the bounded response read succeed. Network loss, DNS failure or
timeout, TLS failures, non-2xx responses, and oversized responses produce
structured failure fields and a non-zero exit status. Resources are owned by
the single command and are released when it exits.

Run the safe negative test after a normal probe. It routes the reserved invalid
hostname to the already-resolved production addresses, so the TLS server name
and hostname check are wrong while the TCP destination remains reachable. The
request must fail because the certificate is not valid for the tested
hostname. The command reports `tls_validation=failed_as_expected` and exits 0
only for that rustls hostname-mismatch result. Connection failure, timeout,
DNS failure, and other certificate errors such as expiry or an unknown issuer
produce a non-zero exit status.

```sh
adb shell /data/local/tmp/prs-t1-agent network-probe "$PROBE_HOST" \
  --invalid-hostname
```

Do not use an IP address as a substitute for the production hostname in the
normal test. The production hostname is required for SNI and certificate
hostname validation. Save the complete stdout and the command exit status in
the #99 hardware test record.

To run the display calibration pattern, keep Android active for a bounded
framebuffer probe:

```sh
# Shell 1: keep the pattern on the panel for 60 seconds.
adb shell /data/local/tmp/prs-t1-agent display-test /dev/graphics/fb0 60 GC16
# Shell 2: capture the framebuffer during that wait.
adb shell '/data/local/tmp/prs-t1-agent capture > /data/local/tmp/display-test.pgm'
adb pull /data/local/tmp/display-test.pgm ./display-test.pgm
```

The pattern remains in the framebuffer after `display-test` exits. A running
Android display service can repaint it, so capture the screen during the wait.
The command does not restore the previous framebuffer contents.

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

Tap the status bar to open Details / Settings. Tap Display test to show the
full-screen grayscale calibration pattern. Press MENU briefly to return to
Details / Settings. A long MENU press still requests a full EPDC redraw.

### Development Markdown document

The integration opens one configured file rather than providing a document
browser. Stage the checked-in smoke-test document before starting the native
runtime:

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
| `PRS_T1_FONT_MONOSPACE` | `/system/fonts/HelveticaMonospacedW1G-Rg.otf` | Optional regular code face; falls back to the regular proportional face if absent. |
| `PRS_T1_FONT_MONOSPACE_BOLD` | `/system/fonts/HelveticaMonospacedW1G-Bd.otf` | Optional bold code face; falls back to the regular monospace face if absent. |
| `PRS_T1_FONT_MONOSPACE_ITALIC` | `/system/fonts/HelveticaMonospacedW1G-It.otf` | Optional italic code face; falls back to the regular monospace face if absent. |
| `PRS_T1_FONT_MONOSPACE_BOLD_ITALIC` | `/system/fonts/HelveticaMonospacedW1G-BdIt.otf` | Optional bold-italic code face; falls back to the regular monospace face if absent. |

The three optional proportional faces fall back to the regular proportional
font bytes when their configured files are unavailable. The optional regular
monospace face keeps the existing fallback to the regular proportional font.
Each optional monospace style face falls back to the loaded regular monospace
bytes. The default T1 monospace family is the complete
`HelveticaMonospacedW1G` family under `/system/fonts`. To read another staged
document from the helper workflow, set
`PRS_T1_DOCUMENT_ROOT` and `PRS_T1_DOCUMENT` in the environment used to launch
the agent. Relative links are resolved by the shared
`FileSystemResourceProvider` inside that root; the provider rejects references
that escape the configured root.

For the real-device font check, stage
[`docs/prs-t1/monospace-font-validation.md`](../../docs/prs-t1/monospace-font-validation.md)
as the startup document. The validation case uses syntax-highlighted Rust
code and records the face, alignment, and visual-quality checks needed on a
PRS-T1.

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

The physical menu button is event0 code 357 (`Unknown` in the old kernel). On
the Home reading surface, a short press invokes reader Back and a hold of at
least one second requests a full GC16 redraw with the EPDC's
`UPDATE_MODE_FULL` flag. On Details / Settings, menu navigation does not invoke
reader Back. A short power press sleeps; a press of at least two seconds
requests reboot.

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

The device-side refresh policy is intentionally separate from the Markdown
crate. It classifies a rendered page's display list as monochrome or grayscale
by looking for loaded images, non-black/white fills and borders, or non-extreme
syntax/text ink. The policy is:

| Event | Region | Waveform | Completion | EPDC mode / cadence |
| --- | --- | --- | --- | --- |
| Initial screen, menu full redraw, details/settings entry or exit | Full screen | `GC16` | Wait | Forced full update |
| Monochrome text page turn | Document region | `DU` | Async | Damage rectangle; the fifth turn is a forced `GC16` cleanup |
| Grayscale/image page turn | Document region | `GC16` | Wait | Forced quality update for the document region |
| Status change while reading | Status bar only | `DU` | Async | Damage stays out of the document region |
| Status/diagnostic change on details | Details dirty region | `DU` | Async | Damage-driven partial update |
| Transient link/interaction feedback | Document region | `DU` | Async | Damage-driven partial update |
| Resume after suspend | Full screen | `GC16` | Wait | Forced full update after the panel's wake-side clear |

The cleanup boundary is four completed fast page turns: the first four
ordinary text turns can remain responsive, and the next text turn waits for and
forces a GC16 update over the document region. Any grayscale/image page, full
redraw, or resume resets that cadence. Status and transient updates do not
consume the page-turn budget. A failed submission does not advance the policy
state, and a page event changes the shared reader's logical page before this
policy is consulted; retrying or promoting its display update therefore never
advances pagination a second time.

The T1's EPDC alignment quantum has not been measured, so damage defaults to
exact one-pixel alignment. The available waveform timings were measured on the
tested reader: DU is about 273--381 ms, GC4 about 614 ms, and GC16 about
700 ms. Those runs established acceptance and latency, not optical ghosting;
the policy's four-turn cleanup cadence is a conservative starting point that
must be revisited after a repeated-turn visual inspection on hardware. A
GC16 cleanup is always preferred before or after content that intentionally
uses grayscale rather than assuming that the fastest accepted waveform is
visually adequate.


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

## Known limitations

- Startup opens one configured document; there is no file browser or persistent
  library UI. Local Markdown links and anchors can still navigate within the
  configured resource root.
- External URLs produce a short T1-owned bottom notice and are not opened in a
  browser. Task-list markers are not interactive. Specialist Markdown uses the
  fallbacks in the [support matrix](../../docs/prs-t1/markdown-reader.md#markdown-support-matrix).
- The native ownership path is a broad zygote/system_server shutdown and needs
  root access. It is not an Android boot integration or a product-ready
  display-owner boundary.
- The refresh policy's four-DU-turn GC16 cadence is a bounded starting policy.
  The runner-side tests cover plan selection, but repeated-turn optical
  ghosting and alignment quantum still require inspection on the physical T1.
- Paths, ioctl behavior, timings, input codes, and suspend behavior are based
  on the observed rooted Android 2.2.1 device. Consult
  [`docs/prs-t1/analysis.md`](../../docs/prs-t1/analysis.md) before applying
  them to another firmware revision.

The older discovery procedure and full evidence ledger remain available in
[`docs/prs-t1/analysis.md`](../../docs/prs-t1/analysis.md); this README is the
operational description of the current crate.
