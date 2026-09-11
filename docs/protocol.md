# PRS-x50 extraction protocol ledger

This project intentionally keeps historical facts, inferred wire formats, and
unresolved questions separate. The x50 read path below has been confirmed
against a connected PRS-350; additional firmware variants remain open.

## Established from surviving documentation

- Sony's older file protocol uses a 16-byte request header:
  `request`, two reserved 32-bit fields, and an extra-payload length.
- The answer also starts with 16 bytes: three reserved 32-bit fields and a
  data length at offset 12.
- File operations use the following identifiers:

  | ID | Operation |
  |---:|---|
  | `0x10` | FileOpen |
  | `0x11` | FileClose |
  | `0x12` | GetSize |
  | `0x14` | SetPosition |
  | `0x15` | GetPosition |
  | `0x16` | FileRead |
  | `0x33` | DirectoryIteratorNew |
  | `0x34` | DirectoryIteratorDispose |
  | `0x35` | DirectoryIteratorGetNext |
  | `0x101` | GetProperty |
  | `0x102` | GetMediaInfo |
  | `0x103` | GetFreeSpace |

- FileOpen carries a 32-bit mode, followed by a 32-bit path length and path
  bytes. This client always sends mode `0` (read-only).
- `FileWrite`, delete, update-mode, and partition-write operations are not
  represented by the Rust read-only command enum.
- The x50 mass-storage gadget uses the vendor SCSI opcode
  `SC_SONY_EXTENDED = 0x20`.

## Implemented assumptions

The current client encodes:

- `FileRead`: 32-bit handle followed by a 32-bit byte count.
- `SetPosition`: 32-bit handle followed by a 64-bit little-endian position.
- integer answers as either 32-bit or 64-bit little-endian values.

These choices are isolated in `protocol.rs` so they can be corrected from a
capture or recovered source without touching the CLI or SCSI layer.

The verified x50 transport uses a 10-byte CDB:

- byte 0 is `SC_SONY_EXTENDED = 0x20`;
- byte 2 is the x50 command;
- byte 4 is the phase marker.

The initialization exchange is command `0x06`, receive-only, phase 1,
with a `0x15c`-byte response that begins with the ASCII string
`Reader` followed by a NUL. The path-based read service uses command
`0x80` for GetSize and `0x81` for FileRead. GetSize sends a
256-byte NUL-padded path and receives a signed 32-bit result. FileRead sends a
0x10c-byte request containing the path, 32-bit offset, requested length, and
total file size; it then receives a four-byte status followed by data phases.
Phase 2 marks the final data chunk, phase 3 an intermediate chunk, and the
helper limits chunks to 0x1000 bytes. The write command `0x82` is
deliberately not exposed.

## Firmware command registry

Static inspection of the PRS-350 `switcher.so` command manager found these
dispatcher IDs. The IDs below are firmware evidence, not a promise that every
command has the same meaning on another x50 revision.

| ID | Firmware command | Current disposition |
|---:|---|---|
| `0x01` | GetUSBProtocolVersion | receive-only probe returned ASCII `01000000` |
| `0x06` | GetProperty | used by the verified initialization exchange |
| `0x08` | GetFreeSpace | read-only framing tested; selector semantics unresolved |
| `0x40` | GetHttpRequest | not exposed |
| `0x41` | SetHttpResponse | not exposed |
| `0x42` | GetHttpNeedRegistration | not exposed |
| `0x43` | GetMarlinState | not exposed |
| `0x50`–`0x66` | DIW/DRM and device identity operations | not exposed; several are sensitive or mutating |
| `0x70` | ReqUpdateChangeMode | stock normal/recovery selector; not exposed |
| `0x80` | UsbFileGetSize | implemented as path-based GetSize |
| `0x81` | UsbFileRead | implemented as path-based FileRead |
| `0x82` | UsbFileWrite | deliberately not exposed |
| `0x83` | UsbFileDelete | deliberately not exposed |
| `0x90` | GetFingerPrint | not exposed |
| `0xa0` | GetInfo | read-only candidate; untested |

For `0x08`, the PRS-350 accepted a four-byte selector and returned eight bytes
for selectors 0–2, all zero. Selector 3 was rejected. This is enough to
confirm the phase shape but not enough to define a useful public API, so the
command remains outside the CLI.

### `ReqUpdateChangeMode` safety finding

The native `switcher.so` implementation of command `0x70` is a stock boot-mode
control, not a general command runner. Its fixed `system()` call sites are
consistent with this behavior:

```text
mode == 0:  /usr/local/sony/bin/nblconfig -ksel normal; reboot
mode != 0: cp /opt/sony/ebook/bin/UsbUpdater /opt0/UsbUpdater;
           /usr/local/sony/bin/nblconfig -ksel recovery; reboot
```

The captured normal root filesystem does not contain
`/opt/sony/ebook/bin/UsbUpdater`, so the purpose and availability of that copy
source on a retail unit still need to be resolved. The selector itself writes
the persistent NBL boot configuration and reboots; it is therefore outside the
read-only CLI even though the commands are fixed and do not accept an
arbitrary shell string. It is the likely stock bridge from normal USB mode to
the recovery image, whose `DIAG` branch can expose the USB CDC-ACM getty
described in `docs/firmware-analysis.md`.

The historical `ebook_msc` source in the retained host artifacts removes the
remaining ambiguity about the public interface: its `um normal` and
`um recovery` commands call `MSC_ReqChangeMode(0)` and
`MSC_ReqChangeMode(1)`, respectively. That utility is an intended Sony/x50
maintenance operation, not a newly invented packet format. It still performs
a reboot and persistent boot-mode change, so the project deliberately does not
reimplement or invoke it as part of the read-only tool.

The retained Windows DLL also resolves the wire-level sizes. Its exported
`MSC_ReqChangeMode` wrapper calls the common request routine with command
`0x70`, a four-byte input containing the mode, and a four-byte output for the
device result. The mode is passed as a native 32-bit integer, so the host
representation is little-endian. This is enough to prepare a future controlled
observation, but not a reason to expose the operation in a read-only CLI: it
selects a persistent boot slot and reboots immediately.

### Recovery rollback constraint

Static inspection does not yet establish a safe rollback after selecting
recovery. The recovery `DIAG` path launches the USB serial getty and then
returns from `update_check.sh`; it does not call `nblconfig -ksel normal`.
The only stock recovery path that explicitly selects normal is `safe_reboot()`,
used by update/error handling. That path is not reached by the empty-Memory-
Stick diagnostic branch, and a `guest` shell cannot be assumed to have
permission to rewrite the NBL MTD configuration.

The project must therefore not send `MSC_ReqChangeMode(1)` until one of these
rollback paths is verified independently: a root-capable physical UART
console, a documented stock recovery action that selects normal, or a
host-visible recovery command that is still serviced after the gadget switches
from mass storage to CDC-ACM. An empty Memory Stick can select `DIAG`, but it
is not a rollback mechanism.

### Historical `login_update` shell package

The archived PRS-350 update tools contain a package named
`PRS-350 Updater.package` under `login_update/`. Its encrypted payload was
verified offline against the captured `Info.img`: the package checksum and
signature match, and the decrypted tar contains only `shadow` and `update.sh`.
It contains no root filesystem image, partition-write command, or flash tool.

The package script runs as the recovery updater's root process. It selects the
normal boot slot, starts the stock `gadget.sh serial` path, mounts the Data
volume read/write, and bind-mounts a package-provided shadow file over
`/etc/shadow`. That shadow file gives `root` and `guest` empty passwords. The
serial gadget loads `g_serial.ko` with ACM enabled and runs a 9600-baud getty
on `ttygserial`, so the expected host-side result is a USB CDC-ACM device and
a root login without changing the installed firmware.

This is the strongest non-flashing shell route found so far, but it is not
risk-free. Testing it writes one package to the user Data volume, sends the
state-changing recovery selector (`0x70` with mode `1`), and reboots. The
package has not been executed on the reader. A controlled trial must preserve
the existing MTD backups, unmount/eject the Data volume cleanly, keep the
reader powered and connected, monitor kernel/device events, and remove the
package after returning to normal mode. Do not send the selector implicitly
from the read-only CLI.

The package was also passed through the recovery image's own
`updater-functions` on the host. Header/model validation, the package checksum,
the RSA signature check, and tar extraction all returned success. The host
needed only the legacy DES/AES provider and MD5 KDF compatibility that the
reader's older OpenSSL supplies; no package bytes or selector request were sent
to the reader.

### Controlled normal-mode serial-gadget test

A second, temporary signed package tested the normal boot path without changing
the firmware image. Its `update.sh` bind-mounted a package-provided replacement
for `/usr/local/sony/bin/gadget.sh`. When normal `rc` invoked that script with
the `storage` argument, the replacement instead unloaded `g_file_storage`,
loaded `g_serial.ko use_acm=1`, and started the stock 9600-baud `getty` on
`ttygserial`.

The package was generated and verified offline, then installed through the
normal-mode service hook. It was 11,280 bytes with SHA-256
`5e3a658f55f071fdb3c2c0210964c631fb7899b20b0acd1303692ed1595482e4`. The
reader enumerated as `/dev/ttyACM0`; `/dev/sg0`, `/dev/sg1`, `/dev/sda`, and
`/dev/sdb` were absent. The host saw the expected Linux login banner, and the
reader's touch screen and hardware buttons remained interactive while USB was
connected.

This isolates the charging screen to the mass-storage path rather than USB
power/VBUS alone. It also shows that a serial-only normal-mode transport is a
viable base for a replacement UI's host communication. A composite gadget that
also exposes mass storage may still trigger the native switcher's charging
state and needs a separate test. The package's bind mount lives in `/tmp`, so a
normal reboot removes the test and restores the stock `gadget.sh`; the temporary
root getty should not be retained in a production package.

### Validated CDC-ACM application protocol

The first replacement-UI transport is deliberately line-oriented so it can be
debugged from a terminal before a binary framing layer is needed:

```text
host -> reader: PRS1 PING\n
reader -> host: PRS1 OK PONG\n
host -> reader: PRS1 INFO\n
reader -> host: PRS1 OK INFO model=PRS-350 transport=cdc-acm\n
host -> reader: PRS1 STATUS\n
reader -> host: PRS1 OK STATUS ui=stock-alive\n
host -> reader: PRS1 REBOOT\n
reader -> host: PRS1 OK REBOOTING\n
```

Unsupported requests return `PRS1 ERR unsupported-request`. The host
implementation is in `src/serial_protocol.rs` and `src/serial.rs`; the
CLI exposes it as `prsctl serial-ping /dev/ttyACM0`,
`prsctl serial-info /dev/ttyACM0`, and `prsctl serial-status /dev/ttyACM0`.
The separate `prsctl serial-reboot /dev/ttyACM0` command sends only the
explicit reboot request; it does not provide a shell or arbitrary command
execution.

This was validated end-to-end on the reader. The current temporary package is
11,280 bytes, passed the offline package verifier, and has SHA-256
`2c206776fbeb8cccf5e30d9eb1ab4cd3adb90d3ecf8ee81918081629dc9888bb`. With
the reader connected, the host observed `/dev/ttyACM0` (no `/dev/sg*` or
`/dev/sd*`), and all three read-only commands returned the expected responses;
the reboot command returned the reader to normal mass-storage mode before the
package was reapplied. The device-side service must inherit `ttygserial` as
stdin/stdout and use the BusyBox shell's `printf`; this firmware has no
`/bin/printf`, and reopening the tty from a background shell process did not
provide a usable endpoint. The firmware also lacks a `/bin/ps` applet, so the
status probe checks `/proc/*/cmdline` directly for the stock `tinyhttp` UI.

The package also carried the temporary empty-password shadow file, bound after
the normal boot script's `/etc/shadow` tmpfs setup. That supplied a root serial
console during debugging; the final protocol service itself does not launch a
getty and the shadow override must not be retained in a production package.

### Experimental binary development channel

The temporary root service now has two binary-transfer commands for iterating
on a replacement UI without rebuilding and signing an updater package for every
run. They are deliberately separate from the read-only SCSI transport and are
available only while the development service package is installed.

`serial-screenshot` sends `PRS1 CAPTURE\n`. The reader replies with a header such
as:

```text
PRS1 OK FRAMEBUFFER width=600 height=800 format=gray8 bytes=480000\n
```

It then sends exactly `bytes` of row-major 8-bit framebuffer data. The host
wraps those rows in a PGM (`P5`) file, so the result can be opened by common
image tools without a third-party dependency. The device-side capture uses the
same `/dev/fb0` mapping as the render probe and does not refresh the panel.

`serial-exec` first sends an ARM-binary header and waits for the reader to
switch the tty to raw mode:

```text
PRS1 EXEC <byte-count> <crc32>\n
reader -> host: PRS1 READY EXEC\n
repeat until the declared length is sent:
  host -> reader: <up to 1024 raw bytes>
  reader -> host: PRS1 ACK EXEC bytes=<cumulative-byte-count>\n
```

The host waits for an acknowledgement after every chunk; this is required by
the stock 9600-baud gadget path because a blind raw stream can overrun the
reader's tty input queue. The payload is capped at 1 MiB and the reader verifies CRC32 before writing
`/tmp/prs350-upload`, marking it executable, and spawning it with standard
input, output, and error detached from the protocol. The response reports its
PID; this is intentional because a replacement UI is expected to remain
running. At the current 9600-baud service setting, a 1 MiB transfer is expected
to take on the order of minutes; the nominal CDC-ACM line rate can be raised in
a controlled test, but the old gadget implementation may ignore that setting.
The upload command is intentionally powerful and should not be enabled in a
normal production image.

The development gadget also starts a separate USB-disconnect watcher. If the
protocol service is stuck in a transfer, reset the USB device from the host:

```text
tools/prs350-usb-reset.sh /dev/ttyACM0
```

The bus reset is observed by the independent watcher, which synchronizes and
reboots the reader. If the host kernel rejects `USBDEVFS_RESET` while the CDC
driver owns the interfaces, the helper falls back to de-authorizing the exact
USB device through sysfs, which produces the same disconnect event. This is a
userspace recovery path for a wedged protocol reader; it cannot recover a kernel
hang or a loss of device power. The stock
`g_serial.ko` exposes only one CDC-ACM port and the firmware's gadget scripts
remove mass storage before loading it, so a true second USB interface would
require building a composite gadget module against Sony's 2.6.23 kernel.

The host commands are:

```text
prsctl serial-screenshot /dev/ttyACM0 screen.pgm
prsctl serial-exec /dev/ttyACM0 ./prs350-agent-test
prsctl serial-shell /dev/ttyACM0 'cat /proc/cmdline'
```

`serial-shell` uses the same ready/raw framing and per-chunk acknowledgements,
with a 4 KiB command limit, and returns up to 64 KiB of combined standard
output and standard error. It runs
`/bin/sh -c` as root in the development service, so it is intentionally a
debug-only facility and must not be included in a production image.

### Rebuilding the development package

The package sidecar is tracked in `tools/prs350-serial-service.sh`,
`tools/prs350-serial-gadget.sh`, and `tools/prs350-update.sh`. The package
builder copies those files and the cross-built `device-agent` into a fresh
directory, then invokes the externally obtained Sony signing helpers:

```text
tools/build-prs350-dev-package.sh \
  /path/to/PRS350_update_tools/login_update \
  /path/to/shadow \
  device-agent/target/armv5te-unknown-linux-musleabi/release/prs350-agent \
  /tmp/prs350-dev-package
```

If the host OpenSSL needs the legacy-provider wrapper used during this
development session, set `PRS350_OPENSSL_WRAPPER_DIR` before invoking the
builder. The builder refuses to reuse an existing output directory, verifies
the package with `update_test.sh`, prints its SHA-256, and truncates the
temporary signing key on exit. The historical updater tools, `Info.img`, and
the shadow file remain external inputs and are not stored in Git.

The ARM helper remains conservatively built for ARM926EJ-S/ARMv5TE because the
reader rejected an ARM1176JZF-S build with `Illegal instruction`. Its static
Rust bootstrap currently supplies the atomic helper symbols needed by the
single-process test agent; a long-running UI should use a proper target runtime
and synchronization implementation.

## First device-session questions

1. Does the exact x50 file service vary across PRS-x50 firmware versions?
2. Which negative file-service status values should be mapped to user-facing
   diagnostics?
3. What selector and storage state does `GetFreeSpace` require on each x50
   variant?
4. Are there additional read-only x50 commands worth exposing, such as
   directory enumeration or device properties?

The `probe` command only issues standard SCSI INQUIRY. The x50
initialization and file exchanges are performed by `get`.
