#!/bin/sh
# Test-only orchestrator. Never run against a production node.
set -eu
[ -f /.dockerenv ] && [ -f /tmp/monitor-mieru-disposable ] || { echo 'requires an explicitly prepared disposable container' >&2; exit 1; }
TEST_BIN=${1:-/tmp/monitor-agent-mieru-test}
DRIVER=${2:-/tmp/mieru-e2e-driver.py}
pkill -x mita || true
rm -f /opt/monitor/config/mieru-state.json /tmp/mieru-ready /tmp/mieru-traffic-done
printf '{}' > /opt/monitor/config/mieru.json
MIERU_E2E=1 "$TEST_BIN" mieru::tests::real_mita_apply_reload_rollback_and_traffic --ignored --nocapture > /tmp/mieru-test-result.log 2>&1 &
mieru_test_pid=$!
trap 'kill "$mieru_test_pid" 2>/dev/null || true' EXIT
if ! python3 "$DRIVER"; then cat /tmp/mieru-test-result.log; exit 1; fi
if ! wait "$mieru_test_pid"; then cat /tmp/mieru-test-result.log; exit 1; fi
cat /tmp/mieru-test-result.log
