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
