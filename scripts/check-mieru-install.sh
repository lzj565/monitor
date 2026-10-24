#!/bin/sh
# Exercise installer lifecycle without touching host services, users, or /opt.
set -eu
INSTALLER=${1:-install.sh}
TEST_DIR=$(mktemp -d)
trap 'rm -rf "$TEST_DIR"' EXIT
fail() { echo "$*" >&2; exit 1; }
eval "$(sed -n '/^# BEGIN mieru helpers$/,/^# END mieru helpers$/p' "$INSTALLER" | sed '1d;$d;s|/etc/systemd/system/monitor-mieru.service|$ROOT/unit|g;s|/etc/init.d/monitor-mieru|$ROOT/rc|g;s|/etc/mita|$ROOT/system-mita|g;s|/var/lib/mita|$ROOT/data|g;s|/run/monitor-mieru|$ROOT/run|g;s/rc-service/rc_service/g;s/rc-update/rc_update/g')"
mkdir -p "$TEST_DIR/fixture"
printf '#!/bin/sh\necho "mita 3.39.0"\n' > "$TEST_DIR/fixture/mita"
tar -czf "$TEST_DIR/archive" -C "$TEST_DIR/fixture" mita
curl() { [ "${MOCK_DOWNLOAD_FAIL:-0}" = 0 ] || return 1; while [ "$#" -gt 0 ]; do if [ "$1" = -o ]; then cp "$TEST_DIR/archive" "$2"; return; fi; shift; done; }
sha256sum() { printf '%s fixture\n' "${MOCK_SHA:-$MIERU_SHA}"; }
getent() { return 0; }
id() { return 0; }
chown() { return 0; }
systemctl() { echo "$*" >> "$ROOT/calls"; case "$1" in is-active) [ "$MOCK_RUNNING" = 1 ] ;; restart) [ "${MOCK_RESTART_FAIL:-0}" = 0 ] ;; esac; }
rc_service() { echo "$*" >> "$ROOT/calls"; case "$2" in status) [ "$MOCK_RUNNING" = 1 ] ;; restart) [ "${MOCK_RESTART_FAIL:-0}" = 0 ] ;; esac; }
rc_update() { echo "$*" >> "$ROOT/calls"; }
SERVER=https://hub.example.com
for INIT in systemd openrc; do
 ROOT="$TEST_DIR/$INIT"; CONFIG_DIR="$ROOT/config"; TMP_DIR="$ROOT/tmp"; mkdir -p "$CONFIG_DIR" "$TMP_DIR"
 MOCK_RUNNING=0; MIERU_OPTION=''; MIERU_STATE=unchanged
 manage_mieru
 [ ! -e "$ROOT/mita" ] || fail 'legacy command installed mita'
 for ARCH in x86_64 aarch64; do
  MIERU_OPTION=--mieru; manage_mieru
  [ -x "$ROOT/mita" ] && [ -f "$ROOT/.mieru-managed" ] || fail 'missing managed mita'
  [ "$(cat "$CONFIG_DIR/mieru.json")" = '{"portBindings":[],"users":[],"loggingLevel":"WARN"}' ] || fail 'initial config differs'
 done
 if grep -E '^(enable|restart|monitor-mieru restart|add) ' "$ROOT/calls" >/dev/null; then fail 'first install started or enabled service'; fi
 printf '%s\n' '{"users":[{"name":"test","password":"fixture"}]}' > "$CONFIG_DIR/mieru.json"
 cp "$CONFIG_DIR/mieru.json" "$ROOT/saved-config"
 MOCK_RUNNING=1; manage_mieru; cmp "$CONFIG_DIR/mieru.json" "$ROOT/saved-config"
 cp "$ROOT/mita" "$ROOT/saved-binary"
 if (MOCK_SHA=bad; manage_mieru) >/dev/null 2>&1; then fail 'bad SHA accepted'; fi
 cmp "$ROOT/mita" "$ROOT/saved-binary"
 if (MOCK_DOWNLOAD_FAIL=1; manage_mieru) >/dev/null 2>&1; then fail 'failed download accepted'; fi
 cmp "$ROOT/mita" "$ROOT/saved-binary"
 MIERU_OPTION=--no-mieru; manage_mieru
 [ ! -e "$ROOT/mita" ] && [ ! -e "$ROOT/.mieru-managed" ] && [ -f "$ROOT/.mieru-preserved" ] || fail 'uninstall state incorrect'
 cmp "$CONFIG_DIR/mieru.json" "$ROOT/saved-config"
 MIERU_OPTION=--mieru; MOCK_RUNNING=0; manage_mieru
 cmp "$CONFIG_DIR/mieru.json" "$ROOT/saved-config"
 MIERU_OPTION=--no-mieru; manage_mieru; rm "$ROOT/.mieru-preserved"
 if (MIERU_OPTION=--mieru; manage_mieru) >/dev/null 2>&1; then fail 'unmanaged preserved files overwritten'; fi
 done
printf '%s\n' 'mieru installer lifecycle, architectures, checksum, download and ownership checks passed'
