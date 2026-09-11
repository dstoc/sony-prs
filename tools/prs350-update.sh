#!/bin/sh

# Development-only updater payload. It installs the temporary serial sidecar
# and ARM agent into /tmp, then binds the gadget hook before normal startup.
PATH=/bin:/sbin:/usr/bin:/usr/sbin
export PATH

/bin/cp ./serial-gadget-protocol.sh /tmp/prs350-serial-gadget.sh
/bin/cp ./prs350-serial-service.sh /tmp/prs350-serial-service.sh
/bin/cp ./prs350-agent /tmp/prs350-agent
/bin/cp ./shadow /tmp/prs350-shadow
/bin/chmod 755 /tmp/prs350-serial-gadget.sh /tmp/prs350-serial-service.sh /tmp/prs350-agent
/bin/mount --bind /tmp/prs350-serial-gadget.sh /usr/local/sony/bin/gadget.sh || exit 1
/bin/sync
exit 0
