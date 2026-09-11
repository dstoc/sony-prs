#!/bin/sh

# Development-only root service for the PRS-350 normal-mode serial hook.
# PRS1 EXEC and PRS1 SHELL intentionally accept host-supplied code/commands.
PATH=/bin:/sbin:/usr/bin:/usr/sbin
export PATH

/bin/stty 9600 icanon -echo -ixon -ixoff min 1 time 0 || exit 1
while IFS= read -r line; do
    case "$line" in
        'PRS1 PING') printf 'PRS1 OK PONG\n' ;;
        'PRS1 INFO') printf 'PRS1 OK INFO model=PRS-350 transport=cdc-acm\n' ;;
        'PRS1 STATUS')
            if /bin/grep -q 'tinyhttp' /proc/*/cmdline 2>/dev/null; then
                printf 'PRS1 OK STATUS ui=stock-alive\n'
            else
                printf 'PRS1 OK STATUS ui=stock-missing\n'
            fi
            ;;
        'PRS1 REBOOT')
            printf 'PRS1 OK REBOOTING\n'
            /bin/sync
            /bin/sleep 1
            /sbin/reboot >/dev/null 2>&1
            ;;
        'PRS1 PROBE') /tmp/prs350-agent ;;
        'PRS1 RENDER') /tmp/prs350-agent render ;;
        'PRS1 CAPTURE') /tmp/prs350-agent capture ;;
        'PRS1 EXEC '*)
            set -- $line
            if [ "$#" -ne 4 ] || [ "$1" != "PRS1" ] || [ "$2" != "EXEC" ]; then
                printf 'PRS1 ERR invalid-exec-header\n'
                continue
            fi
            /bin/stty 9600 raw -echo -ixon -ixoff min 0 time 10 || {
                printf 'PRS1 ERR cannot-enter-upload-mode\n'
                continue
            }
            printf 'PRS1 READY EXEC\n'
            /bin/sleep 1
            /tmp/prs350-agent receive-exec "$3" "$4"
            status=$?
            /bin/stty 9600 icanon -echo -ixon -ixoff min 1 time 0
            [ "$status" -eq 0 ] || printf 'PRS1 ERR upload-agent-failed\n'
            ;;
        'PRS1 SHELL '*)
            set -- $line
            if [ "$#" -ne 4 ] || [ "$1" != "PRS1" ] || [ "$2" != "SHELL" ]; then
                printf 'PRS1 ERR invalid-shell-header\n'
                continue
            fi
            /bin/stty 9600 raw -echo -ixon -ixoff min 0 time 10 || {
                printf 'PRS1 ERR cannot-enter-shell-mode\n'
                continue
            }
            printf 'PRS1 READY SHELL\n'
            /bin/sleep 1
            /tmp/prs350-agent receive-shell "$3" "$4"
            status=$?
            /bin/stty 9600 icanon -echo -ixon -ixoff min 1 time 0
            [ "$status" -eq 0 ] || printf 'PRS1 ERR shell-agent-failed\n'
            ;;
        *) printf 'PRS1 ERR unsupported-request\n' ;;
    esac
done
