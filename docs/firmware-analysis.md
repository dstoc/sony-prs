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

### `tinyhttp`

Despite its name, `tinyhttp` is the main reader application process. The
normal init script starts `tinyhttp.sh` in the background after mounting the
firmware filesystems. The wrapper then:

- reads a saved exit code from the previous application run;
- sets the application `PATH` and `LD_LIBRARY_PATH`;
- brings up loopback;
- launches `/opt/sony/ebook/application/tinyhttp -d bootcode=<code>`;
- logs the command, return value, and exit-code file to the embedded log
  device; and
- translates application exit codes into data/card formatting, reboot,
  update/recovery, or power-off actions.

The `tinyhttp` executable itself is only about 3.6 KiB. Its real runtime is
`libtinyhttp.so` (about 2 MiB), which contains the Fsk application loop, the
Kinoma VM, the Fsk UI/file/network APIs, and the Sony application bindings.
The library exports both HTTP client and HTTP server APIs, but the static
image inspection has not established that an externally reachable HTTP
listener is enabled. The name should not be taken to mean that this is merely
an ordinary web server.

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

The account database makes this more useful than a boot-log-only console:

```text
/etc/passwd: root ... /bin/ash
/etc/passwd: guest ... /bin/ash
/etc/shadow: guest::0:0:99999:7:::
/etc/securetty: ttymxc0, ttymxc1, tty1, ttygserial
```

The empty `guest` password is evidence that the stock login path may accept a
blank password; it still needs to be confirmed on the physical console. The
shell would initially be an unprivileged `guest` shell, not root. The image
contains BusyBox `ash`, `login`, `getty`, and common diagnostic applets, but no
`sshd`, `ftpd`, or SUID/SGID helper was found. Extracted CramFS ownership and
mode bits are not authoritative, so the absence of SUID/SGID files is a useful
negative result rather than a security guarantee.

The safe first connection procedure is:

1. Use a USB-TTL adapter configured for 3.3 V logic, not an RS-232 adapter.
2. With the reader powered off, identify GND and the signal pads by board
   markings, continuity, or a scope. Do not connect the pad marked `V`.
3. Power the reader from its normal battery/USB arrangement and connect only
   GND, reader TX to adapter RX, and reader RX to adapter TX.
4. Open the adapter at `115200 8N1`; the inittab entry uses a local getty on
   `ttymxc0`.
5. At the login prompt, try the `guest` account and submit an empty password.

Do not guess the header pin order or apply external power. The serial header's
physical accessibility and the blank-password behavior remain unverified until
the reader is connected and observed.

The separate recovery CramFS independently contains the same
`::respawn:/sbin/getty -L 115200 ttymxc0 vt102` entry, creates `/dev/ttymxc0`
as character device major 207/minor 16, and carries the same `guest` account
with an empty password. The captured kernel image also contains the default
`console=ttymxc0` argument and MXC early-serial-console strings. This makes the
UART evidence independent of the normal ebook application. Recovery's init
remounts its own root filesystem read-write and runs only the updater check, so
it is a useful fallback for console observation but should not be selected or
written to without a documented recovery procedure.

### Other shell-capable surfaces checked

The image contains a latent USB CDC-ACM path in
`/usr/local/sony/bin/gadget.sh serial`. That branch removes mass storage,
loads `g_serial.ko`, creates `/dev/ttygserial`, and runs a `9600` baud getty in
an infinite loop. However, normal `/etc/init.d/rc` invokes only
`gadget.sh storage`, and no retained script, XML resource, or native binary
calls the `serial` branch. Switching to it would also unmount the public data
volumes, so it is not a safe experiment to trigger blindly through an
unverified command.

The normal boot does not start networking, SSH, `inetd`, telnet, lighttpd, or
Avahi. BusyBox still contains `httpd`, `telnetd`, `inetd`, and `nc` applets, but
the corresponding service configuration is commented out or not dispatched by
the boot script. The `tinyhttp` process is the Kinoma/Fsk reader application;
its library has generic HTTP server APIs, but no enabled external listener has
been established.

The recovery updater contains a second, more concrete USB-shell path. Its
`update_check.sh` starts in `UPDATE` mode and, when a Memory Stick is present
without a model-specific `Updater.package` or `Console.package`, changes the
mode to `DIAG`. The final dispatch is:

```sh
/usr/local/sony/bin/gadget.sh serial &
```

The recovery `gadget.sh serial` branch removes `g_file_storage`, loads
`arcotg_udc` and `g_serial.ko use_acm=1`, creates `/dev/ttygserial` as major
127/minor 0, and runs:

```sh
/sbin/getty -L 9600 ttygserial vt102
```

`ttygserial` is also listed in `/etc/securetty`, and the recovery account
database has the same `guest` shell with an empty password as the normal image.
Thus, if the reader is already in this recovery diagnostic branch, the host
should see a USB CDC-ACM serial interface and a login prompt at 9600 baud.
This is stronger evidence than the unused normal-image script branch, but it
has not been exercised on the live reader.

The recovery init script reaches this code only after the bootloader has
selected the recovery rootfs. Its `rc` invokes `update_check.sh`, while the
normal rootfs invokes a different, application-oriented updater script and
does not dispatch `gadget.sh serial`. The recovery script also has a fallback
key sequence (`HOME`, `NEXT`, `OPTION`, `PREV`, `SIZE`) while checking for a
missing direct update package; this is a diagnostic/update control path, not a
general shell trigger. The script can also unpack and execute a signed
`update.sh`, so no package or diagnostic entry point should be supplied merely
to test the console.

The live reader observed during this analysis currently enumerates as one
USB Mass Storage interface (Sony `054c:031e`), with no CDC-ACM interface. That
confirms it is still in normal storage mode; it does not test the recovery
diagnostic branch. Selecting recovery with `nblconfig -ksel recovery` changes
persistent boot selection and was not run. The safe order is therefore to
verify the physical UART first, and only consider a documented, reversible
recovery-boot observation after a complete read-only capture and recovery plan
exist.

### Native `system()` audit

Both `ebookSystem.so` and `kbook.so` import libc `system()`, so this was checked
as a possible software-only shell route. The recovered ARM call sites are
consistent with fixed internal maintenance commands:

| Library/function | Static command or input | Assessment |
|---|---|---|
| `ebookSystem.so` / `doSystemStateChange` | `/usr/local/sony/bin/nblconfig -bootdone` | fixed boot bookkeeping |
| `ebookSystem.so` / `doWatchDog` | `/opt/sony/ebook/bin/compulsion.sh 1` or `... 0` | fixed watchdog action |
| `ebookSystem.so` / `CMWrapperSetNTPDateTime` | command pointer held in an internal WAN/NTP structure | no caller-controlled command path identified |
| `kbook.so` / EULA and version helpers | `/opt/sony/ebook/bin/euladec.sh`, `rm`, `mkdir`, `mtdmount`, `grep`/`awk`, `umount`, `rmdir` | fixed update housekeeping |

No direct `system(command)` binding is exported to the Kinoma scripts, and no
test-mode XML resource supplies a process-spawn or shell API. This makes the
physical UART the primary non-flashing route; the USB serial branch and test
mode remain secondary investigation targets, not confirmed shell access.

## Where behavior lives

The ebook UI is a Kinoma/Fsk application. Its structure is approximately:

```text
Linux init
└── tinyhttp
    └── Fsk VM (kconfig.xml)
        ├── application.xml
        │   └── applicationStart.xml
        │       └── resources/scripts/main.xml
        ├── other XML views, skins, layouts, and localized assets
        ├── *.xsb / *.xso compiled bytecode modules
        └── *.so ARM native extensions
```

This is visible in the shipped files rather than inferred from filenames:
`kconfig.xml` declares the Fsk root VM and native extensions, `application.xml`
loads the application bytecode and `applicationStart.xml`, and the latter
loads `resources/scripts/main.xml` as the main 600x800 view. The XML files
contain Fsk view descriptions plus JavaScript-like functions inside `code`,
`function`, and CDATA elements. The application image contains 88 XML files,
including the home, settings, browser, test, and document-viewer screens.

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

There is no obvious stock plug-in directory or application installer in this
image. New script behavior must be referenced by an existing XML entry point,
and it can only call APIs already exposed by the loaded Fsk extensions. New
hardware-facing behavior requires a compatible ARM native extension or a
change to one of Sony's existing libraries.

This model was extensible enough for the historical PRS+ project: its source
and installer target the 350/650/950 family, and its community documentation
describes adding menus, key bindings, games, and other JavaScript-based
features. One documented technique adds code to `applicationStart.xml` to
load `/Data/autorun.js` at startup. See the [PRS+ source repository](https://github.com/natowi/prs-plus),
the [PRS+ feature list](https://github.com/natowi/prs-plus/wiki), and the
[documented autorun hook](https://www.mobileread.com/forums/showthread.php?page=2&s=8e09a0ed017d4929d5a9a4089b03f330&t=64510).
That is evidence of a practical extension path, not proof that an unmodified
stock image already contains the hook.

The original build environment is not present in the extracted image. The
current Kinoma open-source project documents `application.xml` as a legacy
project format and supports embedded Linux targets, but the PRS-350 image
depends on an older, device-specific Fsk runtime and Sony native extensions.
Modern Kinoma tooling is therefore useful for understanding the format, not
an immediately compatible drop-in build system.

## Stock autorun references

The normal application has one direct startup reference:

```xml
<document href="applicationStart.xml"/>
```

Its `initialized` function currently registers the USB dispatcher, reports
startup progress, and optionally delays settings loading. It does not load an
external JavaScript file.

The stock image does contain `autorun` references, but they belong to the
factory/test UI:

- `resources/tests/autorun.xml` and `autorunAssets.xml` define the test-mode
  autorun screen;
- `resources/tests/650.xml` calls `kbook.autoRunRoot.exitIf(model)` when leaving
  that test screen;
- `main.xml` tracks `EXIST_SD_AUTORUN`, `EXIST_MS_AUTORUN`, and
  `EXIST_INTERNAL_AUTORUN` to display test-mode state and choose the test data
  directory.

A search of the extracted tree and raw `mtdblock15` image found no literal
`autorun.js` or `/Data/autorun` startup hook. The external autorun technique
described above is therefore a modification pattern from PRS+, not a latent
stock feature in this image.

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

The firmware’s USB-mediated HTTP bridge is documented separately in
[http-patch-usb-analysis.md](http-patch-usb-analysis.md). That analysis also
covers the matching host-side `DeviceAccessor.dll` exports and explains why
the path is not a separate USB networking interface.
