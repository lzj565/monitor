#!/bin/sh
set -eu

INSTALLER=${1:-install.sh}
TMP=$(mktemp -d "${TMPDIR:-/tmp}/monitor-api-port.XXXXXX")
trap 'rm -rf "$TMP"' EXIT HUP INT TERM
mkdir -p "$TMP/bin"

cat >"$TMP/bin/ss" <<'SS'
#!/bin/sh
cat "$TEST_SS_FILE"
SS
chmod +x "$TMP/bin/ss"
PATH="$TMP/bin:$PATH"
export PATH

cat >"$TMP/agent" <<'AGENT'
#!/bin/sh
case "$1:$2" in
internal:api-port-from-config) cat "$TEST_LEGACY_PORT_FILE" 2>/dev/null || exit 3 ;;
internal:tcp-port-listening)
	case " $TEST_BUSY_PORTS " in *" $3 "*) exit 0 ;; *) exit 1 ;; esac
	;;
esac
exit 2
AGENT
chmod +x "$TMP/agent"

# Exercise the actual installer functions without running root/network setup.
eval "$(sed -n '/# BEGIN sing-box API port helpers/,/# END sing-box API port helpers/p' "$INSTALLER" | sed '1d;$d')"

info() { printf '%s\n' "$*" >>"$TEST_LOG"; }
assert_eq() { [ "$1" = "$2" ] || { echo "expected '$2', got '$1'" >&2; exit 1; }; }
reset_case() {
	: >"$TEST_SS_FILE"
	: >"$TEST_LOG"
	: >"$ENV_FILE"
	: >"$TEST_LEGACY_PORT_FILE"
	TEST_BUSY_PORTS=""
	SINGBOX_API_PORT=""
	FAIL_REASON=""
}

TEST_SS_FILE="$TMP/ss.out"
TEST_LOG="$TMP/log"
TEST_LEGACY_PORT_FILE="$TMP/legacy-port"
ENV_FILE="$TMP/agent.env"
SING_BOX_CONFIG="$TMP/config.json"
BIN="$TMP/agent"
API_PORT_HELPER="$TMP/agent"
export TEST_SS_FILE TEST_LEGACY_PORT_FILE TEST_BUSY_PORTS

reset_case
select_singbox_api_port
assert_eq "$SINGBOX_API_PORT" 9001
grep -Fx 'selected sing-box V2Ray API port: 9001' "$TEST_LOG" >/dev/null

reset_case
port=9001
while [ "$port" -le 9099 ]; do
	[ "$port" -eq 9002 ] || printf 'LISTEN 0 128 [::]:%s [::]:*\n' "$port" >>"$TEST_SS_FILE"
	port=$((port + 1))
done
select_singbox_api_port
assert_eq "$SINGBOX_API_PORT" 9002
grep -Fx 'port 9001 is already in use' "$TEST_LOG" >/dev/null
grep -Fx 'selected sing-box V2Ray API port: 9002' "$TEST_LOG" >/dev/null

for bind in 127.0.0.1 0.0.0.0 '[::]'; do
	reset_case
	printf 'LISTEN 0 128 %s:9001 %s:*\n' "$bind" "$bind" >"$TEST_SS_FILE"
	if ! api_port_is_listening 9001; then
		echo "listener on $bind was not detected" >&2
		exit 1
	fi
done

reset_case
printf 'SINGBOX_API_PORT=9005\n' >"$ENV_FILE"
printf 'LISTEN 0 128 0.0.0.0:9005 0.0.0.0:*\n' >"$TEST_SS_FILE"
select_singbox_api_port
assert_eq "$SINGBOX_API_PORT" 9005
grep -Fx 'reusing sing-box V2Ray API port: 9005' "$TEST_LOG" >/dev/null

reset_case
printf '9004\n' >"$TEST_LEGACY_PORT_FILE"
printf 'LISTEN 0 128 127.0.0.1:9004 0.0.0.0:*\n' >"$TEST_SS_FILE"
select_singbox_api_port
assert_eq "$SINGBOX_API_PORT" 9004
grep -Fx 'reusing sing-box V2Ray API port: 9004' "$TEST_LOG" >/dev/null

reset_case
port=9001
while [ "$port" -le 9099 ]; do
	TEST_BUSY_PORTS="$TEST_BUSY_PORTS $port"
	printf 'LISTEN 0 128 0.0.0.0:%s 0.0.0.0:*\n' "$port" >>"$TEST_SS_FILE"
	port=$((port + 1))
done
export TEST_BUSY_PORTS
if select_singbox_api_port; then
	echo "expected port exhaustion to fail" >&2
	exit 1
fi
assert_eq "$FAIL_REASON" 'unable to find available sing-box V2Ray API port in range 9001-9099'

echo "installer sing-box API port selection: ok"
