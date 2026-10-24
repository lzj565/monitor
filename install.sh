#!/bin/sh
# Installs monitor-agent as a systemd or OpenRC service.
#   curl -fsSL https://hub.example.com/install.sh | sh -s -- --server URL --token TOKEN [options]
#   curl -fsSL https://hub.example.com/install.sh | sh -s -- --server URL --register KEY [options]
#   curl -fsSL https://hub.example.com/install.sh | sh -s -- --upgrade
#   curl -fsSL https://hub.example.com/install.sh | sh -s -- --uninstall
set -eu
# rc-update and other administrative tools reside in sbin, which a root shell
# entered through `su` without `-` may omit from PATH.
PATH="$PATH:/usr/sbin:/sbin"

# Keep binaries, component configuration, and hub data under one root.
ROOT="/opt/monitor"
BIN="$ROOT/monitor-agent"
CONFIG_DIR="$ROOT/config"
ENV_FILE="$CONFIG_DIR/agent.env"
REGISTRATION_FILE="$CONFIG_DIR/registration.env"
UNIT_FILE="/etc/systemd/system/monitor-agent.service"
RC_FILE="/etc/init.d/monitor-agent"
LOG_FILE="/var/log/monitor-agent.log"
SING_BOX_VERSION="1.14.4"
SING_BOX="$ROOT/sing-box"
SING_BOX_CONFIG="$CONFIG_DIR/sb.json"
SING_BOX_UNIT="/etc/systemd/system/sing-box.service"
SING_BOX_RC="/etc/init.d/sing-box"
SING_BOX_UNIT_BACKUP="$ROOT/sing-box.service.before-monitor"
SING_BOX_RC_BACKUP="$ROOT/sing-box.openrc.before-monitor"
CURRENT_STEP=""
FAIL_REASON="command failed"
TMP_AGENT=""
TMP_DIR=""
AGENT_CANDIDATE=""
SING_BOX_STAGED=""
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
SINGBOX_API_PORT=""
API_PORT_HELPER=""
MIERU_OPTION=""
MIERU_STATE=unchanged
SINGBOX_OPTION=""
REALM_OPTION=""
REALM="$ROOT/realm"
REALM_CONFIG="$CONFIG_DIR/realm.json"
REALM_UNIT="/etc/systemd/system/monitor-realm.service"
REALM_RC="/etc/init.d/monitor-realm"
REALM_STATE="unchanged"
REALM_STAGED=""

if [ -t 1 ] && [ -z "${NO_COLOR+x}" ]; then
	C_BLUE=$(printf '\033[1;34m')
	C_GREEN=$(printf '\033[32m')
	C_YELLOW=$(printf '\033[33m')
	C_RED=$(printf '\033[31m')
	C_CYAN=$(printf '\033[36m')
	C_RESET=$(printf '\033[0m')
else
	C_BLUE=""; C_GREEN=""; C_YELLOW=""; C_RED=""; C_CYAN=""; C_RESET=""
fi

step() { CURRENT_STEP="$2"; printf '\n%s[%s]%s %s\n' "$C_BLUE" "$1" "$C_RESET" "$2"; }
info() { printf '• %s\n' "$*"; }
detail() {
	case "$1" in
	路径 | 安装位置 | Binary | Agent\ Path | Version | 版本 | Config | 配置检查)
		printf '    %-14s %s%s%s\n' "$1" "$C_CYAN" "$2" "$C_RESET"
		;;
	*) printf '    %-14s %s\n' "$1" "$2" ;;
	esac
}
success() { printf '%s✓%s %s\n' "$C_GREEN" "$C_RESET" "$*"; }
warn() { printf '%s!%s %s\n' "$C_YELLOW" "$C_RESET" "$*" >&2; }
error() { printf '%s✗%s %s\n' "$C_RED" "$C_RESET" "$*" >&2; }
cleanup() {
	[ -z "$TMP_AGENT" ] || rm -f "$TMP_AGENT"
	[ -z "$TMP_DIR" ] || rm -rf "$TMP_DIR"
	[ -z "$AGENT_CANDIDATE" ] || rm -f "$AGENT_CANDIDATE"
	[ -z "$SING_BOX_STAGED" ] || rm -f "$SING_BOX_STAGED"
	[ -z "$REALM_STAGED" ] || rm -f "$REALM_STAGED"
}
on_exit() {
	status=$?
	cleanup
	if [ "$status" -ne 0 ] && [ -n "$CURRENT_STEP" ]; then
		error "安装失败"
		detail "阶段" "$CURRENT_STEP"
		detail "原因" "$FAIL_REASON"
	fi
}
trap 'on_exit' EXIT
trap 'exit 1' HUP INT TERM

fail() { FAIL_REASON="$*"; exit 1; }

# BEGIN sing-box API port helpers
# `ss` reports the local endpoint in column 4. Compare its port without
# assuming a particular bind address: a wildcard, loopback, or another local
# address all reserve the same port for this TCP API.
api_port_is_listening() {
	port=$1
	if command -v ss >/dev/null 2>&1; then
		listeners=$(ss -H -lnt 2>/dev/null) || listeners=""
		if [ -n "$listeners" ]; then
			printf '%s\n' "$listeners" | awk -v port="$port" '
				$1 == "LISTEN" && $4 ~ /:[0-9]+$/ {
					local = $4
					sub(/^.*:/, "", local)
					if (local == port) found = 1
				}
				END { exit !found }
			'
			return $?
		fi
		# An empty, successful `ss` result means there are no TCP listeners.
		ss -H -lnt >/dev/null 2>&1 && return 1
	fi
	"${API_PORT_HELPER:-$BIN}" internal tcp-port-listening "$port" >/dev/null 2>&1
}

# Keep 9001 as the preferred default. When it is occupied, inspect the rest
# of the allocation range in a randomized order so installations do not all
# converge on the same next port.
random_api_port_order() {
	seed=$(od -An -N4 -tu4 /dev/urandom 2>/dev/null | tr -d '[:space:]')
	[ -n "$seed" ] || seed=$(date +%s)
	awk -v seed="$seed" 'BEGIN { srand(seed); for (port = 9002; port <= 9099; port++) printf "%.12f %d\n", rand(), port }' |
		sort -n | awk '{print $2}'
}

find_available_api_port() {
	API_PORT_CANDIDATE=""
	if api_port_is_listening 9001; then
		info "port 9001 is already in use"
	else
		status=$?
		[ "$status" -eq 1 ] || return "$status"
		API_PORT_CANDIDATE=9001
		return 0
	fi

	for port in $(random_api_port_order); do
		if api_port_is_listening "$port"; then
			info "port $port is already in use"
		else
			status=$?
			[ "$status" -eq 1 ] || return "$status"
			API_PORT_CANDIDATE=$port
			return 0
		fi
	done
	return 1
}

valid_api_port() {
	case "$1" in "" | *[!0-9]*) return 1 ;; esac
	# Strip leading zeroes before shell arithmetic (some /bin/sh implementations
	# interpret a leading zero as octal).
	port=$(printf '%s' "$1" | sed 's/^0*//')
	[ -n "$port" ] || port=0
	[ "$port" -ge 1 ] && [ "$port" -le 65535 ] || return 1
	SINGBOX_API_PORT=$port
}

select_singbox_api_port() {
	SAVED_API_PORT=$(sed -n 's/^SINGBOX_API_PORT=//p' "$ENV_FILE" 2>/dev/null | tail -n 1)
	if [ -n "$SAVED_API_PORT" ]; then
		valid_api_port "$SAVED_API_PORT" || {
			FAIL_REASON="invalid SINGBOX_API_PORT in $ENV_FILE"
			return 1
		}
		info "reusing sing-box V2Ray API port: $SINGBOX_API_PORT"
		return 0
	fi

	LEGACY_API_PORT=$("${API_PORT_HELPER:-$BIN}" internal api-port-from-config "$SING_BOX_CONFIG" 2>/dev/null || true)
	if [ -n "$LEGACY_API_PORT" ] && valid_api_port "$LEGACY_API_PORT"; then
		info "reusing sing-box V2Ray API port: $SINGBOX_API_PORT"
		return 0
	fi

	if ! find_available_api_port; then
		FAIL_REASON="unable to find available sing-box V2Ray API port in range 9001-9099"
		return 1
	fi
	SINGBOX_API_PORT=$API_PORT_CANDIDATE
	info "selected sing-box V2Ray API port: $SINGBOX_API_PORT"
}
# END sing-box API port helpers

# Print cgroup memory limits for this process and each visible parent cgroup.
# cgroup limits are hierarchical, so a parent limit still applies when the
# process's own cgroup says "max".
emit_ancestor_memory_limits() {
	ANCESTOR_DIR=$1
	ANCESTOR_ROOT=$2
	shift 2
	case "$ANCESTOR_DIR" in
	"$ANCESTOR_ROOT"/) ANCESTOR_DIR=$ANCESTOR_ROOT ;;
	esac
	case "$ANCESTOR_DIR" in
	"$ANCESTOR_ROOT" | "$ANCESTOR_ROOT"/*) ;;
	*) return 0 ;;
	esac

	while :; do
		for ANCESTOR_FILE in "$@"; do
			[ -r "$ANCESTOR_DIR/$ANCESTOR_FILE" ] || continue
			ANCESTOR_VALUE=$(cat "$ANCESTOR_DIR/$ANCESTOR_FILE" 2>/dev/null || true)
			case "$ANCESTOR_VALUE" in
			"" | max | *[!0-9]*) continue ;;
			esac
			printf '%s\n' "$ANCESTOR_VALUE"
		done
		[ "$ANCESTOR_DIR" = "$ANCESTOR_ROOT" ] && break
		ANCESTOR_PARENT=${ANCESTOR_DIR%/*}
		[ "$ANCESTOR_PARENT" != "$ANCESTOR_DIR" ] || break
		ANCESTOR_DIR=$ANCESTOR_PARENT
		case "$ANCESTOR_DIR" in
		"$ANCESTOR_ROOT" | "$ANCESTOR_ROOT"/*) ;;
		*) break ;;
		esac
	done
}

emit_cgroup_memory_limits() {
	CGROUP_V2_PATH=$(awk -F: '$1 == "0" && $2 == "" { print $3; exit }' /proc/self/cgroup 2>/dev/null || true)
	if [ -n "$CGROUP_V2_PATH" ]; then
		CGROUP_ROOT=/sys/fs/cgroup
		CGROUP_DIR="${CGROUP_ROOT%/}${CGROUP_V2_PATH}"
		[ "$CGROUP_DIR" != "$CGROUP_ROOT/" ] || CGROUP_DIR=$CGROUP_ROOT
		[ -d "$CGROUP_DIR" ] || CGROUP_DIR=$CGROUP_ROOT
		emit_ancestor_memory_limits "$CGROUP_DIR" "$CGROUP_ROOT" memory.max memory.high
		return 0
	fi

	CGROUP_V1_PATH=$(awk -F: '$2 ~ /(^|,)memory(,|$)/ { print $3; exit }' /proc/self/cgroup 2>/dev/null || true)
	[ -n "$CGROUP_V1_PATH" ] || return 0
	CGROUP_V1_ROOT=$(awk '$3 == "cgroup" { n = split($4, opts, ","); for (i = 1; i <= n; i++) if (opts[i] == "memory") { print $2; exit } }' /proc/mounts 2>/dev/null || true)
	[ -n "$CGROUP_V1_ROOT" ] || return 0
	CGROUP_V1_DIR="${CGROUP_V1_ROOT%/}${CGROUP_V1_PATH}"
	[ "$CGROUP_V1_DIR" != "$CGROUP_V1_ROOT/" ] || CGROUP_V1_DIR=$CGROUP_V1_ROOT
	[ -d "$CGROUP_V1_DIR" ] || CGROUP_V1_DIR=$CGROUP_V1_ROOT
	emit_ancestor_memory_limits "$CGROUP_V1_DIR" "$CGROUP_V1_ROOT" memory.limit_in_bytes memory.soft_limit_in_bytes
}

detect_effective_memory() {
	MEMTOTAL_BYTES=$(awk '$1 == "MemTotal:" { printf "%.0f\n", $2 * 1024; exit }' /proc/meminfo 2>/dev/null || true)
	{
		[ -z "$MEMTOTAL_BYTES" ] || printf '%s\n' "$MEMTOTAL_BYTES"
		emit_cgroup_memory_limits
	} | awk '
		$1 ~ /^[0-9]+$/ && ($1 + 0) > 0 && ($1 + 0) < 9e18 {
			value = $1 + 0
			if (!found || value < minimum) minimum = value
			found = 1
		}
		END { if (found) printf "%.0f\n", minimum }
	'
}

calculate_gomemlimit() {
	awk -v bytes="$1" 'BEGIN {
		if (bytes <= 128 * 1024 * 1024) fraction = 0.3125
		else if (bytes <= 256 * 1024 * 1024) fraction = 0.50
		else if (bytes <= 512 * 1024 * 1024) fraction = 0.65
		else fraction = 0.80
		printf "%.0fB\n", int(bytes * fraction)
	}'
}

set_singbox_memory_limit() {
	SINGBOX_GOMEMLIMIT=""
	SINGBOX_SYSTEMD_MEMORY_ENV=""
	SINGBOX_OPENRC_MEMORY_ENV=""
	SINGBOX_EFFECTIVE_MEMORY=$(detect_effective_memory)
	if [ -n "$SINGBOX_EFFECTIVE_MEMORY" ]; then
		SINGBOX_GOMEMLIMIT=$(calculate_gomemlimit "$SINGBOX_EFFECTIVE_MEMORY")
		SINGBOX_SYSTEMD_MEMORY_ENV="Environment=\"GOMEMLIMIT=$SINGBOX_GOMEMLIMIT\""
		SINGBOX_OPENRC_MEMORY_ENV="export GOMEMLIMIT='$SINGBOX_GOMEMLIMIT'"
		detail "sing-box 内存软限" "$SINGBOX_GOMEMLIMIT"
	else
		warn "无法探测有效内存，sing-box 将不设置 GOMEMLIMIT"
	fi
}

print_summary() {
	step 5 "安装完成摘要"
	printf '\n%s========================================%s\n' "$C_GREEN" "$C_RESET"
	printf ' %s安装完成%s\n' "$C_GREEN" "$C_RESET"
	printf '%s========================================%s\n' "$C_GREEN" "$C_RESET"
	printf '\n系统\n'
	detail "OS" "$OS_NAME"
	detail "Architecture" "$MACHINE"
	detail "Init" "$INIT"
	printf '\nMonitor Agent\n'
	detail "Binary" "$BIN"
	if [ "$INIT" = systemd ]; then SERVICE_NAME=monitor-agent.service; else SERVICE_NAME=monitor-agent; fi
	detail "Service" "$SERVICE_NAME"
	detail "Status" "$AGENT_STATUS"
	printf '\nsing-box\n'
	detail "Source" "$SING_BOX_SOURCE"
	if [ "$SING_BOX_SOURCE" = system ]; then
		detail "Binary" "$SING_BOX_SYSTEM_PATH"
		if [ "$SING_BOX_SYSTEM_PATH" = "$SING_BOX" ]; then
			detail "Agent Path" "$SING_BOX"
		else
			detail "Agent Path" "$SING_BOX -> $SING_BOX_SYSTEM_PATH"
		fi
	else
		detail "Binary" "$SING_BOX"
	fi
	detail "Version" "${SING_BOX_VER:-未知}"
	detail "Config" "$SING_BOX_CONFIG"
	if [ "$SING_BOX_SOURCE" = installer ]; then
		if [ "$INIT" = systemd ]; then SERVICE_NAME=sing-box.service; else SERVICE_NAME=sing-box; fi
		detail "Service" "$SERVICE_NAME"
	fi
	detail "Status" "$SING_BOX_STATUS"
	printf '\nmieru\n'
	detail "Status" "$MIERU_STATE"
	printf '\nRealm\n'
	detail "Status" "$REALM_STATE"
	if [ "$REALM_STATE" != unchanged ]; then
		detail "Config" "$REALM_CONFIG"
		if [ "$REALM_STATE" != uninstalled ]; then
			detail "Version" "2.9.6"
			detail "Binary" "$REALM"
			if [ "$INIT" = systemd ]; then
				detail "配置后启动" "systemctl enable --now monitor-realm"
			else
				detail "配置后启动" "rc-update add monitor-realm default; rc-service monitor-realm start"
			fi
		fi
	fi
	printf '\n常用命令\n'
	if [ "$INIT" = systemd ]; then
		detail "Agent 状态" "systemctl status monitor-agent"
		detail "Agent 日志" "journalctl -u monitor-agent -f"
		detail "Agent 重启" "systemctl restart monitor-agent"
		detail "sing-box 状态" "systemctl status sing-box"
		detail "sing-box 日志" "journalctl -u sing-box -f"
		detail "sing-box 重启" "systemctl restart sing-box"
	else
		detail "Agent 状态" "rc-service monitor-agent status"
		detail "Agent 重启" "rc-service monitor-agent restart"
		detail "sing-box 状态" "rc-service sing-box status"
		detail "sing-box 重启" "rc-service sing-box restart"
	fi
	detail "配置检查" "$SING_BOX check -c $SING_BOX_CONFIG"
}

# BEGIN Realm helpers
realm_owned() {
	[ ! -L "$REALM" ] && [ ! -L "$REALM_UNIT" ] && [ ! -L "$REALM_RC" ] &&
	[ -f "$ROOT/.realm-managed" ] &&
		{ [ ! -e "$REALM_UNIT" ] || grep -F '# Managed by monitor installer' "$REALM_UNIT" >/dev/null; } &&
		{ [ ! -e "$REALM_RC" ] || grep -F '# Managed by monitor installer' "$REALM_RC" >/dev/null; }
}

manage_realm() {
	[ -n "$REALM_OPTION" ] || return 0
	CURRENT_STEP="Realm"
	if ! realm_owned; then
		if [ -e "$REALM" ] || [ -L "$REALM" ] || [ -e "$REALM_UNIT" ] || [ -L "$REALM_UNIT" ] || [ -e "$REALM_RC" ] || [ -L "$REALM_RC" ]; then
			fail "Realm paths are not owned by monitor; refusing to overwrite or remove them"
		fi
		if [ "$REALM_OPTION" = --no-realm ]; then REALM_STATE=uninstalled; return 0; fi
	fi
	REALM_RUNNING=""
	if [ "$INIT" = systemd ]; then
		systemctl is-active --quiet monitor-realm && REALM_RUNNING=1
	else
		rc-service monitor-realm status >/dev/null 2>&1 && REALM_RUNNING=1
	fi
	if [ "$REALM_OPTION" = --no-realm ]; then
		if [ "$INIT" = systemd ]; then
			if [ -e "$REALM_UNIT" ]; then
				systemctl disable --now monitor-realm || fail "cannot stop/disable Realm"
			fi
		else
			if [ -e "$REALM_RC" ]; then
				rc-service monitor-realm stop || fail "cannot stop Realm"
				if rc-update show default | grep -E '^[[:space:]]*monitor-realm[[:space:]]' >/dev/null; then
					rc-update del monitor-realm default || fail "cannot disable Realm"
				fi
			fi
		fi
		rm -f "$REALM" "$REALM_UNIT" "$REALM_RC" "$ROOT/.realm-managed"
		[ "$INIT" != systemd ] || systemctl daemon-reload
		REALM_STATE=uninstalled
		return 0
	fi
	case "$ARCH" in
	x86_64) REALM_SHA=b1cc335547bea8bb2a88178bef12ec7f2363e36200e7ea1d4e1e67627929bf65 ;;
	aarch64) REALM_SHA=f4c0318dd86854da483dcb7645b4f39cae2cc3f91c688fef969d53220b949488 ;;
	*) fail "unsupported Realm architecture" ;;
	esac
	mkdir -p "$TMP_DIR/realm"
	REALM_ARCHIVE="$TMP_DIR/realm.tar.gz"
	TRIES=0
	while :; do
		CODE=$(curl -sSL --max-time 300 -w '%{http_code}' "${SERVER%/}/realm/$ARCH" -o "$REALM_ARCHIVE") || fail "Realm download failed"
		[ "$CODE" = 503 ] && [ "$TRIES" -lt 5 ] || break
		TRIES=$((TRIES + 1))
		sleep 5
	done
	[ "$CODE" = 200 ] || fail "Realm download failed (HTTP $CODE)"
	[ "$(sha256sum "$REALM_ARCHIVE" | awk '{print $1}')" = "$REALM_SHA" ] || fail "Realm checksum mismatch"
	# Extract only the executable, never arbitrary archive paths.
	REALM_MEMBER=$(tar -tzf "$REALM_ARCHIVE" | sed -n '/^\(\.\/\)\{0,1\}realm$/p' | head -n 1)
	[ -n "$REALM_MEMBER" ] || fail "Realm archive has no executable"
	tar -xOzf "$REALM_ARCHIVE" "$REALM_MEMBER" > "$TMP_DIR/realm/realm" || fail "cannot extract Realm"
	chmod 0755 "$TMP_DIR/realm/realm"
	"$TMP_DIR/realm/realm" --version | grep -F '2.9.6' >/dev/null || fail "Realm version check failed"
	if [ ! -e "$REALM_CONFIG" ] && [ ! -L "$REALM_CONFIG" ]; then
		printf '%s\n' '{"log":{"level":"warn"},"endpoints":[]}' > "$REALM_CONFIG"
		chmod 0600 "$REALM_CONFIG"
	fi
	REALM_STAGED="$ROOT/.realm.$$"
	install -m 0755 "$TMP_DIR/realm/realm" "$REALM_STAGED"
	mv -f "$REALM_STAGED" "$REALM"
	REALM_STAGED=""
	touch "$ROOT/.realm-managed"
	if [ "$INIT" = systemd ]; then
		cat > "$REALM_UNIT" <<UNIT
# Managed by monitor installer
[Unit]
Description=Monitor Realm relay
After=network-online.target
Wants=network-online.target
[Service]
ExecStart=$REALM -c $REALM_CONFIG
Restart=on-failure
RestartSec=5
[Install]
WantedBy=multi-user.target
UNIT
		chmod 0644 "$REALM_UNIT"
		systemctl daemon-reload
	else
		cat > "$REALM_RC" <<RC
#!/sbin/openrc-run
# Managed by monitor installer
command="$REALM"
command_args="-c $REALM_CONFIG"
command_background=true
pidfile="/run/monitor-realm.pid"
output_log="/var/log/monitor-realm.log"
error_log="/var/log/monitor-realm.log"
depend() {
    need net
}
RC
		chmod 0755 "$REALM_RC"
	fi
	REALM_STATE="installed; configure before starting"
	if [ -n "$REALM_RUNNING" ]; then
		if [ "$INIT" = systemd ]; then
			systemctl restart monitor-realm && systemctl is-active --quiet monitor-realm || fail "Realm restart failed"
		else
			rc-service monitor-realm restart && rc-service monitor-realm status >/dev/null || fail "Realm restart failed"
		fi
		REALM_STATE=running
	fi
}
# END Realm helpers

# BEGIN mieru helpers
manage_mieru() {
    [ -n "$MIERU_OPTION" ] || return 0
    CURRENT_STEP=mieru
    MIERU="$ROOT/mita"
    MIERU_CONFIG="$CONFIG_DIR/mieru.json"
    MIERU_UNIT=/etc/systemd/system/monitor-mieru.service
    MIERU_RC=/etc/init.d/monitor-mieru
    if [ ! -f "$ROOT/.mieru-managed" ] && [ ! -f "$ROOT/.mieru-preserved" ]; then
        for path in "$MIERU" "$MIERU_CONFIG" "$MIERU_UNIT" "$MIERU_RC" /etc/mita /var/lib/mita /run/monitor-mieru; do
            [ ! -e "$path" ] && [ ! -L "$path" ] || fail "mieru paths are not owned by monitor: $path"
        done
        if command -v mita >/dev/null 2>&1; then fail "an unmanaged mita installation already exists"; fi
    fi
    for path in "$MIERU" "$MIERU_CONFIG" "$ROOT/.mieru-managed" "$ROOT/.mieru-preserved" "$MIERU_UNIT" "$MIERU_RC"; do
        [ ! -L "$path" ] || fail "mieru managed path is a symlink: $path"
    done
    if [ "$MIERU_OPTION" = --no-mieru ]; then
        if [ -f "$ROOT/.mieru-managed" ]; then
            if [ "$INIT" = systemd ]; then systemctl disable --now monitor-mieru || fail "cannot stop mieru"; else
                rc-service monitor-mieru stop || fail "cannot stop mieru"
                rc-update del monitor-mieru default >/dev/null 2>&1 || true
            fi
            touch "$ROOT/.mieru-preserved"
            rm -f "$MIERU" "$MIERU_UNIT" "$MIERU_RC" "$ROOT/.mieru-managed"
            [ "$INIT" != systemd ] || systemctl daemon-reload
        fi
        MIERU_STATE=uninstalled
        return 0
    fi
    case "$ARCH" in
        x86_64) MIERU_SHA=ea35fdef84bef1378f32b74a2d5a883d508a49dd4e64dba9a3a8363beed5798c ;;
        aarch64) MIERU_SHA=853eb8c3221ce294afffd7d690dbece929c4e774ed3d81c209b71446b515d1c6 ;;
        *) fail "unsupported mieru architecture" ;;
    esac
    curl -fsSL --max-time 300 "${SERVER%/}/mieru/$ARCH" -o "$TMP_DIR/mita.tar.gz" || fail "mita download failed"
    [ "$(sha256sum "$TMP_DIR/mita.tar.gz" | awk '{print $1}')" = "$MIERU_SHA" ] || fail "mita checksum mismatch"
    MIERU_MEMBER=$(tar -tzf "$TMP_DIR/mita.tar.gz" | sed -n '/^\(\.\/\)\{0,1\}mita$/p' | head -n 1)
    [ -n "$MIERU_MEMBER" ] || fail "mita archive has no executable"
    tar -xOzf "$TMP_DIR/mita.tar.gz" "$MIERU_MEMBER" > "$TMP_DIR/mita" || fail "cannot extract mita"
    chmod 0755 "$TMP_DIR/mita"
    "$TMP_DIR/mita" version | grep -F '3.39.0' >/dev/null || fail "mita version mismatch"
    getent group mita >/dev/null 2>&1 || { groupadd --system mita 2>/dev/null || addgroup -S mita; }
    id mita >/dev/null 2>&1 || { useradd --system -g mita --home-dir /nonexistent --shell /usr/sbin/nologin mita 2>/dev/null || adduser -S -D -H -G mita mita; }
    mkdir -p /var/lib/mita /run/monitor-mieru
    chown mita:mita /var/lib/mita /run/monitor-mieru
    chmod 0750 /var/lib/mita /run/monitor-mieru
    if [ ! -f "$MIERU_CONFIG" ]; then
        (umask 077; printf '%s\n' '{"portBindings":[],"users":[],"loggingLevel":"WARN"}' > "$MIERU_CONFIG")
    fi
    chown mita:mita "$MIERU_CONFIG"
    chmod 0600 "$MIERU_CONFIG"
    # Keep the previous binary and running state until the replacement passes health checks.
    MIERU_RUNNING=""
    if [ "$INIT" = systemd ]; then systemctl is-active --quiet monitor-mieru && MIERU_RUNNING=1 || true; else rc-service monitor-mieru status >/dev/null 2>&1 && MIERU_RUNNING=1 || true; fi
    [ ! -f "$MIERU" ] || cp -p "$MIERU" "$TMP_DIR/mita.old"
    [ ! -f "$MIERU_UNIT" ] || cp -p "$MIERU_UNIT" "$TMP_DIR/mieru-unit.old"
    [ ! -f "$MIERU_RC" ] || cp -p "$MIERU_RC" "$TMP_DIR/mieru-rc.old"
    install -m 0755 "$TMP_DIR/mita" "$ROOT/.mita.$$"
    mv -f "$ROOT/.mita.$$" "$MIERU"
    if [ "$INIT" = systemd ]; then
        cat > "$MIERU_UNIT" <<UNIT
# Managed by monitor installer
[Unit]
Description=Monitor mieru server
After=network-online.target
Wants=network-online.target
[Service]
User=mita
Group=mita
RuntimeDirectory=monitor-mieru
RuntimeDirectoryMode=0750
Environment=MITA_CONFIG_JSON_FILE=$MIERU_CONFIG
Environment=MITA_UDS_PATH=/run/monitor-mieru/mita.sock
ExecStart=$MIERU run
Restart=on-failure
RestartSec=5
[Install]
WantedBy=multi-user.target
UNIT
        chmod 0644 "$MIERU_UNIT"
        systemctl daemon-reload
    else
        cat > "$MIERU_RC" <<RC
#!/sbin/openrc-run
# Managed by monitor installer
command="$MIERU"
command_args="run"
command_user="mita:mita"
supervisor="supervise-daemon"
respawn_delay=5
export MITA_CONFIG_JSON_FILE="$MIERU_CONFIG"
export MITA_UDS_PATH=/run/monitor-mieru/mita.sock
start_pre() { checkpath --directory --owner mita:mita --mode 0750 /run/monitor-mieru; }
depend() { need net; }
RC
        chmod 0755 "$MIERU_RC"
    fi
    touch "$ROOT/.mieru-managed"
    if [ -n "$MIERU_RUNNING" ]; then
        if [ "$INIT" = systemd ]; then
            if ! systemctl restart monitor-mieru || ! systemctl is-active --quiet monitor-mieru; then
                [ ! -f "$TMP_DIR/mita.old" ] || cp -p "$TMP_DIR/mita.old" "$MIERU"
                [ ! -f "$TMP_DIR/mieru-unit.old" ] || cp -p "$TMP_DIR/mieru-unit.old" "$MIERU_UNIT"
                systemctl daemon-reload
                systemctl restart monitor-mieru || true
                fail "mita upgrade failed; restored previous binary"
            fi
        else
            if ! rc-service monitor-mieru restart || ! rc-service monitor-mieru status; then
                [ ! -f "$TMP_DIR/mita.old" ] || cp -p "$TMP_DIR/mita.old" "$MIERU"
                [ ! -f "$TMP_DIR/mieru-rc.old" ] || cp -p "$TMP_DIR/mieru-rc.old" "$MIERU_RC"
                rc-service monitor-mieru restart || true
                fail "mita upgrade failed; restored previous binary"
            fi
        fi
    fi
    MIERU_STATE=installed
}
# END mieru helpers

# BEGIN component profile helpers
# Keep a newly issued token across a later profile/download failure, without replacing the running Agent env.
pending_registration_token() {
    [ -f "$REGISTRATION_FILE" ] && [ ! -L "$REGISTRATION_FILE" ] || return 0
    PENDING_SERVER=$(sed -n 's/^MONITOR_SERVER=//p' "$REGISTRATION_FILE")
    [ "${PENDING_SERVER%/}" = "${SERVER%/}" ] || return 0
    sed -n 's/^MONITOR_TOKEN=//p' "$REGISTRATION_FILE"
}
cache_registration() {
    [ ! -L "$CONFIG_DIR" ] && [ ! -L "$REGISTRATION_FILE" ] || fail "registration cache path is a symlink"
    install -d -m 0755 "$CONFIG_DIR"
    REGISTRATION_STAGED=$(mktemp "$CONFIG_DIR/.registration.XXXXXX") || fail "cannot stage registration cache"
    (umask 077; printf 'MONITOR_SERVER=%s\nMONITOR_TOKEN=%s\n' "$SERVER" "$TOKEN" > "$REGISTRATION_STAGED") || fail "cannot save registration cache"
    chmod 0600 "$REGISTRATION_STAGED"
    mv -f "$REGISTRATION_STAGED" "$REGISTRATION_FILE"
}

load_component_profile() {
    info "读取节点已保存的安装选项"
    curl -fsSL --max-time 30 -H "Authorization: Bearer $TOKEN" "${SERVER%/}/api/agent/components" -o "$TMP_DIR/components.profile" || fail "cannot read saved component choices; no programs or services changed"
    PROFILE_SINGBOX_COUNT=0 PROFILE_REALM_COUNT=0 PROFILE_MIERU_COUNT=0
    PROFILE_REALM='' PROFILE_MIERU=''
    while IFS= read -r PROFILE_LINE || [ -n "$PROFILE_LINE" ]; do
        case "$PROFILE_LINE" in
            singbox=1) PROFILE_SINGBOX_COUNT=$((PROFILE_SINGBOX_COUNT + 1)) ;;
            realm=0) PROFILE_REALM_COUNT=$((PROFILE_REALM_COUNT + 1)); PROFILE_REALM=--no-realm ;;
            realm=1) PROFILE_REALM_COUNT=$((PROFILE_REALM_COUNT + 1)); PROFILE_REALM=--realm ;;
            mieru=0) PROFILE_MIERU_COUNT=$((PROFILE_MIERU_COUNT + 1)); PROFILE_MIERU=--no-mieru ;;
            mieru=1) PROFILE_MIERU_COUNT=$((PROFILE_MIERU_COUNT + 1)); PROFILE_MIERU=--mieru ;;
            *) fail "invalid saved component choices" ;;
        esac
    done < "$TMP_DIR/components.profile"
    [ "$PROFILE_SINGBOX_COUNT:$PROFILE_REALM_COUNT:$PROFILE_MIERU_COUNT" = 1:1:1 ] || fail "incomplete saved component choices"
    [ -n "$REALM_OPTION" ] || REALM_OPTION=$PROFILE_REALM
    [ -n "$MIERU_OPTION" ] || MIERU_OPTION=$PROFILE_MIERU
    SINGBOX_OPTION=--sing-box
}
# END component profile helpers

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
    --mieru | --no-mieru)
        [ -z "$MIERU_OPTION" ] || [ "$MIERU_OPTION" = "$1" ] || { echo "conflicting mieru flags" >&2; exit 2; }
        MIERU_OPTION=$1; shift ;;
    --skip-sing-box) echo "sing-box is required; --skip-sing-box is no longer supported" >&2; exit 2 ;;
    --sing-box)
        [ -z "$SINGBOX_OPTION" ] || [ "$SINGBOX_OPTION" = "$1" ] || { echo "conflicting sing-box flags" >&2; exit 2; }
        SINGBOX_OPTION=$1; shift ;;
	--realm | --no-realm)
		[ -z "$REALM_OPTION" ] || [ "$REALM_OPTION" = "$1" ] || { echo "--realm and --no-realm are mutually exclusive" >&2; exit 2; }
		REALM_OPTION=$1; shift ;;
	*) echo "unknown option: $1" >&2; exit 2 ;;
	esac
done

SINGBOX_OPTION=--sing-box

step 0 "检查系统环境"
[ "$(id -u)" = 0 ] || { CURRENT_STEP="检查系统环境"; FAIL_REASON="请使用 root 用户运行安装程序"; fail "$FAIL_REASON"; }
if [ -r /etc/os-release ]; then
	OS_NAME=$(sed -n 's/^PRETTY_NAME=//p' /etc/os-release | head -n 1 | tr -d '"')
else
	OS_NAME=$(uname -s)
fi
[ -n "$OS_NAME" ] || OS_NAME="未知 Linux"
MACHINE=$(uname -m)
if [ -d /run/systemd/system ] && command -v systemctl >/dev/null 2>&1; then
	INIT=systemd
elif [ -e /run/openrc/softlevel ] && command -v rc-service >/dev/null 2>&1 && command -v rc-update >/dev/null 2>&1; then
	INIT=openrc
else
	CURRENT_STEP="检查系统环境"
	FAIL_REASON="不支持当前 init system；目前支持 systemd 和 OpenRC"
	exit 1
fi
detail "系统" "$OS_NAME"
detail "架构" "$MACHINE"
detail "Init" "$INIT"
detail "用户" "$(id -un)"
success "系统检查完成"

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
	rm -f "$UNIT_FILE" "$RC_FILE" "$LOG_FILE" "$BIN" "$BIN.old" "$ENV_FILE" "$REGISTRATION_FILE" "$ROOT/agent.env"
	systemctl daemon-reload 2>/dev/null || true
	userdel monitor-agent 2>/dev/null || deluser monitor-agent 2>/dev/null || true
	rmdir "$CONFIG_DIR" "$ROOT" 2>/dev/null || true
	success "monitor-agent 已卸载"
	exit 0
fi

# BEGIN config migration
# Copy legacy files without overwriting the new configuration or removing the
# originals: an interrupted upgrade can still run the previous service.
migrate_config() {
	install -d -m 0755 "$CONFIG_DIR"
	if [ ! -e "$ENV_FILE" ] && [ ! -L "$ENV_FILE" ] && [ -f "$ROOT/agent.env" ]; then
		install -m 0600 "$ROOT/agent.env" "$ENV_FILE"
	fi
	if [ ! -e "$SING_BOX_CONFIG" ] && [ ! -L "$SING_BOX_CONFIG" ]; then
		for legacy in "$ROOT/sb.json" /etc/sing-box/config.json; do
			[ -f "$legacy" ] || continue
			install -m 0600 "$legacy" "$SING_BOX_CONFIG"
			info "已复制 $legacy 到 ${SING_BOX_CONFIG}，原文件保留"
			break
		done
	fi
}
# END config migration
migrate_config

# Reinstalls the binary with what this machine already holds. It is the one
# command a whole fleet can be upgraded with, because it carries no credential
# and names no node: the token and the hub address come from the env file, which
# only an install writes. Registering is not reached, so it can neither add a
# node nor spend a key, and a machine with nothing installed is told to use the
# panel's command rather than quietly becoming a new node.
if [ -n "$UPGRADE" ]; then
	[ -z "$TOKEN$REGISTER" ] ||
		{ echo "--upgrade takes no --token or --register; it reuses what this machine holds" >&2; exit 2; }
	TOKEN=$(sed -n 's/^[[:space:]]*MONITOR_TOKEN=//p' "$ENV_FILE" 2>/dev/null | tail -n 1)
	[ -n "$SERVER" ] || SERVER=$(sed -n 's/^MONITOR_SERVER=//p' "$ENV_FILE" 2>/dev/null | tail -n 1)
	[ -n "$TOKEN" ] && [ -n "$SERVER" ] || {
		echo "no agent is installed here: $ENV_FILE holds no token and hub address." >&2
		echo "install it with the command from the panel instead" >&2
		exit 2
	}
fi

[ -n "$SERVER" ] && { [ -n "$TOKEN" ] || [ -n "$REGISTER" ]; } || {
	echo "usage: install.sh --server URL (--token TOKEN | --register KEY) [--interval SECONDS] [--iface LIST] [--insecure] [--realm | --no-realm] [--mieru | --no-mieru] [--sing-box]" >&2
	echo "       install.sh --upgrade [--iface LIST] [--interval SECONDS] [--realm | --no-realm] [--mieru | --no-mieru] [--sing-box]" >&2
	echo "       install.sh --uninstall" >&2
	echo "--name NAME names the node --register creates, which is otherwise named after its id. < > \" ' are dropped from it; the panel renames a node freely" >&2
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
	if [ -n "$INTERVAL" ]; then info "保留上次安装的 --interval $INTERVAL"; else INTERVAL=1; fi
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
	[ -z "$IFACE" ] || info "保留上次安装的 --iface $IFACE"
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
		warn "--insecure 将通过未加密的 HTTP 连接远程 Hub"
		echo "         the token and every report travel in the clear, and the binary" >&2
		echo "         installed below is fetched over the same unverified channel" >&2
	fi
	;;
esac
case "$(uname -m)" in
x86_64 | amd64) ARCH=x86_64; SING_BOX_ARCH=amd64 ;;
aarch64 | arm64) ARCH=aarch64; SING_BOX_ARCH=arm64 ;;
*) echo "unsupported architecture: $(uname -m)" >&2; exit 1 ;;
esac

# The hub relays the binary, so a node need only reach the hub it already talks
# to: an IPv6-only or blocked machine cannot resolve github.com. A hub unable to
# fetch releases itself is configured with a GitHub proxy in its own settings,
# which is why none is requested here.
step 1 "安装 monitor-agent"
detail "架构" "$ARCH"
detail "安装位置" "$BIN"
URL="${SERVER%/}/agent/$ARCH"
TMP_AGENT=$(mktemp)

info "正在下载 monitor-agent"
# The hub relays four downloads at once and queues the rest for 30 seconds. A
# batch run on more machines than drain in that time is turned away with 503,
# which is retried here rather than failing the machine. Any other refusal is
# final and shown with the hub's own reason, which --fail would discard.
TRIES=0
while :; do
	CODE=$(curl -sSL --max-time 300 -w '%{http_code}' "$URL" -o "$TMP_AGENT" 2>/dev/null) || { FAIL_REASON="下载 monitor-agent 失败"; exit 1; }
	[ "$CODE" = 503 ] && [ "$TRIES" -lt 5 ] || break
	TRIES=$((TRIES + 1))
	warn "Hub 正在处理中，5 秒后重试"
	sleep 5
done
[ "$CODE" = 200 ] || { FAIL_REASON="monitor-agent 下载失败 (HTTP $CODE)"; exit 1; }
# A relay can answer 200 with something other than the program, such as a
# mirror's error page. Checked before the running agent is stopped, so a batch
# run through such a relay leaves each machine on the agent it had, rather than
# on bytes that cannot start while this script reports success.
[ "$(head -c 4 "$TMP_AGENT")" = "$(printf '\177ELF')" ] || { FAIL_REASON="下载内容不是 Linux 可执行文件"; fail "$FAIL_REASON"; }
success "下载完成"

# Resolve the local API port before registration or replacing the installed
# Agent. A full range must fail without spending a registration key or leaving
# a machine half-upgraded.
API_PORT_HELPER=$TMP_AGENT
info "checking sing-box V2Ray API port..."
select_singbox_api_port || fail "$FAIL_REASON"

# Downloaded before the registration below, because that step spends a node: the
# key returns a token and the panel gains a row, while the env file recording it
# is only written once the binary is in place. A download that fails after
# registering therefore leaves an unusable node behind, and the rerun -- the
# documented way to recover -- registers a second one.
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
		HELD=$(sed -n 's/^[[:space:]]*MONITOR_TOKEN=//p' "$ENV_FILE" 2>/dev/null || true)
	fi
    PENDING_TOKEN=$(pending_registration_token)
    [ -z "$PENDING_TOKEN" ] || HELD=$PENDING_TOKEN
	# --name as given, which the hub trims and bounds. The hostname is not sent
	# in its place: it often carries the machine's address or the provider's
	# label, and the name reaches the public page. Without one the hub names
	# the node after its id.
	echo "registering ${NAME:-this machine} with the hub"
	# curl sends no header at all for an empty $HELD. The status follows the body
	# on a line of its own, so a refusal shows the hub's own reason: a closed
	# window, a lockout, an entry that is not an https domain and a database
	# error share one exit status under --fail. A request that got no response
	# stops here, with curl's own message. The name travels on stdin: as an
	# argument, one beginning with @ would be read as a file to send.
    REGISTER_REALM=0 REGISTER_MIERU=0
    [ "$REALM_OPTION" != --realm ] || REGISTER_REALM=1
    [ "$MIERU_OPTION" != --mieru ] || REGISTER_MIERU=1
	REPLY=$(printf '%s' "$NAME" | curl -sS --max-time 30 -w '\n%{http_code}' -H "Authorization: Bearer $REGISTER" \
		-H "X-Node-Token: $HELD" -H "X-Install-Realm: $REGISTER_REALM" -H "X-Install-Mieru: $REGISTER_MIERU" --data-binary @- "${SERVER%/}/api/agent/register" 2>/dev/null) || { FAIL_REASON="向 Hub 注册 monitor-agent 失败"; exit 1; }
	CODE=$(printf '%s\n' "$REPLY" | tail -n 1)
	TOKEN=$(printf '%s\n' "$REPLY" | sed '$d')
	if [ "$CODE" != 200 ]; then
		# The hub answers in one line of text. A proxy or CDN in front may answer
		# with a page of HTML instead, of which the first line is enough.
		printf 'registration failed (HTTP %s)\n' "$CODE" >&2
		[ -z "$HELD" ] || warn "如果节点已删除或 token 已重新签发，本机持有的 token 将不再有效"
		exit 1
	fi
	[ -n "$TOKEN" ] || { FAIL_REASON="Hub 响应中没有 token"; fail "$FAIL_REASON"; }
    cache_registration
	if [ "$TOKEN" = "$HELD" ]; then
        # Batch defaults are only for new registrations. Existing nodes retain their saved profile.
        REALM_OPTION='' MIERU_OPTION=''
		info "此主机已注册，保留当前 token、名称和安装选择"
	elif [ -n "$HELD" ]; then
		warn "旧 token 已不再对应有效节点，本次已注册为新节点；如旧节点仍存在，请在面板删除"
	fi
fi

load_component_profile

# Keep the existing registration and credential flow above intact. Stage the
# validated agent beside its destination and rename atomically so a failed copy
# cannot truncate the currently installed executable.
install -d -m 0755 "$ROOT"
AGENT_CANDIDATE="$ROOT/.monitor-agent.$$"
install -m 0755 "$TMP_AGENT" "$AGENT_CANDIDATE"
[ ! -f "$BIN" ] || [ -f "$BIN.old" ] || cp "$BIN" "$BIN.old"
mv -f "$AGENT_CANDIDATE" "$BIN"
success "monitor-agent binary 安装完成"

# The system package binary is left in place, while the Agent and service use
# this release's self-built binary with V2Ray API support.
SING_BOX_SOURCE=installer
SING_BOX_STATUS=unknown
SING_BOX_VER=""
step 2 "检查 sing-box"
set_singbox_memory_limit
SING_BOX_SOURCE="installer"
SING_BOX_SYSTEM_PATH=$(command -v sing-box 2>/dev/null || true)
SING_BOX_VER=""
if [ -n "$SING_BOX_SYSTEM_PATH" ]; then
	SING_BOX_VER=$("$SING_BOX_SYSTEM_PATH" version 2>/dev/null | sed -n '1s/.*version[[:space:]]*//p')
	detail "系统 sing-box" "$SING_BOX_SYSTEM_PATH (${SING_BOX_VER:-版本未知})，保留系统包文件"
else
	detail "系统 sing-box" "未安装"
fi

TMP_DIR=$(mktemp -d "${TMPDIR:-/tmp}/monitor-install.XXXXXX")
SING_BOX_CANDIDATE="$TMP_DIR/sing-box"
SING_BOX_SHA="$TMP_DIR/sing-box.sha256"
SING_BOX_URL="${SERVER%/}/sing-box/$SING_BOX_ARCH"
info "正在检查自构建 sing-box $SING_BOX_VERSION（含 with_v2ray_api）"
curl -fsSL --max-time 60 "$SING_BOX_URL.sha256" -o "$SING_BOX_SHA" || { FAIL_REASON="下载 sing-box 校验和失败"; exit 1; }
EXPECTED_SHA=$(sed -n '1{s/[[:space:]].*//;p;}' "$SING_BOX_SHA" | tr 'A-F' 'a-f')
case "$EXPECTED_SHA" in
	????????????????????????????????????????????????????????????????) ;;
	*) FAIL_REASON="sing-box 校验和格式不正确"; fail "$FAIL_REASON" ;;
esac
case "$EXPECTED_SHA" in *[!0-9a-fA-F]*) FAIL_REASON="sing-box 校验和格式不正确"; fail "$FAIL_REASON" ;; esac
if [ -f "$SING_BOX" ] && [ -x "$SING_BOX" ]; then
	INSTALLED_SING_BOX_SHA=$(sha256sum "$SING_BOX" | awk '{print $1}')
	if [ "$INSTALLED_SING_BOX_SHA" = "$EXPECTED_SHA" ]; then
		SING_BOX_CANDIDATE=$SING_BOX
		info "已安装的 sing-box 校验和匹配，跳过二进制下载"
	fi
fi
if [ "$SING_BOX_CANDIDATE" != "$SING_BOX" ]; then
	info "正在下载自构建 sing-box"
	curl -fsSL --max-time 300 "$SING_BOX_URL" -o "$SING_BOX_CANDIDATE" || { FAIL_REASON="下载自构建 sing-box 失败"; exit 1; }
	ACTUAL_SHA=$(sha256sum "$SING_BOX_CANDIDATE" | awk '{print $1}')
	[ "$ACTUAL_SHA" = "$EXPECTED_SHA" ] || { FAIL_REASON="自构建 sing-box 校验和不匹配"; fail "$FAIL_REASON"; }
fi
chmod 0755 "$SING_BOX_CANDIDATE"
VERSION_OUTPUT=$("$SING_BOX_CANDIDATE" version 2>&1) || { FAIL_REASON="自构建 sing-box binary 无法运行"; fail "$FAIL_REASON"; }
printf '%s\n' "$VERSION_OUTPUT" | grep -F "version $SING_BOX_VERSION" >/dev/null || {
	FAIL_REASON="自构建 sing-box 版本不匹配"
	fail "$FAIL_REASON"
}
printf '%s\n' "$VERSION_OUTPUT" | grep -F "with_v2ray_api" >/dev/null || {
	FAIL_REASON="sing-box binary 不包含 with_v2ray_api"
	fail "$FAIL_REASON"
}
SING_BOX_VER=$(printf '%s\n' "$VERSION_OUTPUT" | sed -n '1s/.*version[[:space:]]*//p')

install -d -m 0755 "$ROOT" "$CONFIG_DIR"

if [ ! -e "$SING_BOX_CONFIG" ] && [ ! -L "$SING_BOX_CONFIG" ]; then
	(
		umask 077
		cat >"$SING_BOX_CONFIG" <<CONFIG
{
  "outbounds": [
    {
      "type": "direct",
      "tag": "direct"
    }
  ]
}
CONFIG
	)
fi
"$BIN" internal set-api-port-in-config "$SING_BOX_CONFIG" "$SINGBOX_API_PORT" || {
	FAIL_REASON="could not set sing-box V2Ray API listen address to 127.0.0.1:$SINGBOX_API_PORT"
	fail "$FAIL_REASON"
}
"$SING_BOX_CANDIDATE" check -c "$SING_BOX_CONFIG" || {
	FAIL_REASON="当前配置不能被自构建 sing-box $SING_BOX_VERSION 校验: $SING_BOX_CONFIG"
	fail "$FAIL_REASON"
}
if [ -e "$SING_BOX" ] && [ -d "$SING_BOX" ]; then
	FAIL_REASON="$SING_BOX 是目录，无法安装 sing-box binary"
	fail "$FAIL_REASON"
fi
if [ "$SING_BOX_CANDIDATE" != "$SING_BOX" ]; then
	SING_BOX_STAGED="$ROOT/.sing-box.$$"
	install -m 0755 "$SING_BOX_CANDIDATE" "$SING_BOX_STAGED"
	mv -f "$SING_BOX_STAGED" "$SING_BOX"
	SING_BOX_STAGED=""
fi
detail "Agent 路径" "$SING_BOX"
detail "版本" "$SING_BOX_VER"
success "自构建 sing-box 安装及配置校验完成"

manage_realm
manage_mieru

step 3 "初始化系统服务"
if [ "$INIT" = systemd ]; then
	if { [ -e "$SING_BOX_UNIT" ] || [ -L "$SING_BOX_UNIT" ]; } &&
		! grep -F "$SING_BOX" "$SING_BOX_UNIT" >/dev/null 2>&1 &&
		[ ! -e "$SING_BOX_UNIT_BACKUP" ]; then
		cp -p "$SING_BOX_UNIT" "$SING_BOX_UNIT_BACKUP"
	fi
	[ ! -L "$SING_BOX_UNIT" ] || rm -f "$SING_BOX_UNIT"
	cat >"$SING_BOX_UNIT" <<UNIT
[Unit]
Description=sing-box service
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
${SINGBOX_SYSTEMD_MEMORY_ENV}
ExecStart=$SING_BOX run -c $SING_BOX_CONFIG
Restart=on-failure
RestartSec=3

[Install]
WantedBy=multi-user.target
UNIT
	chmod 0644 "$SING_BOX_UNIT"
else
	if { [ -e "$SING_BOX_RC" ] || [ -L "$SING_BOX_RC" ]; } &&
		! grep -F "$SING_BOX" "$SING_BOX_RC" >/dev/null 2>&1 &&
		[ ! -e "$SING_BOX_RC_BACKUP" ]; then
		cp -p "$SING_BOX_RC" "$SING_BOX_RC_BACKUP"
	fi
	[ ! -L "$SING_BOX_RC" ] || rm -f "$SING_BOX_RC"
	cat >"$SING_BOX_RC" <<RC
#!/sbin/openrc-run
description="sing-box service"
${SINGBOX_OPENRC_MEMORY_ENV}
command="$SING_BOX"
command_args="run -c $SING_BOX_CONFIG"
supervisor="supervise-daemon"
respawn_delay=3

depend() {
	need net
}
RC
	chmod 0755 "$SING_BOX_RC"
fi

rm -f "$ROOT/.skip-sing-box"

# The token lives in a root-only environment file rather than the unit, keeping
# it out of `systemctl cat` and the world-readable journal. 0600 root is what
# keeps it private, since $ROOT itself is readable and holds the binaries. Set in
# a subshell, because the unit file written below is read by anyone debugging
# with `systemctl cat` and need not be 0600.
(
	umask 077
	cat >"$ENV_FILE" <<ENV
MONITOR_SERVER=$SERVER
MONITOR_TOKEN=$TOKEN
SINGBOX_API_PORT=$SINGBOX_API_PORT
ENV
	[ -z "$IFACE" ] || printf 'MONITOR_IFACE=%s\n' "$IFACE" >>"$ENV_FILE"
)
chmod 0600 "$ENV_FILE"
[ "$(pending_registration_token)" != "$TOKEN" ] || rm -f "$REGISTRATION_FILE"

# The new agent is not running. The binary it replaced is put back and started
# again, so a failed upgrade leaves the machine reporting as before; the unit
# and env file just written suit that binary as well, since an upgrade keeps the
# token and the settings. A first install has nothing to put back.
not_started() {
	FAIL_REASON="monitor-agent 未能启动；查看 $1"
	error "$FAIL_REASON"
	[ -f "$BIN.old" ] || exit 1
	mv -f "$BIN.old" "$BIN"
	if [ "$INIT" = openrc ]; then
		rc-service monitor-agent restart >/dev/null 2>&1 || true
	else
		systemctl restart monitor-agent || true
	fi
	warn "已恢复之前的 monitor-agent binary 并尝试重新启动"
	exit 1
}

if [ "$INIT" = openrc ]; then
	cat >"$RC_FILE" <<RC
#!/sbin/openrc-run
description="monitor agent"
command="$BIN"
command_args="--interval $INTERVAL${INSECURE:+ --insecure}"
supervisor="supervise-daemon"
respawn_delay=5
output_log="$LOG_FILE"
error_log="$LOG_FILE"

depend() {
	need net
}

# The token stays in the root-only env file rather than the service script.
start_pre() {
	set -a
	. "$ENV_FILE"
	set +a
}
RC
	chmod 0755 "$RC_FILE"
	success "系统服务定义已初始化"
	step 4 "启动服务"
	if [ "$SING_BOX_SOURCE" = installer ]; then
		rc-update add sing-box default >/dev/null
		rc-service sing-box restart || { FAIL_REASON="sing-box 启动失败"; fail "$FAIL_REASON"; }
		rc-service sing-box status >/dev/null 2>&1 || { FAIL_REASON="sing-box 未能进入运行状态"; fail "$FAIL_REASON"; }
	elif [ "$SING_BOX_SOURCE" != skipped ]; then
		rc-service sing-box status >/dev/null 2>&1 || warn "sing-box 已安装，但当前未检测到运行状态"
	fi
	rc-update add monitor-agent default >/dev/null
	rc-service monitor-agent restart || not_started "$LOG_FILE"
	# supervise-daemon reports the service started while it respawns an agent
	# that exits at once, so the process itself is what is looked for, inside
	# the respawn delay. pidof rather than pgrep -x, which BusyBox matches
	# against the full path.
	sleep 3
	pidof monitor-agent >/dev/null || not_started "$LOG_FILE"
	rm -f "$BIN.old"
	AGENT_STATUS=running
	SING_BOX_STATUS=unknown
	if [ "$SING_BOX_SOURCE" != skipped ]; then
		if rc-service sing-box status >/dev/null 2>&1; then SING_BOX_STATUS=running; else SING_BOX_STATUS=not-running; fi
	fi
	success "monitor-agent 安装并启动完成"
	CURRENT_STEP=""
	print_summary
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
ProtectSystem=full
ReadWritePaths=$CONFIG_DIR
ProtectHome=yes
NoNewPrivileges=yes
RestrictSUIDSGID=yes
PrivateTmp=yes
PrivateDevices=yes
# AF_NETLINK is how getifaddrs(3) obtains this host's own addresses from the
# kernel; without it the agent reports none.
RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6 AF_NETLINK
MemoryMax=64M

[Install]
WantedBy=multi-user.target
UNIT

success "系统服务定义已初始化"
step 4 "启动服务"
if [ "$SING_BOX_SOURCE" = installer ]; then
	systemctl daemon-reload
	systemctl enable sing-box.service >/dev/null
	systemctl restart sing-box.service || { FAIL_REASON="sing-box 启动失败"; fail "$FAIL_REASON"; }
	if systemctl is-active --quiet sing-box.service; then SING_BOX_STATUS=running; else FAIL_REASON="sing-box 未能进入 active 状态"; fail "$FAIL_REASON"; fi
elif [ "$SING_BOX_SOURCE" != skipped ]; then
	if systemctl is-active --quiet sing-box.service; then SING_BOX_STATUS=running; else SING_BOX_STATUS=not-running; warn "sing-box 已安装，但当前未检测到运行状态"; fi
fi
systemctl daemon-reload
systemctl enable monitor-agent.service >/dev/null
systemctl restart monitor-agent.service || not_started "journalctl -u monitor-agent -n 20"
# Type=simple counts the service started once it is forked, so `restart` above
# succeeds also for one that fails at once -- a user it cannot resolve
# (217/USER), a binary that exits -- and is then restarted every RestartSec.
# Checked inside that window, so a batch run shows the failure on the machine
# where it happened rather than a line reading "installed".
sleep 3
systemctl is-active --quiet monitor-agent.service || not_started "journalctl -u monitor-agent -n 20"
rm -f "$BIN.old"
AGENT_STATUS=running
success "monitor-agent 安装并启动完成"
CURRENT_STEP=""
print_summary
