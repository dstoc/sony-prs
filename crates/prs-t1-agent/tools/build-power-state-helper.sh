#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
libdl=${1:-/tmp/prs-t1-libdl.so}
output=${2:-"$script_dir/../../../target/prs-t1-power-state"}

if [ ! -f "$libdl" ]; then
    echo "missing Android libdl stub: $libdl" >&2
    echo "pull it with: adb pull /system/lib/libdl.so $libdl" >&2
    exit 1
fi

zig cc \
    -target arm-linux-gnueabi \
    -marm \
    -nostdlib \
    -O2 \
    -fno-stack-protector \
    -fno-sanitize=undefined \
    -Wl,-e,_start \
    -Wl,--dynamic-linker,/system/bin/linker \
    -Wl,-rpath,/system/lib \
    -Wl,--hash-style=sysv \
    -Wl,-z,norelro \
    -Wl,--allow-shlib-undefined \
    "$libdl" \
    "$script_dir/power-state-helper.c" \
    -o "$output"

file "$output"
