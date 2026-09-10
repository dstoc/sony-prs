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

## First device-session questions

1. Does the exact x50 file service vary across PRS-x50 firmware versions?
2. Which negative file-service status values should be mapped to user-facing
   diagnostics?
3. Are there additional read-only x50 commands worth exposing, such as
   directory enumeration or device properties?

The `probe` command only issues standard SCSI INQUIRY. The x50
initialization and file exchanges are performed by `get`.
