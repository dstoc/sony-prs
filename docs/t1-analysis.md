# Sony PRS-T1 analysis

This document tracks the read-only analysis and filesystem acquisition of the
connected Sony PRS-T1. It is separate from the PRS-350 notes because the T1 is
an Android-based reader with an eMMC partition layout rather than the PRS-350
x50 MTD layout.

## Current status

The reader is detected and the Sony x50 command framing is accepted. Standard
SCSI INQUIRY, initialization, partition-size queries, and a small partition
read work. Larger filesystem reads currently cause a USB transport reset; the
reader is now left in a wedged post-reset state and needs a physical USB
disconnect/reconnect before another clean acquisition pass.

No write, update-mode, reboot, or partition-mutating command has been sent to
the T1.

## Host access

The reader identifies as USB vendor/product `054c:05c2` and exposes three SCSI
logical units:

| Node | Inquiry product |
|---|---|
| `/dev/sg0` / `/dev/bsg/0:0:0:0` | `PRS-T1` |
| `/dev/sg1` / `/dev/bsg/0:0:0:1` | `PRS-T1 SD` |
| `/dev/sg2` / `/dev/bsg/0:0:0:2` | `PRS-T1 Setting` |

The host initially had the loadable `sg` module installed but not loaded;
`modprobe sg` restored `/dev/sg0`–`/dev/sg2`. The `agent` user is a member of
the `prs350` group. The active udev rules grant that group mode `0660` access
to both the T1 BSG and SCSI-generic nodes.

The host-side client also supports Linux `/dev/bsg/*` through the SG v4 BSG
ioctl and includes BSG nodes in `prsctl scan`. This is useful on hosts where
the legacy `sg` module is unavailable.

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
reader timed out. It is preserved as:
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
to avoid producing a mostly zero-filled image while the reader was wedged:

| Artifact | Size | SHA-256 / note |
|---|---:|---|
| `mmcblk2p1.partial.img` | 49,152 | `073ccffff8ad2cd21dd09eeb1357d6f0e9955ca04cde1baee407a7a92d410c5f` |
| `mmcblk2p1.dump3.img` | 692,224 | interrupted forensic pass; bad map retained |
| `mmcblk2p1.dump5.img` | 147,456 | interrupted after the reader wedged at offset 0 |

The `dump3` and `dump5` images are incomplete diagnostics, not complete
filesystems. A fresh physical reconnect is required before starting the next
pass.

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
the `usb-storage` driver. The reset-aware, chunk-addressable reader is now
implemented in the host client; it can retry or mark individual failed ranges
and resume without overwriting preserved partial data. The next acquisition
step is a fresh physical reconnect followed by a clean pass.
