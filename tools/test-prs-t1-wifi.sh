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
grep -Fq 'set_property("ctl.stop", DHCP_SERVICE)' "$wifi_source"
grep -Fq 'const DHCP_SERVICE_STATE_PROPERTY: &str = "init.svc.dhcpcd"' "$wifi_source"
grep -Fq 'const DHCP_STOP_TIMEOUT: Duration = Duration::from_secs(10)' "$wifi_source"
grep -Fq 'wait_for_dhcp_service_stop()' "$wifi_source"
grep -Fq 'dhcp_service_is_stopped' "$wifi_source"
grep -Fq 'run_helper("stop-supplicant")' "$wifi_source"
grep -Fq 'run_helper("unload-driver")' "$wifi_source"
grep -Fq 'ASSOCIATION_TIMEOUT: Duration = Duration::from_secs(60)' "$wifi_source"
grep -Fq 'DHCP_TIMEOUT: Duration = Duration::from_secs(30)' "$wifi_source"
grep -Fq 'strip_prefix("wpa_state=")' "$wifi_source"
grep -Fq 'dhcp.wlan0.result' "$wifi_source"
grep -Fq '"/data/system/wpa_supplicant"' "$wifi_source"
grep -Fq '"/data/misc/wifi/sockets"' "$wifi_source"
grep -Fq '"/dev/socket/wpa_"' "$wifi_source"
grep -Fq 'wifi.association_source' "$wifi_source"
grep -Fq 'supplicant_crashed' "$wifi_source"
! grep -Fq 'wpa_ctrl_wlan0' "$wifi_source"

python3 - "$wifi_source" <<'PY'
from pathlib import Path
import sys

source = Path(sys.argv[1]).read_text()
shutdown = source[source.index("fn shutdown()"):source.index("fn stop_dhcp()")]
stop_dhcp = source[source.index("fn stop_dhcp()"):source.index("fn wait_for_dhcp_service_stop()")]

assert shutdown.index("let dhcp_result = stop_dhcp();") < shutdown.index(
    'run_helper("stop-supplicant")'
)
assert shutdown.index('run_helper("stop-supplicant")') < shutdown.index(
    'run_helper("unload-driver")'
)
assert stop_dhcp.index('set_property("ctl.stop", DHCP_SERVICE)') < stop_dhcp.index(
    "wait_for_dhcp_service_stop()"
)

candidate_paths = [
    "/data/system/wpa_supplicant",
    "/data/misc/wifi/sockets",
    "/data/misc/wifi/wpa_supplicant",
    "/dev/socket/wpa_",
]
candidate_source = source[source.index("const WPA_CONTROL_SOCKET_DIRS"):source.index("const WPA_ANDROID_SOCKET_PREFIX")]
assert all(path in candidate_source for path in candidate_paths[:3])
assert "/dev/socket/wpa_" in source
PY

echo 'PRS-T1 Wi-Fi shim and lifecycle contract checks passed'
