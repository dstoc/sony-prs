# PRS-350 `mtdblock15` analysis

The first complete read of the connected PRS-350 was saved as
`mtdblock15.img`. The image is intentionally ignored by Git (`*.img`), so
keep the image and its digest together when moving the capture between
machines:

```text
size: 33554432 bytes
sha256: c9eaca3950efff60f259791b2c7c2c94ea56ffe7bbec94891db2ce528b012714
```

## Partition identity and filesystem

The live x50 namespace reports these relevant MTD sizes:

| Device | Size | Identity |
|---|---:|---|
| `/dev/mtdblock12` | 10485760 bytes | USB Launcher (`SETTING`) |
| `/dev/mtdblock15` | 33554432 bytes | system/root CramFS image |
| `/dev/mtdblock16` | 1608384512 bytes | USB Data (`READER`) |

The Launcher and Data identities are correlated from the firmware's
`gadget.sh` label lookup and the matching host-side USB disk sizes/labels.
The current x50 service returns zero bytes for virtual `/proc/mtd`, so the
partition labels themselves are not directly readable through this interface.

`mtdblock15.img` begins with a little-endian CramFS filesystem:

```text
Linux Compressed ROM File System
logical size: 14405632 bytes
blocks: 7649
files: 827
```

The remaining bytes are the fixed-size MTD block container/padding. This is a
system image, not the public `READER` storage partition. It can be extracted
without mounting it:

```sh
fakeroot fsck.cramfs --extract=/tmp/prs350-mtdblock15-root mtdblock15.img
```

The destination must not already exist; choose another path if needed.

`fakeroot` avoids the unprivileged `mknod` failure for `/dev/console`. The
extracted tree contains the PRS-350 root filesystem, including the Sony
application under `/opt/sony/ebook` and the USB gadget helper under
`/usr/local/sony/bin/gadget.sh`.

## Cross-checks

The image contains `/opt1/info/model` with the value `PRS-350`. Reading the
same path through the live x50 service succeeds:

```sh
./target/release/prsctl get /dev/sg0 /opt1/info/model ./model-live.txt
```

The live `/usr/local/sony/bin/gadget.sh` also matches the extracted copy. Its
storage mode identifies the `Data` and `Launcher` MTD partitions and exports
them through the USB mass-storage gadget; this explains why the public
reader partition is exposed as `/dev/mtdblock15` in the x50 file namespace.

## Firmware follow-up

The broader partition map is now recorded in
[firmware-analysis.md](firmware-analysis.md). The important distinction is
that block 15 is the normal-mode root filesystem, while block 4 is a separate
recovery/diagnostic root filesystem. Block 10 is the tiny `Id` filesystem;
block 2 is the `Info` area used by the updater identity logic.

The public Data partition is `/dev/mtdblock16` on this unit, but it is not the
interesting behavior surface. Keep writes, deletes, update packages, and raw
arbitrary commands outside the CLI boundary while the boot and recovery path
is being understood.
