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
export TMP_DIR SERVER TOKEN SINGBOX_OPTION DOWNLOAD_FAIL CONFIG_DIR REGISTRATION_FILE
CONFIG_DIR=$PROFILE_TEST_DIR/config
REGISTRATION_FILE=$CONFIG_DIR/registration.env
curl() { [ "${DOWNLOAD_FAIL:-0}" = 0 ] || return 1; while [ "$#" -gt 0 ]; do if [ "$1" = -o ]; then cp "$PROFILE_TEST_DIR/response" "$2"; return; fi; shift; done; }
printf 'singbox=1\n' > "$PROFILE_TEST_DIR/response"
load_component_profile
[ "$SINGBOX_OPTION" = --sing-box ] || fail 'sing-box choice lost'
printf 'singbox=0\nrealm=1\nmieru=1\n' > "$PROFILE_TEST_DIR/response"
load_component_profile
[ "$SINGBOX_OPTION" = --no-sing-box ] || fail 'legacy profile not accepted'
# A failed profile request must not lose the freshly registered identity.
cache_registration
[ "$(pending_registration_token)" = "$TOKEN" ] || fail 'pending registration token was lost'
case "$(ls -l "$REGISTRATION_FILE")" in -rw-------*) ;; *) fail 'registration cache is not private' ;; esac
[ -z "$(SERVER=https://other-hub.example; pending_registration_token)" ] || fail 'token reused for another Hub'
for fixture in 'realm=0\n' 'singbox=1\nsingbox=0\n' 'singbox=1\nextra=1\n'; do
 printf '%b' "$fixture" > "$PROFILE_TEST_DIR/response"
 if (load_component_profile) >/dev/null 2>&1; then fail 'invalid profile accepted'; fi
done
if (DOWNLOAD_FAIL=1; load_component_profile) >/dev/null 2>&1; then fail 'download failure accepted'; fi
printf '%s\n' 'component profiles: current and legacy replies, malformed replies and download failure passed'
