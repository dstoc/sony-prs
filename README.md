# Sony PRS reader tools

This repository contains read-only host tooling for Sony x50 readers, plus
explicitly separate PRS-350 development controls and an ARM-side agent. The
PRS-350 work is the first validated target; PRS-T1 acquisition and analysis
are tracked separately.

## Workspace

- `scsi-transport`: Linux SG_IO and BSG transport primitives.
- `sony-x50`: Sony x50 read-only transport and legacy file protocol.
- `prsctl`: read-only host CLI for probing and extracting reader files.
- `prs350-wire`: PRS-350 development serial wire format.
- `prs350-devctl`: stateful PRS-350 development serial controls.
- `prs350-agent`: cross-compiled ARM-side PRS-350 development agent.

The SCSI layers contain no write, delete, update-mode, flash, or arbitrary
command API. The development tools are intentionally separate because they
can reboot a reader, update its framebuffer, execute an ARM binary, or run a
root shell through the temporary PRS-350 service package.

## Build and test

```sh
cargo test --workspace
cargo build --workspace --release
```

On Linux, access to `/dev/sgN` normally requires root or membership in the
group owning the SCSI-generic devices.

## Read-only CLI

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

These commands require the temporary PRS-350 development package and should
not be treated as a production interface.

## Documentation and artifacts

- [PRS-350 documentation](docs/prs350/)
- [PRS-T1 analysis](docs/prs-t1/analysis.md)
- [ignored device-dump archive](docs/prs350/device-dumps.md)
- [workspace crates](crates/)
- [PRS-350 helper scripts](tools/prs350/)

Historical downloads, firmware images, extracted filesystems, screenshots, and
device identity material remain outside Git under `device-dumps/`.
