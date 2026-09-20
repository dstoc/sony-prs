#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
helper_source="$repo_root/crates/prs-t1-agent/tools/wifi-helper.c"
helper_build="$repo_root/crates/prs-t1-agent/tools/build-wifi-helper.sh"
wifi_source="$repo_root/crates/prs-t1-agent/src/wifi.rs"

for symbol in \
  wifi_load_driver \
  wifi_unload_driver \
  wifi_start_supplicant \
  wifi_stop_supplicant; do
  grep -Fq "\"$symbol\"" "$helper_source"
done

grep -Fq 'dlopen("/system/lib/libhardware_legacy.so", 2)' "$helper_source"
grep -Fq -- '-Wl,--dynamic-linker,/system/bin/linker' "$helper_build"
grep -Fq -- '-Wl,--allow-shlib-undefined' "$helper_build"
grep -Fq 'set_property("ctl.stop", "dhcpcd")' "$wifi_source"
grep -Fq 'run_helper("stop-supplicant")' "$wifi_source"
grep -Fq 'run_helper("unload-driver")' "$wifi_source"
grep -Fq 'ASSOCIATION_TIMEOUT: Duration = Duration::from_secs(60)' "$wifi_source"
grep -Fq 'DHCP_TIMEOUT: Duration = Duration::from_secs(30)' "$wifi_source"
grep -Fq 'strip_prefix("wpa_state=")' "$wifi_source"
grep -Fq 'dhcp.wlan0.result' "$wifi_source"

echo 'PRS-T1 Wi-Fi shim and lifecycle contract checks passed'
