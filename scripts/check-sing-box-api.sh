#!/bin/sh
set -eu

if [ "$#" -ne 1 ]; then
	echo "usage: check-sing-box-api.sh SING_BOX_BINARY" >&2
	exit 2
fi

SING_BOX=$1
case "$SING_BOX" in /*) ;; *) SING_BOX="$PWD/$SING_BOX" ;; esac
PROTO_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
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

cat >"$TMP_DIR/config.json" <<'CONFIG'
{
  "inbounds": [],
  "outbounds": [{"type":"direct","tag":"direct"}],
  "experimental": {
    "v2ray_api": {
      "listen": "127.0.0.1:9001",
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
	if grpcurl -plaintext -proto "$PROTO_DIR/xray-stats-smoke.proto" \
		127.0.0.1:9001 v2ray.core.app.stats.command.StatsService/GetSysStats \
		>"$TMP_DIR/result.json" 2>"$TMP_DIR/grpcurl.log"; then
		grep -Eq '"(uptime|numGoroutine)"' "$TMP_DIR/result.json" || {
			cat "$TMP_DIR/result.json" >&2
			echo "StatsService returned no system statistics" >&2
			exit 1
		}
		echo "sing-box V2Ray/Xray StatsService on 127.0.0.1:9001: ok"
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
echo "StatsService did not answer on 127.0.0.1:9001" >&2
exit 1
