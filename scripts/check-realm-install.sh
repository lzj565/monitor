#!/bin/sh
# shellcheck disable=SC2034
# Variables below are consumed by installer helpers loaded with eval.
# Exercise lifecycle without root, networking or a real init system.
set -eu
TEST_DIR=$(mktemp -d)
trap 'rm -rf "$TEST_DIR"' EXIT
INSTALLER=$(CDPATH='' cd -- "$(dirname "$0")/.." && pwd)/install.sh
fail() { echo "$*" >&2; exit 1; }
info() { printf '%s\n' "$*"; }
success() { printf '%s\n' "$*"; }
eval "$(sed -n '/# BEGIN Realm helpers/,/# END Realm helpers/p' "$INSTALLER" | sed '1d;$d;s/rc-service/rc_service/g;s/rc-update/rc_update/g')"
mkdir -p "$TEST_DIR/fixture"
printf '#!/bin/sh\necho "realm 2.9.6"\n' > "$TEST_DIR/fixture/realm"
tar -czf "$TEST_DIR/fixture.tar.gz" -C "$TEST_DIR/fixture" realm
curl() {
    [ "${MOCK_DOWNLOAD_FAIL:-0}" = 0 ] || return 1
    while [ "$#" -gt 0 ]; do
        if [ "$1" = -o ]; then cp "$TEST_DIR/fixture.tar.gz" "$2"; break; fi
        shift
    done
    printf 200
}
sha256sum() { printf '%s  fixture\n' "${MOCK_SHA:-$REALM_SHA}"; }
systemctl() {
    echo "$*" >> "$TEST_DIR/calls"
    case "$1" in
    is-active) [ "$MOCK_RUNNING" = 1 ] ;;
    disable) [ "$MOCK_STOP_FAIL" = 0 ] ;;
    esac
}
rc_service() {
    echo "$*" >> "$TEST_DIR/calls"
    case "$2" in
    status) [ "$MOCK_RUNNING" = 1 ] ;;
    stop) [ "$MOCK_STOP_FAIL" = 0 ] ;;
    esac
}
rc_update() { echo "$*" >> "$TEST_DIR/calls"; }
SERVER=https://hub.example.com
for INIT in systemd openrc; do
    ROOT="$TEST_DIR/$INIT"
    CONFIG_DIR="$ROOT/config"
    TMP_DIR="$ROOT/tmp"
    REALM="$ROOT/realm"
    REALM_CONFIG="$CONFIG_DIR/realm.json"
    REALM_UNIT="$ROOT/unit"
    REALM_RC="$ROOT/rc"
    mkdir -p "$CONFIG_DIR" "$TMP_DIR"
    MOCK_RUNNING=0 MOCK_STOP_FAIL=0
    REALM_OPTION='' REALM_STATE=unchanged
    manage_realm
    [ ! -e "$REALM" ] || fail 'legacy command installed Realm'
    for ARCH in x86_64 aarch64; do
        REALM_OPTION=--realm
        manage_realm
        [ -x "$REALM" ] && [ -f "$REALM_CONFIG" ] || fail 'initial install missing files'
        [ "$REALM_STATE" = 'installed; configure before starting' ] || fail 'initial install started service'
        if [ "$INIT" = systemd ]; then
            grep -F "ExecStart=$REALM -c $REALM_CONFIG" "$REALM_UNIT" >/dev/null
        else
            sh -n "$REALM_RC"
        fi
    done
    printf '%s\n' '{"endpoints":[{"listen":"127.0.0.1:1","remote":"127.0.0.1:2"}]}' > "$REALM_CONFIG"
    cp "$REALM_CONFIG" "$ROOT/saved"
    if grep -E '^(enable|add) ' "$TEST_DIR/calls" >/dev/null; then fail 'initial install enabled service'; fi
    MOCK_RUNNING=1
    manage_realm
    [ "$REALM_STATE" = running ] || fail 'running service was not restarted'
    cmp "$REALM_CONFIG" "$ROOT/saved"
    cp "$REALM" "$ROOT/binary-before"
    REALM_OPTION=''
    manage_realm
    cmp "$REALM" "$ROOT/binary-before"
    REALM_OPTION=--realm
    if (MOCK_DOWNLOAD_FAIL=1; manage_realm) 2>/dev/null; then fail 'failed download accepted'; fi
    cmp "$REALM" "$ROOT/binary-before"
    if (MOCK_SHA=bad; manage_realm) 2>/dev/null; then fail 'bad checksum accepted'; fi
    cmp "$REALM" "$ROOT/binary-before"
    REALM_OPTION=--no-realm
    if (MOCK_STOP_FAIL=1; manage_realm) 2>/dev/null; then fail 'failed stop accepted'; fi
    [ -x "$REALM" ] || fail 'failed stop deleted binary'
    manage_realm
    [ ! -e "$REALM" ] && [ ! -e "$REALM_UNIT" ] && [ ! -e "$REALM_RC" ] || fail 'uninstall left managed files'
    cmp "$REALM_CONFIG" "$ROOT/saved"
    manage_realm
    printf 'third party\n' > "$REALM"
    if (manage_realm) 2>/dev/null; then fail 'third party installation removed'; fi
    grep -F 'third party' "$REALM" >/dev/null
    rm "$REALM"
done
if sh "$INSTALLER" --realm --no-realm > "$TEST_DIR/error" 2>&1; then fail 'conflicting flags accepted'; fi
grep -F 'mutually exclusive' "$TEST_DIR/error" >/dev/null
printf 'Realm lifecycle, ownership, checksum and failure checks passed\n'
