#!/bin/sh
# Meson run script wrapper for gity

set -e

BUILD_PROFILE="$1"
BINARY_PATH="$MESON_BUILD_ROOT/target/$BUILD_PROFILE/gity"
# Schema is compiled to build root as gschemas.compiled
SCHEMA_DIR="$MESON_BUILD_ROOT"

if [ ! -f "$BINARY_PATH" ]; then
    echo "Error: Binary not found at $BINARY_PATH" >&2
    echo "Please build the project first with: meson compile" >&2
    exit 1
fi

exec env GSETTINGS_SCHEMA_DIR="$SCHEMA_DIR" "$BINARY_PATH" "$@"

