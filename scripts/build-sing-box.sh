#!/bin/sh
set -eu

if [ "$#" -ne 3 ]; then
	echo "usage: build-sing-box.sh SOURCE_DIR ARCH OUTPUT" >&2
	exit 2
fi

SOURCE_DIR=$1
ARCH=$2
OUTPUT=$3
VERSION=1.14.1

case "$ARCH" in
amd64 | arm64) ;;
*) echo "unsupported sing-box architecture: $ARCH" >&2; exit 2 ;;
esac

[ -f "$SOURCE_DIR/go.mod" ] || { echo "sing-box source is missing at $SOURCE_DIR" >&2; exit 1; }
[ "$(sed -n 's/^module //p' "$SOURCE_DIR/go.mod" | head -n 1)" = "github.com/sagernet/sing-box" ] || {
	echo "unexpected sing-box source module" >&2
	exit 1
}

# Naive outbound requires the Linux Chromium/cronet toolchain (or a separate
# libcronet.so at runtime). The released binary is a single standalone file,
# so use sing-box's non-naive tag set when building without CGO.
BUILD_TAGS=$(cat "$SOURCE_DIR/release/DEFAULT_BUILD_TAGS_OTHERS")
BUILD_TAGS="$BUILD_TAGS,with_v2ray_api"
LDFLAGS=$(cat "$SOURCE_DIR/release/LDFLAGS")
OUTPUT_DIR=$(dirname "$OUTPUT")
mkdir -p "$OUTPUT_DIR"
OUTPUT="$(cd "$OUTPUT_DIR" && pwd)/$(basename "$OUTPUT")"

(
	cd "$SOURCE_DIR"
	CGO_ENABLED=0 GOOS=linux GOARCH="$ARCH" go build \
		-trimpath -buildvcs=false \
		-tags "$BUILD_TAGS" \
		-ldflags "$LDFLAGS" \
		-o "$OUTPUT" ./cmd/sing-box
)

if [ "$ARCH" = "$(go env GOARCH)" ]; then
	VERSION_OUTPUT=$("$OUTPUT" version 2>&1) || {
		printf '%s\n' "$VERSION_OUTPUT" >&2
		echo "built sing-box binary did not run" >&2
		exit 1
	}
	printf '%s\n' "$VERSION_OUTPUT"
	printf '%s\n' "$VERSION_OUTPUT" | grep -F "version $VERSION" >/dev/null || {
		echo "sing-box binary is not version $VERSION" >&2
		exit 1
	}
	printf '%s\n' "$VERSION_OUTPUT" | grep -F "with_v2ray_api" >/dev/null || {
		echo "sing-box binary was built without with_v2ray_api" >&2
		exit 1
	}
else
	BUILD_INFO=$(go version -m "$OUTPUT")
	printf '%s\n' "$BUILD_INFO" | grep -F "with_v2ray_api" >/dev/null || {
		echo "cross-built sing-box binary was built without with_v2ray_api" >&2
		exit 1
	}
fi
