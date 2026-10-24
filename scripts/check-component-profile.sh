#!/bin/sh
# Saved install choices must be read and validated before changing installed services.
set -eu
INSTALLER=${1:-install.sh}
PROFILE_TEST_DIR=$(mktemp -d)
trap 'rm -rf "$PROFILE_TEST_DIR"' EXIT
fail() { echo "$*" >&2; exit 1; }
info() { :; }
eval "$(sed -n '/^# BEGIN component profile helpers$/,/^# END component profile helpers$/p' "$INSTALLER" | sed '1d;$d')"
TMP_DIR=$PROFILE_TEST_DIR SERVER=https://hub.example TOKEN=fixture-token
export TMP_DIR SERVER TOKEN REALM_OPTION MIERU_OPTION SINGBOX_OPTION DOWNLOAD_FAIL CONFIG_DIR REGISTRATION_FILE
CONFIG_DIR=$PROFILE_TEST_DIR/config
REGISTRATION_FILE=$CONFIG_DIR/registration.env
curl() { [ "${DOWNLOAD_FAIL:-0}" = 0 ] || return 1; while [ "$#" -gt 0 ]; do if [ "$1" = -o ]; then cp "$PROFILE_TEST_DIR/response" "$2"; return; fi; shift; done; }
for realm in 0 1; do
 for mieru in 0 1; do
  printf 'singbox=1\nrealm=%s\nmieru=%s\n' "$realm" "$mieru" > "$PROFILE_TEST_DIR/response"
  REALM_OPTION='' MIERU_OPTION='' SINGBOX_OPTION=''
  load_component_profile
  [ "$SINGBOX_OPTION" = --sing-box ] || fail 'sing-box was not mandatory'
  if [ "$realm" = 1 ]; then [ "$REALM_OPTION" = --realm ]; else [ "$REALM_OPTION" = --no-realm ]; fi
  if [ "$mieru" = 1 ]; then [ "$MIERU_OPTION" = --mieru ]; else [ "$MIERU_OPTION" = --no-mieru ]; fi
 done
done
# Explicit flags override only this run; the loader never writes saved intent.
REALM_OPTION=--no-realm MIERU_OPTION=--no-mieru
load_component_profile
[ "$REALM_OPTION:$MIERU_OPTION" = --no-realm:--no-mieru ] || fail 'explicit choices lost'
# A failed profile request must not lose the freshly registered identity.
cache_registration
[ "$(pending_registration_token)" = "$TOKEN" ] || fail 'pending registration token was lost'
case "$(ls -l "$REGISTRATION_FILE")" in -rw-------*) ;; *) fail 'registration cache is not private' ;; esac
[ -z "$(SERVER=https://other-hub.example; pending_registration_token)" ] || fail 'token reused for another Hub'
for fixture in 'singbox=0\nrealm=0\nmieru=0\n' 'singbox=1\nrealm=0\n' 'singbox=1\nrealm=0\nrealm=1\nmieru=0\n' 'singbox=1\nrealm=0\nmieru=0\nextra=1\n'; do
 printf '%b' "$fixture" > "$PROFILE_TEST_DIR/response"
 if (load_component_profile) >/dev/null 2>&1; then fail 'invalid profile accepted'; fi
done
if (DOWNLOAD_FAIL=1; load_component_profile) >/dev/null 2>&1; then fail 'download failure accepted'; fi
if sh "$INSTALLER" --skip-sing-box > "$PROFILE_TEST_DIR/skip.log" 2>&1; then fail 'skip flag accepted'; fi
grep -F 'sing-box is required' "$PROFILE_TEST_DIR/skip.log" >/dev/null
printf '%s\n' 'component profiles: saved combinations, explicit overrides, malformed replies, failure and mandatory sing-box passed'
