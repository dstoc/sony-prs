# PRS-350 firmware and behavior analysis

This document records the firmware-bearing areas found on the connected
PRS-350. All device observations here were read-only. The project does not
provide a write, erase, update, or arbitrary SCSI operation.

## Firmware map

The x50 file service exposes the MTD block devices as files. Reading the
first 4 KiB of each block identified the following layout:

| MTD block | Size | Observation | Likely role |
|---:|---:|---|---|
| `0` | 2,004,877,312 | large raw view; updater code uses it as a package source | aggregate/raw flash or update view; not decoded |
| `1` | 524,288 | `NC` header | boot/config metadata; not decoded |
| `2` | 131,072 | model record at offset 16; matches the historical `Info.img` format in its first 5 KiB | `Info` and update identity material |
| `3` | 4,194,304 | ARM code and embedded Linux 2.6.23 strings | executable kernel/boot region |
| `4` | 6,291,456 | CramFS, logical size 4,038,656, 620 files | recovery/diagnostic root filesystem |
| `5` | 1,048,576 | raw binary | device-specific runtime/diagnostic data; not decoded |
| `6` | 1,048,576 | begins with `EBKL` and a timestamped log record | event/log area |
| `7` | 1,048,576 | raw binary | device-specific state; not decoded |
| `8` | 1,048,576 | all `ff` in the sampled header | currently erased/unused area |
| `9` | 4,194,304 | byte-for-byte identical to block 3 in the complete capture | duplicate executable copy or boot slot |
| `10` | 131,072 | CramFS, logical size 4,096, 2 files | `Id` key/identity filesystem |
| `11` | 209,715,200 | FAT16 label `DICTIONARY` | dictionary content |
| `12` | 10,485,760 | FAT16 label `SETTING` | USB Launcher volume |
| `13` | 8,388,608 | FAT12 label `KAPPDATA` | application data/state |
| `14` | 67,108,864 | FAT16 label `PRELOAD` | preloaded content |
| `15` | 33,554,432 | CramFS, logical size 14,405,632, 827 files | normal-mode root filesystem |
| `16` | 1,608,384,512 | FAT volume label `READER` | public ebook/data volume |

Blocks 17–20 were present as device nodes in the firmware, but the live x50
service returned status `-16` for them. The labels for blocks 11–16 are also
consistent with the mount points in the normal boot script and with the two
USB volumes observed on the host.

The 4 MiB block 3 capture contains the string:

```text
Linux version 2.6.23 ... #2 PREEMPT Wed Aug 4 23:07:12 JST 2010
```

That establishes a kernel payload in this region. Blocks 3 and 9 have the
same complete 4 MiB image, so they are duplicate copies or slots. The exact
boundaries between bootloader, kernel, and padding still need to be recovered
from the flash table or bootloader metadata.

## Boot and recovery flow

The normal root filesystem contains `/etc/init.d/rc`. Its relevant order is:

1. create MTD and device nodes through `boot_api` and `rc_ap`;
2. run `/usr/local/sony/bin/update_check.sh`;
3. mount the `Id`, `Opt0`, `Dic`, and `Preload` filesystems;
4. start `gadget.sh storage`, which exports the `Data` and `Launcher` MTDs;
5. start `/opt/sony/ebook/bin/tinyhttp.sh`, which launches the ebook UI.

The recovery CramFS at block 4 has a similar low-level init but ends after
`update_check.sh` and contains factory diagnostics rather than the normal
ebook application. The scripts switch between normal and recovery selection
with `/usr/local/sony/bin/nblconfig -ksel normal|recovery`.

The root filesystem also defines a serial console:

```text
::respawn:/sbin/getty -L 115200 ttymxc0 vt102
```

This is the most promising low-risk experimentation surface if the board’s
serial pins or an existing service connector can be reached. SSH is present
but disabled in its init script, and telnet is commented out in `inetd.conf`.
The presence of the console in the image does not by itself prove that the
connector is accessible on a retail unit.

A PRS-350 teardown reports a 2.54 mm serial header on the main PCB and
identifies the interface as 3.3 V; its power pad is marked `V` and is
unpowered while the reader is shut down. See the
[MobileRead teardown](https://www.mobileread.com/forums/showthread.php?s=76b1df5fd6824b272e3ff685aa8cc183&t=193321).
The public write-up does not establish the header's TX/RX/GND pin order, so
that order must be verified from the board before connecting anything.

## Where behavior lives

| Surface | Examples | What can be learned or changed |
|---|---|---|
| Boot shell scripts | `rc`, `gadget.sh`, `tinyhttp.sh`, `compulsion.sh` | startup order, USB mode, reboot/power behavior, update dispatch |
| Kinoma application resources | `kconfig.xml`, `deviceConfig.xml`, `resources/*.xml` | UI flow, labels, menus, device configuration, scripted behavior |
| Native application libraries | `ebookSystem.so`, `kbook.so`, `switcher.so`, viewer libraries | hardware integration, protocol handlers, rendering, behavior not represented in XML |
| Kernel and modules | block 3/9 payloads and `/lib/modules/2.6.23` | USB gadget, input, display, storage, low-level hardware behavior |
| Persistent FAT state | `KAPPDATA`, `SETTING`, `PRELOAD` | settings, app state, bundled content; less suitable for core behavior |

The normal UI is therefore not stored in the public `READER` partition. A
practical first target for a behavior experiment would be a copied resource
file in the normal root image, followed by an offline rebuilt CramFS image.
Changing compiled behavior would require ARM-compatible binary analysis or
replacement and is a later step.

## Official update path

The firmware update scripts show a gated path for persistent changes:

- `update_check.sh` searches the Memory Stick and `Data` volumes for a
  model-specific updater package;
- `updater-functions` reads update identity material from the `Info` MTD;
- the package header is DES-decrypted and its package metadata is
  AES-128-CBC-decrypted;
- checksums and an RSA signature are verified before the package is unpacked;
- the package's `update.sh` performs the actual partition operation and can
  select recovery mode on failure.

The historical `PRS350_update_tools` archive contains scripts for building
and testing packages, including a rootfs-flash example that targets the
partition labelled `Rootfs`. This is useful for understanding the format, but
it does not make a live flash safe: a malformed or incompatible image can
leave the device in recovery or unbootable.

Do not copy private/device-specific key material into the repository or share
it in logs. Keep the full original captures, hashes, and a tested recovery
procedure before attempting any persistent modification.

## Recommended next steps

1. Preserve the read-only captures already made and record hashes for any new
   firmware image.
2. Recover the MTD partition table from bootloader output, diagnostic logs, or
   the kernel image rather than inferring names from size alone.
3. Use the serial console, if physically available, for runtime-only tests
   and to learn the normal/recovery boot selection behavior.
4. Select one harmless XML/resource change, rebuild the normal CramFS in a
   separate workspace, and validate its size and contents offline.
5. Only after a recovery path is demonstrated should the update-package
   format be considered for a sacrificial device.

The current `prsctl` boundary intentionally stops before step 6: it can read
firmware files and images, but cannot write them.
