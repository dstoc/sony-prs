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
| `network-probe HOSTNAME_OR_HTTPS_URL [--invalid-hostname] [--inject-network-loss STAGE]` | Read-only | Resolves the supplied host and performs a bounded HTTPS `GET /health` with rustls certificate and hostname validation. The optional negative test must fail hostname validation. The diagnostic-only fault stage is `dns`, `tls`, or `response`. |
| `sync [HTTPS_ENDPOINT] [FRAMEBUFFER]` | Controls Wi-Fi, display, and tmpfs | Enables Wi-Fi for one reader authorization and synchronization attempt, shows only the public approval URL as a QR code, polls for the read-only boot session, streams and validates the current tar bundle, and disables Wi-Fi on every exit path. |
| `wifi-up` | Controls Wi-Fi | Loads the legacy driver, starts the supplicant, waits for WPA `COMPLETED`, starts DHCP, and waits for `dhcp.wlan0.result=BOUND`. |
| `wifi-down` | Controls Wi-Fi | Stops `dhcpcd`, waits up to 10 seconds for its service to stop or disappear, then stops the supplicant and unloads the Wi-Fi driver. |
| `wifi-probe HOSTNAME_OR_HTTPS_URL [--invalid-hostname] [--inject-network-loss STAGE]` | Controls Wi-Fi and network | Runs `wifi-up`, runs the HTTPS network probe, and always attempts `wifi-down`. The diagnostic-only fault stage is `dns`, `tls`, or `response`. |
| `render-test [FRAMEBUFFER] [SECONDS] [WAVEFORM] [WAIT\|NOWAIT]` | Writes framebuffer | Draws a centered 200x120 RGB565 marker, requests a bounded EPDC update, captures the mapping, waits, and restores the original rectangle. Defaults to 3 seconds, `GC16`, and `WAIT`. |
| `display-test [FRAMEBUFFER] [SECONDS] [WAVEFORM]` | Writes framebuffer | Draws a full-screen grayscale calibration pattern with fill/text swatches, gradients, a grayscale ramp, and 1-, 2-, 4-, and 8-pixel lines. Defaults to 60 seconds and `GC16`; it leaves the pattern visible and does not restore the previous framebuffer. |
| `standalone-test [FRAMEBUFFER] [standby\|mem]` | Owns framebuffer/input | Runs the long-lived native shell after `zygote` and `system_server` have stopped. It starts one synchronization attempt after boot, enters sleep after the configured inactivity period, starts one asynchronous attempt after each sleep/wake transition, and exposes **Sync now** in Details / Settings. The default suspend mode is T1 EINK `standby`. |
| `launch-standalone [FRAMEBUFFER] [standby\|mem]` | Stops Android framework | Root `su` entry point. It detaches into a new session, stops zygote, waits for the framework to exit, and enters `standalone-test`. |

Only `render-test`, `display-test`, `standalone-test`, `launch-standalone`,
`sync`, `wifi-up`, `wifi-down`, and `wifi-probe` mutate device state. `sync`
holds polling and reader credentials only in RAM. It validates the untrusted
bundle before atomically replacing `PRS_T1_LIBRARY_ROOT/current`; an empty
inbox atomically clears the current library and reports a successful
synchronization boundary. Authorization failure, network loss, stale object,
invalid bundle, or tmpfs-capacity failure leaves the current library unchanged.
`render-test` is bounded and restores the bytes it changes,
but it still requires a reader-side recovery route and physical observation of
the panel. `display-test` is bounded but leaves its full-screen pattern visible
when it exits. Use `capture` during its wait to record the framebuffer and use a
physical camera to compare the panel output. The command does not stop Android
display services, so another display owner can repaint the screen.

### PRSync reader synchronization

Run `sync` from the native display-owner session:

```sh
PRS_T1_DOCUMENT_ROOT=/mnt/prs-reader/current \
  /data/local/tmp/prs-t1-agent sync
```

The command starts Wi-Fi, creates a reader authorization request, and shows
the public approval URL as a QR code. The polling secret and read-only session
token remain in process memory. After approval, the client fetches the
manifest, streams the uncompressed tar into the configured tmpfs, validates
the archive with `prs-sync-bundle`, enforces the fixed 16 MiB encoded archive limit,
and atomically updates the `current` symlink. It attempts to disable
Wi-Fi after success and after every failure.

The `sync` command does not persist authorization secrets or the downloaded
archive. The long-lived native shell keeps the read-only session in RAM for the
powered-on boot session. A later wake-triggered or manual attempt reuses that
session until the server expiry or a session rejection requires authorization
again. Each attempt enables Wi-Fi before authorization, keeps it enabled
through the complete attempt, and disables it after success or failure. The
scheduler does not retry an attempt while the reader remains awake. The next
automatic attempt occurs after a later sleep/wake transition. A manual **Sync
now** action remains available at any time the UI permits it. The scheduler
reports recoverable failures with `sync.failure_kind`, including
`network_loss`, `authorization_failure`, `authorization_expired`,
`session_rejected`, `stale_object`, `invalid_bundle`, and
`tmpfs_insufficient`. An empty inbox reports `sync.result=cleared` and removes
the current library at that synchronization boundary. A failed attempt does
not replace or clear the current library.

On the PRS-T1, the sync command opens the framebuffer before it starts Wi-Fi
and keeps that same `NativeDisplay` through synchronization and Wi-Fi shutdown.
This is a vendor driver/HAL lifecycle constraint. It is not an authorization
or TLS requirement.

The native shell starts its boot attempt after it opens the display and input
devices. It polls the cooperative synchronization task during the normal UI
loop, so reading remains responsive while Wi-Fi is active. A successful
replacement is handed to the reader at an idle boundary. A failed attempt
leaves the current local bundle in place and records the structured failure in
Details / Settings.

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
state, the resolved control-socket path, the DHCP result, and the explicit
60-second association and 30-second DHCP limits. `wifi-down` is explicit and
runs the required shutdown sequence: stop `dhcpcd`, wait for the service to
report `stopped` or disappear, stop the supplicant, then unload the driver.
The wait is bounded at 10 seconds. The command attempts all three cleanup
steps even when one step fails.

The PRS-T1's native init configuration exposes the WPA control socket at
`/dev/socket/wpa_wlan0`. The agent tries these paths in order:

1. `/dev/socket/wpa_wlan0`
2. `/data/misc/wifi/sockets/wpa_ctrl_wlan0`
3. `/data/system/wpa_supplicant/wlan0`
4. `/data/misc/wifi/sockets/wlan0`
5. `/data/misc/wifi/wpa_supplicant/wlan0`

The first path is the device-confirmed endpoint. The second path is retained
for the firmware-specific fallback observed during physical validation. The
remaining paths cover Android 2.2 and other legacy layouts without reading the
saved supplicant configuration. Although `wpa_ctrl_*` commonly names a client
socket, this PRS-T1 firmware exposes its fallback server with that name. The
agent uses unique temporary client sockets under `/data/local/tmp` and does
not create, modify, or print Wi-Fi credentials.

Diagnostics include `wifi.association_source` after a successful status read.
Failure output distinguishes a missing control socket, permission failure,
supplicant crash, unavailable supplicant, control read timeout, protocol error,
and ordinary association timeout. All control paths are bounded probes; the
agent does not fall back to interface state or DHCP as proof of WPA
association. The DHCP wait accepts the device's `ok` result as well as
`BOUND` and `bound`.

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

### Deterministic network-loss validation

The explicit `--inject-network-loss STAGE` option is a diagnostic-only test
seam. It accepts `dns`, `tls`, or `response` and is available on both
`network-probe` and `wifi-probe`.

- `dns` returns `failure_stage=dns failure_kind=injected_dns_network_loss`
  before the resolver can return an address.
- `tls` resolves the production host and reaches the real TLS server, then
  fails the server-certificate step with
  `failure_stage=tls failure_kind=injected_tls_network_loss`.
- `response` completes DNS, TCP, and TLS, receives HTTPS response headers, and
  interrupts body handling with
  `failure_stage=response failure_kind=injected_response_network_loss`.

The normal DNS, connect, request, response, and 64 KiB body limits remain in
place. The TLS injection uses a verifier that fails closed; it does not accept
a certificate or weaken normal certificate validation. The response injection
drops the response when the body-read seam returns, so it does not retain a
connection or write output.

For physical recovery validation, replace the placeholder with the production
host used by the ordinary probe. Save each failed command's complete output
and exit status. `wifi-probe` must print the requested `failure_stage` and
`failure_kind`, complete its shutdown steps, and return a non-zero status.
Run an ordinary probe after each injected failure and require
`wifi.stage=off`, `wifi.snapshot=after`, and `result=success`:

```sh
PROBE_HOST='your-production-host.example'

run_injected_loss() {
  stage="$1"
  set +e
  adb shell /data/local/tmp/prs-t1-agent wifi-probe "$PROBE_HOST" \
    --inject-network-loss "$stage" > "prs-t1-loss-$stage.txt" 2>&1
  status=$?
  set -e
  test "$status" -ne 0
  adb shell /data/local/tmp/prs-t1-agent wifi-probe "$PROBE_HOST"
}

run_injected_loss dns
run_injected_loss tls
run_injected_loss response
```

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

Run the safe negative test after a normal probe. It resolves the production
hostname and pins the TCP connection to those addresses. It keeps the
production hostname for SNI, then asks rustls's normal WebPki verifier to check
the certificate against the reserved `invalid.prs-t1.invalid` name. This makes
the request reach the real TLS peer while preserving certificate-chain,
trust-root, and hostname validation. The command reports
`tls_validation=failed_as_expected`, `failure_stage=tls`, and
`failure_kind=tls_hostname_validation_failed`, then exits 0. Connection
failure, timeout, DNS failure, and other certificate errors such as expiry or
an unknown issuer produce a non-zero exit status. The output identifies the
production SNI as `sni_hostname` and the deliberate verifier identity as
`tested_hostname`.

```sh
adb shell /data/local/tmp/prs-t1-agent network-probe "$PROBE_HOST" \
  --invalid-hostname
```

Do not use an IP address as a substitute for the production hostname in either
test. The production hostname is required for SNI and for the real peer
selection. Save the complete stdout and the command exit status in the #99
hardware test record.

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

Details / Settings includes the session-scoped **Fullscreen reader** toggle.
It is off when the native agent starts. When enabled, the shared reader layout
sets the status-bar height to zero, reflows the active document around its
logical content anchor, and keeps the one-pixel reading progress indicator.
The status bar remains visible on Details / Settings and other operational
screens. The toggle does not persist across a restart.

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
| `PRS_T1_SYNC_URL` | `https://prs-reader.dstoc.workers.dev` | HTTPS base URL for the PRSync Worker. |
| `PRS_T1_LIBRARY_ROOT` | `/mnt/prs-reader` | Absolute tmpfs root for bundle staging and the atomic `current` symlink. |
| `PRS_T1_TMPFS_LIMIT_BYTES` | `50331648` | Device-specific bound for the current library, streamed archive, and extraction staging. |
| `PRS_T1_SLEEP_INACTIVITY_SECONDS` | `300` | Awake inactivity period before the native shell enters sleep. The default is five minutes. |
| `PRS_T1_FRAMEBUFFER` | `/dev/graphics/fb0` | Framebuffer used by `sync` when no path argument is supplied. |
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
activated URL in a transient bottom-right overlay with URL text and a QR code.
The overlay is dismissed by the next touch or physical button press. The agent
does not launch a browser or make a network request for the external URL.

On the reader, the hardware left and right keys (`KEY_LEFT` code 105 and
`KEY_RIGHT` code 106) perform one previous/next reader-page operation per
physical press. Repeat events are ignored, and these keys do nothing outside
the reader. Home (`KEY_HOME` code 102) returns from Details / Settings or
Display Test to the current reading position; from the reader it navigates to
the current bundle's entry point. Back (`KEY_BACK` code 158) walks the native
view history before delegating to reader history. A short physical menu press
opens Details / Settings only from the reader. A menu hold of at least one
second retains its existing full GC16 redraw behavior, and the completed hold
consumes its release. UI history remains separate from document/page/cursor
history, and transient overlays are not history entries.

Tap the status bar to open **Details / Settings**. The details page groups the
live snapshot under:

- **Settings** — the **Debug messages** and **Fullscreen reader** toggles are
  off by default. Enable debug messages when troubleshooting. Fullscreen is a
  session-only reader presentation setting.
- **Power** — battery state, temperature, voltage, AC, USB, and supported power states.
- **Connectivity** — Wi-Fi interface/link/supplicant state, USB gadget state, and ADB.
- **Synchronization** — the current sync state and the latest failure summary.
- **System** — uptime, framebuffer state/rotation, Android process state, and wake lock.
- **Storage** — available space on `/data` and `/mnt/sdcard`.
- **Diagnostics** — compact system and input counters/state kept separate from the user-facing status.

The page also provides full-width **Reboot**, **Power off**, and **Back to
reading** targets, plus **Sync now**, **Return to entry point**, and **Display
test**. User-facing status is separated from compact **Diagnostics** telemetry;
the action pane has its own spaced region below the status content. Every action
inverts while its touch is held and restores on release using a bounded partial
refresh. Touch release is accepted from the event shapes observed on
the T1: `BTN_TOUCH=0`, `ABS_MT_TRACKING_ID=-1`, or
`ABS_MT_TOUCH_MAJOR=0`, committed by `SYN_REPORT`. The legacy `ABS_X/Y` path
maps the panel's advertised 800x600 axes to the logical 600x800 coordinates
required by fbdev rotation `3` before hit testing.

The reader feedback strip between the status bar and document content is quiet
in normal mode. It shows actionable errors until the next touch or physical
button press. Diagnostic input, navigation, power, timing, and synchronization
messages appear there only when **Debug messages** is enabled. An active sync
uses the status bar's operational state area as **Syncing**; successful and
unchanged syncs return to the normal status bar without a feedback message.

The physical menu button is event0 code 357 (`Unknown` in the old kernel). On
the reader, a short press opens Details / Settings. Outside the reader, a
short press is a no-op. A hold of at least one second requests a full GC16
redraw with the EPDC's `UPDATE_MODE_FULL` flag. A short power press sleeps; a
press of at least two seconds requests reboot.

## Display and refresh model

The T1 exposes a 600x800 visible RGB565 framebuffer with a 1216-byte stride
and a larger virtual buffer. `NativeDisplay` establishes fbdev rotation `3`
through `FBIOPUT_VSCREENINFO` after the first writable mapping, then re-queries
the driver before the first render. It repeats this check after wake. The
runtime therefore never assumes that the visible image is tightly packed in
the mapped framebuffer.

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
3. after the configured five-minute inactivity period, or on a short power
   press, renders a standby screen and requests sleep;
4. waits for `/sys/power/wait_for_fb_wake`, handles the wake-side power event,
   reacquires the lock, refreshes framebuffer metadata, redraws, and starts
   one asynchronous synchronization attempt; and
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
`/sys/power/state`. The Android 2.2 compatibility helper must be linked with
the reader's own `/system/lib/libdl.so`; see the [power-state helper build
instructions](build.md#power-state-helper) for the build and staging commands.

The optional [T1 launcher](../../tools/prs-t1-launcher/README.md) adds a
small Android 2.2/API 8 Home activity labelled **Native UI**. It invokes the
root handoff when selected from the Home resolver; it does not contain the
native renderer and does not make the native UI persistent.

## USB wake/recovery while native UI owns the reader

In standalone native mode, ADB can remain online after `zygote` and
`system_server` have stopped. That does not mean Android's input stack is
available: `adb shell input keyevent 116` sends a request through Android's
`input` command and framework input dispatcher, so it cannot reach the native
loop after those services are gone.

The native runtime reads Linux evdev nodes directly and waits for
`KEY_POWER` (type `1`, code `116`). Before choosing a node, inspect the live
reader rather than assuming that `event2` is stable across firmware or runtime
states:

```sh
adb shell /data/local/tmp/prs-t1-agent input
```

Use the `event_info` names and the reported capabilities to find the power
node. On the tested reader the candidates are `/dev/input/event2`
(`wm831x_on`) and `/dev/input/event4` (`sub_cpu_pwrbutton`). If the mapping is
unclear, confirm a candidate while the reader is awake with the finite event
probe and a physical press; select the node that reports
`type_name=KEY code=116 code_name=KEY_POWER`:

```sh
adb shell /data/local/tmp/prs-t1-agent events /dev/input/event2 10
```

Replace `event2` in the following sequence with the confirmed node. The
sequence injects a raw Linux press, synchronization event, release, and final
synchronization event:

```sh
adb shell '
  sendevent /dev/input/event2 1 116 1
  sendevent /dev/input/event2 0 0 0
  sendevent /dev/input/event2 1 116 0
  sendevent /dev/input/event2 0 0 0
'
```

This works because `sendevent` writes directly to the same evdev path that the
native runtime polls; it bypasses Android input dispatch entirely. After the
native loop receives the event, it invokes
`/data/local/tmp/prs-t1-power-state on`, which calls Sony's
`set_screen_state(1)` through the vendor bridge. Keep that helper built from
the reader's matching `/system/lib/libdl.so` and staged as described in the
[power-state helper build instructions](build.md#power-state-helper).

Raw event injection is a development/recovery technique only. It is not a
normal end-user wake path and should not be added to a product-facing support
procedure. If ADB is unavailable entirely, USB cannot wake the reader
remotely; use the physical power button or the hardware reset procedure.

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
- External URLs produce a transient T1-owned bottom-right text and QR overlay.
  The overlay is not opened in a browser and does not make a network request.
  Task-list markers are not interactive. Specialist Markdown uses the fallbacks
  in the [support matrix](../../docs/prs-t1/markdown-reader.md#markdown-support-matrix).
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
