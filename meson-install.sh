#!/bin/sh
# Meson install script wrapper for gity binary

set -e

BUILD_PROFILE="$1"
BINDIR="$2"

BINARY_PATH="$MESON_BUILD_ROOT/target/$BUILD_PROFILE/gity"
INSTALL_DIR="$MESON_INSTALL_DESTDIR_PREFIX/$BINDIR"

if [ ! -f "$BINARY_PATH" ]; then
    echo "Error: Binary not found at $BINARY_PATH" >&2
    exit 1
fi

install -Dm755 "$BINARY_PATH" "$INSTALL_DIR/gity"
