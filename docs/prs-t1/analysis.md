# Sony PRS-T1 analysis

This document tracks the read-only analysis and filesystem acquisition of the
connected Sony PRS-T1. It is separate from the PRS-350 notes because the T1 is
an Android-based reader with an eMMC partition layout rather than the PRS-350
x50 MTD layout.

## Current status

The reader is detected and the Sony x50 command framing is accepted. Standard
SCSI INQUIRY, initialization, partition-size queries, and filesystem reads all
work while the reader is in its on-device `data transfer mode`. A complete
read-only image set was acquired after a physical reconnect.

The original filesystem images and boot-area captures remain preserved. The
T1 was subsequently modified only through its user-visible update route:
the Western minimal-root package and the matching enable-ADB package were
staged and applied. No partition write was issued by the host SCSI client.
ADB now provides a root shell. A documented stock recovery-selector request
was tested earlier and the normal boot selector was restored afterward.

The native display work has now completed a bounded, reversible render test.
The T1-specific e-ink update ABI was recovered from its installed vendor
gralloc library, and a native ARM test wrote and refreshed a centered marker
while Android remained running. The marker was restored afterward. No Android
process has been stopped and no boot or partition image has been modified by
the native display test.

## Host access

The reader identifies as USB vendor/product `054c:05c2` and exposes three SCSI
logical units:

| Node | Inquiry product |
|---|---|
| `/dev/sg1` / `/dev/bsg/1:0:0:0` | `PRS-T1` |
| `/dev/sg2` / `/dev/bsg/1:0:0:1` | `PRS-T1 SD` |
| `/dev/sg3` / `/dev/bsg/1:0:0:2` | `PRS-T1 Setting` |

The host initially had the loadable `sg` module installed but not loaded;
`modprobe sg` restored the SCSI-generic nodes. After the physical reconnect,
Linux enumerated the same LUNs as host ID 1 rather than host ID 0. The `agent`
user is a member of the `prs350` group. The active udev rules grant that group
mode `0660` access
to both the T1 BSG and SCSI-generic nodes.

The host-side client also supports Linux `/dev/bsg/*` through the SG v4 BSG
ioctl and includes BSG nodes in `prsctl scan`. This is useful on hosts where
the legacy `sg` module is unavailable.

## Native display and input inventory

With root ADB connected on 2026-09-15, the T1 exposes a Linux framebuffer and
standard evdev input devices while Android is running. No process was stopped
and no device node was written during this inventory.

The framebuffer is:

| Property | Observation |
|---|---|
| Device | `/dev/graphics/fb0`, major/minor `29,0` (`/dev/fb0` is absent) |
| Permissions | `0666`, owner `root`, group `graphics` |
| Kernel driver | `mxc_epdc_fb` (`/proc/fb`) |
| Bits per pixel | `16` |
| Stride | `1216` bytes |
| Virtual size | `608x1792` |
| Modes | `600x800` and `800x600` |
| Rotation | `3` |
| State | `0` |

The sysfs device resolves to
`/sys/devices/platform/mxc_epdc_fb/graphics/fb0`. The 16-bpp framebuffer and
the larger virtual geometry mean the native capture code must use the queried
line length and visible dimensions; it must not assume a tightly packed
600x800 8-bit buffer. The `mxc_epdc_fb` driver is an important lead for the
T1-specific refresh ioctl and update semantics.

### Vendor refresh ABI

The installed `/system/lib/hw/gralloc.imx5x.so` was pulled and inspected on
2026-09-15. Its update routine contains these constants:

| Operation | ioctl | Evidence |
|---|---:|---|
| `MXCFB_SET_AUTO_UPDATE_MODE` | `0x4004462d` | vendor gralloc call with mode `0` |
| `MXCFB_SEND_UPDATE` | `0x4044462e` | vendor gralloc call with a `0x44`-byte payload |
| `MXCFB_WAIT_FOR_UPDATE_COMPLETE` | `0x4004462f` | vendor gralloc call with an update marker |

The native agent models the payload as a 0x44-byte `mxcfb_update_data`: the
standard update rectangle, waveform mode, update mode, marker, ambient
temperature sentinel, flags, alternate-buffer fields, and trailing reserved
words. The test uses the GC16 waveform, partial update mode, ambient
temperature (`0x1000`), and marker IDs 1 and 2 for the test and restore.

The framebuffer mapping rejects `msync()` with `EINVAL` on this image. The
vendor gralloc does not call `msync()` either, so the native test relies on the
shared read/write mapping followed by the EPDC update ioctl.

### Partial refresh and waveform timing

The original native runtime repainted the complete 600x800 screen in response
to each completed touch frame and submitted a full-screen update rectangle.
Although the update payload selected `UPDATE_MODE_PARTIAL`, that only made the
request eligible for partial processing; it did not make the requested region
small. The runtime first moved to semantic regions: touch details use
`(left=20, top=350, width=560, height=94)`; key and power details use
`(left=20, top=434, width=560, height=210)`; initial and ownership-sensitive
redraws remain full-screen.

Device-side timing showed that region size is not the primary latency control
when using GC16. A full-screen GC16 update completed in about 703--714 ms, a
560x490 GC16 update in about 703--712 ms, and a 560x94 GC16 touch update in
about 710 ms after the update queue was clear. The waveform choice is much
more significant for this panel. The same reversible 200x120 marker probe
completed in:

| Waveform | Device-side update time | Result |
|---|---:|---|
| `DU` (1-bit) | ~273 ms | accepted; fastest tested |
| `GC4` (4-level grayscale) | ~614 ms | accepted |
| `GC16` (16-level grayscale) | ~700 ms | accepted |
| `A2` (2-level) | ~687 ms | accepted, but not fast on this firmware |

The live native touch-region test then completed three `DU` updates in
377--381 ms. The variability versus the smaller marker probe is consistent
with panel/update-queue state, but still represents roughly half the GC16
latency. The native diagnostic screen is deliberately black and white, making
`DU` a reasonable fast path for its transient details. Repeated `DU` updates
may trade grayscale quality and ghosting for responsiveness; this has not yet
been characterized optically on the T1.

### Asynchronous partial submissions

The blocking wait was then separated from the update submission. A bounded
`DU NOWAIT` marker probe returned from `MXCFB_SEND_UPDATE` in 0 ms, left the
marker intact during a one-second interval, and was followed by a normal GC16
restore update. This demonstrates on this firmware that a partial update can
be queued without synchronously waiting for the entire waveform.

The native runtime now submits touch, key, and power-detail updates with
`completion=nowait`. It retains the most recent update marker and drains that
marker before any full or status redraw, including the full redraw used before
suspend. A live injected touch frame produced:

```text
display refresh region=(20,350 560x94) waveform=DU completion=nowait elapsed_ms=0 status=ok
waited for pending display marker=15 elapsed_ms=0
display refresh region=(20,88 560x490) waveform=GC16 completion=wait elapsed_ms=707 status=ok
```

This removes the application-side ~374 ms stall observed with a blocking DU
update. It does not mean the panel has finished changing at 0 ms; it means the
driver accepted and queued the update while the native loop remained able to
process input. The driver queue behavior under rapid repeated touches and the
optical ghosting produced by repeated DU updates still need a physical visual
test. A periodic GC16 cleanup policy should be based on that observation, not
on framebuffer-memory captures alone.

The probe accepts only the four waveform values above even though other MXC
EPDC headers define additional modes. Acceptance of an ioctl does not by
itself establish good visual output, so unsupported or poor modes should be
tested with the bounded marker and restored with GC16. The next useful test is
to repeat many `DU` updates in one small region, make a visual observation
after the sequence, and determine when a GC16 cleanup refresh is needed. A
framebuffer capture can verify the memory contents but is not an optical
capture of the e-ink panel.

### Pixel damage tracking

The display pass now renders the complete logical screen into an owned,
tightly packed RGB565 image instead of drawing directly into the shared mmap.
After the first completed full-screen update, the native display retains a
completion-tracked shadow image. Each later redraw compares both bytes of every
visible RGB565 pixel against that shadow and submits the smallest enclosing
changed rectangle. Identical frames are skipped. The semantic region remains a
waveform hint and a fallback for an invalid shadow; if the actual change
escapes that hint, the update is promoted to GC16 rather than silently using
the DU fast path.

Mapped framebuffer writes are serialized behind one outstanding update.
Asynchronous transient submissions still return without waiting, but their
rectangle pixels are snapshotted and the next write waits for their marker
before touching the mmap. The shadow advances only after the corresponding
marker completes. This is a correctness baseline; a future display worker can
coalesce newer frames while one update is in flight. No EPDC alignment quantum
has been established yet, so damage padding and alignment default to one pixel
and remain configurable in the pure damage planner.

### Document refresh policy

MR-17 keeps all waveform and damage decisions in `prs-t1-agent`. The shared
Markdown reader only supplies a page display list; the T1 adapter classifies
that list as monochrome or grayscale. A loaded raster image, a non-black/white
fill or border, or non-extreme syntax/text ink makes a page grayscale. This
keeps code backgrounds, table header fills, gray syntax spans, and images away
from the fast one-bit page-turn path without adding device concepts to
`prs-markdown`.

The selected policy is:

| Event | Dirty region | Policy |
| --- | --- | --- |
| First draw, full redraw, details entry/exit, or resume | Full screen | Forced, synchronous GC16 |
| Monochrome text page turn | Document viewport | Asynchronous DU for four completed turns, then forced synchronous GC16 cleanup |
| Grayscale/image page turn | Document viewport | Forced, synchronous GC16 |
| Status-only change while reading | Status bar | Asynchronous DU; the document region is not submitted |
| Details/status/input diagnostics | Existing details region | Asynchronous DU with pixel damage |
| Retained link/interaction message | Document viewport | Asynchronous DU with pixel damage |

The first four ordinary text page turns are intentionally queued with DU. The
fifth waits for the preceding marker and submits a forced GC16 update over the
document region, which bounds the fast-update run and provides a quality
cleanup. Any grayscale/image page or forced full redraw resets the counter;
status and transient updates do not consume it. `NativeDisplay` still compares
the complete packed frame and promotes a change outside the semantic hint to
GC16. This keeps unexpected status/document changes from being silently
omitted.

The policy is stateful only at the display boundary. `ReaderEvent::PageChanged`
and navigation events update the logical reader page first; choosing DU,
promoting to GC16, waiting for a marker, or retrying the same rendered frame
does not call pagination again. A failed submission does not advance the
cleanup counter.

The available waveform choices were accepted by the tested T1 firmware and
measured in the earlier marker experiments: DU was about 273 ms for a small
marker and 377--381 ms for live touch-region updates, GC4 about 614 ms, GC16
about 700 ms, and A2 about 687 ms. GC16 is therefore used for intentional
grayscale rather than A2 or GC4 based solely on latency. These tests measured
driver acceptance and completion time; they did not measure optical ghosting,
because a framebuffer capture cannot see the panel surface. The four-turn
cleanup cadence is consequently a conservative starting policy. A hardware
follow-up should turn pages repeatedly through the checked-in plain-text,
table/code, and image documents, inspect the panel after each four-turn run,
and adjust the constant only from that optical observation. No fresh hardware
was connected in the MR-17 implementation runner.

The input devices are:

| Node | Name | Handlers / role |
|---|---|---|
| `/dev/input/event0` | `gpio-keys` | hardware keys |
| `/dev/input/event1` | `SONY IR Touchpanel` | touch coordinates |
| `/dev/input/event2` | `wm831x_on` | power/on input |
| `/dev/input/event3` | `phxlit_vbus` | VBUS switch events |
| `/dev/input/event4` | `sub_cpu_pwrbutton` | power button |

The event nodes are `0660 root:input`; a non-root native process will need the
`input` group or an appropriate udev/device-permission arrangement. The T1 has
`/dev/sub_cpu` (major 10, minor 63), not `/dev/subcpu`; this is the kernel's
sub-CPU SPI byte interface for update mode, not the normal input path. The
driver returns `EFAULT` when it is read in normal mode, so it is not a useful
replacement for the power evdev nodes. The PRS-350 sub-CPU input path must not
be copied into the T1 agent. The touch device reports `EV_KEY`, `EV_ABS`, and
`EV_SYN`. The native ioctl probe reported these capabilities for `event1`:

```text
name="SONY IR Touchpanel" id=0013:0000:0000:0001
EV_SYN EV_KEY EV_ABS
ABS_X min=0 max=800
ABS_Y min=0 max=600
ABS_MT_TOUCH_MAJOR min=0 max=31
ABS_MT_POSITION_X min=0 max=0
ABS_MT_POSITION_Y min=0 max=0
```

The zero ranges for the multitouch compatibility axes suggest that the useful
coordinates are the legacy ABS_X/ABS_Y pair, but this still needs confirmation
from a bounded raw event capture. The key devices expose ordinary `EV_KEY`
events.

## Native probe and screenshot

The first, historical `prs-t1-agent` ARMv5-musl build was copied to
`/data/local/tmp/prs-t1-agent` and run through root ADB while Android remained
running. The read-only `probe` completed without stopping any Android process.
It confirmed the framebuffer metadata above and observed `dispd`, `netd`,
`zygote`, `system_server`, and `/sbin/adbd` running. The init configuration
defines services for `dispd`, `adbd`, `netd`, `zygote`, `wpa_supplicant`, and
`dhcpcd`; at the time of the probe, `wpa_supplicant` and `dhcpcd` were stopped.

The Android service manager has a registered `SurfaceFlinger` service, and
`dumpsys SurfaceFlinger` reports active 600x800 layers, 608-pixel buffer
strides, and several allocated display buffers. The process listing identifies
`/system/bin/dispd` as running rather than a process named `surfaceflinger`.
This makes `dispd` and the SurfaceFlinger-compatible service path the first
display-ownership candidates; it is not evidence that zygote should be
stopped.

The first open-file ownership snapshot refined this: `system_server` held four
descriptors for `/dev/graphics/fb0` and descriptors for all five event devices,
while `dispd` and `zygote` held no framebuffer descriptor. The PIDs are
ephemeral, but the descriptor targets were:

```text
system_server -> /dev/graphics/fb0 (four descriptors)
system_server -> /dev/input/event0..event4
```

This does not prove which Java service issued the framebuffer ioctls, but it
does show that stopping zygote would be a broad way of killing the current
direct owner: zygote is the parent of `system_server`. The working hypothesis
is now to identify the Sony display code inside or below `system_server`, or
use a controlled display-service transition, before considering any zygote
stop. `dispd` remains relevant because it is an init-managed display service,
but it did not hold the node during this idle snapshot.

The native `capture` command opened `/dev/graphics/fb0` read-only and emitted a
valid 600x800, 16-bpp-to-gray PGM. The preserved ignored artifact is
`device-dumps/prs-t1/captures/native-agent-framebuffer.pgm`:

```text
size: 480015 bytes
sha256: 520e1cda1b9fd1589e24e7cad13074d7b9c1aed616f1aa78d11c08d17c2357aa
```

The old T1 ADB daemon closes `adb exec-out`, while direct binary output through
`adb shell` is PTY-translated to CRLF. The reliable capture route is to redirect
the PGM to `/data/local/tmp` on the reader and retrieve it with `adb pull`.
No framebuffer write or refresh ioctl has been attempted. Display-process
ownership, redraw races, and the effect of stopping only the Sony UI layer
remain to be tested.

The bounded raw evdev capture now runs against `event1` for a finite duration,
without `EVIOCGRAB` or event injection. Its first three-second idle sample
returned zero events; a physical touch/key sample is still needed to validate
the event values and coordinate orientation.

### Reversible native render test

Before the write-capable test, a fresh stock framebuffer capture after reboot
was preserved as `device-dumps/prs-t1/captures/stock-after-render-recovery.pgm`:

```text
size: 480015 bytes
sha256: d0ce520160b0863257ba57724dd1be5585a11755f61861e9962b9d47afb93d01
```

The first harness attempt wrote its marker but stopped at `msync()` with
`EINVAL`, before issuing an update. The reader was rebooted through the normal
ADB route to restore a clean Android display state. The corrected test then:

1. opened `/dev/graphics/fb0` read/write and mapped it once with
   `PROT_READ|PROT_WRITE`;
2. backed up and replaced the centered rectangle `(left=200, top=340,
   width=200, height=120)` with a black-and-white RGB565 marker;
3. sent the vendor partial GC16 update and waited for marker `1`;
4. captured the marker from the existing mapping into
   `native-custom-render-test.pgm`;
5. observed that the mapped rectangle still differed from the backup before
   and after the five-second wait; and
6. restored the original bytes and sent a second update with marker `2`.

The captured custom-render artifact is:

```text
size: 480015 bytes
sha256: d75b76a5f4ce592ffdaf0a94f0c6f79eaa6daf471b7c9d3fe437354d0543e536
```

A read-only capture taken after the restore has sha256
`8dad2ad301c64952381982e1b419338989624353ed0b791c89f531db1a5bf9f1`.
Its whole-frame hash differs from the pre-test capture because Android changed
pixels outside the test rectangle, but the 200x120 test rectangle itself is
byte-for-byte identical in the two stock captures. This supports restoration
of the bytes touched by the test, not a claim that the entire Android screen
was frozen.

The update calls returned successfully, the framebuffer marker stayed present
for the full wait, and `adbd`, `zygote`, and `dispd` remained running. This is
the first evidence that native writes plus the T1 refresh ABI can operate
beside the stock Android stack. It is not an optical screenshot of the panel,
and it does not yet prove that another Android component will not redraw the
same region during a longer native UI run. Display ownership and the smallest
component that can be suspended remain unverified; zygote and
`system_server` were not stopped.

### Input-triggered redraw

A stricter 60-second render test ran with the marker visible while the native
agent also recorded `/dev/input/event1` (the touchpanel) and
`/dev/input/event0` (gpio-keys). The pre-input capture was visually converted
to PNG and showed the marker over page 2 of the stock Android UI. During the
wait, a physical tap and button press were performed. The logs recorded:

```text
touch event1: ABS code 54 value 771, ABS code 53 value 73, then SYN_REPORT
button event0: key code 105 value 1, then value 0
```

The touch coordinates correspond to the lower-left page-2 navigation control;
key code 105 is `KEY_LEFT` on this input device. The post-input framebuffer
capture showed Android page 1, with the native marker gone. The agent reported
`framebuffer_marker_changed_after_wait=true` but
`exact_marker_preserved_after_wait=false`; the latter is the important result.
The marker region changed because the existing Android UI redrew the screen in
response to input. No Android process was stopped, grabbed, or signaled.

This confirms that direct framebuffer access is not sufficient for a
persistent native UI while Android remains the active display owner. The next
display investigation must determine the smallest safe ownership/refresh
boundary; touch and button mapping should follow that decision.

### Idle timer and power observations

A 90-second render test with Android fully running and no input reported
`exact_marker_preserved_after_wait=true`. This does not prove that every
periodic status update is harmless, but it shows no timer/status redraw occurred
during this interval. The capture is preserved as
`native-render-idle-90s.pgm` with SHA-256
`49c9f7bf1b2362a07ec048250004e61cb074b67ccf9c3894d45057ccac983d4c`.

The kernel exposes `/sys/power/state` (`standby mem`) and the Android-era
`/sys/power/wake_lock` and `wake_unlock` interfaces, owned `0660 radio:system`.
Root native code should be able to hold a named kernel wake lock while its UI
is active, preventing suspend without relying on `PowerManagerService`.
`/sys/android_power/request_state` is absent even though the zygote
`onrestart` stanza in `/init.rc` still attempts to write it; that init stanza is
stale on this image. Kernel wake sources, including USB and the reader's power
controller, remain present independently of zygote, but Android policy for
sleep/wake and power-key handling lives in `system_server` and would be lost
when zygote is stopped.

### Power-button and reset escape paths

The T1 exposes two power-like Linux input devices: `wm831x_on` on `event2` and
`sub_cpu_pwrbutton` on `event4`. Both advertise Linux `KEY_POWER` (code 116),
and the available `qwerty.kl` maps that code to Android `POWER WAKE`. Android's
higher-level short/long power-button behavior is therefore normally handled by
the framework window policy in `system_server`; it is unavailable while zygote
is stopped.

There is no separate reset input node in the current inventory. The matching
Sony kernel source shows that `sub_cpu_pwrbutton` is a platform input driver:
the sub-CPU IRQ thread reads the charger/status register and emits `KEY_POWER`
when `EXT_C` bit `0x04` changes. The same source shows that `wm831x_on` emits
`KEY_POWER` from the WM831x ON-pin interrupt and polls the ON-pin status for
release. The live device reports `power_key_enable` as enabled and status
register `0x3f` (`EXT_C`) as `0x00` when idle. A physical press captured during
the investigation produced no event on either evdev node and no corresponding
interrupt-count change, so the missing signal is below our native event
consumer rather than a short/long-press policy issue. The physical switch,
PMIC ON pin, and sub-CPU status path still need electrical/IRQ-level
correlation before a native UI can depend on them. It should not be treated as
the primary escape route without that test.

The verified recovery route is root ADB followed by a normal `adb reboot`.
`adbd` remained running during zygote isolation, and a normal reboot restored
the framework. A native UI should also hold a kernel wake lock while zygote is
stopped so that recovery does not depend on a suspended USB link.

### User-mode sleep/wake design

In a user-mode native UI, the replacement for Android's power policy should be
an explicit state machine:

1. **Active:** hold a named lock through `/sys/power/wake_lock`, poll the power
   sources (`event2` `wm831x_on` and `event4` `sub_cpu_pwrbutton`), and keep the
   framebuffer mapped only while rendering.
2. **Sleep request:** finish any e-ink update, preserve the last displayed
   image, release the wake lock, request EINK `standby` through
   `/sys/power/state`, and wait on `/sys/power/wait_for_fb_wake` rather than
   assuming the state write is synchronous.
3. **Wake:** let the PMIC/kernel wake source resume the process, reacquire the
   lock immediately, re-query `/dev/graphics/fb0` while retaining its existing
   mapping, drain the wake key, and redraw the complete native screen before
   accepting normal input.
4. **Long power press:** measure press duration from `KEY_POWER` events and
   choose a native action such as sleep, reboot, or shutdown. A hardware reset
   remains the last-resort escape.

The `power/wakeup` attributes exist under all five input devices but read back
empty on this legacy kernel, so wake capability is not yet proven from sysfs.
The `wm831x_on` PMIC path is the best candidate for a zygote-independent wake;
touch wake and the exact long-press/reset behavior require a reader-side
test. The development configuration should hold the wake lock continuously,
which also keeps the USB/ADB recovery path available. Production user mode
can later add suspend once the PMIC wake path is verified.

### Standalone runtime implementation

The native crate now has an opt-in `standalone-test` mode. It requires zygote
to be stopped first, holds `prs-t1-native-test` through the kernel wake-lock
interface, renders a full-screen diagnostic pattern, reads event0/event1/event2
and event4, displays raw touch/key data, and handles power-key duration. A
short power press requests EINK `standby`; a power press of at least two
seconds invokes `/system/bin/reboot`.

The first smoke run exposed that the framebuffer changed to `yoffset=896` after
zygote stopped. The runtime was corrected to honor both visible framebuffer
offsets. The corrected smoke capture was visually inspected after PGM-to-PNG
conversion and showed the complete native pattern; a synthetic key updated the
on-screen diagnostics. The captures are preserved as
`standalone-test-smoke-offset.pgm` and `standalone-test-after-key.pgm`.

The first suspend test used an interactive ADB shell. A synthetic short power
press reached the native state machine and caused `/sys/power/state` to enter
the suspend path, after which the USB/ADB link disappeared. The shell-owned
native process did not survive that disconnect; on wake, init had restarted
`zygote` and `system_server`. Launching the process with redirected standard
streams and an ignored `HUP` trap detached it from the ADB shell, and the
process remained present while zygote was stopped. This is now the required
development launch form.

During a monitored physical-button test, no event arrived on any of
`event0`–`event4` and no new kernel suspend message was emitted; the native
process remained active. The synthetic path therefore proves the native sleep
handler, but the physical PMIC/sub-CPU power-button path is not yet verified in
zygote-isolated mode. The actual physical wake and long-power reboot paths
still need a working physical power event (or a lower-level PMIC/sub-CPU
investigation).

### Normal-mode physical power-button test

After a normal reboot, the live kernel reported `power_key_enable` as enabled.
Two independent 30-second native readers were attached directly to
`event2` (`wm831x_on`) and `event4` (`sub_cpu_pwrbutton`) while the reader's
physical power button was held for approximately eight seconds and released.
Both readers reported `event_count=0`. The IRQ baseline was 8 for
`SPI_SUB_INT_IRQ` and 1 for `wm831x_on`; no change attributable to the button
hold was observed, and the WM831x status count remained 1. A follow-up status
read showed sub-CPU status register `0x3f` / `EXT_C` equal to `0x00`, with the
power-key bit `0x04` clear. The two additional sub-CPU IRQs seen afterward
occurred during the explicit status-query interaction, not the physical hold.

The `gpio-keys` node's advertised key list also excludes code 116
(`KEY_POWER`). This rules out the ordinary evdev consumer as the missing
layer: the stock drivers are enabled, but this physical switch is not
currently producing the PMIC ON-pin interrupt or the sub-CPU status transition
that would feed either power evdev node. The next investigation should trace
the switch and its board-level connection to the WM831x or sub-CPU, using the
service documentation or electrical/IRQ observation; user-space input handling
cannot recover a signal absent at that layer.

### USB/ADB power-key gate

The matching `sub_cpu-pwrbutton.c` source adds an important condition before
emitting `event4`: unless `CONFIG_USB_G_SERIAL_MODULE` is set, it allows the
power key only when AC is online and charger detection is complete, or when
neither USB nor AC is online. The running kernel reports
`CONFIG_USB_G_SERIAL` unset, and the live power-supply nodes report
`sub_cpu_usb/online=1` and `sub_cpu_ac/online=0` while the ADB cable is
connected. That combination suppresses the sub-CPU power-key event by design.

This explains why a physical press can appear dead in the current ADB-connected
test even under an otherwise normal Android boot. The WM831x `event2` path is
separate and should still be observed if the switch is wired to the PMIC ON
pin, but the device's symptoms point toward the gated sub-CPU path. The next
non-invasive verification is to run detached event readers, unplug USB, press
the button, reconnect USB, and inspect their persisted logs and the IRQ delta.

That verification was performed in normal Android: with USB connected, an
approximately eight-second hold produced no power action; after USB was
unplugged, a brief physical press put the reader to sleep, and reconnecting
USB woke it again. The reconnect enumerated as mass storage only, so ADB was
not available to retrieve the detached logs, but the reader-side behavior
confirms that the earlier no-event result was caused by the USB/charger gate,
not by a dead physical switch. A native test must either run with USB
disconnected or account for this firmware policy; development recovery should
use Wi-Fi ADB or a separate non-USB route when testing power behavior.

### Raw input injection

Root ADB can invoke the T1's `/system/bin/input`, but this old build only
supports `text` and `keyevent`, and Android-level DPAD keyevents did not
navigate the reader UI. The gpio-key path can be reproduced with root
`sendevent`: injecting Linux key code 106 (`KEY_RIGHT`) navigated from page 1
to page 2, and key code 105 (`KEY_LEFT`) returned to page 1. This gives us a
repeatable way to exercise Android input without physical access to the
reader. The native agent itself remains read-only with respect to input.

### Zygote ownership boundary test

With `dispd` and `adbd` left running, `stop zygote` stopped the Android
`system_server` process and removed the framework services, including
SurfaceFlinger. A 15-second native render test plus an injected raw button
event reported `exact_marker_preserved_after_wait=true`, unlike the same test
with Android running. This is strong evidence that zygote/system_server is the
effective redraw and input-response boundary for this firmware.

Starting zygote again in place was not a valid recovery route: the newly
started `system_server` repeatedly crashed in `PackageManager` with a
`NullPointerException`. A normal reboot restored root ADB, `zygote`,
`system_server`, and `dispd`. We therefore must use a reboot-based recovery
after any zygote stop until a cleaner framework restart sequence is found.

### Detached suspend and wake observation

The detached standalone test was then run with zygote stopped and USB
disconnected so that the firmware power-key gate would permit the physical
button. A brief press put the reader to sleep. On suspend, however, the
display immediately changed from the native diagnostic pattern to the stock
Reader sleep screen (the “Reader is in sleep mode ...” message and a book
cover). This shows that at least one suspend-time vendor or kernel display
path can replace the framebuffer contents even though zygote and the Android
framework were stopped.

Pressing the physical power button again while the stock sleep screen was
shown produced no visible change. Reconnecting USB woke the reader, but it
still displayed the stock sleep screen. The reconnect again exposed only the
Mass Storage USB interface; ADB was therefore unavailable for the pending
post-wake inspection of `prs-t1-agent`, zygote, `system_server`, `dispd`, and
the native runtime log. The next test step is to restore the ADB-enabled USB
configuration and determine whether the native process resumed and failed to
redraw, or whether it was stopped/replaced during suspend.

The matching Sony EPDC source resolves the display ownership question. The
kernel driver allocates a second, hidden framebuffer-sized buffer for the
standby image. Android supplies the stock sleep image through the driver's
`MXCFB_WRITE_SSCREEN` ioctl; during its early-suspend callback the driver
submits that hidden buffer with `use_standbyscreen=true`, independently of
zygote, SurfaceFlinger, or `dispd`. Its late-resume callback powers the EPDC
back up, clears the panel, and clears the visible framebuffer. A native UI
must therefore expect its image to be replaced on suspend and issue a fresh
full-screen update after resume. The source is from Sony's [PRS-T1 Linux
source archive](https://oss.sony.net/Products/Linux/Audio/PRS-T1JP_20140702.html).

The hardware reset restored the normal framework and USB ADB interface. The
saved native runtime log from that first run contained `error: Invalid
argument (os error 22)` after the initial standalone-test banner, consistent
with the process exiting on its first post-resume framebuffer operation. The
instrumented build identified that failure as a second mmap of `/dev/graphics/fb0`:
the driver reported normal `600x800`, `smem_len=2179072` metadata but rejected
the new mapping on every retry.

The standby test was then extended to supply a native image through
`MXCFB_WRITE_SSCREEN` immediately before suspend. The reader displayed
`KERNEL STANDBY IMAGE`, proving that the native process can replace the stock
standby screen. The runtime was corrected to retain the original framebuffer
mapping across suspend and re-query only the framebuffer metadata after wake.

A subsequent manual test produced the following complete sequence in the
detached log:

```text
suspend request returned: Ok(())
wake lock reacquire returned: Ok(())
post-resume framebuffer 600x800 virtual=608x1792 smem_len=2179072 stride=1216 offsets=(0, 0)
framebuffer mapping retained across resume
framebuffer remap returned: Ok(())
```

The display showed `WOKE - INPUT READY`, confirming that the native process
survived suspend, refreshed the post-resume offsets, and reached its redraw
path. The user reported that this appeared immediately while testing the
power button; the exact wake source (a second power transition versus USB
resume during the same interaction) still needs event-level logging if that
distinction matters. The retained-mapping fix is committed as `4ad50fc`.

### Wake-source instrumentation

The legacy kernel exposes no `wakeup_count`, wake-reason, or equivalent sysfs
interface. The native runtime now logs every `KEY_POWER` event with its source
(`event2` PMIC or `event4` sub-CPU), value, and input timestamp. It also logs
the elapsed time spent in the `/sys/power/state` suspend request and displays
that duration in the wake message as `WOKE AFTER NMS`. This distinguishes an
actual immediate wake from a normal wake whose retained e-ink image was
observed before the user noticed the redraw.

The diagnostic build is committed as `d99b419`. The next controlled run should
disconnect USB, initiate one short press, wait without touching the reader,
and record the displayed duration and persisted event log. A sub-second
duration with a power event would justify a release/debounce guard; a longer
duration with no power event would point to another wake source such as USB.

### Native EINK standby wake failure

The vendor power-state bridge was then installed at
`/data/local/tmp/prs-t1-power-state`. Its `on` operation returned success while
the reader was awake, and the native runtime logged that it selected the
vendor `standby` operation rather than the raw `/sys/power/state` fallback.
The reader displayed the native pre-suspend/standby status, so both the
framebuffer standby image and the vendor suspend request were reached.

With zygote stopped, USB disconnected, and the native runtime running, the
power-key log was:

```text
power event source=E4 value=1 ... mode=ACTIVE
power event source=E4 value=0 ... mode=ACTIVE
power release source=E4 duration_ms=255 action=SLEEP
drawing pre-suspend screen
standby screen supplied
requesting suspend mode=EINK_STANDBY
requesting vendor suspend helper state=standby
suspend request queued: Ok(())
```

The second physical power press produced no wake-side event, no
`wait_for_fb_wake` return, and no post-resume redraw before the reader had to
be reset. Reconnecting USB while suspended did not wake or re-enumerate the
ADB interface; after reset, ADB returned and the log was preserved. This
distinguishes the current failure from a post-resume framebuffer redraw bug:
the native process never reached its wake handler. The current evidence does
not yet identify whether the physical button failed to assert a Linux wake
interrupt in this zygote-free EINK path or whether the kernel resumed without
delivering the event. A direct vendor-resume/control experiment and kernel
wake-source observation are the next steps.

The 9 ms result then identified a more fundamental issue: on this early-suspend
kernel, writing `mem` to `/sys/power/state` only queues the suspend work and
returns; it does not wait for suspend or wake. The native runtime was therefore
reacquiring its wake lock and redrawing immediately, producing a self-induced
instant wake. The live T1 exposes `/sys/power/wait_for_fb_wake` as a blocking
wake barrier. The runtime now waits on that barrier after queuing suspend,
then reacquires its lock and redraws only after a real display wake. This fix
is committed as `e6fd8c8` and is ready for the next physical-button test.

The next test confirmed that the barrier prevented the self-induced wake, but
the physical power button did not wake the reader from the `mem` path. The
Sony sub-main driver sets its standby flag only for
`EARLY_SUSPEND_MODE_NORMAL`; that path disables `SPI_SUB_INT` wake during
suspend. The runtime now requests `standby` instead, which selects
`EARLY_SUSPEND_MODE_EINK` and leaves the sub-CPU wake path enabled. The active
native image is retained in EINK standby; the hidden standby buffer remains
supplied for the normal-mode path. This change is ready for a physical wake
test.

The next EINK test was run with USB physically disconnected before the reader
was put to sleep. It produced the same result: the log stopped after
`suspend request queued: Ok(())`, with no return from
`/sys/power/wait_for_fb_wake`; reset was required. This rules out USB power-key
gating as the sole explanation for the native wake failure.

The Sony kernel sources narrow the remaining issue to wake routing below the
native process. EINK early-suspend mode leaves the sub-CPU interrupt path
enabled, but the `wm831x_on` input driver does not call `enable_irq_wake()`.
The separate `wake_request_from_sub_cpu` PMIC handler only acquires an Android
wake lock; it does not itself resume the system or report a key. The native
process cannot repair either kernel-level condition from user space. A
controlled stock-Android test then confirmed that a second physical press
does wake the normal `mem` suspend path with USB disconnected. The hardware,
PMIC, and stock kernel wake configuration are therefore functional; the
failure is specific to the native EINK suspend path. The native runtime now
accepts an optional `mem` mode so it can reproduce the stock early-suspend
path for the next test, without removing the EINK mode used for comparison.

The stock-style native `mem` test then also failed to produce a visible wake,
while normal Android continued to sleep and wake correctly. The important
difference is that the Android framework handles the power key after the
hardware wake interrupt and requests `on`, which clears the kernel's pending
early-suspend request and runs the late-resume handlers. The native runtime
had been blocked in `wait_for_fb_wake` and could not perform that handoff. It
now polls the power evdev nodes during the wake wait and writes `on` when the
first wake-side power event arrives; the display barrier is still used before
the framebuffer is redrawn.

### Native UI status panel

On 2026-09-16, the native status collector was integrated into the
`standalone-test` framebuffer UI. The panel refreshes its read-only snapshot
every five seconds and continues to show the native input and power diagnostics
on the same screen. The displayed fields are deliberately compact so they fit
the 600x800 T1 panel: battery level/state, temperature and AC, USB power and
ADB process state, Wi-Fi interface/supplicant state, free space on `/data` and
`/mnt/sdcard`, framebuffer state/rotation, and zygote/dispd process state.

The deployed test was started with zygote stopped. A device-side framebuffer
capture returned `600x800 16bpp`; the host-side grayscale conversion produced
an 8-bit PNG. The captured screen showed `BAT 100 FULL`, `TEMP 25 AC OFF`,
`USB ON ADB RUN`, `WIFI WLAN0 OFF`, `SUPP STOPPED`, `DATA 22067K FREE`,
`SD 1396884K FREE`, `FB ACTIVE ROT 3`, and `ZYGOTE STOP DISP RUN`. This is the
first screenshot of the custom status rendering itself, rather than only a
local preview. The first capture also exposed a decorative target overlapping
the first label; that target was removed and the clean recapture showed the
same values with `DATA 21602K FREE`.

The five-second refresh was then verified without physical interaction. A
temporary 2 MiB file under `/data` changed the displayed free-space value to
`DATA 19529K FREE` after the polling interval; the file was removed afterward.
The native process was terminated by rebooting; ADB returned after about 15
seconds and zygote, `system_server`, `dispd`, and `adbd` were all present again
after normal boot completed.

A second zygote-stopped run injected a safe legacy touch sequence through
`event1`: `ABS_X=73`, `ABS_Y=771`, followed by `SYN_REPORT`. The native panel
displayed `TOUCH X 73 Y 771` and incremented its touch-event counter. The
runtime now accepts both the T1's legacy `ABS_X/ABS_Y` axes and the multitouch
position codes, leaving coordinate-format handling ready for the next physical
sample.

The event diagnostics now label known Linux event types and codes while still
displaying their numeric values. This covers the observed `KEY_LEFT` (105),
`KEY_RIGHT` (106), `KEY_POWER` (116), `ABS_X`/`ABS_Y`, and multitouch position
codes. The native shell now applies the selected physical orientation to the
legacy touch axes; higher-level gesture semantics remain outside its scope.

The final presentation pass resized the four diagnostic panels and moved the
lower status block down so all rendered text has clear space from the panel
borders. A fresh 600x800 framebuffer capture confirmed the corrected layout;
the reader was then rebooted and returned to the normal Android services.

The standalone runtime opens and validates the single writable mapping, then
performs a fail-closed ownership check with `/system/bin/ps` before it draws or
acquires the kernel wake lock. If either `zygote` or `system_server` is present,
it refuses to start, reports both process states, and performs no framebuffer
update ioctl or pixel write. On this T1, Android's existing framebuffer mapping
may instead make the initial writable mmap return `EINVAL`; that is also a
safe refusal path. The stopped-framework path must therefore be launched only
after zygote has been stopped.

The repeatable host helper `crates/prs-t1-agent/tools/native-test.sh` now
encapsulates this sequence. Its `start` action pushes the release binary,
stops zygote, waits for zygote and `system_server` to disappear, and launches
the test detached from the ADB shell. Its `status` action prints the process
and read-only device snapshot, and its `reboot` action restores normal Android
and waits for ADB. The complete `start`/`status`/`reboot` path was exercised on
2026-09-16 without physical interaction; the reader returned with zygote,
`system_server`, `dispd`, and `adbd` running.

### Vendor power-state bridge

The installed `/system/lib/libhardware_legacy.so` was pulled from the reader
and inspected. It exports Sony's `set_screen_state(int)` entry point, with
state values `1=on`, `0=mem`, and `2=standby`. The function opens the legacy
power-state files and writes the selected state through Android's vendor
library rather than through Java code.

A small old-style ARM executable was built with the T1's `/system/bin/linker`
as its interpreter and dynamically resolved `set_screen_state()` with
`dlopen()`/`dlsym()`. A modern PIE build crashed in the Android 2.2 linker;
the non-PIE `ET_EXEC` build ran successfully. Invoking the helper with
`standby` queued `request_suspend_state: standby`. After the reader showed its
sleep screen, a second physical power-button press woke it while the stock
Android framework was still running. This confirms the vendor transition and
the stock wake path independently, but does not yet prove that the helper
alone resolves the zygote-stopped native wake failure.

The native runtime now uses `/data/local/tmp/prs-t1-power-state` when present
for both the suspend state and the wake-side `on` handoff, with the direct
`/sys/power/state` write retained as a fallback. The helper source and build
script are kept with the T1 crate for the next isolated zygote-stopped test.

### Read-only UI status inventory

While physical interaction is paused, the native agent now has a read-only
`status` command for values that can be polled while the reader is awake or
while Android's Java framework has been stopped. The first live inventory
reported:

```text
battery.status=Full
battery.capacity_percent=100
battery.voltage_uv=4200000
battery.temperature=26
power.ac_online=false
power.usb_online=true
wifi.interface=wlan0
wifi.interface_present=false
wifi.supplicant_state=stopped
adb.persist_enabled=true
adb.service_state=running
adb.process_running=true
android.zygote_running=true
android.system_server_running=true
```

The T1 exposes `sub_cpu_battery`, `sub_cpu_ac`, and `sub_cpu_usb` under
`/sys/class/power_supply`. The kernel battery node is more useful than the
legacy `dumpsys battery` result here: the latter currently reports
`present=false` despite the kernel reporting a full 4.2 V, 100% battery. The
USB power node is a reliable cable/power indication (`online=1` while the
reader is connected), but it is not the same thing as a live host ADB
transport. The device-side USB gadget sysfs directory is absent and
`sys.usb.config`/`sys.usb.state` are empty on this boot, so gadget functions
are reported as unknown; the USB descriptor remains the host-side authority
for whether the ADB interface actually enumerated.

Wi-Fi is currently disabled or not brought up: `wifi.interface` is `wlan0`,
but no `wlan0` netdev or `/proc/net/wireless` entry exists. The status code
therefore reports interface presence, operstate, carrier, and signal as
separate fields so a future UI can distinguish unavailable from connected
state when Wi-Fi is enabled. It also reports `adbd`, `zygote`,
`system_server`, and `dispd` process presence from `/system/bin/ps`, which does
not require the Android services to be running.

The command was also verified after `stop zygote`: it continued to report the
kernel battery, USB, Wi-Fi, and ADB fields, with
`android.zygote_running=false` and `android.system_server_running=false`,
while `adbd` and `dispd` remained present. A subsequent `adb reboot` restored
the normal framework. ADB re-enumerated about 14 seconds after the reboot,
before `system_server` was ready; the latter was present by about 20 seconds.
This makes separate ADB-daemon, USB-transport, and framework-readiness states
useful to the UI rather than a single generic "connected" flag.

The expanded live snapshot also reports `power.supported_states=standby mem`,
`screen.framebuffer_state=0`, `screen.rotate=3`, 22,067 KiB available on
`/data`, and 1,396,884 KiB available on `/mnt/sdcard`. The screen values are
kernel framebuffer state rather than Android display-service state, and the
storage values are intentionally kept in the T1 toolbox's KiB units so the UI
does not have to infer a block size. No thermal sysfs class is present on this
firmware, so thermal data remains an explicit future `unknown` field rather
than being fabricated from battery temperature.

## Exposed storage

The T1 file service exposes the internal eMMC as
`/dev/block/mmcblk2`. The whole-device size and partition sizes returned by
the read-only `size` probe are:

| Path | Bytes | Approximate size | Known role |
|---|---:|---:|---|
| `/dev/block/mmcblk2` | 1,958,739,968 | 1.82 GiB | internal eMMC |
| `/dev/block/mmcblk2p1` | 10,485,760 | 10 MiB | recovery filesystem |
| `/dev/block/mmcblk2p2` | 10,485,760 | 10 MiB | recovery/root image |
| `/dev/block/mmcblk2p3` | 1,024 | 1 KiB | partition-table container |
| `/dev/block/mmcblk2p4` | 1,514,995,712 | 1.41 GiB | `READER` storage |
| `/dev/block/mmcblk2p5` | 16,801,792 | 16 MiB | fonts |
| `/dev/block/mmcblk2p6` | 142,630,912 | 136 MiB | dictionaries |
| `/dev/block/mmcblk2p7` | 10,510,336 | 10 MiB | `SETTING` storage |
| `/dev/block/mmcblk2p8` | 41,967,616 | 40 MiB | preload |
| `/dev/block/mmcblk2p9` | 50,356,224 | 48 MiB | user data |
| `/dev/block/mmcblk2p10` | 134,242,304 | 128 MiB | Android system |

`/dev/block/mmcblk0` and its tested partition names are not exposed through
the T1 file-service namespace. All three logical SCSI units returned the same
`mmcblk2` size table and the same 1 KiB p3 contents; the correct target for
each physical storage role still needs to be established from successful
large reads.

## Acquisition attempts

The p3 partition was read completely through all three SCSI units. Each copy
was 1,024 bytes and had SHA-256:

```text
db5f06eab591be67d4257ca518a91ec8128bbe48ba945c3395ad575b36b05a04
```

The first p10 attempt through `/dev/sg0` produced 352,256 bytes before the
reader timed out. It was temporarily preserved as, then moved to Trash during
cleanup:
`device-dumps/prs-t1/raw/mmcblk2p10.partial.img`.

```text
size: 352256 bytes
sha256: 2a1b522156dabd6fb5247d7e6fcdd1600e11bd14630bfa1503d2c10ac8d51b41
```

A p8 attempt produced 12,288 bytes before the same failure; it was a
diagnostic sample and is not yet part of the preserved archive.

The p1, p2, p5, p6, and p9 attempts failed before producing data in the
current high-level copier. The p7 attempt previously produced 20,480 bytes
before timing out.

Range probes show that the failures are not confined to one fixed offset. For
example, p1 offsets 45,056 and 53,248 read successfully, while the 512-byte
ranges from 49,152 through 53,248 repeatedly timed out. Other p1, p2, p8, p9,
and p10 ranges also alternated between successful reads and resets. The exact
p1 range was reproducible through all three logical units, indicating a
problem on the shared internal eMMC path rather than one SCSI LUN.

The host client now has a reconnect-and-resume copier and an explicit forensic
`dump` mode. It retries the same offset after a SCSI failure, falls back to
512-byte reads around persistent failures, and records any final unreadable
sectors in a sidecar bad-range map while zero-filling only those sectors in the
output image. Several test passes were intentionally stopped before completion
to avoid producing a mostly zero-filled image while the reader was wedged. These
historical artifacts were moved to Trash after the complete pass; their sizes
and hashes remain here for reference:

| Historical artifact (removed) | Size | SHA-256 / note |
|---|---:|---|
| `mmcblk2p1.partial.img` | 49,152 | `073ccffff8ad2cd21dd09eeb1357d6f0e9955ca04cde1baee407a7a92d410c5f` |
| `mmcblk2p1.dump3.img` | 724,480 | interrupted forensic pass |
| `mmcblk2p1.dump5.img` | 233,472 | interrupted after the reader wedged at offset 0 |

The `dump3` and `dump5` images are incomplete diagnostics, not complete
filesystems. The next pass used a fresh physical reconnect and the reader's
data-transfer mode.

## Complete data-transfer-mode pass

After rebooting the reader and selecting its on-device `data transfer mode`, a
fresh sequential pass through `/dev/sg1` completed every partition at the
reported size. The images and sidecar maps are preserved under
`device-dumps/prs-t1/raw/` with the `data-transfer` suffix. Each bad-map file
contains only its remote path and size header; no failed ranges were recorded.

| Artifact | Size | SHA-256 |
|---|---:|---|
| `mmcblk2p1.data-transfer.img` | 10,485,760 | `9217112e1b0065ad6b3612ad8c68851b606299048f32715cba863baa2ba940e6` |
| `mmcblk2p2.data-transfer.img` | 10,485,760 | `2ebc40bb98d2dc4d2c155739224efeca2c776c1099a5604525c8b3343e1ae597` |
| `mmcblk2p3.data-transfer.img` | 1,024 | `db5f06eab591be67d4257ca518a91ec8128bbe48ba945c3395ad575b36b05a04` |
| `mmcblk2p4.data-transfer.img` | 1,514,995,712 | `82136fcca8750b726d80ceb4e10d6770bff2fd6d09517332ce99b4ae152f907c` |
| `mmcblk2p5.data-transfer.img` | 16,801,792 | `6dd00525fd42af2a14e4df20bd0802a85dfe235425f3056a4ae7a8ac1214d12c` |
| `mmcblk2p6.data-transfer.img` | 142,630,912 | `8a6c0ef528bb2ba270d07ef6dd2f28995377d709254e1eca47f881c80f57dcc6` |
| `mmcblk2p7.data-transfer.img` | 10,510,336 | `248a54138399cb047fba5e86ee9ee2aebd681bbc1b6cf74b8ca095b2e00b4a5f` |
| `mmcblk2p8.data-transfer.img` | 41,967,616 | `f59f78495cd806812b439d96764b943789d47f9cf741120a1e7a9c9ba83263b8` |
| `mmcblk2p9.data-transfer.img` | 50,356,224 | `e72a09e25ed59ffbbc0d58e4207c4fe069c210bddfda0eea3bc5bbce813f14ea` |
| `mmcblk2p10.data-transfer.img` | 134,242,304 | `4d0959f86e59d486912ee210de23b1b205f6317a1eea9090359f29dbbfe7d34f` |

`file` identifies p1 and p9 as ext4, p6 and p8 as ext2, p2 and p5 as
compressed ROMFS images, p4 and p7 as FAT filesystems, p10 as ext2, and p3 as
an MBR/extended partition-table container. The total image set is
1,932,477,440 bytes.

## Recovery selector experiment

With the reader in data-transfer mode, the documented Sony `0x70` selector was
sent to `/dev/sg1` with mode `1` (recovery) as a four-byte little-endian value.
The T1 returned a zero result and then displayed its shutdown/boot sequence.
After boot, however, the host saw only the single-interface USB mass-storage
gadget again: no `/dev/ttyACM*`, no `/dev/ttyUSB*`, and no ADB interface. ADB
is also not installed on this host. Thus no recovery root shell was exposed by
this attempt.

The normal selector (mode `0`) was then sent once. The T1 rebooted again and
returned as the normal mass-storage device with the three expected SCSI LUNs.
The recovery selector was not resent. The likely next route is the T1-specific
physical recovery-button sequence or a known T1 rescue/ADB-enabled recovery
payload; the PRS-350 updater-package route is not assumed to apply to this
Android-based T1.

## Western minimal root package

The reader's Sony UI reports firmware version `1.0.00.09270` under the Sony
Reader Settings -> About -> Device Information screen. This is distinct from
the Android build identifiers captured from the system image (`FRG83`, Android
2.2.1). The reported version confirms that this is an early Western firmware;
the exact root-package compatibility should still be treated cautiously until
the matching restore package is kept available.

The Western Flavor link on the [MobileRead PRST1 rooting page](https://wiki.mobileread.com/wiki/PRST1_Rooting_and_Tweaks)
was downloaded from:

```text
https://projects.mobileread.com/reader/users/porkupan/PRST1/flash_packages/minimal-root.zip
```

The download is a 14,481,424-byte outer ZIP containing a password-protected
inner `minimal-root.zip`. The public archive password is `mrdev`. The usable
inner archive is preserved as
`device-dumps/prs-t1/packages/minimal-root-western.zip`:

```text
outer sha256: 13e7356f8a41dbf5f52e95cffeab6cebd1d0f22618646d3ac03f86cde4dfaf78
inner sha256: 96f90c7271f2406ac7f4405e311f70b61bab34b6faa2ba1a7948f6bfe09a5bae
inner size:  14,481,214 bytes (59 files)
```

The inner package contains `PRS-T1 Updater.package`, the Windows helper files,
`sdcard/updates`, `sdcard/tmp/do_update.sh`, and the recovery helper. The
updater package SHA-256 is
`0018a4d3246dab24b79e8643bd0e6dff6546eed4c707491aa7473bfd18e31415`.
On 2026-09-15, after confirming the Sony UI firmware version
`1.0.00.09270`, the contents of `sdcard/` and `PRS-T1 Updater.package` were
copied to the internal `READER` volume. The destination contains the expected
`tmp/`, `updates/`, and updater marker, and all 28 payload files compare
byte-for-byte with the package. This staging was followed by a clean unmount
and one recovery-selector reboot; the resulting boot showed the launcher
chooser containing `ADWLauncher EX` and the stock `Home` launcher, consistent
with the root package having been applied.

After the minimal-root boot, the T1's USB gadget still exposed only the normal
mass-storage interface. No host-side `/dev/ttyACM*` or `/dev/ttyUSB*` node and
no ADB interface were present. The minimal package contains the Windows
`usbser.sys` host driver but no `adbd` binary or ADB configuration payload.

## Enable-ADB package

The historical `enable-adb.zip` package was downloaded from:

```text
https://projects.mobileread.com/reader/users/porkupan/PRST1/flash_packages/enable-adb.zip
```

The official server reports 367,448 bytes (the requested size was 367,488
bytes). The ZIP test passed and its SHA-256 is
`2b616e95c880fa29131ec3e26ac2951445e982cb66b2ea2dff3d6f1d8a3851b1`. It is
preserved as `device-dumps/prs-t1/packages/enable-adb.zip`.

This is not just a settings toggle: it contains `tmp/ramdisk-adb.uimg` and
`tmp/nboote.bin`. Its `do_update.sh` writes those payloads directly to the
internal eMMC at fixed offsets and then switches the boot selector.

The existing per-partition images do not include the unpartitioned boot area,
so the offsets were verified with a separate read-only raw-range acquisition
from `/dev/block/mmcblk2`. At `0x00500000` the T1 returned a U-Boot uImage named
`Normal Rootfs`, with ARM load and entry addresses `0x70308000`, matching the
ADB ramdisk's magic, name, and addresses. At `0x00f00000` it returned the
current boot environment, including `rawtable=0xF40000` and a boot command
that reads the normal ramdisk from `0x2800` for `0x1F4` sectors. The package's
`nboote.bin` has the same environment and changes that count to `0x258`
sectors, matching the package script's 600-sector write. These observations
verify both package offsets for this T1 without writing anything. The small
captures are preserved under `device-dumps/prs-t1/boot-area/`.

### Application and verification

After the exact boot offsets were verified and the matching restore route was
confirmed, the old minimal-root `updates/` staging tree was moved to the
reversible hidden directory `.previous-minimal-root-updates` on `READER`. The
six enable-ADB files were then copied to the root of `READER`, compared
byte-for-byte with the archive, and the volume was cleanly unmounted. One
recovery-selector reboot was issued. The reader re-enumerated with mass storage
and a second USB interface identified as `Reader Android ADB`.

Using the temporary official Android Platform-Tools client, `adb devices -l`
reported device serial `148427501398348`; `adb shell id` and `adb shell su -c
id` both returned `uid=0(root) gid=0(root)`. The device-side ADB daemon is
present at `/sbin/adbd`. This confirms that enable-ADB succeeded and provides
a root shell over USB. The original partition and boot-area backups remain
preserved for rollback work.

## USB failure evidence

During the larger reads, the host reported:

```text
usb 1-5: reset high-speed USB device number 5 using xhci_hcd
sd 0:0:0:2: Power-on or device reset occurred
sd 0:0:0:1: Power-on or device reset occurred
sd 0:0:0:0: Power-on or device reset occurred
```

The userspace error is SCSI host status `0x0003` (`DID_TIME_OUT`) with no
device status or driver status. After the reset, the USB interface is bound to
the `usb-storage` driver. The reset-aware, chunk-addressable reader remains
implemented in the host client for future recovery work, but it was not needed
during the complete data-transfer-mode pass.

## Minimal-root application cleanup

Once root ADB was available, the minimal-root additions were inventoried by
their package paths and backed up locally under
`device-dumps/prs-t1/packages/minimal-root-installed-apks/`. The following
packages were then removed with `pm uninstall`:

```text
com.android.calculator2
org.adwfreak.launcher
jackpal.androidterm
com.speedsoftware.rootexplorer
org.geometerplus.zlibrary.ui.android
org.coolreader
org.ebookdroid
com.dropbox.android
com.menny.android.anysoftkeyboard
com.socialnmobile.colordict
com.cooliris.media
com.coinsoft.android.barshortcuts
com.citc.colors
com.android.packageinstaller
```

`com.noshufou.android.su` was a system APK, so Android refused a normal
package uninstall. Its `Superuser.apk` was copied to the T1's
`/data/local/tmp/Superuser.apk.minimal-root.backup` and removed from
`/system/app/`; the host-side APK backup is the preferred recovery copy.

After reboot, all 15 minimal-root package names were absent from
`pm list packages -f`. `/system` was read-only again, ADB still provided a
root shell, `/system/bin/su` remained setuid-root, and
`/system/xbin/su` remained a symlink to it. Neither `su` path was modified.

## Native long-press reboot test

On 2026-09-16, the native runtime was launched after a clean reboot with the
vendor display state explicitly forced to `on`; this avoids the normal Android
idle timer entering standby before the native wake lock is acquired. The
runtime reached its active loop with zygote and `system_server` stopped. An
initial attempt to hold the power button while USB remained connected produced
no input event. After USB was physically disconnected, the same test succeeded.

The reader recorded the power transition from the sub-CPU source (`event4`):

```text
power event source=E4 value=1 timestamp_us=132553018 mode=ACTIVE
power event source=E4 value=0 timestamp_us=136554709 mode=ACTIVE
power release source=E4 duration_ms=4001 action=REBOOT
```

The reader rebooted normally. ADB returned and the post-reboot status showed
`zygote`, `system_server`, `dispd`, and `adbd` running again. This validates the
native long-press reboot escape route, and confirms that USB should be
disconnected for physical power-button tests. Native short-press sleep/wake
remains a separate unresolved test; a failed wake still requires the hardware
reset button or a reboot performed while ADB is available.

## Native `mem` sleep/wake test

On 2026-09-16, the native runtime was launched with the display explicitly
forced `on`, zygote and `system_server` stopped, and the suspend mode set to
`mem`. USB was physically disconnected before the power-button test. The
reader briefly updated the pre-suspend display and then showed a visibly
corrupted/static e-ink image while asleep; this is a remaining standby-image
quality issue, not evidence that the native state machine failed.

The preserved runtime log shows the complete wake handoff:

```text
power release source=E4 duration_ms=253 action=SLEEP
drawing pre-suspend screen
standby screen supplied
requesting suspend mode=NORMAL_MEM
requesting vendor suspend helper state=mem
suspend request queued: Ok(())
wake-side power event source=E4 value=1
requesting early resume
requesting vendor resume helper state=on
wake-side power event source=E4 value=0
display wake barrier released bytes=5
suspend/wake wait returned: Ok(()) elapsed_ms=5314
wake lock reacquire returned: Ok(())
post-resume framebuffer 600x800 virtual=608x1792 smem_len=2179072 stride=1216 offsets=(0, 896)
framebuffer mapping retained across resume
framebuffer remap returned: Ok(())
```

The user observed the native screen return to `STATE ACTIVE` after the second
power press. Reconnecting USB did not re-enumerate ADB, so the hardware reset
button was used to recover. After reset, ADB returned with zygote,
`system_server`, `dispd`, and `adbd` running. This validates the native
sleep-to-wake control path with USB disconnected, while leaving the displayed
standby image and post-wake USB/ADB re-enumeration for further investigation.

## Native EINK standby sleep/wake test

The EINK `standby` mode was tested on 2026-09-16 after the successful `mem`
comparison. USB was physically disconnected before the short power press. The
reader displayed a stable `STATE SLEEPING` / `EINK STANDBY MODE` screen, and
the user then woke it with one brief power press. The native UI returned to
`STATE ACTIVE`; its `wake power ignored` message confirms that the wake-side
release was suppressed by the two-second post-wake guard rather than being
misinterpreted as a second sleep request.

The preserved log recorded:

```text
power release source=E4 duration_ms=263 action=SLEEP
drawing pre-suspend screen
standby screen supplied
requesting suspend mode=EINK_STANDBY
requesting vendor suspend helper state=standby
suspend request queued: Ok(())
wake-side power event source=E4 value=1
requesting early resume
requesting vendor resume helper state=on
display wake barrier released bytes=4
suspend/wake wait returned: Ok(()) elapsed_ms=823
wake lock reacquire returned: Ok()
post-resume framebuffer 600x800 virtual=608x1792 smem_len=2179072 stride=1216 offsets=(0, 0)
framebuffer mapping retained across resume
framebuffer remap returned: Ok(())
```

This validates the complete native EINK suspend/wake control path, including
the wake-event handoff and retained framebuffer mapping. As with the `mem`
test, reconnecting USB did not restore ADB; a hardware reset was required to
recover normal Android and retrieve the preserved log. The EINK standby image
was stable in this run, unlike the transient corruption observed during the
earlier `mem` test.

## Native wake-time ADB rebind

The first native EINK wake tests restored the display and input path but did
not restore host ADB after the USB cable was reconnected. A normal Android
control test showed that the service itself can be restarted safely:

```text
stop adbd
start adbd
```

ADB re-enumerated after that stop/start sequence. The T1 is using the legacy
USB-ADB driver rather than the modern `/sys/class/android_usb/android0` gadget
interface: `/dev/android_adb` and `/dev/android_adb_enable` are present, while
the modern gadget directory is absent. The old Android ADB implementation
opens `/dev/android_adb_enable` to enable the USB function and separately
reopens `/dev/android_adb` to register the transport, which explains why the
service restart is useful but timing-sensitive. See the
[Android 2.2.3 ADB USB implementation](https://android.googlesource.com/platform/system/core/%2B/android-2.2.3_r2/adb/usb_linux_client.c).

An initial native implementation restarted `adbd` immediately after issuing
the vendor resume request. That did not make ADB return after a later cable
reconnect. The runtime now marks the restart pending during wake and waits for
`/sys/class/power_supply/sub_cpu_usb/online` to report `1`; it then runs
`stop adbd`, waits briefly, and runs `start adbd`. This is deliberately tied to
USB physical presence, not to a claim that the host transport is already
available.

On 2026-09-16, this deferred strategy was tested with USB disconnected during
EINK standby and reconnected after the native UI returned to the active state.
The preserved log shows the relevant sequence:

```text
wake-side power event source=E4 value=1 timestamp_us=315562853
requesting early resume
requesting vendor resume helper state=on
deferring adbd restart until USB reconnect
display wake barrier released bytes=4
suspend/wake wait returned: Ok(()) elapsed_ms=1664
wake lock reacquire returned: Ok(())
post-resume framebuffer 600x800 virtual=608x1792 smem_len=2179072 stride=1216 offsets=(0, 896)
framebuffer mapping retained across resume
power event source=E4 value=0 timestamp_us=315740310 mode=ACTIVE
restarting adbd after wake
adbd restart requested
```

The host then reported the T1 as an ADB device without pressing reset. This
confirms that the native suspend/wake path can preserve the development route:
the required sequence is to reconnect USB after wake and allow the deferred
restart to run. A hardware reset remains the recovery path if the native
process or USB controller does not return far enough to observe the reconnect.

## Tap-to-launch native UI

The native UI now has a small Android 2.2/API 8 Home replacement APK at
`tools/prs-t1-launcher/`. Its `MAIN`/`HOME`/`DEFAULT` Activity is labelled
`Native UI`; selecting it from Android's Home-app resolver invokes the
installed root `su` binary and exits. The companion
`prs-t1-launch` script starts a detached native runtime, stops zygote, waits
for `zygote` and `system_server` to disappear, and then execs
`prs-t1-agent standalone-test`. Android therefore only supplies the tap
surface; the native process owns the display after the handoff, and reboot or
reset remains the return path.

On 2026-09-16, the signed v1 APK was built and installed as
`org.prs.t1.nativeui`. Package Manager registered
`org.prs.t1.nativeui/.LauncherActivity` as a launcher Activity. The first
installed APK was stale when the source was changed: it still invoked the old
`su -c /data/local/tmp/prs-t1-launch` handoff, which stopped zygote but did not
leave `prs-t1-agent` running. A manual `start zygote` restored the processes
briefly but caused repeated `system_server` PackageManager crashes with
`Can't get mount service`; a normal reboot was required to restore Android's
service ordering. This confirms that starting zygote in place is not a valid
recovery procedure.

The launcher now deliberately uses the restored `su`/Superuser companion path;
the separate custom setuid helper was removed. The staged script redirects
away from the Activity pipes and invokes the agent's privileged handoff mode.
That mode forks and calls `setsid()` before stopping zygote: the Android 2.2
shell has no `[`/`test` command, and, more importantly, an APK-launched shell
background job remains in the zygote process group and is killed when zygote
stops. The earlier handoff log ended immediately after `su handoff started`,
with zygote and `system_server` absent and no agent process, confirming this
failure boundary. The current APK, handoff script, and rebuilt ARM agent still
use the corrected handoff. A direct ADB-side launch verified the detached
agent path, and a subsequent manual Home → Native UI → Superuser approval test
successfully launched the native screen. No automatic startup hook was added.

## Native UI shell

The first real shell pass replaces the earlier diagnostic pattern with a
document-oriented layout. The home page now reserves a 48-pixel single-line
status bar and leaves the remaining canvas quiet for future document and image
content. The bar reports battery percentage, Wi-Fi state, USB presence, ADB
state, the native active/sleeping mode, and the local date/time from the T1's
`date` command. The current visual direction is a solid black strip with crisp
white 20x20 binary sprites cropped from the generated icon sheet and fit to
their individual glyph bounds. Battery and clock retain short values; Wi-Fi,
USB, and ADB are packed tightly after the battery and show only their sprites
when active. The normal Active label is hidden, leaving the mode slot for
states such as Sleeping or Rebooting. Only `HH:MM` remains in the strip and the
full date is shown in Details. Fixed outer margins and a reserved clock slot
prevent longer status values from crowding adjacent groups. This keeps the
status bar legible without introducing gray text, grid lines, or thin
low-contrast controls that would be unreliable on the panel.

The status bar is also the first navigation surface. A touch release in its
area toggles a details page. That page retains the useful diagnostics from the
standalone test—battery health and voltage, power state, Wi-Fi and supplicant,
USB gadget state, ADB service state, framebuffer/Android ownership, wake lock,
storage, and raw touch/key observations—but gives the power actions priority.
Details now groups the rows under Power, Connectivity, System, Storage, and
Input headings, formats battery percentage and voltage as user-facing values,
and shows the full local date/time. The bottom controls request Android reboot,
power-off through the T1 reboot utility's verified `-p` option, or return to
the document canvas. They retain 48-pixel touch targets with a 20-pixel bottom
margin, and hit testing is constrained to the drawn button width. Physical
short/long power-button behavior remains unchanged: short press sleeps and a
long press reboots.

The touchpanel inventory reports a useful screen-sized multitouch coordinate
pair (`ABS_MT_POSITION_X/Y`, observed as x=73/y=771 for a lower-left tap).
`BTN_TOUCH` release events delimit taps; movement continues to update the
diagnostics without triggering navigation. The multitouch stream reports the
portrait-sized screen coordinate order, so portrait uses it directly and
landscape swaps it into the active 800x600 surface. The legacy `ABS_X/Y`
compatibility path remains displayed for investigation and is normalized from
the advertised 800x600 physical axes to the active screen orientation before
hit testing. Portrait swaps the axes for the existing fbdev rotation `3`;
landscape keeps the axes in their native order at rotation `0`. The decoder
also accepts
`ABS_MT_TRACKING_ID=-1` as a release for drivers that do not emit
`BTN_TOUCH=0`. A synthetic legacy-axis sequence successfully navigated into
details and activated the lower `Back to reading` action.

A controlled physical capture resolved why the first implementation did not
navigate on touch. The panel reported `ABS_MT_TOUCH_MAJOR=12` on press and
`ABS_MT_TOUCH_MAJOR=0` on release, with `ABS_MT_POSITION_X/Y` and
`SYN_MT_REPORT`/`SYN_REPORT` framing, but emitted neither `BTN_TOUCH` nor
`ABS_MT_TRACKING_ID` in the sample. The decoder now treats the zero touch-major
frame as a pending release and activates the tap at the following
`SYN_REPORT`. A synthetic sequence matching this exact physical event shape
passes the runtime test. The updated ARM binary was deployed and the same
event sequence was verified in both directions: the status bar opened Details
and the bottom `Back to reading` target returned Home.

The generated icon sheet was cropped into 20x20, thresholded binary sprites in
`crates/prs-t1-agent/assets/`; each sprite is embedded directly and rendered
without a PNG decoder. The renderer uses those monochrome glyphs plus
`embedded-graphics`'s `FONT_8X13`, `FONT_8X13_BOLD`, and `FONT_10X20` through a
custom RGB565 `DrawTarget`. This replaces the earlier hand-written 5x7 glyph
table, adds lowercase and punctuation coverage, and lets status, heading, and
button labels use measured text bounds for horizontal and vertical centering.
The details actions now use three stacked full-width touch targets so their
visual and hit-test geometry share the same rectangles. The dependency remains
`default-features = false`; the historical optimized ARMv5 release binary was
623,112 bytes. A general-purpose renderer such as `tiny-skia` would bring
more capability than this shell needs and its default feature set is less
appropriate for the T1's small ARMv5 runtime.

### Menu-button full refresh

The first key device, `/dev/input/event0` (E0), reports the physical menu
button as event type `KEY`, code 357, value 1 on press and 0 on release. The
kernel names this code `Unknown`; it is the `E0 Unknown C357` control seen in
the diagnostics. A hold of at least one second now requests `DirtyArea::Full`.
That path clears and redraws the entire framebuffer using the existing GC16
waveform, which is the slowest and highest-quality mode currently exposed by
the T1 EPDC ABI, and sets `UPDATE_MODE_FULL` (`1`) in the ioctl payload. The
earlier implementation only supplied a full-sized rectangle while retaining
`UPDATE_MODE_PARTIAL` (`0`), which was enough for an accepted ioctl but did not
reliably request the panel's flashing/full-refresh behavior. Touch, key, and
status updates continue to use partial mode. The timer is checked by the main
loop so a device without key-repeat events still triggers while the button
remains down; repeat and release timestamps provide event-driven fallbacks.
The trigger is latched so one physical hold cannot cause repeated full
refreshes.
