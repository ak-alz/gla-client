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

if [ -x "$BIN_DIR/growth-layer-agent" ]; then
    "$BIN_DIR/growth-layer-agent" --unregister-autostart || true
fi

command -v gnome-extensions >/dev/null 2>&1 && gnome-extensions disable growth-layer-agent@growthlayer.app >/dev/null 2>&1 || true
rm -rf "$EXT_DIR"

rm -f "$BIN_DIR/growth-layer-agent"
rm -f "$DESKTOP_DIR/growth-layer-agent.desktop"
rm -f "$AUTOSTART_DIR/growth-layer-agent.desktop"
for size_dir in "$ICON_BASE"/*/apps/growth-layer-agent.png; do
    rm -f "$size_dir"
done

command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q "$DESKTOP_DIR" || true

echo "uninstalled growth-layer-agent (local data/log/queue left untouched)"
