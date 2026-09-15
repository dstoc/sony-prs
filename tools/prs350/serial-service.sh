#!/bin/sh

# Development-only Rust service for the PRS-350 normal-mode serial hook.
# PRS1 EXEC and PRS1 SHELL intentionally accept host-supplied code/commands.
PATH=/bin:/sbin:/usr/bin:/usr/sbin
export PATH

/bin/stty 9600 icanon -echo -ixon -ixoff min 1 time 0 || exit 1
exec /tmp/prs350-agent service
