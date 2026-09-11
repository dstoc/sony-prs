# prsctl

`prsctl` is a Linux-native, strictly read-only extraction tool for the Sony
PRS-350 and related PRS-x50 readers. The first milestone is to retrieve a
device file or MTD block image over the reader's vendor SCSI extension, without
using the Windows-only `ebook_msc.exe` stack.

## Current status

The project currently contains:

- direct Linux `SG_IO` bindings for standard SCSI commands;
- standard SCSI INQUIRY probing through `/dev/sgN`;
- `/dev/sg*` scanning and offline request/answer packet inspection;
- explicit little-endian encoding and decoding for the documented Sony file
  protocol;
- legacy read-only packet-protocol building blocks for FileOpen, GetSize,
  SetPosition, FileRead, and FileClose;
- a verified read-only implementation of the PRS-x50 `0x20`
  `SC_SONY_EXTENDED` transport, including its initialization and
  path-based file reads;
- offline unit tests using a scripted transport;
- no write, delete, update-mode, flash, or arbitrary-command API.

The x50 CDB, initialization exchange, GetSize, and FileRead phases have been
validated against a connected PRS-350. `get` is intended for reader-side
paths such as `/dev/mtdblock1`; the older `/Data/tmp/info/model` example is
not present in this PRS-350 x50 filesystem.

## Build

Rust is required. With a current stable toolchain:

```sh
cargo test
cargo build --release
```

On Linux, access to `/dev/sgN` normally requires root or membership in the
group owning SCSI generic devices.

## Usage

```sh
  prsctl probe /dev/sgN
  prsctl scan
  prsctl get /dev/sgN /dev/mtdblock1 mtdblock1.img
  prsctl serial-ping /dev/ttyACM0
  prsctl serial-info /dev/ttyACM0
  prsctl serial-status /dev/ttyACM0
  prsctl serial-reboot /dev/ttyACM0
  prsctl serial-probe /dev/ttyACM0
  prsctl serial-render /dev/ttyACM0
  prsctl serial-screenshot /dev/ttyACM0 screen.pgm
  prsctl serial-exec /dev/ttyACM0 ./my-arm-test
  prsctl serial-shell /dev/ttyACM0 'cat /proc/cmdline'
  prsctl decode-request captured-request.bin
  prsctl decode-answer captured-answer.bin
```

Output files are created exclusively and are never overwritten.

## Safety boundary

The SCSI remote command enum intentionally contains no write-capable operation.
There is no CLI path for `FileWrite`, delete, update-mode changes, partition
writes, or arbitrary SCSI commands. The serial protocol has separate explicit
experimental controls for reboot, framebuffer rendering, and uploading/executing
a bounded ARM test binary. Those commands are intended only for the temporary
root service package used during development; they are not part of the
read-only SCSI interface. Local output files are created exclusively.

See [the protocol ledger](docs/protocol.md) for the known wire format and the
questions that remain device-dependent. See [the artifact list](docs/artifacts.md)
for the historical source and binary references.

The reproducible download helper is [tools/fetch-historical.sh](tools/fetch-historical.sh).
The development sidecar and package builder are in
[tools/](tools/); they require the historical Sony updater tools and a local
shadow file, neither of which is committed.
The first-device checklist is [docs/device-session.md](docs/device-session.md).
The first complete PRS-350 image and filesystem findings are recorded in
[docs/mtdblock15-analysis.md](docs/mtdblock15-analysis.md).
The firmware partition map, boot flow, and behavior-modification roadmap are
in [docs/firmware-analysis.md](docs/firmware-analysis.md).
The static analysis of the `httpPatchUSB` HTTP-over-USB bridge and matching
host DLL is in [docs/http-patch-usb-analysis.md](docs/http-patch-usb-analysis.md).
