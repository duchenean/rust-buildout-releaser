#!/usr/bin/env bash
set -euo pipefail

REPO="${BLDR_REPO:-duchenean/rust-buildout-releaser}"
INSTALL_DIR="${BLDR_INSTALL_DIR:-/usr/local/bin}"
VERSION="${BLDR_VERSION:-latest}"

if ! command -v tar >/dev/null 2>&1; then
  echo "tar is required to unpack bldr" >&2
  exit 1
fi

if command -v curl >/dev/null 2>&1; then
  DOWNLOADER="curl -sSfL"
elif command -v wget >/dev/null 2>&1; then
  DOWNLOADER="wget -qO-"
else
  echo "Either curl or wget is required to download bldr" >&2
  exit 1
fi

ARCH=$(uname -m)
case "$ARCH" in
  x86_64|amd64)
    TARGET="x86_64-unknown-linux-gnu"
    ;;
  aarch64|arm64)
    TARGET="aarch64-unknown-linux-gnu"
    ;;
  *)
    echo "Unsupported architecture: $ARCH" >&2
    exit 1
    ;;
esac

get_latest_tag() {
  local api_url="https://api.github.com/repos/${REPO}/releases/latest"
  $DOWNLOADER "$api_url" \
    | sed -n 's/^[[:space:]]*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' \
    | head -n1
}

TAG="$VERSION"
if [ "$VERSION" = "latest" ]; then
  TAG=$(get_latest_tag)
fi

if [ -z "$TAG" ]; then
  echo "Could not determine release tag" >&2
  exit 1
fi

ASSET="bldr-${TAG}-${TARGET}.tar.gz"
DOWNLOAD_URL="https://github.com/${REPO}/releases/download/${TAG}/${ASSET}"

TMPDIR=$(mktemp -d)
cleanup() { rm -rf "$TMPDIR"; }
trap cleanup EXIT

ARCHIVE_PATH="$TMPDIR/$ASSET"

if [[ "$DOWNLOADER" == curl* ]]; then
  $DOWNLOADER "$DOWNLOAD_URL" -o "$ARCHIVE_PATH"
else
  $DOWNLOADER "$DOWNLOAD_URL" > "$ARCHIVE_PATH"
fi

tar -xzf "$ARCHIVE_PATH" -C "$TMPDIR"

install -m 755 "$TMPDIR/bldr" "$INSTALL_DIR/bldr"

echo "bldr installed to $INSTALL_DIR/bldr"
