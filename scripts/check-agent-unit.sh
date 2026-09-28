#!/bin/sh
# Guard the systemd unit emitted by install.sh against losing the write access
# needed by sing-box config replacement and the agent config directory.
set -eu

INSTALLER=${1:-install.sh}
UNIT=$(sed -n '/^cat >"\$UNIT_FILE" <<UNIT$/,/^UNIT$/p' "$INSTALLER" | sed '1d;$d')

printf '%s\n' "$UNIT" | grep -Fx '[Service]' >/dev/null || {
	echo "monitor-agent unit has no [Service] section" >&2
	exit 1
}
printf '%s\n' "$UNIT" | grep -Fx 'ProtectSystem=full' >/dev/null || {
	echo "monitor-agent unit must retain ProtectSystem=full" >&2
	exit 1
}
[ "$(printf '%s\n' "$UNIT" | grep -Fxc 'ReadWritePaths=/etc/sing-box /etc/monitor-agent')" -eq 1 ] || {
	echo "monitor-agent unit must have one combined ReadWritePaths entry" >&2
	exit 1
}
[ "$(printf '%s\n' "$UNIT" | grep -Fxc 'RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6 AF_NETLINK')" -eq 1 ] || {
	echo "monitor-agent unit must allow AF_UNIX, AF_INET, AF_INET6, and AF_NETLINK" >&2
	exit 1
}
if printf '%s\n' "$UNIT" | grep -Fx 'RestrictAddressFamilies=AF_INET AF_INET6 AF_NETLINK' >/dev/null; then
	echo "monitor-agent unit must not omit AF_UNIX" >&2
	exit 1
fi

if printf '%s\n' "$UNIT" | grep -E '^(ReadOnlyPaths|InaccessiblePaths|TemporaryFileSystem|BindReadOnlyPaths)=' >/dev/null; then
	echo "monitor-agent unit contains a conflicting systemd path restriction" >&2
	exit 1
fi
