# PRS-T1 native agent

This crate is the experimental home for a native PRS-T1 binary that can own a
display loop and receive touch/button input. It is intentionally separate from
the PRS-350 agent: the T1 is an Android 2.2.1 reader with a different boot,
display, input, and service model.

The binary currently implements the hardware-discovery milestone plus an
explicit opt-in standalone runtime test: framebuffer inspection and capture,
Android process/service inspection, evdev capability inspection, a bounded
reversible render test, and a full-screen native power/input test. Replacing
Android at boot is not part of this milestone.

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

The preferred host workflow is binary-safe `adb exec-out` when the device's
ADB daemon supports it:

```sh
adb push ./prs-t1-agent /data/local/tmp/prs-t1-agent
adb shell chmod 755 /data/local/tmp/prs-t1-agent
adb exec-out /data/local/tmp/prs-t1-agent capture > t1-screen.pgm
```

If the installed ADB client does not support `exec-out`, the agent can write a
capture to `/data/local/tmp/t1-screen.pgm` and the host can retrieve it with
`adb pull`. The capture should be repeated before and after an Android screen
transition; matching metadata and changing pixels will distinguish a usable
buffer from a stale or shadow display surface. A screenshot is also the first
artifact to preserve before any process-ownership experiment.

This T1's old ADB daemon closes the `exec-out` channel, and direct binary output
through `adb shell` is PTY-translated to CRLF. The tested workflow is therefore
to redirect on the reader and pull the file:

```sh
adb shell '/data/local/tmp/prs-t1-agent capture > /data/local/tmp/t1-screen.pgm'
adb pull /data/local/tmp/t1-screen.pgm ./t1-screen.pgm
```

## T1 refresh ABI and bounded render test

The installed `/system/lib/hw/gralloc.imx5x.so` contains the T1's vendor
framebuffer update path. Its ioctl constants and payload size are:

```text
MXCFB_SET_AUTO_UPDATE_MODE       0x4004462d
MXCFB_SEND_UPDATE                0x4044462e  (0x44-byte payload)
MXCFB_WAIT_FOR_UPDATE_COMPLETE   0x4004462f
```

The `render-test` command is the first write-capable device operation. It
requires the known T1 RGB565 format, opens `/dev/graphics/fb0` read/write,
backs up a centered 200x120 rectangle, draws a black-and-white marker, asks
the EPDC driver to update that region with the GC16 waveform, captures the
current framebuffer mapping, waits for a bounded interval, and restores and
refreshes the original rectangle. It does not stop or signal Android
processes. The PGM proves the native marker was written to the framebuffer;
only observation of the reader can confirm the physical e-ink panel result.

The old T1 framebuffer mapping rejects `msync()` with `EINVAL`; the agent now
follows the vendor gralloc behavior and relies on the shared mapping followed
by the update ioctl. The tested deployment route is:

```sh
adb push ./prs-t1-agent /data/local/tmp/prs-t1-agent
adb shell chmod 755 /data/local/tmp/prs-t1-agent
adb shell '/data/local/tmp/prs-t1-agent render-test /dev/graphics/fb0 5 > /data/local/tmp/t1-render-test.pgm'
adb pull /data/local/tmp/t1-render-test.pgm ./native-custom-render-test.pgm
```

The first successful run kept the marker in framebuffer memory for the full
five-second wait, returned successfully from both update requests, and
restored the original rectangle. `adbd`, `zygote`, and `dispd` remained
running. This does not yet prove that Android will not redraw the region in a
long-running native UI.

A follow-up 60-second run recorded a physical touch and `KEY_LEFT` button
press while the marker was active. The exact marker was not preserved after
input: Android navigated from page 2 back to page 1 and redrew the framebuffer
over the marker. No Android process was stopped. Persistent native rendering
therefore needs a display-ownership boundary before touch/button mapping is
useful.

Root ADB can reproduce the hardware button path with `sendevent`; Linux key
codes 106 (`KEY_RIGHT`) and 105 (`KEY_LEFT`) navigated the reader between its
two home pages. The Android `input keyevent` utility is present but did not
navigate this vendor UI with the corresponding Android DPAD keycodes.

Stopping `zygote` also stopped `system_server` and removed the framework
display/input services; the marker then survived a raw button injection. A
manual `start zygote` entered a `PackageManager` crash loop, so normal reboot
is currently the safe recovery path after this ownership experiment.

A 90-second no-input render test with Android running preserved the exact
marker, so no timer/status redraw was observed during that interval. This is
not a guarantee against every future status update. The kernel exposes
`/sys/power/wake_lock` and `wake_unlock`, providing a possible native keep-awake
mechanism if zygote is stopped; Android's framework sleep/wake and power-key
policy would still be unavailable.

The T1 exposes `wm831x_on` and `sub_cpu_pwrbutton` as separate `KEY_POWER`
evdev sources. Android normally handles short/long power presses in
`system_server`; that handler disappears when zygote is stopped. No separate
reset node has been identified, so the verified escape route remains root ADB
plus normal reboot. A native UI should hold a kernel wake lock while testing
zygote isolation.

User-mode sleep/wake needs a native state machine: hold
`/sys/power/wake_lock` while active, release it before requesting `standby`,
wait on the T1's `/sys/power/wait_for_fb_wake` barrier, reacquire the lock
after a real wake, refresh the framebuffer metadata while retaining its
mapping, and redraw the complete screen. The EINK `standby` request is
preferred for native mode because the normal `mem` early-suspend path disables
the sub-CPU power-button wake IRQ. Power-key duration is handled from
`event2`/`event4`.

## Standalone native runtime test

`standalone-test` is intended for manual development tests after zygote has
been stopped. It opens all relevant input nodes, holds the legacy kernel wake
lock, renders a full-screen diagnostic pattern, and redraws the pattern with
the most recent touch/key values. It does not depend on Android Java services.

The development procedure is:

```sh
adb push ./prs-t1-agent /data/local/tmp/prs-t1-agent
adb shell chmod 755 /data/local/tmp/prs-t1-agent
adb shell stop zygote
adb shell 'trap "" HUP; /data/local/tmp/prs-t1-agent standalone-test /dev/graphics/fb0 </dev/null >/data/local/tmp/prs-t1-agent.log 2>&1 &'
```

The optional final argument selects the suspend request: `standby` (the
default) uses the T1 EINK early-suspend mode, while `mem` uses Android's
normal early-suspend path. For example, the stock-style wake test is:

```sh
adb shell 'trap "" HUP; /data/local/tmp/prs-t1-agent standalone-test /dev/graphics/fb0 mem </dev/null >/data/local/tmp/prs-t1-agent-mem.log 2>&1 &'
```

The `HUP` trap and redirected standard streams are important on this old T1:
the ADB USB link disappears during suspend, and a process left attached to the
interactive ADB shell is otherwise lost before it can handle resume. The
diagnostic process can be checked with `adb shell ps` and its startup errors
with `adb shell cat /data/local/tmp/prs-t1-agent.log` while the reader is
awake.

While the test is running:

1. Touch the screen and press hardware keys; the diagnostic display should
   show the raw source, event type, code, value, coordinates, and event counts.
2. Press and release a power key briefly. The test displays a sleep status,
   supplies the native standby image, releases its wake lock, requests the
   selected suspend mode, and waits for `/sys/power/wait_for_fb_wake`. During
   that wait it continues monitoring the power evdev nodes; a wake-side power
   event causes it to request `on`, matching the framework's early-resume
   handoff, before it reacquires the lock and redraws.
3. Hold a power key for at least two seconds. The test requests `/system/bin/reboot`.

The smoke test has verified the full-screen pattern, wake-lock acquisition,
and on-screen key data while zygote is stopped. A synthetic `KEY_POWER` pair
also reached the native state machine and entered the kernel suspend path. The
first attempt was attached to the ADB shell, so the process disappeared when
USB went away and Android restarted zygote/system_server on resume. A detached
launch survived that shell lifecycle. The normal `mem` path was then shown to
return immediately because `/sys/power/state` is asynchronous, and it did not
leave the sub-CPU power-button wake path usable. The current test uses EINK
`standby` plus the display-wake barrier. If the reader does not wake, use the
hardware reset or `adb reboot` recovery route. After any zygote stop, a normal
reboot is the supported way to restore Android.

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
adb shell 'ls -l /dev/fb0 /dev/graphics/fb0 2>/dev/null'
adb shell 'cat /proc/fb 2>/dev/null; cat /proc/devices 2>/dev/null'
adb shell 'for f in name bits_per_pixel virtual_size stride mode modes state blank rotate; do echo --- $f; cat /sys/class/graphics/fb0/$f 2>/dev/null; done'
adb shell 'getprop | grep -i -E "fb|display|eink|screen"'
```

The probe must record:

- the exact path (`/dev/fb0`, `/dev/graphics/fb0`, or another node);
- permissions, owner, and group;
- `FBIOGET_VSCREENINFO` and `FBIOGET_FSCREENINFO` values;
- width, height, virtual dimensions, stride, bits per pixel, and pixel format;
- whether `mmap` succeeds and whether the visible buffer is 8-bit grayscale;
- any Sony/e-ink-specific update ioctl or helper library used by the stock UI.

The first native test is the `capture` method above: mmap and capture the
buffer without changing it. The bounded `render-test` now provides the next
controlled step: draw a reversible marker, request the vendor EPDC update,
capture the mapped result, wait several seconds, and check whether another
process redraws it before restoring the original bytes.

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
restore the stock UI and reboot. The reversible render test is the initial
display step; it has not yet demonstrated long-running ownership. Do not
modify boot images or init scripts for this phase. A `/data/local/tmp` binary
and an ADB-started process are the preferred deployment route.

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

## Current implementation

The current `probe` command runs read-only framebuffer, Android, and input
inventory. The `input` command performs evdev ioctl capability queries without
opening an event stream, grabbing a device, or injecting events. The `capture`
command opens `/dev/graphics/fb0` read-only, maps the framebuffer with
`PROT_READ`, converts the visible RGB565 pixels to an 8-bit grayscale PGM, and
writes only the PGM stream to stdout. The `render-test` command is explicitly
write-capable and is limited to the centered, reversible test described above.
The `standalone-test` command is a separate long-running write-capable runtime
for manual zygote-isolated power, display, and input testing.

The `events` command opens one evdev node read-only and logs a finite raw event
stream. For example, `prs-t1-agent events /dev/input/event1 10` captures ten
seconds of touch input. It uses non-blocking reads and never calls
`EVIOCGRAB`; it should be run while Android is active and only with a recovery
route available.

The ARMv5 musl build has been deployed and tested on this T1's ARMv7
userspace. It successfully captured a 600x800 screen and identified the
touchpanel's absolute axes. The bounded raw evdev logger now runs against
`event1` without grabbing the device or injecting events. Its first three-second
idle sample contained no events. The native render test also completed with
the stock Android services still active; input now confirms that Android
redraws over the native framebuffer. The standalone runtime smoke test now
draws the full-screen pattern, holds the wake lock, and displays synthetic key
data with zygote stopped. Manual suspend/resume and long-power reboot remain.
