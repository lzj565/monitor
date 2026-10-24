#!/bin/sh
set -eu
ROOT=/opt/monitor
case "$1" in
  enable) touch "$ROOT/mieru.enabled" ;;
  disable) rm -f "$ROOT/mieru.enabled" ;;
  is-enabled) test -f "$ROOT/mieru.enabled" ;;
  is-active) test -f "$ROOT/mieru.pid" && kill -0 "$(cat "$ROOT/mieru.pid")" 2>/dev/null ;;
  restart|start)
    if test -f "$ROOT/mieru.pid"; then kill "$(cat "$ROOT/mieru.pid")" 2>/dev/null || true; sleep .3; fi
    su -s /bin/sh mita -c 'MITA_CONFIG_JSON_FILE=/opt/monitor/config/mieru.json MITA_UDS_PATH=/run/monitor-mieru/mita.sock /opt/monitor/mita run >> /tmp/mita-test.log 2>&1 & echo $!' > "$ROOT/mieru.pid"
    sleep .4
    kill -0 "$(cat "$ROOT/mieru.pid")"
    ;;
  stop) if test -f "$ROOT/mieru.pid"; then kill "$(cat "$ROOT/mieru.pid")" 2>/dev/null || true; rm -f "$ROOT/mieru.pid"; fi ;;
  *) exit 1 ;;
esac
