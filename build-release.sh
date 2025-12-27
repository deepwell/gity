#!/bin/bash
# Build and install release version

set -e

BUILD_DIR="build-release"
PREFIX="${1:-/usr}"

# Configure Meson build for release
if [ ! -d "$BUILD_DIR" ]; then
    meson setup "$BUILD_DIR" --buildtype=release --prefix="$PREFIX"
fi

# Build
meson compile -C "$BUILD_DIR"

# Install (may require sudo if prefix is system directory)
echo "Installing to $PREFIX (may require sudo)..."
if [ "$PREFIX" = "/usr" ] || [ "$PREFIX" = "/usr/local" ]; then
    sudo meson install -C "$BUILD_DIR"
    # Compile schemas in system location
    echo "Compiling GSettings schemas..."
    sudo glib-compile-schemas "$PREFIX/share/glib-2.0/schemas/"
    # Update desktop database
    echo "Updating desktop database..."
    sudo update-desktop-database "$PREFIX/share/applications/" 2>/dev/null || true
    # Update icon cache
    echo "Updating icon cache..."
    sudo gtk-update-icon-cache -t -f "$PREFIX/share/icons/hicolor" 2>/dev/null || true
else
    meson install -C "$BUILD_DIR"
    # Compile schemas in install location
    echo "Compiling GSettings schemas..."
    glib-compile-schemas "$PREFIX/share/glib-2.0/schemas/"
    # Update desktop database
    echo "Updating desktop database..."
    update-desktop-database "$PREFIX/share/applications/" 2>/dev/null || true
    # Update icon cache
    echo "Updating icon cache..."
    gtk-update-icon-cache -t -f "$PREFIX/share/icons/hicolor" 2>/dev/null || true
fi

echo "Installation complete!"

