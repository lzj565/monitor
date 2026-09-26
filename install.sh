#!/bin/sh
# Installs monitor-agent as a systemd or OpenRC service.
#   curl -fsSL https://hub.example.com/install.sh | sh -s -- --server URL --token TOKEN [options]
#   curl -fsSL https://hub.example.com/install.sh | sh -s -- --server URL --register KEY [options]
#   curl -fsSL https://hub.example.com/install.sh | sh -s -- --upgrade
#   curl -fsSL https://hub.example.com/install.sh | sh -s -- --uninstall
set -eu
# rc-update resides in sbin, which a root shell entered through `su` without `-`
# lacks on Debian: su keeps the caller's PATH unless ALWAYS_SET_PATH is set.
PATH="$PATH:/usr/sbin:/sbin"

# Binary and token in one directory, the same one the hub uses, giving a node a
# single path to inspect and a single path to remove.
ROOT="/opt/monitor"
BIN="$ROOT/monitor-agent"
SING_BOX_BIN="$ROOT/sing-box"
SING_BOX_LIB="$ROOT/libcronet.so"
SING_BOX_CONFIG_DIR="/etc/sing-box"
SING_BOX_CONFIG="$SING_BOX_CONFIG_DIR/config.json"
SING_BOX_SYSTEMD_UNIT="/etc/systemd/system/sing-box.service"
SING_BOX_OPENRC_FILE="/etc/init.d/sing-box"
SING_BOX_LOG="/var/log/sing-box.log"
SING_BOX_USER="sing-box"
ENV_FILE="$ROOT/agent.env"
UNIT_FILE="/etc/systemd/system/monitor-agent.service"
RC_FILE="/etc/init.d/monitor-agent"
LOG_FILE="/var/log/monitor-agent.log"

if [ -t 1 ] && [ -z "${NO_COLOR-}" ]; then
	B="$(printf '\033[1m')" D="$(printf '\033[2m')" N="$(printf '\033[0m')" G="$(printf '\033[32m')"
else
	B="" D="" N="" G=""
fi

rule() { printf '  %s────────────────────────────────────────────%s\n' "$D" "$N"; }
ok() { printf '  %s✓%s  %s    %s%s%s\n' "$G" "$N" "$1" "$D" "${2-}" "$N"; }
field() { printf '  %s%s%s    %s\n' "$D" "$1" "$N" "$2"; }

SERVER=""
TOKEN=""
REGISTER=""
NAME=""
IFACE=""
IFACE_SET=""
INTERVAL=""
INSECURE=""
UNINSTALL=""
UPGRADE=""

while [ $# -gt 0 ]; do
	# A flag with no argument: under set -u, `$2` aborts with the shell's own
	# message rather than the usage below, and `shift 2` cannot proceed.
	case "$1" in
	--server | --token | --register | --name | --iface | --interval)
		[ $# -ge 2 ] || { echo "$1 needs a value" >&2; exit 2; } ;;
	esac
	case "$1" in
	--server) SERVER="$2"; shift 2 ;;
	--token) TOKEN="$2"; shift 2 ;;
	--register) REGISTER="$2"; shift 2 ;;
	--name) NAME="$2"; shift 2 ;;
	--iface) IFACE="$2"; IFACE_SET=1; shift 2 ;;
	--interval) INTERVAL="$2"; shift 2 ;;
	--insecure) INSECURE=1; shift ;;
	--uninstall) UNINSTALL=1; shift ;;
	--upgrade) UPGRADE=1; shift ;;
	*) echo "unknown option: $1" >&2; exit 2 ;;
	esac
done

[ "$(id -u)" = 0 ] || { echo "run as root" >&2; exit 1; }

AGENT_FIRST=1
if [ -n "$UPGRADE" ] || [ -f "$BIN" ] || [ -f "$UNIT_FILE" ] || [ -f "$RC_FILE" ]; then
	AGENT_FIRST=""
fi

is_managed_sing_box_systemd_unit() {
	[ -f "$SING_BOX_SYSTEMD_UNIT" ] && [ ! -L "$SING_BOX_SYSTEMD_UNIT" ] &&
		grep -Fqx '# 由 monitor-agent 安装器管理。' "$SING_BOX_SYSTEMD_UNIT"
}

is_managed_sing_box_openrc_file() {
	[ -f "$SING_BOX_OPENRC_FILE" ] && [ ! -L "$SING_BOX_OPENRC_FILE" ] &&
		grep -Fqx '# 由 monitor-agent 安装器管理。' "$SING_BOX_OPENRC_FILE"
}

# Removes exactly what an install writes and nothing else, for both init
# systems: the one present now need not be the one the install found, and
# systemctl fails outright where systemd is not PID 1 (WSL, containers) although
# the install left its files there. Each step therefore tolerates failure. The
# hub may be installed in $ROOT as well, so the directory is removed only once
# empty.
if [ -n "$UNINSTALL" ]; then
	rc-service monitor-agent stop 2>/dev/null || true
	rc-update del monitor-agent default >/dev/null 2>&1 || true
	systemctl disable --now monitor-agent 2>/dev/null || true
	if is_managed_sing_box_systemd_unit; then
		systemctl disable --now sing-box.service >/dev/null 2>&1 || true
		rm -f "$SING_BOX_SYSTEMD_UNIT"
		systemctl daemon-reload >/dev/null 2>&1 || true
	fi
	if is_managed_sing_box_openrc_file; then
		rc-service sing-box stop >/dev/null 2>&1 || true
		rc-update del sing-box default >/dev/null 2>&1 || true
		rm -f "$SING_BOX_OPENRC_FILE"
		rm -f "$SING_BOX_LOG"
	fi
	rm -f "$UNIT_FILE" "$RC_FILE" "$LOG_FILE" "$BIN" "$BIN.old" \
		"$SING_BOX_BIN" "$SING_BOX_BIN.new" "$SING_BOX_LIB" "$SING_BOX_LIB.new" "$ENV_FILE"
	systemctl daemon-reload 2>/dev/null || true
	userdel monitor-agent 2>/dev/null || deluser monitor-agent 2>/dev/null || true
	rmdir "$ROOT" 2>/dev/null || true
	echo "monitor-agent and sing-box uninstalled; configuration preserved at $SING_BOX_CONFIG"
	exit 0
fi

# Reinstalls the binary with what this machine already holds. It is the one
# command a whole fleet can be upgraded with, because it carries no credential
# and names no node: the token and the hub address come from the env file, which
# only an install writes. Registering is not reached, so it can neither add a
# node nor spend a key, and a machine with nothing installed is told to use the
# panel's command rather than quietly becoming a new node.
if [ -n "$UPGRADE" ]; then
	[ -z "$TOKEN$REGISTER" ] ||
		{ echo "--upgrade takes no --token or --register; it reuses what this machine holds" >&2; exit 2; }
	TOKEN=$(sed -n 's/^MONITOR_TOKEN=//p' "$ENV_FILE" 2>/dev/null | tail -n 1)
	[ -n "$SERVER" ] || SERVER=$(sed -n 's/^MONITOR_SERVER=//p' "$ENV_FILE" 2>/dev/null | tail -n 1)
	[ -n "$TOKEN" ] && [ -n "$SERVER" ] || {
		echo "no agent is installed here: $ENV_FILE holds no token and hub address." >&2
		echo "install it with the command from the panel instead" >&2
		exit 2
	}
fi

[ -n "$SERVER" ] && { [ -n "$TOKEN" ] || [ -n "$REGISTER" ]; } || {
	echo "usage: install.sh --server URL (--token TOKEN | --register KEY) [--interval SECONDS] [--iface LIST] [--insecure]" >&2
	echo "       install.sh --upgrade [--iface LIST] [--interval SECONDS]" >&2
	echo "       install.sh --uninstall" >&2
	echo "--name NAME names the node --register creates; the hostname otherwise" >&2
	exit 2
}
# Names the node a registration creates. A token belongs to a node that already
# has a name, which the panel changes; silently dropping the flag there would
# read as a rename that never happened.
[ -z "$NAME" ] || [ -z "$TOKEN" ] ||
	{ echo "--name applies only with --register; rename an existing node in the panel" >&2; exit 2; }
# A setting of this machine, kept by a rerun without the flag for the reason
# given for --iface below: the batch command carries none at the default. It is
# read back from the service definition the last install wrote; a first install
# takes 1.
if [ -z "$INTERVAL" ]; then
	INTERVAL=$(cat "$UNIT_FILE" "$RC_FILE" 2>/dev/null | sed -n \
		-e 's/^ExecStart=.* --interval \([0-9][0-9]*\).*/\1/p' \
		-e 's/^command_args="--interval \([0-9][0-9]*\).*/\1/p' | tail -n 1)
	if [ -n "$INTERVAL" ]; then echo "keeping --interval $INTERVAL from the previous install"; else INTERVAL=1; fi
fi
case "$INTERVAL" in "" | *[!0-9]*) echo "interval must be an integer from 1 to 3600" >&2; exit 2 ;; esac
[ "$INTERVAL" -ge 1 ] && [ "$INTERVAL" -le 3600 ] || { echo "interval must be from 1 to 3600" >&2; exit 2; }
# Which interfaces carry this machine's traffic is known only on the machine,
# and the batch command a fleet shares cannot carry one value per machine. A
# rerun without --iface, the documented upgrade, therefore keeps the value in
# the env file; --iface '' clears it.
#
# A kept value is written back as found, the last assignment being the one
# systemd and OpenRC apply: root wrote it, both already read it, and a hand edit
# with quotes must not block every later upgrade. A value given here is held to
# what the agent accepts -- full names separated by commas, each optionally led
# by one `-` -- since the agent refuses anything else at startup and would
# restart forever while this script reported success. The character set also
# keeps it inert where OpenRC sources the file as shell.
if [ -z "$IFACE_SET" ]; then
	IFACE=$(sed -n 's/^MONITOR_IFACE=//p' "$ENV_FILE" 2>/dev/null | tail -n 1)
	[ -z "$IFACE" ] || echo "keeping --iface $IFACE from the previous install"
else
	case ",$IFACE," in
	*[!A-Za-z0-9._,-]* | *,-,* | *,--*)
		echo "--iface takes full interface names separated by commas, each optionally led by -, not: $IFACE" >&2
		exit 2
		;;
	esac
fi
# A bare host implies TLS, matching the upgrade the agent's ws_url() performs,
# and the same reversal under --insecure where the hub has no TLS to upgrade to.
# Without this the two diverge: the agent would dial wss:// while curl below
# defaults a scheme-less URL to http://, fetching over plaintext the binary about
# to run as root.
if [ -n "$INSECURE" ]; then SCHEME=http; else SCHEME=https; fi
case "$SERVER" in *://*) ;; *) SERVER="$SCHEME://$SERVER" ;; esac
# The agent already refuses plaintext ws:// to a remote hub, since the token
# would travel in the clear. The same address fetches the binary about to run as
# root here, so the same rule applies: over plain HTTP anyone on the path can
# substitute a binary of their own.
#
# --insecure overrides both halves for a hub reached at ip:port with no TLS in
# front, and says so explicitly: this is the one step of the install that cannot
# be corrected afterwards, because a substituted binary is already running as
# root by then.
#
# The test applies to the host alone, with scheme, port and path removed, and
# matches an address rather than a prefix: `127.evil.com` is a registered name
# resolving wherever its owner points it, and reading it as loopback would hand
# this plaintext channel to that owner.
HOST="${SERVER#*://}"
HOST="${HOST%%/*}"
# RFC 3986 places userinfo before the host, so `127.0.0.1:28080@evil.example.com`
# leaves a loopback address where the test below looks while curl, which parses
# the URL correctly, fetches from the owner of that name -- over plain HTTP, with
# the bytes installed 0755 and started as root a few lines below. A hub address
# never requires userinfo; the agent's own ws_url rejects it as well.
case "$HOST" in
*@*) echo "server URL must not contain '@': the host is whatever follows it" >&2; exit 2 ;;
esac
case "$HOST" in
"["*) HOST="${HOST#\[}"; HOST="${HOST%%]*}" ;;
*) HOST="${HOST%%:*}" ;;
esac
# A full dotted quad in 127/8 and nothing shorter, matching what the agent's
# is_loopback() accepts, since Rust's IpAddr parser accepts nothing shorter
# either -- `127.1` is a name to it, not an address. The two must agree, or this
# installs over plaintext against a hub the agent then refuses to dial: the unit
# is written, the service started, and it crash-loops on RestartSec while this
# script has reported success.
#
# The first arm excludes anything containing a letter, which is a registered name
# however it begins, and anything with more than four components, which is not an
# address.
case "$HOST" in
localhost | ::1) LOCAL=1 ;;
*[!0-9.]* | *.*.*.*.*) LOCAL="" ;;
127.[0-9]*.[0-9]*.[0-9]*) LOCAL=1 ;;
*) LOCAL="" ;;
esac
case "$SERVER" in
http://*)
	if [ -z "$LOCAL" ]; then
		[ -n "$INSECURE" ] || {
			echo "refusing plaintext http:// to a remote hub; use https://, or --insecure if it has no TLS" >&2
			exit 2
		}
		echo "warning: --insecure over plain HTTP to $SERVER" >&2
		echo "         the token and every report travel in the clear, and the binary" >&2
		echo "         installed below is fetched over the same unverified channel" >&2
	fi
	;;
esac
if [ -d /run/systemd/system ] && command -v systemctl >/dev/null; then
	INIT=systemd
elif [ -e /run/openrc/softlevel ] && command -v rc-update >/dev/null && command -v rc-service >/dev/null; then
	INIT=openrc
elif [ -f "$UNIT_FILE" ] && command -v systemctl >/dev/null; then
	INIT=systemd
elif [ -f "$RC_FILE" ] && command -v rc-update >/dev/null && command -v rc-service >/dev/null; then
	INIT=openrc
elif command -v systemctl >/dev/null; then
	INIT=systemd
elif command -v rc-update >/dev/null && command -v rc-service >/dev/null; then
	INIT=openrc
else
	echo "this installer needs systemd or OpenRC" >&2
	exit 1
fi

HOST_ARCH=$(uname -m)
case "$HOST_ARCH" in
x86_64 | amd64) ARCH=x86_64 ;;
aarch64 | arm64) ARCH=aarch64 ;;
*) echo "unsupported architecture: $(uname -m)" >&2; exit 1 ;;
esac
case "$ARCH" in
x86_64) SING_BOX_ARCH=amd64 ;;
aarch64) SING_BOX_ARCH=arm64 ;;
esac

# The hub relays the binary, so a node need only reach the hub it already talks
# to: an IPv6-only or blocked machine cannot resolve github.com. A hub unable to
# fetch releases itself is configured with a GitHub proxy in its own settings,
# which is why none is requested here.
URL="${SERVER%/}/agent/$ARCH"
SING_BOX_URL="${SERVER%/}/sing-box/$ARCH"
TMP="$(mktemp)"
SING_BOX_TMPDIR="$(mktemp -d)"
SING_BOX_ARCHIVE="$SING_BOX_TMPDIR/release.tar.gz"
SING_BOX_BINARY="$SING_BOX_TMPDIR/sing-box"
SING_BOX_LIBRARY="$SING_BOX_TMPDIR/libcronet.so"
SING_BOX_CONFIG_TMP=""
trap 'rm -f "$TMP"; rm -rf "$SING_BOX_TMPDIR"; [ -z "$SING_BOX_CONFIG_TMP" ] || rm -f "$SING_BOX_CONFIG_TMP"' EXIT

fetch_asset() {
	FETCH_URL="$1"
	FETCH_FILE="$2"
	FETCH_NAME="$3"
	FETCH_DIAGNOSTICS=""
	echo "downloading $FETCH_NAME"
	TRIES=0
	while :; do
		if CURL_INFO=$(curl -sSL --max-time 300 \
			-w 'http_code=%{http_code} content_type=%{content_type} bytes=%{size_download} time=%{time_total}s remote_ip=%{remote_ip}' \
			"$FETCH_URL" -o "$FETCH_FILE"); then
			CURL_EXIT=0
		else
			CURL_EXIT=$?
		fi
		CODE=$(printf '%s\n' "$CURL_INFO" | sed -n 's/.*http_code=\([0-9][0-9][0-9]\).*/\1/p')
		[ -n "$CODE" ] || CODE=000
		FILE_BYTES=$(wc -c <"$FETCH_FILE" | tr -d ' ')
		FETCH_DIAGNOSTICS="exit=$CURL_EXIT $CURL_INFO file_bytes=$FILE_BYTES"
		if [ "$CURL_EXIT" -ne 0 ]; then
			printf 'download diagnostics (%s): url=%s %s\n' "$FETCH_NAME" "$FETCH_URL" "$FETCH_DIAGNOSTICS" >&2
			echo "curl failed while downloading $FETCH_NAME (exit $CURL_EXIT)" >&2
			return 1
		fi
		[ "$CODE" = 503 ] && [ "$TRIES" -lt 5 ] || break
		TRIES=$((TRIES + 1))
		echo "the hub is busy relaying to other machines; retrying in 5 seconds"
		sleep 5
	done
	if [ "$CODE" != 200 ]; then
		printf 'download diagnostics (%s): url=%s %s\n' "$FETCH_NAME" "$FETCH_URL" "$FETCH_DIAGNOSTICS" >&2
		printf 'download failed (HTTP %s): %s\n' "$CODE" "$(head -n 1 "$FETCH_FILE" | cut -c1-500)" >&2
		return 1
	fi
}

sing_box_archive_diagnostics() {
	printf 'sing-box diagnostics: host_arch=%s mapped_arch=%s url=%s archive=%s\n' \
		"$HOST_ARCH" "$SING_BOX_ARCH" "$SING_BOX_URL" "$SING_BOX_ARCHIVE" >&2
	printf 'sing-box download: %s\n' "$SING_BOX_FETCH_DIAGNOSTICS" >&2
	echo "sing-box archive members:" >&2
	tar -tzf "$SING_BOX_ARCHIVE" 2>&1 | sed -n '1,40p' >&2 || true
}

sing_box_file_diagnostics() {
	printf 'sing-box extracted files:\n' >&2
	ls -ln "$SING_BOX_BINARY" "$SING_BOX_LIBRARY" >&2 || true
	printf 'sing-box binary sha256: %s\n' \
		"$(sha256sum "$SING_BOX_BINARY" 2>/dev/null | awk '{print $1}' || true)" >&2
}

check_sing_box_service_conflict() {
	if [ "$INIT" = systemd ]; then
		if is_managed_sing_box_openrc_file; then
			echo "an installer-managed OpenRC sing-box service already exists; refusing to create a second service" >&2
			return 1
		fi
		if [ -e "$SING_BOX_SYSTEMD_UNIT" ] || [ -L "$SING_BOX_SYSTEMD_UNIT" ]; then
			is_managed_sing_box_systemd_unit || {
				echo "sing-box.service already exists and is not managed by this installer; leaving it unchanged" >&2
				return 1
			}
		elif systemctl show --property=LoadState sing-box.service 2>/dev/null | grep -Fqx 'LoadState=loaded'; then
			echo "sing-box.service is provided by another package; leaving it unchanged" >&2
			return 1
		fi
	else
		if is_managed_sing_box_systemd_unit; then
			echo "an installer-managed systemd sing-box service already exists; refusing to create a second service" >&2
			return 1
		fi
		for SYSTEMD_UNIT in /etc/systemd/system/sing-box.service /run/systemd/system/sing-box.service /usr/local/lib/systemd/system/sing-box.service /usr/lib/systemd/system/sing-box.service /lib/systemd/system/sing-box.service; do
			if [ -e "$SYSTEMD_UNIT" ] || [ -L "$SYSTEMD_UNIT" ]; then
				echo "systemd sing-box unit already exists at $SYSTEMD_UNIT; leaving it unchanged" >&2
				return 1
			fi
		done
		if [ -e "$SING_BOX_OPENRC_FILE" ] || [ -L "$SING_BOX_OPENRC_FILE" ]; then
			is_managed_sing_box_openrc_file || {
				echo "OpenRC sing-box service already exists and is not managed by this installer; leaving it unchanged" >&2
				return 1
			}
		fi
	fi
}

check_sing_box_service_conflict

# 批量下载超过中转并发数时会返回 503，等待后重试。
fetch_asset "$URL" "$TMP" "monitor-agent ($ARCH)"
AGENT_FETCH_DIAGNOSTICS=$FETCH_DIAGNOSTICS
# A relay can answer 200 with something other than the program, such as a
# mirror's error page. Checked before the running agent is stopped, so a batch
# run through such a relay leaves each machine on the agent it had, rather than
# on bytes that cannot start while this script reports success.
[ "$(head -c 4 "$TMP")" = "$(printf '\177ELF')" ] ||
	{
		printf 'monitor-agent download diagnostics: arch=%s url=%s %s\n' \
			"$ARCH" "$URL" "$AGENT_FETCH_DIAGNOSTICS" >&2
		echo "the download is not a Linux executable: $(head -n 1 "$TMP" | tr -cd '[:print:]' | cut -c1-200)" >&2
		exit 1
	}
chmod 0755 "$TMP" || { echo "could not make the monitor-agent download executable" >&2; exit 1; }
AGENT_HELP=$("$TMP" --help 2>&1 || true)
AGENT_VERSION=$(printf '%s\n' "$AGENT_HELP" | sed -n '1{s/^monitor-agent //;p;}' | cut -c1-120)
[ -n "$AGENT_VERSION" ] || AGENT_VERSION=unknown

# 仅从符合架构的归档路径提取 sing-box 和运行库，避免解开其他归档成员。
fetch_asset "$SING_BOX_URL" "$SING_BOX_ARCHIVE" "sing-box ($SING_BOX_ARCH)"
SING_BOX_FETCH_DIAGNOSTICS=$FETCH_DIAGNOSTICS
SING_BOX_MEMBER=$(tar -tzf "$SING_BOX_ARCHIVE" | awk -v arch="$SING_BOX_ARCH" '
	$0 ~ ("^sing-box-[^/]+-linux-" arch "/sing-box$") {
		if (member != "") bad = 1
		member = $0
	}
	END { if (member != "" && !bad) print member }
')
[ -n "$SING_BOX_MEMBER" ] || {
	echo "download is not a supported sing-box archive" >&2
	sing_box_archive_diagnostics
	exit 1
}
SING_BOX_LIBRARY_MEMBER="${SING_BOX_MEMBER%/sing-box}/libcronet.so"
tar -tzf "$SING_BOX_ARCHIVE" | grep -Fqx "$SING_BOX_LIBRARY_MEMBER" ||
	{
		echo "the sing-box archive is missing libcronet.so" >&2
		sing_box_archive_diagnostics
		exit 1
	}
tar -xOzf "$SING_BOX_ARCHIVE" "$SING_BOX_MEMBER" >"$SING_BOX_BINARY" ||
	{
		echo "could not extract sing-box from the release archive" >&2
		sing_box_archive_diagnostics
		sing_box_file_diagnostics
		exit 1
	}
chmod 0755 "$SING_BOX_BINARY" ||
	{
		echo "could not make the sing-box binary executable" >&2
		sing_box_archive_diagnostics
		sing_box_file_diagnostics
		exit 1
	}
tar -xOzf "$SING_BOX_ARCHIVE" "$SING_BOX_LIBRARY_MEMBER" >"$SING_BOX_LIBRARY" ||
	{
		echo "could not extract libcronet.so from the release archive" >&2
		sing_box_archive_diagnostics
		sing_box_file_diagnostics
		exit 1
	}
[ "$(head -c 4 "$SING_BOX_BINARY")" = "$(printf '\177ELF')" ] &&
	[ "$(head -c 4 "$SING_BOX_LIBRARY")" = "$(printf '\177ELF')" ] ||
	{
		echo "sing-box ELF check failed: binary=$(od -An -tx1 -N4 "$SING_BOX_BINARY" | tr -d ' \n') library=$(od -An -tx1 -N4 "$SING_BOX_LIBRARY" | tr -d ' \n')" >&2
		sing_box_archive_diagnostics
		sing_box_file_diagnostics
		exit 1
	}
SING_BOX_VERSION_LOG="$SING_BOX_TMPDIR/version.stderr"
if SING_BOX_VERSION=$(LD_LIBRARY_PATH="$SING_BOX_TMPDIR" "$SING_BOX_BINARY" version 2>"$SING_BOX_VERSION_LOG"); then
	:
else
	VERSION_EXIT=$?
	echo "sing-box version check failed: exit=$VERSION_EXIT binary=$SING_BOX_BINARY LD_LIBRARY_PATH=$SING_BOX_TMPDIR" >&2
	sing_box_archive_diagnostics
	sing_box_file_diagnostics
	echo "sing-box version stderr:" >&2
	[ ! -s "$SING_BOX_VERSION_LOG" ] || head -c 2000 "$SING_BOX_VERSION_LOG" >&2
	echo >&2
	command -v ldd >/dev/null 2>&1 && ldd "$SING_BOX_BINARY" 2>&1 | head -n 40 >&2 || true
	exit 1
fi
SING_BOX_VERSION=$(printf '%s\n' "$SING_BOX_VERSION" | head -n 1 | sed 's/^sing-box version //' | cut -c1-200)

ensure_sing_box_user() {
	SING_BOX_NOLOGIN=/sbin/nologin
	[ -x "$SING_BOX_NOLOGIN" ] || SING_BOX_NOLOGIN=/usr/sbin/nologin
	[ -x "$SING_BOX_NOLOGIN" ] || SING_BOX_NOLOGIN=/bin/false
	if id -u "$SING_BOX_USER" >/dev/null 2>&1; then
		[ "$(id -u "$SING_BOX_USER")" != 0 ] || {
			echo "the sing-box service account must not be root" >&2
			return 1
		}
	else
		if command -v useradd >/dev/null 2>&1; then
			if useradd --help 2>&1 | grep -q BusyBox; then
				useradd -S -H -h /nonexistent -s "$SING_BOX_NOLOGIN" "$SING_BOX_USER" || return 1
			else
				useradd --system --user-group --no-create-home --home-dir /nonexistent --shell "$SING_BOX_NOLOGIN" "$SING_BOX_USER" || return 1
			fi
		elif command -v adduser >/dev/null 2>&1 && adduser --help 2>&1 | grep -q BusyBox; then
			adduser -S -D -H -h /nonexistent -s "$SING_BOX_NOLOGIN" "$SING_BOX_USER" || return 1
		elif command -v adduser >/dev/null 2>&1; then
			adduser --system --group --no-create-home --home /nonexistent --shell "$SING_BOX_NOLOGIN" "$SING_BOX_USER" || return 1
		else
			echo "cannot create the sing-box service account: useradd/adduser is unavailable" >&2
			return 1
		fi
	fi
	SING_BOX_GROUP=$(id -gn "$SING_BOX_USER") || return 1
	[ "$(id -g "$SING_BOX_USER")" != 0 ] || {
		echo "the sing-box service account must not use the root group" >&2
		return 1
	}
}

check_sing_box_config() {
	CHECK_CONFIG="$1"
	CHECK_OUTPUT="$SING_BOX_TMPDIR/config-check.output"
	if LD_LIBRARY_PATH="$SING_BOX_TMPDIR" "$SING_BOX_BINARY" check -c "$CHECK_CONFIG" >"$CHECK_OUTPUT" 2>&1; then
		return 0
	fi
	echo "sing-box config check failed for $CHECK_CONFIG" >&2
	if [ "$SING_BOX_CONFIG_CREATED" = yes ]; then
		[ ! -s "$CHECK_OUTPUT" ] || head -c 1200 "$CHECK_OUTPUT" >&2
		printf '\n' >&2
	else
		echo "details are suppressed because the existing configuration may contain credentials; inspect it with: LD_LIBRARY_PATH=$ROOT $SING_BOX_BIN check -c $CHECK_CONFIG" >&2
	fi
	return 1
}

prepare_sing_box_config() {
	ensure_sing_box_user || return 1
	if [ -L "$SING_BOX_CONFIG_DIR" ]; then
		echo "refusing symlinked sing-box config directory: $SING_BOX_CONFIG_DIR" >&2
		return 1
	fi
	install -d -m 0750 -o root -g "$SING_BOX_GROUP" "$SING_BOX_CONFIG_DIR" || return 1
	if [ -L "$SING_BOX_CONFIG" ] || { [ -e "$SING_BOX_CONFIG" ] && [ ! -f "$SING_BOX_CONFIG" ]; }; then
		echo "sing-box config path must be a regular file, not a symlink: $SING_BOX_CONFIG" >&2
		return 1
	fi
	SING_BOX_CONFIG_CREATED=no
	if [ ! -e "$SING_BOX_CONFIG" ]; then
		SING_BOX_CONFIG_TMP=$(mktemp "$SING_BOX_CONFIG_DIR/.config.json.XXXXXX") || return 1
		cat >"$SING_BOX_CONFIG_TMP" <<'CONFIG' || return 1
{
  "log": {
    "level": "warn",
    "timestamp": true
  },
  "inbounds": [],
  "outbounds": [
    {
      "type": "direct",
      "tag": "direct"
    }
  ]
}
CONFIG
		chown root:"$SING_BOX_GROUP" "$SING_BOX_CONFIG_TMP" || return 1
		chmod 0640 "$SING_BOX_CONFIG_TMP" || return 1
		SING_BOX_CONFIG_CREATED=yes
		check_sing_box_config "$SING_BOX_CONFIG_TMP" || return 1
		if ln "$SING_BOX_CONFIG_TMP" "$SING_BOX_CONFIG" 2>/dev/null; then
			rm -f "$SING_BOX_CONFIG_TMP"
			SING_BOX_CONFIG_TMP=""
		else
			if [ -f "$SING_BOX_CONFIG" ] && [ ! -L "$SING_BOX_CONFIG" ]; then
				rm -f "$SING_BOX_CONFIG_TMP"
				SING_BOX_CONFIG_TMP=""
				SING_BOX_CONFIG_CREATED=no
			else
				echo "could not atomically create $SING_BOX_CONFIG" >&2
				return 1
			fi
		fi
	fi
	check_sing_box_config "$SING_BOX_CONFIG" || return 1
	chown root:"$SING_BOX_GROUP" "$SING_BOX_CONFIG" || return 1
	chmod 0640 "$SING_BOX_CONFIG" || return 1
}

capture_sing_box_state() {
	SING_BOX_WAS_OWNED=no
	SING_BOX_WAS_ACTIVE=no
	SING_BOX_WAS_ENABLED=no
	if [ "$INIT" = systemd ]; then
		if is_managed_sing_box_systemd_unit; then
			SING_BOX_WAS_OWNED=yes
			systemctl is-active --quiet sing-box.service && SING_BOX_WAS_ACTIVE=yes
			systemctl is-enabled --quiet sing-box.service && SING_BOX_WAS_ENABLED=yes
		fi
	else
		if is_managed_sing_box_openrc_file; then
			SING_BOX_WAS_OWNED=yes
			rc-service sing-box status >/dev/null 2>&1 && SING_BOX_WAS_ACTIVE=yes
			rc-update show default 2>/dev/null | grep -Eq '(^|[[:space:]])sing-box([[:space:]]|$)' && SING_BOX_WAS_ENABLED=yes
		fi
	fi
}

write_sing_box_service() {
	if [ "$INIT" = systemd ]; then
		SING_BOX_UNIT_TMP=$(mktemp /etc/systemd/system/.sing-box.service.XXXXXX) || return 1
		cat >"$SING_BOX_UNIT_TMP" <<UNIT || { rm -f "$SING_BOX_UNIT_TMP"; return 1; }
# 由 monitor-agent 安装器管理。
[Unit]
Description=sing-box proxy core
Documentation=https://sing-box.sagernet.org
After=network-online.target nss-lookup.target
Wants=network-online.target

[Service]
Type=simple
User=$SING_BOX_USER
Group=$SING_BOX_GROUP
Environment=LD_LIBRARY_PATH=$ROOT
ExecStartPre=$SING_BOX_BIN check -c $SING_BOX_CONFIG
ExecStart=$SING_BOX_BIN run -c $SING_BOX_CONFIG
ExecReload=/bin/kill -HUP \$MAINPID
Restart=on-failure
RestartSec=5s
TimeoutStartSec=30s
TimeoutStopSec=15s
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes
RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6 AF_NETLINK
CapabilityBoundingSet=
AmbientCapabilities=
UMask=0027

[Install]
WantedBy=multi-user.target
UNIT
		chmod 0644 "$SING_BOX_UNIT_TMP" || { rm -f "$SING_BOX_UNIT_TMP"; return 1; }
		mv -f "$SING_BOX_UNIT_TMP" "$SING_BOX_SYSTEMD_UNIT" || { rm -f "$SING_BOX_UNIT_TMP"; return 1; }
		systemctl daemon-reload || return 1
	else
		SING_BOX_RC_TMP=$(mktemp /etc/init.d/.sing-box.XXXXXX) || return 1
		cat >"$SING_BOX_RC_TMP" <<RC || { rm -f "$SING_BOX_RC_TMP"; return 1; }
#!/sbin/openrc-run
# 由 monitor-agent 安装器管理。
description="sing-box proxy core"
command="$SING_BOX_BIN"
command_args_foreground="run -c $SING_BOX_CONFIG"
command_user="$SING_BOX_USER:$SING_BOX_GROUP"
supervisor="supervise-daemon"
respawn_delay=5
output_log="$SING_BOX_LOG"
error_log="$SING_BOX_LOG"
export LD_LIBRARY_PATH="$ROOT"

depend() {
	need net
}

start_pre() {
	checkpath --file --owner "$SING_BOX_USER:$SING_BOX_GROUP" --mode 0640 "$SING_BOX_LOG"
	"$SING_BOX_BIN" check -c "$SING_BOX_CONFIG"
}
RC
		chmod 0755 "$SING_BOX_RC_TMP" || { rm -f "$SING_BOX_RC_TMP"; return 1; }
		mv -f "$SING_BOX_RC_TMP" "$SING_BOX_OPENRC_FILE" || { rm -f "$SING_BOX_RC_TMP"; return 1; }
	fi
}

restore_sing_box_install() {
	if [ "$INIT" = systemd ]; then
		systemctl stop sing-box.service >/dev/null 2>&1 || true
		if [ "$SING_BOX_WAS_OWNED" = yes ]; then
			cp -p "$SING_BOX_TMPDIR/old-sing-box.service" "$SING_BOX_SYSTEMD_UNIT" || return 1
		else
			systemctl disable sing-box.service >/dev/null 2>&1 || true
			rm -f "$SING_BOX_SYSTEMD_UNIT"
		fi
		systemctl daemon-reload >/dev/null 2>&1 || true
	else
		rc-service sing-box stop >/dev/null 2>&1 || true
		if [ "$SING_BOX_WAS_OWNED" = yes ]; then
			cp -p "$SING_BOX_TMPDIR/old-sing-box.init" "$SING_BOX_OPENRC_FILE" || return 1
		else
			rc-update del sing-box default >/dev/null 2>&1 || true
			rm -f "$SING_BOX_OPENRC_FILE"
		fi
	fi
	if [ "$SING_BOX_OLD_BINARY" = yes ]; then
		cp -p "$SING_BOX_TMPDIR/old-sing-box" "$SING_BOX_BIN.rollback" && mv -f "$SING_BOX_BIN.rollback" "$SING_BOX_BIN" || return 1
	else
		rm -f "$SING_BOX_BIN"
	fi
	if [ "$SING_BOX_OLD_LIBRARY" = yes ]; then
		cp -p "$SING_BOX_TMPDIR/old-libcronet.so" "$SING_BOX_LIB.rollback" && mv -f "$SING_BOX_LIB.rollback" "$SING_BOX_LIB" || return 1
	else
		rm -f "$SING_BOX_LIB"
	fi
	if [ "$SING_BOX_WAS_OWNED" = yes ]; then
		if [ "$INIT" = systemd ]; then
			if [ "$SING_BOX_WAS_ENABLED" = yes ]; then systemctl enable sing-box.service >/dev/null 2>&1 || true; else systemctl disable sing-box.service >/dev/null 2>&1 || true; fi
			if [ "$SING_BOX_WAS_ACTIVE" = yes ]; then systemctl restart sing-box.service >/dev/null 2>&1 || true; fi
		else
			if [ "$SING_BOX_WAS_ENABLED" = yes ]; then rc-update add sing-box default >/dev/null 2>&1 || true; else rc-update del sing-box default >/dev/null 2>&1 || true; fi
			if [ "$SING_BOX_WAS_ACTIVE" = yes ]; then rc-service sing-box start >/dev/null 2>&1 || true; fi
		fi
	fi
}

install_sing_box() {
	prepare_sing_box_config || return 1
	capture_sing_box_state
	SING_BOX_OLD_BINARY=no
	SING_BOX_OLD_LIBRARY=no
	[ ! -f "$SING_BOX_BIN" ] || { cp -p "$SING_BOX_BIN" "$SING_BOX_TMPDIR/old-sing-box" && SING_BOX_OLD_BINARY=yes; } || return 1
	[ ! -f "$SING_BOX_LIB" ] || { cp -p "$SING_BOX_LIB" "$SING_BOX_TMPDIR/old-libcronet.so" && SING_BOX_OLD_LIBRARY=yes; } || return 1
	if [ "$SING_BOX_WAS_OWNED" = yes ]; then
		if [ "$INIT" = systemd ]; then cp -p "$SING_BOX_SYSTEMD_UNIT" "$SING_BOX_TMPDIR/old-sing-box.service" || return 1
		else cp -p "$SING_BOX_OPENRC_FILE" "$SING_BOX_TMPDIR/old-sing-box.init" || return 1; fi
	fi
	install -m 0755 "$SING_BOX_BINARY" "$SING_BOX_BIN.new" &&
		install -m 0644 "$SING_BOX_LIBRARY" "$SING_BOX_LIB.new" &&
		mv -f "$SING_BOX_LIB.new" "$SING_BOX_LIB" &&
		mv -f "$SING_BOX_BIN.new" "$SING_BOX_BIN" || {
			echo "could not install the sing-box binary and runtime library" >&2
			restore_sing_box_install || echo "sing-box rollback also failed; inspect the service and binaries" >&2
			return 1
		}
	write_sing_box_service || {
		echo "could not install the sing-box service definition" >&2
		restore_sing_box_install || echo "sing-box rollback also failed; inspect the service and binaries" >&2
		return 1
	}
	if [ "$SING_BOX_WAS_OWNED" != yes ]; then
		if [ "$INIT" = systemd ]; then
			systemctl enable sing-box.service >/dev/null && systemctl start sing-box.service || {
				echo "sing-box could not be enabled or started; see: journalctl -u sing-box.service -n 50 --no-pager" >&2
				restore_sing_box_install || echo "sing-box rollback also failed; inspect the service and binaries" >&2
				return 1
			}
		else
			rc-update add sing-box default >/dev/null && rc-service sing-box start || {
				echo "sing-box could not be enabled or started; see: $SING_BOX_LOG" >&2
				restore_sing_box_install || echo "sing-box rollback also failed; inspect the service and binaries" >&2
				return 1
			}
		fi
	elif [ "$SING_BOX_WAS_ACTIVE" = yes ]; then
		if [ "$INIT" = systemd ]; then
			systemctl restart sing-box.service || {
				echo "sing-box restart failed; see: journalctl -u sing-box.service -n 50 --no-pager" >&2
				restore_sing_box_install || echo "sing-box rollback also failed; inspect the service and binaries" >&2
				return 1
			}
		else
			rc-service sing-box restart || {
				echo "sing-box restart failed; see: $SING_BOX_LOG" >&2
				restore_sing_box_install || echo "sing-box rollback also failed; inspect the service and binaries" >&2
				return 1
			}
		fi
	fi
	SING_BOX_RUNNING=no
	SING_BOX_ENABLED=no
	if [ "$INIT" = systemd ]; then
		systemctl is-active --quiet sing-box.service && SING_BOX_RUNNING=yes
		systemctl is-enabled --quiet sing-box.service && SING_BOX_ENABLED=yes
	else
		rc-service sing-box status >/dev/null 2>&1 && SING_BOX_RUNNING=yes
		rc-update show default 2>/dev/null | grep -Eq '(^|[[:space:]])sing-box([[:space:]]|$)' && SING_BOX_ENABLED=yes
	fi
	if { [ "$SING_BOX_WAS_OWNED" != yes ] || [ "$SING_BOX_WAS_ACTIVE" = yes ]; } && [ "$SING_BOX_RUNNING" != yes ]; then
		echo "sing-box service did not become active" >&2
		restore_sing_box_install || echo "sing-box rollback also failed; inspect the service and binaries" >&2
		return 1
	fi
}

report_install() {
	if [ "$INIT" = systemd ]; then
		AGENT_SERVICE_FILE=$UNIT_FILE
		SING_BOX_SERVICE_FILE=$SING_BOX_SYSTEMD_UNIT
		AGENT_LOG_COMMAND="journalctl -u monitor-agent -f"
		SING_BOX_LOG_COMMAND="journalctl -u sing-box.service -f"
	else
		AGENT_SERVICE_FILE=$RC_FILE
		SING_BOX_SERVICE_FILE=$SING_BOX_OPENRC_FILE
		AGENT_LOG_COMMAND="tail -f $LOG_FILE"
		SING_BOX_LOG_COMMAND="tail -f $SING_BOX_LOG"
	fi
	if [ "$SING_BOX_RUNNING" = yes ]; then SING_BOX_RUNNING_TEXT=运行中; else SING_BOX_RUNNING_TEXT=已停止; fi
	if [ "$SING_BOX_ENABLED" = yes ]; then SING_BOX_ENABLED_TEXT=已启用; else SING_BOX_ENABLED_TEXT=未启用; fi
	AGENT_DOWNLOAD_SIZE=$(du -h "$TMP" | awk 'NR == 1 { print $1 }')
	SING_BOX_DOWNLOAD_SIZE=$(du -h "$SING_BOX_ARCHIVE" | awk 'NR == 1 { print $1 }')
	printf '\n  %smonitor agent%s  %s·%s  安装器\n' "$B" "$N" "$D" "$N"
	rule
	printf '\n'
	ok "架构" "$HOST_ARCH → Agent $ARCH / sing-box $SING_BOX_ARCH"
	if [ "$AGENT_VERSION" = unknown ]; then
		field "Agent 版本" "$AGENT_VERSION"
	else
		ok "Agent 版本" "$AGENT_VERSION"
	fi
	ok "sing-box 版本" "$SING_BOX_VERSION"
	ok "下载" "Agent ${AGENT_DOWNLOAD_SIZE}；sing-box ${SING_BOX_DOWNLOAD_SIZE}"
	ok "校验" "Agent ELF；sing-box ELF、版本及配置有效"
	ok "服务" "Agent 已启动并开机自启"
	if [ "$SING_BOX_RUNNING" = yes ] && [ "$SING_BOX_ENABLED" = yes ]; then
		ok "sing-box" "运行中并开机自启"
	else
		field "sing-box 状态" "${SING_BOX_RUNNING_TEXT}；开机自启$SING_BOX_ENABLED_TEXT"
	fi
	if [ -n "$AGENT_FIRST" ]; then done_title="安装完成"; else done_title="升级完成"; fi
	printf '\n  %s%s%s\n' "$B" "$done_title" "$N"
	rule
	printf '\n'
	field "Agent" "$BIN"
	field "Agent 服务" "$AGENT_SERVICE_FILE"
	field "sing-box" "$SING_BOX_BIN"
	field "运行库" "$SING_BOX_LIB"
	field "配置文件" "$SING_BOX_CONFIG"
	field "sing-box 服务" "$SING_BOX_SERVICE_FILE"
	rule
	field "Agent 日志" "$AGENT_LOG_COMMAND"
	field "sing-box 日志" "$SING_BOX_LOG_COMMAND"
	if [ "$SING_BOX_CONFIG_CREATED" = yes ]; then
		field "提示" "初始配置没有入站，目前尚无代理监听"
	fi
	printf '\n'
}

# 在注册前完成两个二进制的下载和校验，避免网络失败先占用节点；注册 token 会在
# sing-box 服务安装前写入受限环境文件，服务启动失败后重试不会重复注册节点。
#
# --register exchanges a key for this node's own token, which is what allows one
# command to provision a batch of machines. The key is valid only within the
# window the panel opened and never becomes the credential the agent runs with.
if [ -z "$TOKEN" ]; then
	# Re-running the same command must not add a second node, so the token this
	# machine already holds travels with the key. The hub returns it unchanged
	# while it still opens a node, also after the window has closed; otherwise
	# the request is a new registration, which needs an open window. The env file
	# alone cannot tell a node deleted from the panel, and trusting it would keep
	# a revoked token while this installer reported success.
	#
	# Only for the same hub: a token issued by hub A means nothing to hub B.
	HELD=""
	CACHED=$(sed -n 's/^MONITOR_SERVER=//p' "$ENV_FILE" 2>/dev/null || true)
	if [ "${CACHED%/}" = "${SERVER%/}" ]; then
		HELD=$(sed -n 's/^MONITOR_TOKEN=//p' "$ENV_FILE" 2>/dev/null || true)
	fi
	# --name as given on this machine, or else the hostname, restricted to
	# characters a hostname may contain. The hub trims and bounds either.
	[ -n "$NAME" ] || NAME=$(hostname 2>/dev/null | tr -cd 'A-Za-z0-9._-' | cut -c1-64)
	echo "registering $NAME with the hub"
	# curl sends no header at all for an empty $HELD. The status follows the body
	# on a line of its own, so a refusal shows the hub's own reason: a closed
	# window, a lockout, an entry that is not an https domain and a database
	# error share one exit status under --fail. A request that got no response
	# stops here, with curl's own message. The name travels on stdin: as an
	# argument, one beginning with @ would be read as a file to send.
	REPLY=$(printf '%s' "$NAME" | curl -sS --max-time 30 -w '\n%{http_code}' -H "Authorization: Bearer $REGISTER" \
		-H "X-Node-Token: $HELD" --data-binary @- "${SERVER%/}/api/agent/register") || exit 1
	CODE=$(printf '%s\n' "$REPLY" | tail -n 1)
	TOKEN=$(printf '%s\n' "$REPLY" | sed '$d')
	if [ "$CODE" != 200 ]; then
		# The hub answers in one line of text. A proxy or CDN in front may answer
		# with a page of HTML instead, of which the first line is enough.
		printf 'registration failed (HTTP %s): %s\n' "$CODE" "$(printf '%s\n' "$TOKEN" | head -n 1 | cut -c1-500)" >&2
		[ -z "$HELD" ] || echo "if this machine's node was deleted or its token reissued, the token it holds no longer counts." >&2
		exit 1
	fi
	[ -n "$TOKEN" ] || { echo "the hub answered without a token" >&2; exit 1; }
	if [ "$TOKEN" = "$HELD" ]; then
		echo "this machine is already registered; keeping its token and the name the panel shows"
	elif [ -n "$HELD" ]; then
		echo "the token this machine held no longer opens a node; registered as a new node."
		echo "if that token was reissued rather than its node deleted, delete the old node in the panel."
	fi
fi

# 保存在服务环境文件中的 init 类型供 Agent 选择对应的固定服务命令。
install -d -m 0755 "$ROOT"
(
	umask 077
	cat >"$ENV_FILE" <<ENV
MONITOR_SERVER=$SERVER
MONITOR_TOKEN=$TOKEN
MONITOR_INIT=$INIT
ENV
	[ -z "$IFACE" ] || printf 'MONITOR_IFACE=%s\n' "$IFACE" >>"$ENV_FILE"
)

# sing-box 先完成校验和启动，再停掉现有 Agent，避免其升级因核心启动失败而中断。
install_sing_box || exit 1

# Stop an agent already running here before replacing its binary. The service
# name is fixed, so a reinstall could never start a second copy, but without this
# the new binary lands beneath a live process and only the restart at the end
# picks it up. Stopping first also means the copy does not depend on `install`
# unlinking rather than failing with ETXTBSY. Placed after the download, so a
# node that cannot fetch the binary keeps running.
if [ "$INIT" = openrc ]; then
	rc-service monitor-agent stop 2>/dev/null || true
else
	systemctl stop monitor-agent 2>/dev/null || true
fi
# Kept until the new binary has proved it starts; see not_started. Never over
# an existing copy: a run that died before that check left an unproven binary
# in $BIN, and the copy is the one that ran before it.
[ ! -f "$BIN" ] || [ -f "$BIN.old" ] || cp "$BIN" "$BIN.old"
install -m 0755 "$TMP" "$BIN"

# The new agent is not running. The binary it replaced is put back and started
# again, so a failed upgrade leaves the machine reporting as before; the unit
# and env file just written suit that binary as well, since an upgrade keeps the
# token and the settings. A first install has nothing to put back.
not_started() {
	echo "monitor-agent did not start; see: $1" >&2
	[ -f "$BIN.old" ] || exit 1
	mv -f "$BIN.old" "$BIN"
	if [ "$INIT" = openrc ]; then
		rc-service monitor-agent restart >/dev/null 2>&1 || true
	else
		systemctl restart monitor-agent || true
	fi
	echo "the previous monitor-agent binary is back in place and was restarted" >&2
	exit 1
}

if [ "$INIT" = openrc ]; then
	cat >"$RC_FILE" <<RC
#!/sbin/openrc-run
description="monitor agent"
command="$BIN"
command_args="--interval $INTERVAL${INSECURE:+ --insecure}"
supervisor="supervise-daemon"
command_user="root"
respawn_delay=5
output_log="$LOG_FILE"
error_log="$LOG_FILE"

depend() {
	need net
}

# The token stays in the root-only env file rather than the service script.
# supervise-daemon opens the log as command_user, which is root.
start_pre() {
	checkpath --file --owner root --mode 0600 $LOG_FILE
	set -a
	. $ENV_FILE
	set +a
}
RC
	chmod 0755 "$RC_FILE"
	rc-update add monitor-agent default >/dev/null
	rc-service monitor-agent restart
	# supervise-daemon reports the service started while it respawns an agent
	# that exits at once, so the process itself is what is looked for, inside
	# the respawn delay. pidof rather than pgrep -x, which BusyBox matches
	# against the full path.
	sleep 3
	pidof monitor-agent >/dev/null || not_started "$LOG_FILE"
	rm -f "$BIN.old"
	report_install
	exit 0
fi

cat >"$UNIT_FILE" <<UNIT
[Unit]
Description=monitor agent
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
EnvironmentFile=$ENV_FILE
ExecStart=$BIN --interval $INTERVAL${INSECURE:+ --insecure}
Restart=always
RestartSec=5
# Run as root; filesystem and device access remain bounded by the sandbox below.
User=root
NoNewPrivileges=yes
RestrictSUIDSGID=yes
ProtectSystem=strict
# 将写白名单限定在 sing-box 配置目录，其余系统路径仍保持只读。
ReadWritePaths=/etc/sing-box
ProtectHome=yes
PrivateTmp=yes
PrivateDevices=yes
# AF_UNIX 供 systemctl 连接本机 systemd，AF_NETLINK 供 Agent 读取网卡地址。
RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6 AF_NETLINK
MemoryMax=64M

[Install]
WantedBy=multi-user.target
UNIT

systemctl daemon-reload
systemctl enable monitor-agent >/dev/null
# restart rather than `enable --now`: --now leaves an already-running service
# untouched, so reinstalling over a live agent would keep the old binary
# running.
systemctl restart monitor-agent
# Type=simple counts the service started once it is forked, so `restart` above
# succeeds also for one that fails at once -- a user it cannot resolve
# (217/USER), a binary that exits -- and is then restarted every RestartSec.
# Checked inside that window, so a batch run shows the failure on the machine
# where it happened rather than a line reading "installed".
sleep 3
systemctl is-active --quiet monitor-agent || not_started "journalctl -u monitor-agent -n 20"
rm -f "$BIN.old"
report_install
