#!/usr/bin/env bash
# Installs growth-layer-agent entirely into the current user's own home
# directory — no root, no package manager, works identically on every
# distro (this is the point of the tarball: dpkg/rpm can't be installed
# this way on Arch at all, see build.sh's doc comment). Run from inside
# the extracted tarball directory: ./install.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_DIR="$HOME/.local/bin"
ICON_BASE="$HOME/.local/share/icons/hicolor"
DESKTOP_DIR="$HOME/.local/share/applications"

mkdir -p "$BIN_DIR" "$DESKTOP_DIR"
install -m 755 "$SCRIPT_DIR/bin/growth-layer-agent" "$BIN_DIR/growth-layer-agent"

for size_dir in "$SCRIPT_DIR/icons/hicolor"/*/; do
    size="$(basename "$size_dir")"
    mkdir -p "$ICON_BASE/$size/apps"
    cp "$size_dir/apps/growth-layer-agent.png" "$ICON_BASE/$size/apps/growth-layer-agent.png"
done

# %INSTALL_BIN% -> the real absolute path just installed above — most
# desktop environments don't resolve a bare command through the login
# shell's PATH when launching a .desktop entry, so this must be absolute.
sed "s|%INSTALL_BIN%|$BIN_DIR/growth-layer-agent|" "$SCRIPT_DIR/growth-layer-agent.desktop" \
    > "$DESKTOP_DIR/growth-layer-agent.desktop"

# Best-effort — not every desktop environment has these tools, and a
# missing one is never fatal to the install (the icon still appears
# correctly on next login/re-scan even without a manual cache refresh).
command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q -t -f "$ICON_BASE" || true
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q "$DESKTOP_DIR" || true

echo "installed: $BIN_DIR/growth-layer-agent"
echo "Make sure $BIN_DIR is on your PATH, or launch it from your desktop's application menu."

# Unlike a .deb/.rpm/PKGBUILD install, nothing here can declare or pull
# in runtime shared libraries (that is the actual tradeoff for working
# on every distro without a package manager, not an oversight) — so a
# genuinely bare system can still be missing one. Checked explicitly,
# rather than letting it surface later as a raw "error while loading
# shared libraries" out of --register-autostart below (found by actually
# running this install on a minimal Arch container: libxdo.so.3 was
# missing there, and the dynamic linker's own message gives no hint
# which package provides it).
MISSING_LIBS="$(ldd "$BIN_DIR/growth-layer-agent" 2>/dev/null | awk '/not found/ {print $1}')"
if [ -n "$MISSING_LIBS" ]; then
    echo
    echo "warning: growth-layer-agent is installed but can't run yet — missing shared libraries:"
    echo "$MISSING_LIBS" | sed 's/^/  /'
    echo "On Arch: pacman -S gtk3 glib2 gdk-pixbuf2 xdotool"
    echo "On Debian/Ubuntu: apt install libgtk-3-0 libglib2.0-0 libgdk-pixbuf-2.0-0 libxdo3"
    echo "Install those, then re-run this script (or just re-run: $BIN_DIR/growth-layer-agent --register-autostart)."
    exit 0
fi

# Same step the Windows installer's `[Run]` section performs (see
# agent-bin/src/main.rs's `--register-autostart` doc comment) — a
# per-user systemd `--user` unit, registered here rather than left for
# the user to discover (there is no tray menu item for this; the flag
# is the only entry point, exactly as on Windows). Only reached once the
# binary is confirmed runnable (see the ldd check above).
"$BIN_DIR/growth-layer-agent" --register-autostart || true
