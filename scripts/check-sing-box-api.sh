#!/bin/sh
set -eu

if [ "$#" -lt 1 ] || [ "$#" -gt 2 ]; then
	echo "usage: check-sing-box-api.sh SING_BOX_BINARY [API_PORT]" >&2
	exit 2
fi

SING_BOX=$1
case "$SING_BOX" in /*) ;; *) SING_BOX="$PWD/$SING_BOX" ;; esac
API_PORT=${2:-${SINGBOX_API_PORT:-}}
api_port_is_listening() {
	ss -H -lnt 2>/dev/null | awk -v port="$1" '$1 == "LISTEN" && $4 ~ /:[0-9]+$/ { local=$4; sub(/^.*:/, "", local); if (local == port) found=1 } END { exit !found }'
}
if [ -z "$API_PORT" ]; then
	command -v ss >/dev/null 2>&1 || { echo "pass API_PORT when ss is unavailable" >&2; exit 2; }
	if api_port_is_listening 9001; then
		API_PORT=""
	else
		API_PORT=9001
	fi
	if [ -z "$API_PORT" ]; then
		seed=$(od -An -N4 -tu4 /dev/urandom 2>/dev/null | tr -d '[:space:]')
		[ -n "$seed" ] || seed=$(date +%s)
		for candidate in $(awk -v seed="$seed" 'BEGIN { srand(seed); for (p=9002; p<=9099; p++) printf "%.12f %d\n", rand(), p }' | sort -n | awk '{print $2}'); do
			if ! api_port_is_listening "$candidate"; then API_PORT=$candidate; break; fi
		done
	fi
fi
[ -n "$API_PORT" ] || { echo "unable to find available sing-box V2Ray API port in range 9001-9099" >&2; exit 1; }
case "$API_PORT" in "" | *[!0-9]*) echo "API_PORT must be an integer from 1 to 65535" >&2; exit 2 ;; esac
[ "$API_PORT" -ge 1 ] && [ "$API_PORT" -le 65535 ] || { echo "API_PORT must be an integer from 1 to 65535" >&2; exit 2; }
PROTO_DIR=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
TMP_DIR=$(mktemp -d "${TMPDIR:-/tmp}/sing-box-api-smoke.XXXXXX")
PID=""
cleanup() {
	if [ -n "$PID" ]; then
		kill "$PID" 2>/dev/null || true
		wait "$PID" 2>/dev/null || true
	fi
	rm -rf "$TMP_DIR"
}
trap cleanup EXIT HUP INT TERM

cat >"$TMP_DIR/config.json" <<CONFIG
{
  "inbounds": [],
  "outbounds": [{"type":"direct","tag":"direct"}],
  "experimental": {
    "v2ray_api": {
      "listen": "127.0.0.1:$API_PORT",
      "stats": {"enabled": true, "inbounds": [], "users": []}
    }
  }
}
CONFIG

"$SING_BOX" check -c "$TMP_DIR/config.json"
"$SING_BOX" run -c "$TMP_DIR/config.json" >"$TMP_DIR/sing-box.log" 2>&1 &
PID=$!

attempt=0
while [ "$attempt" -lt 30 ]; do
	if grpcurl -plaintext -import-path "$PROTO_DIR" -proto xray-stats-smoke.proto \
		"127.0.0.1:$API_PORT" v2ray.core.app.stats.command.StatsService/GetSysStats \
		>"$TMP_DIR/result.json" 2>"$TMP_DIR/grpcurl.log"; then
		grep -Eq '"(uptime|numGoroutine)"' "$TMP_DIR/result.json" || {
			cat "$TMP_DIR/result.json" >&2
			echo "StatsService returned no system statistics" >&2
			exit 1
		}
		echo "sing-box V2Ray/Xray StatsService on 127.0.0.1:$API_PORT: ok"
		exit 0
	fi
	if ! kill -0 "$PID" 2>/dev/null; then
		cat "$TMP_DIR/sing-box.log" >&2
		cat "$TMP_DIR/grpcurl.log" >&2
		echo "sing-box stopped before the StatsService became available" >&2
		exit 1
	fi
	attempt=$((attempt + 1))
	sleep 1
done
cat "$TMP_DIR/sing-box.log" >&2
cat "$TMP_DIR/grpcurl.log" >&2
echo "StatsService did not answer on 127.0.0.1:$API_PORT" >&2
exit 1
