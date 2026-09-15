# PRS-350 recovery startup

This is the verified normal-mode-to-recovery workflow for the connected
PRS-350. It is a state-changing operation: the reader persistently selects its
recovery boot slot and reboots. Keep the original MTD captures and a rollback
path available before using it.

## Important limitations

- The Linux `prsctl` CLI intentionally does not expose the boot-mode selector.
- The PRS-350 has no Memory Stick slot, so the generic Memory Stick diagnostic
  startup path is unavailable.
- The recovery key sequence (`HOME`, `NEXT`, `OPTION`, `PREV`, `SIZE`) is only
  checked after recovery startup has already been selected and a direct update
  has failed. It does not interact with the hardware reset button or override
  normal boot selection.
- The current reader state has only exposed a zero-capacity mass-storage LUN.
  In that state the Sony x50 service is unavailable and the selector returns
  SCSI `CHECK CONDITION`; do not repeatedly resend it.

## Validated shell-assisted procedure

The procedure below is the one that was successfully exercised on this reader.
It uses the historical signed package from `PRS350_update_tools` to provide a
temporary recovery root shell without flashing a firmware partition.

1. Keep the reader on stable power and stop file managers or automounters from
   accessing its volumes.

2. Confirm a healthy normal-mode enumeration. The expected devices are:

   ```text
   /dev/sg0  Sony PRS-350
   /dev/sg1  Sony PRS-350 Launcher
   /dev/sda READER
   /dev/sdb SETTING
   ```

   The package belongs on the root of the `READER` volume, not `SETTING`.

3. Mount `READER` and copy the original package using its recovery filename:

   ```sh
   sudo mount -t vfat -o rw,shortname=winnt,iocharset=utf8 \
     /dev/sda /mnt/prs350
   sudo cp '/path/to/PRS-350 Updater.package' \
     '/mnt/prs350/PRS-350 Updater.package'
   sync
   sudo umount /mnt/prs350
   ```

   The package used in the validated trial was 11,280 bytes with SHA-256:

   ```text
   6a35343c7fe4a27b8f32dca31a05d05927dfe7e81205bca25666c04188846f0a
   ```

   The normal-mode service filename, `PRS-350 SP Updater.package`, is a
   different path and should not be substituted here.

4. With the volume cleanly unmounted, send the stock Sony/x50
   `MSC_ReqChangeMode(1)` operation using the historical `ebook_msc` utility's
   `um recovery` command. The wire-level operation is:

   ```text
   CDB:         20 00 70 00 01 00 00 00 00 00
   data-out:    01 00 00 00
   response:    command 0x20, byte 2 = 0x70, phase = 2, four-byte result
   ```

   The mode value is a native little-endian 32-bit integer. This command is
   not equivalent to a generic SCSI reset: it writes the persistent NBL boot
   selection and reboots immediately.

5. Monitor the host while the reader changes modes:

   ```sh
   sudo journalctl -kf --no-pager
   lsusb -t
   ```

   The expected transition is that mass storage disappears and a USB CDC-ACM
   device appears as `/dev/ttyACM0`, identified as `Gadget Serial`.

6. The recovery package's updater provides a temporary root login on that
   serial interface at 9600 baud. After collecting diagnostics, use the
   package-provided normal-slot path and `/sbin/reboot` to return to normal
   mode. Then confirm that `READER` and `SETTING` reappear and remove any
   remaining package or marker only after the volume is mounted read-only and
   the device state is understood.

## Selector details

Static analysis of `switcher.so` shows that command `0x70` dispatches:

```text
mode == 0:  nblconfig -ksel normal; reboot
mode != 0: cp /opt/sony/ebook/bin/UsbUpdater /opt0/UsbUpdater;
           nblconfig -ksel recovery; reboot
```

The historical utility calls these operations `um normal` and `um recovery`.
The selector is therefore an intended Sony maintenance interface, but it is
not safe to invoke blindly: recovery does not provide a guaranteed normal-slot
rollback on this hardware without a root-capable shell or another verified
recovery action.

For the full command registry, package analysis, and rollback discussion, see
[`docs/protocol.md`](protocol.md), [`docs/device-session.md`](device-session.md),
and [`docs/firmware-analysis.md`](firmware-analysis.md).
