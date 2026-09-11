#!/bin/sh

# Development-only replacement for the stock storage gadget hook.
PATH=/bin:/sbin:/usr/bin:/usr/sbin
export PATH

/bin/mount --bind /tmp/prs350-shadow /etc/shadow || exit 1
/bin/umount /Data /Data2 >/dev/null 2>&1 || true
/sbin/rmmod g_file_storage >/dev/null 2>&1 || true
/sbin/rmmod g_serial >/dev/null 2>&1 || true
/sbin/lsmod | /bin/grep -q '^arcotg_udc' || \
  /sbin/insmod /lib/modules/2.6.23/kernel/drivers/usb/gadget/arcotg_udc.ko || exit 1
[ -e /dev/ttygserial ] || /bin/mknod /dev/ttygserial c 127 0 || exit 1
/sbin/lsmod | /bin/grep -q '^g_serial' || \
/sbin/insmod /lib/modules/2.6.23/kernel/drivers/usb/gadget/g_serial.ko use_acm=1 || exit 1
/bin/sleep 2
/tmp/prs350-agent watch-usb /dev/ttygserial >/dev/null 2>&1 &
/tmp/prs350-serial-service.sh </dev/ttygserial >/dev/ttygserial 2>&1 &
exit 0
