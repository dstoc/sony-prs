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

No filesystem write, package copy, or partition-mutating command has been sent
to the T1. One documented stock recovery-selector request was tested and the
normal boot selector was restored afterward.

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

After the root-package boot, the T1's USB gadget still exposes only the normal
mass-storage interface. No host-side `/dev/ttyACM*` or `/dev/ttyUSB*` node and
no ADB interface were present. The minimal package contains the Windows
`usbser.sys` host driver but no `adbd` binary or ADB configuration payload, so
ADB is not currently enabled.

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
