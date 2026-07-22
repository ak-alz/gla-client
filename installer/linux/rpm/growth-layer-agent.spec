# Runtime dependencies are left to rpmbuild's built-in dependency
# generator (it scans the packaged binary's dynamic symbol table the
# same way `ldd`/`dpkg-shlibdeps` do and adds versioned Requires
# automatically) rather than hand-listing every shared library here —
# same reasoning as the .deb side's use of `dpkg-shlibdeps` in
# ../deb/build.sh: a hand-maintained list can silently drift from what
# the binary actually links against.
Name: growth-layer-agent
Version: %{_agent_version}
Release: 1%{?dist}
Summary: Growth Layer desktop agent
License: Proprietary
BuildArch: x86_64
%global _binary_payload w2.xzdio
# Soft dependency — GNOME does not ship AppIndicator/tray support out
# of the box on Fedora-family distros either (same gap as Debian's
# stock GNOME, see ../deb's Recommends for the full reasoning); KDE/
# XFCE/Cinnamon already have native tray support and don't need this.
Recommends: gnome-shell-extension-appindicator

%description
Lightweight per-user desktop agent that collects activity signals for
the Growth Layer product. Runs entirely in user space; never requires
root at runtime. See AG-LNX-003 in
CROSS_PLATFORM_LIGHTWEIGHT_CLIENT_AUTOPILOT.md.

%install
mkdir -p %{buildroot}/usr/bin
install -m 755 %{_agent_bin_path} %{buildroot}/usr/bin/growth-layer-agent
for size_dir in %{_agent_core_dir}/installer/linux/icons/hicolor/*/; do
    size=$(basename "$size_dir")
    mkdir -p "%{buildroot}/usr/share/icons/hicolor/$size/apps"
    install -m 644 "$size_dir/apps/growth-layer-agent.png" \
        "%{buildroot}/usr/share/icons/hicolor/$size/apps/growth-layer-agent.png"
done
mkdir -p %{buildroot}/usr/share/applications
sed 's|%INSTALL_BIN%|/usr/bin/growth-layer-agent|' \
    %{_agent_core_dir}/installer/linux/tarball/growth-layer-agent.desktop \
    > %{buildroot}/usr/share/applications/growth-layer-agent.desktop

# GNOME Shell extension source — see ../deb/build.sh's comment: staged
# read-only here, copied into the user's own home by %post (a GNOME
# Shell extension only loads from there).
mkdir -p %{buildroot}/usr/share/growth-layer-agent/gnome-extension
install -m 644 %{_agent_core_dir}/installer/linux/gnome-extension/metadata.json \
    %{_agent_core_dir}/installer/linux/gnome-extension/extension.js \
    %{buildroot}/usr/share/growth-layer-agent/gnome-extension/

%files
/usr/bin/growth-layer-agent
/usr/share/icons/hicolor/*/apps/growth-layer-agent.png
/usr/share/applications/growth-layer-agent.desktop
/usr/share/growth-layer-agent/gnome-extension/metadata.json
/usr/share/growth-layer-agent/gnome-extension/extension.js

%post
# See ../deb/postinst's doc comment for the full reasoning this mirrors
# exactly: grant `input`-group membership (without it keyboard/mouse
# activity is silently, permanently counted as zero), then start the
# agent — both on login going forward (XDG autostart, no session
# needed) and right now (best-effort systemd --user + immediate
# background launch) — nothing installed by this package used to ever
# launch it, a real user-hit gap, not a hypothetical.
REAL_USER="${SUDO_USER:-}"
if [ -z "$REAL_USER" ] || [ "$REAL_USER" = "root" ]; then
    REAL_USER="$(logname 2>/dev/null || true)"
fi
if [ -n "$REAL_USER" ] && [ "$REAL_USER" != "root" ] && id "$REAL_USER" >/dev/null 2>&1; then
    # `usermod -aG input` fails SILENTLY if the group doesn't exist yet —
    # found by actually testing on a bare container, not a hypothetical
    # (every mainstream desktop distro creates it via udev by default,
    # but this costs nothing as a defensive fallback).
    if ! getent group input >/dev/null 2>&1; then
        groupadd -r input 2>/dev/null || true
    fi
    if command -v usermod >/dev/null 2>&1 && usermod -aG input "$REAL_USER" 2>/dev/null; then
        echo "growth-layer-agent: added '$REAL_USER' to the 'input' group (log out and back in, or reboot, for this to take effect)"
    else
        echo "growth-layer-agent: could not add '$REAL_USER' to the 'input' group — run manually: sudo usermod -aG input $REAL_USER, then log out and back in"
    fi

    REAL_HOME="$(getent passwd "$REAL_USER" | cut -d: -f6)"
    if [ -n "$REAL_HOME" ] && [ -f /usr/share/applications/growth-layer-agent.desktop ]; then
        mkdir -p "$REAL_HOME/.config/autostart"
        cp /usr/share/applications/growth-layer-agent.desktop "$REAL_HOME/.config/autostart/growth-layer-agent.desktop"
        chown "$REAL_USER": "$REAL_HOME/.config/autostart/growth-layer-agent.desktop" 2>/dev/null || true
    fi

    # GNOME Shell extension — see ../deb/postinst's doc comment for the
    # full reasoning (org.gnome.Shell.Eval gated behind unsafe mode since
    # GNOME 41).
    EXT_SRC=/usr/share/growth-layer-agent/gnome-extension
    EXT_UUID=growth-layer-agent@growthlayer.app
    if [ -n "$REAL_HOME" ] && [ -d "$EXT_SRC" ]; then
        EXT_DEST="$REAL_HOME/.local/share/gnome-shell/extensions/$EXT_UUID"
        mkdir -p "$EXT_DEST"
        cp "$EXT_SRC/metadata.json" "$EXT_SRC/extension.js" "$EXT_DEST/"
        chown "$REAL_USER": "$EXT_DEST" "$EXT_DEST/metadata.json" "$EXT_DEST/extension.js" 2>/dev/null || true
    fi

    if command -v runuser >/dev/null 2>&1; then
        # `runuser` alone hands the child none of the graphical session's
        # environment — this %post runs as root, which never had
        # DISPLAY/DBUS_SESSION_BUS_ADDRESS to begin with. Tray icons
        # register over the session D-Bus (StatusNotifierItem), so
        # without forwarding this, the agent starts but has nowhere to
        # put its icon — found by actually installing on a real desktop
        # session, not a hypothetical. `/run/user/<uid>/bus` is the
        # standard systemd-logind session bus socket, present only when
        # that user has an active graphical login right now.
        REAL_UID="$(id -u "$REAL_USER" 2>/dev/null || true)"
        RUNTIME_DIR="/run/user/$REAL_UID"
        if [ -n "$REAL_UID" ] && [ -S "$RUNTIME_DIR/bus" ]; then
            runuser -u "$REAL_USER" -- env XDG_RUNTIME_DIR="$RUNTIME_DIR" DBUS_SESSION_BUS_ADDRESS="unix:path=$RUNTIME_DIR/bus" growth-layer-agent --register-autostart >/dev/null 2>&1 || true
            # `systemctl --user restart`, not a bare `setsid ... &`: see
            # ../deb/postinst's doc comment for the full reasoning — a raw
            # detached process is invisible to systemd, so every upgrade
            # used to accumulate one more orphaned agent process racing
            # the others to register the same tray icon, a real, user-hit
            # cause of "sometimes no icon at all".
            runuser -u "$REAL_USER" -- env XDG_RUNTIME_DIR="$RUNTIME_DIR" DBUS_SESSION_BUS_ADDRESS="unix:path=$RUNTIME_DIR/bus" systemctl --user restart GrowthLayerAgent.service >/dev/null 2>&1 || true
            echo "growth-layer-agent: started now — look for the tray icon (will also start automatically at next login)"

            if [ -d "$REAL_HOME/.local/share/gnome-shell/extensions/$EXT_UUID" ] && runuser -u "$REAL_USER" -- env XDG_RUNTIME_DIR="$RUNTIME_DIR" DBUS_SESSION_BUS_ADDRESS="unix:path=$RUNTIME_DIR/bus" gnome-extensions enable "$EXT_UUID" >/dev/null 2>&1; then
                echo "growth-layer-agent: GNOME Shell extension enabled (needed for active-app tracking on GNOME) — takes effect after the next log out/in"
            fi
        else
            echo "growth-layer-agent: will start automatically at next login (no active desktop session detected right now to start it immediately)"
        fi
    fi
else
    echo "growth-layer-agent: could not determine the real user to grant 'input' group access to — run manually: sudo usermod -aG input \$USER, then log out and back in"
fi
exit 0

%preun
# Deliberately a no-op — see %post and ../deb/prerm's doc comment.
exit 0
