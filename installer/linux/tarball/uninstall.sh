#!/usr/bin/env bash
# Reverses install.sh — removes exactly what it placed, nothing more
# (does not touch $HOME/.local/share/growth-layer-agent's data dir/log/
# queue, mirroring the other installers: uninstalling the binary is not
# the same decision as deleting local activity data). Run from anywhere:
# ~/.local/bin/growth-layer-agent must still exist for the autostart
# unregister step below to run against the right binary.
set -euo pipefail

BIN_DIR="$HOME/.local/bin"
ICON_BASE="$HOME/.local/share/icons/hicolor"
DESKTOP_DIR="$HOME/.local/share/applications"
AUTOSTART_DIR="$HOME/.config/autostart"
EXT_DIR="$HOME/.local/share/gnome-shell/extensions/growth-layer-agent@growthlayer.app"
KWIN_SCRIPT_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/kwin/scripts/growthlayeragent"

if [ -x "$BIN_DIR/growth-layer-agent" ]; then
    "$BIN_DIR/growth-layer-agent" --unregister-autostart || true
fi

command -v gnome-extensions >/dev/null 2>&1 && gnome-extensions disable growth-layer-agent@growthlayer.app >/dev/null 2>&1 || true
rm -rf "$EXT_DIR"

# KWin script — unload it from the running KWin first (it would keep
# pushing to a bus name nobody owns otherwise), then stop it from coming
# back at the next KWin start, then remove it. All best-effort: none of
# these tools exist outside KDE.
command -v qdbus6 >/dev/null 2>&1 && qdbus6 org.kde.KWin /Scripting unloadScript growthlayeragent >/dev/null 2>&1 || true
command -v kwriteconfig6 >/dev/null 2>&1 && kwriteconfig6 --file kwinrc --group Plugins --key growthlayeragentEnabled --delete >/dev/null 2>&1 || true
rm -rf "$KWIN_SCRIPT_DIR"

rm -f "$BIN_DIR/growth-layer-agent"
rm -f "$DESKTOP_DIR/growth-layer-agent.desktop"
rm -f "$AUTOSTART_DIR/growth-layer-agent.desktop"
for size_dir in "$ICON_BASE"/*/apps/growth-layer-agent.png; do
    rm -f "$size_dir"
done

command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q "$DESKTOP_DIR" || true

echo "uninstalled growth-layer-agent (local data/log/queue left untouched)"
