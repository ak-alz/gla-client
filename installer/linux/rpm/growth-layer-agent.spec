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

%files
/usr/bin/growth-layer-agent
/usr/share/icons/hicolor/*/apps/growth-layer-agent.png
/usr/share/applications/growth-layer-agent.desktop

%post
# The one real thing worth doing here (see ../deb/postinst's doc comment
# for the full reasoning this mirrors exactly): grant the real installing
# user `input`-group membership, without which keyboard/mouse activity is
# silently, permanently counted as zero — a real, user-hit bug, not a
# hypothetical. Autostart itself stays a no-op here, still registered by
# the agent from a real user session (lifecycle::Autostart), unreachable
# from this root-run scriptlet regardless.
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
else
    echo "growth-layer-agent: could not determine the real user to grant 'input' group access to — run manually: sudo usermod -aG input \$USER, then log out and back in"
fi
exit 0

%preun
# Deliberately a no-op — see %post and ../deb/prerm's doc comment.
exit 0
