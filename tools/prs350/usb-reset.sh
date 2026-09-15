#!/bin/sh
set -eu

if [ "$#" -ne 1 ]; then
    echo "usage: $0 /dev/ttyACM0" >&2
    exit 2
fi

TTY=$1
if [ ! -e "$TTY" ]; then
    echo "error: serial device does not exist: $TTY" >&2
    exit 1
fi

TTY_NAME=$(basename "$TTY")
SYSFS=$(readlink -f "/sys/class/tty/$TTY_NAME/device")
BUSNUM=
DEVNUM=
while [ "$SYSFS" != / ]; do
    if [ -r "$SYSFS/busnum" ] && [ -r "$SYSFS/devnum" ]; then
        BUSNUM=$(sed -n '1p' "$SYSFS/busnum")
        DEVNUM=$(sed -n '1p' "$SYSFS/devnum")
        break
    fi
    SYSFS=$(dirname "$SYSFS")
done
if [ -z "$BUSNUM" ] || [ -z "$DEVNUM" ]; then
    echo "error: could not resolve USB bus and device numbers for $TTY (sysfs=$SYSFS)" >&2
    exit 1
fi

DEVICE=$(printf '/dev/bus/usb/%03d/%03d' "$BUSNUM" "$DEVNUM")
if python3 - "$DEVICE" <<'PY'
import fcntl
import sys

USBDEVFS_RESET = 0x5514
path = sys.argv[1]
with open(path, 'rb', buffering=0) as device:
    try:
        fcntl.ioctl(device.fileno(), USBDEVFS_RESET, 0)
    except OSError as exc:
        print(f'USBDEVFS_RESET failed: {exc}', file=sys.stderr)
        raise SystemExit(75)
print(f'reset USB device {path}')
PY
then
    exit 0
fi

if [ -w "$SYSFS/authorized" ]; then
    printf '0\n' >"$SYSFS/authorized"
    printf 'deauthorized USB device %s; the reader watcher should reboot it\n' "$DEVICE"
    exit 0
fi

echo "error: USB reset was rejected and $SYSFS/authorized is not writable" >&2
exit 1
