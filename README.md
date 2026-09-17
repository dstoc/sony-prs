# Sony PRS reader tools

This repository contains read-only host tooling for Sony x50 readers and
device-side experiments for two readers with different operating systems.

The active development focus is the PRS-T1 native UI in
[`crates/prs-t1-agent`](crates/prs-t1-agent/). It is now a usable, manually
launched native UI prototype: it renders a custom status bar and paginated
Markdown reading surface, retains the diagnostics screen, reads touch and
hardware input, tracks pixel damage, and handles development sleep/wake and
reboot actions. It still requires a rooted reader and an explicit ADB or
launcher handoff; it is not a persistent replacement for the stock Android UI
yet.

PRS-350 work is on hold while waiting for hardware unbricking. The existing
PRS-350 controls remain available for development when hardware access returns,
but they are not the current project focus.

## Workspace

| Path | Purpose |
| --- | --- |
| `crates/scsi-transport` | Linux SG_IO and BSG transport primitives. |
| `crates/sony-x50` | Sony x50 read-only transport and legacy file protocol. |
| `crates/prsctl` | Read-only host CLI for probing and extracting reader files. |
| `crates/prs350-wire` | PRS-350 development serial wire format. |
| `crates/prs350-devctl` | Stateful PRS-350 development serial controls. |
| `crates/prs350-agent` | Cross-compiled ARM-side PRS-350 development agent. |
| `crates/prs-t1-agent` | Native PRS-T1 framebuffer, input, status, and UI runtime. |
| `crates/prs-markdown` | Hardware-independent Markdown document, layout, pagination, navigation, and rendering boundaries. |
| `tools/prs-t1-launcher` | Optional Android 2.2 Home entry point for the T1 runtime. |

The SCSI layers contain no write, delete, update-mode, flash, or arbitrary
command API. The development tools are intentionally separate because they can
reboot a reader, update its framebuffer, execute an ARM binary, or run a root
shell through a temporary PRS-350 service package.

## Build and test

From the repository root:

```sh
cargo test --workspace
cargo build --workspace --release
```

The workspace test command covers the protocol crates and the pure T1 display,
input, status, runtime, and pixel-damage tests. The regular release build is a
host build; it does not produce a T1-compatible ARM executable. See the
[T1 build and deployment guide](crates/prs-t1-agent/build.md) for the
ARMv5TE/musl cross-build and the optional
[tap-to-launch APK](tools/prs-t1-launcher/build.md).

Merging a Release Please PR publishes the matching ARM executable in the
`prs-t1-agent-vX.Y.Z` GitHub Release as `prs-t1-agent-armv5te`. See the
[automated release notes](crates/prs-t1-agent/build.md#automated-github-releases)
for the Conventional Commit rules and artifact checks.

On Linux, access to `/dev/sgN` normally requires root or membership in the
group owning the SCSI-generic devices.

## PRS-T1 native UI

The reusable reader architecture is documented in
[`docs/prs-t1/markdown-reader.md`](docs/prs-t1/markdown-reader.md). The
`prs-markdown` crate owns document semantics and viewport-relative page
layouts; the T1 agent owns device input, framebuffer access, and refresh
policy.

The T1 agent is tested against a rooted Sony PRS-T1 running Android 2.2.1. The
observed device exposes a 600x800 RGB565 framebuffer at
`/dev/graphics/fb0`, an EPDC refresh ioctl, and evdev touch, key, and power
devices. Those paths and observations are firmware-specific; inspect the
device before applying the commands to another T1.

Build the deployable binary with the command in the
[T1 build guide](crates/prs-t1-agent/build.md), then copy it while Android is
running:

```sh
adb wait-for-device
adb push target/armv5te-unknown-linux-musleabi/release/prs-t1-agent \
  /data/local/tmp/prs-t1-agent
adb shell chmod 755 /data/local/tmp/prs-t1-agent
```

The agent commands are split by risk:

| Command | Access | Purpose |
| --- | --- | --- |
| `probe` | Read-only | Framebuffer, Android-process, input, and status inventory. |
| `status` | Read-only | Battery, power, USB, Wi-Fi, ADB, screen, storage, and process snapshot. |
| `input` | Read-only | Evdev capability inventory without opening an event stream. |
| `events` | Read-only | Finite raw evdev capture; never grabs or injects events. |
| `capture` | Read-only | RGB565 framebuffer capture as an 8-bit grayscale PGM. |
| `render-test` | Writes framebuffer | Bounded marker update followed by restoration of the original rectangle. |
| `standalone-test` | Owns framebuffer/input | Long-running UI test after Android framework ownership has been stopped. |
| `launch-standalone` | Stops Android framework | Root `su` handoff that detaches, stops zygote, and enters the native UI test. |

For example, read the live status and capture a screen without sending binary
data through the old T1 ADB PTY:

```sh
adb shell /data/local/tmp/prs-t1-agent status
adb shell '/data/local/tmp/prs-t1-agent capture > /data/local/tmp/t1-screen.pgm'
adb pull /data/local/tmp/t1-screen.pgm ./t1-screen.pgm
```

The native UI's status bar opens a details page. The home canvas renders the
configured development Markdown document through the shared `prs-markdown`
reader; see [`crates/prs-t1-agent/README.md`](crates/prs-t1-agent/README.md)
for staging and environment overrides. The details page groups the device
snapshot under Power, Connectivity, System, Storage, and Input and provides
reboot, power-off, and return actions. The runtime polls status every five
seconds, keeps status-only document redraws inside the status-bar region, and
uses the device-side refresh policy described in the T1 guide: ordinary text
page turns use queued DU updates with periodic GC16 cleanup, while pages with
images or intentional gray paint use synchronous GC16. An unexpected change
outside a semantic dirty hint is still promoted to GC16 by the damage layer.
A long menu-button hold forces a full EPDC redraw.

The standalone path is a development ownership experiment, not a boot change.
It requires root, stops `zygote` and `system_server`, and can make the reader
unresponsive to Android services. Use the helper for the documented detached
test and recovery flow:

```sh
crates/prs-t1-agent/tools/native-test.sh start
crates/prs-t1-agent/tools/native-test.sh status
crates/prs-t1-agent/tools/native-test.sh reboot
```

The optional launcher APK presents **Native UI** from Android's Home resolver
and invokes the same detached handoff. It is a tap-to-launch development
entry point, not automatic startup. Reboot or the hardware reset button is the
supported recovery route if the native process or USB link does not return.

Remaining T1 work includes deciding on a durable display-ownership boundary,
characterizing repeated DU updates and when to schedule GC16 cleanup, improving
standby-image quality, and deciding whether a persistent startup integration is
worth the recovery and Android-compatibility cost. The detailed hardware
evidence and test history are in [the T1 analysis](docs/prs-t1/analysis.md).

## Read-only host CLI

```text
prsctl scan
prsctl probe /dev/sgN
prsctl get /dev/sgN DEVICE_PATH OUTPUT
prsctl size /dev/sgN DEVICE_PATH
prsctl read /dev/sgN DEVICE_PATH OFFSET COUNT
prsctl dump /dev/sgN DEVICE_PATH OUTPUT BADMAP
prsctl decode-request PACKET
prsctl decode-answer PACKET
```

Output files are created exclusively and are never overwritten.

## PRS-350 development controls

These commands require the temporary PRS-350 development package and should
not be treated as a production interface:

```text
prs350-devctl ping /dev/ttyACM0
prs350-devctl info /dev/ttyACM0
prs350-devctl status /dev/ttyACM0
prs350-devctl probe /dev/ttyACM0
prs350-devctl screenshot /dev/ttyACM0 OUTPUT.pgm
prs350-devctl render /dev/ttyACM0
prs350-devctl reboot /dev/ttyACM0
prs350-devctl exec /dev/ttyACM0 ARM_BINARY
prs350-devctl shell /dev/ttyACM0 COMMAND
```

## Documentation and artifacts

- [Documentation index](docs/README.md)
- [T1 native UI guide](crates/prs-t1-agent/README.md)
- [T1 build and deployment guide](crates/prs-t1-agent/build.md)
- [T1 analysis and device evidence](docs/prs-t1/analysis.md)
- [Markdown reader architecture](docs/prs-t1/markdown-reader.md)
- [T1 launcher guide](tools/prs-t1-launcher/README.md)
- [PRS-350 documentation](docs/prs350/)
- [Workspace crates](crates/)
- [PRS-350 helper scripts](tools/prs350/)

Historical downloads, firmware images, extracted filesystems, screenshots, and
device identity material remain outside Git under `device-dumps/`.
