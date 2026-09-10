# First PRS-350 session

The first session should be treated as protocol discovery. Do not install
PRS+, switch update mode, or write to the reader until the read path is
understood and the original state has been archived.

## Host preparation

Install a Rust toolchain and build the binary:

```sh
cargo test
cargo build --release
```

Check that Linux exposes SCSI generic devices and identify the reader:

```sh
lsusb -nn
ls -l /dev/sg*
sudo ./target/release/prsctl scan
```

## Safe first commands

Run standard SCSI INQUIRY only:

```sh
sudo ./target/release/prsctl probe /dev/sgN
```

This command does not send the Sony vendor opcode. It confirms whether the
candidate generic-SCSI node identifies as a Sony reader.

The `get` command initializes the x50 file service and uses the recovered
path-based read protocol. A safe first extraction is a small MTD block and a
new output path:

```sh
./target/release/prsctl get /dev/sgN /dev/mtdblock1 ./mtdblock1.img
```

The historical /Data/tmp/info/model path is not present in the PRS-350 x50
file namespace tested here. Do not start with a large MTD image; confirm the
small read first and compare its packet ordering against `ebook_msc`.

## What to capture if the exchange fails

Record:

```sh
uname -a
lsusb -v -d 054c: 2>/dev/null
ls -l /dev/sg*
dmesg | tail -100
```

The most useful next artifact, if a read fails, is a USB/SCSI trace showing the
command CDB, data-out transaction, status phase, data-in transaction, and
returned length. That will settle remaining questions in [the protocol
ledger](protocol.md).

## Controlled non-flashing shell trial

Static analysis found a historical, PRS-350-specific `login_update` package
that should expose a temporary root shell from recovery without replacing the
firmware. This experiment has not been run on the reader. It is still
state-changing: it writes one package to the Data volume and invokes the
stock recovery selector, which reboots the reader.

Before authorizing a trial:

1. Keep the reader connected to stable power and USB. Preserve the existing
   MTD captures and record the package hash from the local update-tools archive.
2. Confirm the target by label and model. On this host the reader currently
   appears as `/dev/sda` (`READER`, 1.5 GiB) and `/dev/sdb` (`SETTING`, 10 MiB,
   `PRS-350 Launcher`). The package belongs on the root of the `READER`
   volume, never the Launcher volume.
3. Confirm neither reader volume is mounted, and stop any file manager or
   automounter that could remount it while the package is being copied.
4. In separate host terminals, monitor `journalctl -kf --no-pager` and
   `lsusb -t`. The expected transition is mass storage disappearing, followed
   by a CDC-ACM serial interface at 9600 baud.

The package copy must be completed and flushed before the volume is cleanly
unmounted/ejected. Only then should the verified host-side `0x70` request be
sent with recovery mode `1`. Do not use a generic SCSI command or improvise a
packet: the exact four-byte little-endian framing is recorded in
[the protocol ledger](protocol.md).

If the serial interface appears, try the stock getty at 9600 baud and use the
package's empty-password root account. The package's reboot is commented out;
after collecting runtime evidence, `/sbin/reboot` should return to the already
selected normal slot. Once normal mass storage returns, remove the updater
package and cleanly eject the volume. If the expected serial interface does
not appear, do not repeatedly resend the selector; preserve kernel and USB
logs and use the physical UART/recovery procedure before taking another step.

When using an inline Python heredoc for the serial bridge, do not call
`termios.tcgetattr(0)`: standard input is the heredoc pipe, not the terminal.
The bridge must open `/dev/tty` explicitly for keyboard input and output.

This procedure is intentionally not part of `prsctl`: the CLI remains
read-only and has no update-mode, package-copy, reboot, or arbitrary-SCSI
operation.

### Verified outcome

The trial succeeded on the connected reader. After the recovery selector, the
host exposed `/dev/ttyACM0`; the corrected bridge reached the recovery getty,
and logging in as root with the package's blank password produced
`uid=0(root) gid=0(root)`. This confirms the non-flashing shell route described
above. Reboot to the selected normal slot and remove the package before
considering the session complete.
