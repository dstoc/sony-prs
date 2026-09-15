# PRS-T1 native agent

This crate is the experimental home for a native PRS-T1 binary that can own a
display loop and receive touch/button input. It is intentionally separate from
the PRS-350 agent: the T1 is an Android 2.2.1 reader with a different boot,
display, input, and service model.

The binary is currently only a buildable scaffold. The first implementation
milestone is hardware discovery and a safe display/input probe, not replacing
Android at boot.

## Design goal

Run a small native process on the rooted T1 which can:

1. discover and open the real framebuffer device;
2. render a known test pattern and restore or redraw a known screen;
3. discover the touch and hardware-key input path;
4. receive input without depending on Android's Java UI stack; and
5. keep ADB and Wi-Fi available for development and recovery.

The initial control path is root ADB. We should deploy test binaries to a
temporary location such as `/data/local/tmp` and start them manually. A T1
host-control CLI or persistent startup hook can be added after the device-side
ownership model is understood.

## Early screenshot method

Screenshot capture should come before any native drawing. It gives us a
low-risk way to prove that the framebuffer node, geometry, stride, pixel
format, and mmap path are correct while Android remains fully active.

The first device-side interface will be a read-only `capture` mode. It will
open the framebuffer read-only, query `fb_var_screeninfo` and
`fb_fix_screeninfo`, mmap only with `PROT_READ`, convert the visible pixels to
an 8-bit grayscale PGM stream, and write that stream to stdout. It must never
write the mapped framebuffer or issue a display-refresh ioctl.

The preferred host workflow is binary-safe `adb exec-out`:

```sh
adb push target/arm-linux-androideabi/release/prs-t1-agent /data/local/tmp/prs-t1-agent
adb shell chmod 755 /data/local/tmp/prs-t1-agent
adb exec-out /data/local/tmp/prs-t1-agent capture > t1-screen.pgm
```

If the installed ADB client does not support `exec-out`, the agent can write a
capture to `/data/local/tmp/t1-screen.pgm` and the host can retrieve it with
`adb pull`. The capture should be repeated before and after an Android screen
transition; matching metadata and changing pixels will distinguish a usable
buffer from a stale or shadow display surface. A screenshot is also the first
artifact to preserve before any process-ownership experiment.

## What we know about this T1

- Sony firmware reports `1.0.00.09270`.
- The system image identifies itself as Android `2.2.1` / `FRG83`.
- Root ADB is available through `/sbin/adbd` after the enable-ADB package.
- The device is a rooted Android system, not the PRS-350's small Linux
  userspace, so its framebuffer and input ownership must be measured on the
  running device.

These are observations from the current reader, not assumptions about every
PRS-T1 firmware revision.

## Important Android boundary

Do not stop zygote as the first step. Zygote is the parent for the Dalvik/Java
side of Android and stopping it will normally take down `system_server` and
most framework services, including ActivityManager, WindowManager,
PackageManager, and Android input dispatch. It may leave a root `adbd` process
running if init owns it, but that must be verified rather than assumed. Wi-Fi
drivers and `wpa_supplicant` may also remain alive, while framework-mediated
network management and the visible settings UI may not.

A native process that opens the kernel framebuffer and input device directly
does not inherently need zygote, `system_server`, or the launcher to be
stopped. The likely first problem is ownership and redraw races: Android's
`surfaceflinger`, the Sony reader application, or an e-ink display service may
continue to write the same device after our process renders.

The preferred progression is therefore:

```text
observe Android
    -> run native probe beside Android
    -> identify the display/input owner
    -> stop or suspend only the owner/UI layer
    -> retain system_server, adbd, and Wi-Fi
    -> consider surfaceflinger or zygote only as measured experiments
```

## Hardware checks before writing device code

All checks in this section should be read-only. Capture the output in the T1
analysis log and keep a stock reboot route available.

### 1. Identify the framebuffer node and driver

Check both common Android paths and inspect the kernel's registration:

```sh
adb shell 'id; ls -l /dev/fb* /dev/graphics/fb* 2>/dev/null'
adb shell 'cat /proc/fb 2>/dev/null; cat /proc/devices 2>/dev/null'
adb shell 'find /sys/class/graphics -maxdepth 2 -type f -print -exec sh -c "echo --- \"$1\"; cat \"$1\"" sh {} \; 2>/dev/null'
adb shell 'getprop | grep -i -E "fb|display|eink|screen"'
```

The probe must record:

- the exact path (`/dev/fb0`, `/dev/graphics/fb0`, or another node);
- permissions, owner, and group;
- `FBIOGET_VSCREENINFO` and `FBIOGET_FSCREENINFO` values;
- width, height, virtual dimensions, stride, bits per pixel, and pixel format;
- whether `mmap` succeeds and whether the visible buffer is 8-bit grayscale;
- any Sony/e-ink-specific update ioctl or helper library used by the stock UI.

The first native test should be the `capture` method above: mmap and
checksum/capture the buffer without changing it. A later test can draw a small,
reversible marker in a controlled area, capture the result, wait several
seconds, and determine whether another process redraws over it.

### 2. Find who owns or writes the framebuffer

Record the Android process inventory and look for display-related processes:

```sh
adb shell 'ps'
adb shell 'service list'
adb shell 'dumpsys SurfaceFlinger 2>/dev/null'
adb shell 'dumpsys input 2>/dev/null'
adb shell 'cat /proc/mounts'
```

For each candidate process (`surfaceflinger`, `system_server`, the Sony reader
application, launcher, or an e-ink service), inspect open descriptors and the
command line. The exact toolbox on this old image may not include `lsof` or a
full `fuser`, so `/proc` inspection is the fallback:

```sh
adb shell 'for p in /proc/[0-9]*; do n=$(cat "$p/cmdline" 2>/dev/null | tr "\\000" " "); for f in "$p"/fd/*; do t=$(readlink "$f" 2>/dev/null); case "$t" in /dev/fb*|/dev/graphics/fb*) echo "$p $n $f -> $t";; esac; done; done'
```

An open descriptor is evidence of interest, not proof that the process is
actively rendering. We should also compare framebuffer bytes before and after
an Android screen transition and collect `logcat` messages during a native
test.

### 3. Identify input without injecting events

Check the standard Linux input devices and any Sony-specific controller:

```sh
adb shell 'ls -l /dev/input /dev/subcpu 2>/dev/null'
adb shell 'cat /proc/bus/input/devices 2>/dev/null'
adb shell 'getevent -pl 2>/dev/null'
```

Then implement a native read-only event probe which opens candidate nodes
non-blocking and prints raw event records. We need to determine:

- whether touch arrives through `/dev/input/event*`, `/dev/subcpu`, or an
  Android/vendor daemon;
- the event device name, ABS ranges, coordinate orientation, and pressure/tool
  fields;
- hardware key codes and press/release semantics; and
- whether Android continues consuming the same events while our process reads
  them. Reading an evdev device may distribute or compete for events depending
  on the driver, so this must be tested with a physical touch and a recovery
  path.

No event injection, `EVIOCGRAB`, or input-device writes belong in the first
probe.

### 4. Establish process and service dependencies

Before stopping anything, capture the process tree, init service definitions,
and network/ADB state:

```sh
adb shell 'ps -p 1; cat /init.rc 2>/dev/null; cat /init*.rc 2>/dev/null'
adb shell 'ps | grep -E "adbd|zygote|system_server|surfaceflinger|wpa_supplicant|netd|dhcpcd"'
adb shell 'getprop sys.usb.config; getprop init.svc.adbd; getprop init.svc.wpa_supplicant'
adb shell 'ip addr 2>/dev/null; iwconfig 2>/dev/null'
```

The exact Android 2.2 service names and init files are device-specific. We
need to distinguish:

- processes started directly by init, which may survive a Java framework stop;
- processes supervised or recreated by `system_server`; and
- the Sony UI process which can be stopped independently.

After every process experiment, verify both `adb shell id` and the Wi-Fi
connection before proceeding. Keep one ADB shell open while experimenting and
have the physical power/recovery route ready.

## Staged implementation plan

### Phase A — read-only inventory

Add `framebuffer` and `input` modules which only enumerate paths, query ioctl
metadata, inspect input capabilities, and report process ownership. Add host
tests for parsing and coordinate conversion using captured output.

### Phase B — native probe beside Android

Cross-compile a static ARM binary, copy it with ADB, and run it while the stock
launcher is visible. Implement framebuffer capture first, then a no-injection
input logger. Confirm whether direct framebuffer writes are visible and whether
Android redraws them.

### Phase C — controlled UI ownership

Identify the smallest Sony UI process or service responsible for redraws. Stop
only that component, if possible, while leaving init, `adbd`, Wi-Fi,
`surfaceflinger`, and the Java framework running. Add a watchdog or manual ADB
restart command before making the native loop persistent.

### Phase D — replace the visible UI temporarily

Run a native test screen for a bounded period, exercise touch and buttons, then
restore the stock UI and reboot. Do not modify boot images or init scripts for
this phase. A `/data/local/tmp` binary and an ADB-started process are the
preferred deployment route.

### Phase E — investigate deeper Android shutdown only if necessary

If the display remains owned after the UI layer is stopped, test the relevant
display service in isolation. Only after measuring the effects should we
consider stopping `surfaceflinger`, `system_server`, or zygote. Each experiment
must have a documented command, expected ADB/Wi-Fi impact, observation, and
reboot recovery.

## Proposed crate layout

```text
crates/prs-t1-agent/
├── Cargo.toml
├── README.md
└── src/
    ├── main.rs         # CLI and lifecycle policy
    ├── android.rs      # read-only process/service inspection
    ├── framebuffer.rs  # fb discovery, mmap, format, and e-ink updates
    ├── input.rs        # evdev/vendor input discovery and decoding
    └── runtime.rs      # controlled render/input loop
```

The crate should not initially depend on Android Java APIs or a graphical
toolkit. Keep the native surface small, use direct Linux syscalls/ioctls where
needed, and make every device-mutating operation an explicit opt-in mode.

## Current scaffold

The current binary exits with a scaffold status and performs no device I/O.
The next code change should implement Phase A's read-only framebuffer and
process inventory, then use the rooted T1 to fill in the unknowns above.
