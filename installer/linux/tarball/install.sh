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
AUTOSTART_DIR="$HOME/.config/autostart"

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

# XDG autostart — a copy of the same .desktop entry, just in
# ~/.config/autostart/ instead of ~/.local/share/applications/. Complements
# --register-autostart below (systemd --user): this one needs nothing but
# a file copy, no active D-Bus session required, so it's the more
# reliable "starts at next login" guarantee of the two.
mkdir -p "$AUTOSTART_DIR"
cp "$DESKTOP_DIR/growth-layer-agent.desktop" "$AUTOSTART_DIR/growth-layer-agent.desktop"

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

# Without this, `/dev/input/event*` can never be opened and keyboard/
# mouse activity is silently, permanently counted as zero (see
# linux-collector::evdev_counter's own doc comment) — a real bug this
# closes at install time instead of leaving undiscoverable. Run as the
# real user already (unlike a .deb/.rpm postinst, no $SUDO_USER
# detection needed — `$USER` here already IS the real user), `sudo` only
# for the one privileged step. `-n` (non-interactive) first so a
# passwordless-sudo setup doesn't need to explain itself; falls back to
# an interactive prompt only if that fails.
if ! groups "$USER" 2>/dev/null | grep -qw input; then
    # `usermod -aG input` fails if the group doesn't exist yet — found
    # by actually testing on a bare container with no udev installed,
    # not a hypothetical (every mainstream desktop distro creates it by
    # default, but this costs nothing as a defensive fallback).
    if ! getent group input >/dev/null 2>&1; then
        sudo -n groupadd -r input 2>/dev/null || sudo groupadd -r input 2>/dev/null || true
    fi
    if sudo -n usermod -aG input "$USER" 2>/dev/null || sudo usermod -aG input "$USER"; then
        echo "added '$USER' to the 'input' group (log out and back in, or reboot, for this to take effect)"
    else
        echo "warning: could not add '$USER' to the 'input' group — run manually: sudo usermod -aG input \$USER, then log out and back in"
    fi
fi

# Start it now too — a real gap found after shipping: everything above
# installed and registered autostart for NEXT login, but nothing ever
# launched the agent for the CURRENT session, so a user still had to
# know to run it by hand once. `setsid` detaches it from this script's
# own process group so it keeps running after install.sh exits.
setsid "$BIN_DIR/growth-layer-agent" >/dev/null 2>&1 &
echo "started growth-layer-agent now — look for the tray icon"
