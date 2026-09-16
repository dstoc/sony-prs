#!/bin/sh

set -eu

REMOTE_AGENT=${REMOTE_AGENT:-/data/local/tmp/prs-t1-agent}
REMOTE_LOG=${REMOTE_LOG:-/data/local/tmp/prs-t1-native-test.log}
LOCAL_AGENT=${PRS_T1_AGENT_BINARY:-target/armv5te-unknown-linux-musleabi/release/prs-t1-agent}
FRAMEBUFFER=${PRS_T1_FRAMEBUFFER:-/dev/graphics/fb0}
SUSPEND_MODE=${PRS_T1_SUSPEND_MODE:-standby}

usage() {
    printf '%s\n' \
        "usage: $0 start|status|reboot" \
        "" \
        "start   push and launch the detached native test after stopping zygote" \
        "status  print the device process/status snapshot" \
        "reboot  restore normal Android and wait for ADB"
}

require_adb() {
    if ! adb get-state >/dev/null 2>&1; then
        printf '%s\n' 'ADB device is not available' >&2
        exit 1
    fi
}

framework_stopped() {
    ps_output=$(adb shell ps 2>/dev/null || true)
    ! printf '%s\n' "$ps_output" | rg -q '(^|[ /])zygote($| )|system_server'
}

wait_for_framework_stopped() {
    attempt=1
    while [ "$attempt" -le 10 ]; do
        if framework_stopped; then
            return 0
        fi
        sleep 1
        attempt=$((attempt + 1))
    done
    printf '%s\n' 'zygote/system_server did not stop within 10 seconds' >&2
    exit 1
}

wait_for_adb() {
    attempt=1
    while [ "$attempt" -le 40 ]; do
        if adb get-state >/dev/null 2>&1; then
            printf 'adb-online-after=%ss\n' "$attempt"
            return 0
        fi
        sleep 1
        attempt=$((attempt + 1))
    done
    printf '%s\n' 'ADB did not return within 40 seconds' >&2
    exit 1
}

start_test() {
    require_adb
    if [ ! -f "$LOCAL_AGENT" ]; then
        printf 'native agent not found: %s\n' "$LOCAL_AGENT" >&2
        exit 1
    fi
    if adb shell ps | rg -q '[/ ]prs-t1-agent([ -]|$)'; then
        printf '%s\n' 'a native test process is already running' >&2
        exit 1
    fi

    adb push "$LOCAL_AGENT" "$REMOTE_AGENT"
    adb shell chmod 755 "$REMOTE_AGENT"
    adb shell stop zygote
    wait_for_framework_stopped
    adb shell "trap \"\" HUP; $REMOTE_AGENT standalone-test $FRAMEBUFFER $SUSPEND_MODE </dev/null >$REMOTE_LOG 2>&1 &"
    sleep 2
    if ! adb shell ps | rg -q '[/ ]prs-t1-agent([ -]|$)'; then
        printf '%s\n' 'native test did not remain running; recent log:' >&2
        adb shell "cat $REMOTE_LOG" >&2 || true
        exit 1
    fi
    printf 'native test running; log=%s\n' "$REMOTE_LOG"
}

status() {
    require_adb
    adb shell ps | rg 'zygote|system_server|dispd|adbd|prs-t1-agent' || true
    adb shell "$REMOTE_AGENT status"
}

reboot_reader() {
    require_adb
    adb reboot
    wait_for_adb
}

case ${1:-} in
    start)
        start_test
        ;;
    status)
        status
        ;;
    reboot)
        reboot_reader
        ;;
    -h|--help)
        usage
        ;;
    *)
        usage >&2
        exit 2
        ;;
esac
