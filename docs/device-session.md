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
