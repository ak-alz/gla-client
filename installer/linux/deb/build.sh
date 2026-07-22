#!/usr/bin/env bash
# Builds growth-layer-agent_<version>_amd64.deb from the already-built
# release binary. Run on Linux only (uses dpkg-deb/dpkg-shlibdeps).
#
# This stages a binary .deb directly with dpkg-deb rather than going
# through a full debhelper/dpkg-buildpackage source-package build:
# there is no upstream Debian-archive target here (this is an internal
# product agent, not something bound for Debian proper), so the extra
# machinery a native source package needs (a `debian/rules` driving
# Cargo through dh_auto_build, vendored-crate policy, etc.) would add
# real complexity without a corresponding real benefit. `dpkg-shlibdeps`
# is still used below for the one part that DOES matter for
# correctness — accurate, versioned runtime library dependencies —
# rather than hand-typing a Depends: list that could silently drift
# from what the binary actually links against.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
AGENT_CORE_DIR="$(cd "$SCRIPT_DIR/../../.." && pwd)"
VERSION="${1:-$(grep -m1 '^version' "$AGENT_CORE_DIR/crates/agent-bin/Cargo.toml" | sed -E 's/version = "(.*)"/\1/')}"
TARGET_DIR="${CARGO_TARGET_DIR:-$AGENT_CORE_DIR/target}"
BIN_PATH="${AGENT_BIN_PATH:-$TARGET_DIR/release/growth-layer-agent}"
OUT_DIR="${OUT_DIR:-$SCRIPT_DIR/dist}"
ICONS_DIR="$AGENT_CORE_DIR/installer/linux/icons/hicolor"

if [ ! -x "$BIN_PATH" ]; then
    echo "error: release binary not found at $BIN_PATH (build with: cargo build --release -p agent-bin)" >&2
    exit 1
fi

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

mkdir -p "$STAGE/usr/bin" "$STAGE/usr/share/doc/growth-layer-agent" "$STAGE/DEBIAN"
cp "$BIN_PATH" "$STAGE/usr/bin/growth-layer-agent"
chmod 755 "$STAGE/usr/bin/growth-layer-agent"
strip "$STAGE/usr/bin/growth-layer-agent"

for size_dir in "$ICONS_DIR"/*/; do
    size="$(basename "$size_dir")"
    mkdir -p "$STAGE/usr/share/icons/hicolor/$size/apps"
    cp "$size_dir/apps/growth-layer-agent.png" "$STAGE/usr/share/icons/hicolor/$size/apps/growth-layer-agent.png"
done
mkdir -p "$STAGE/usr/share/applications"
sed "s|%INSTALL_BIN%|/usr/bin/growth-layer-agent|" \
    "$AGENT_CORE_DIR/installer/linux/tarball/growth-layer-agent.desktop" \
    > "$STAGE/usr/share/applications/growth-layer-agent.desktop"

# GNOME Shell extension source, staged read-only under /usr/share — the
# per-user copy under ~/.local/share/gnome-shell/extensions/ is made by
# postinst (a GNOME Shell extension must live under the user's own home
# to be loaded at all; there is no system-wide extension directory
# GNOME Shell honors the same way). See gnome_extension.rs's module doc
# comment for what this closes (GNOME active-window detection) and its
# real caveat (untested against a real GNOME session).
mkdir -p "$STAGE/usr/share/growth-layer-agent/gnome-extension"
cp "$AGENT_CORE_DIR/installer/linux/gnome-extension/metadata.json" \
   "$AGENT_CORE_DIR/installer/linux/gnome-extension/extension.js" \
   "$STAGE/usr/share/growth-layer-agent/gnome-extension/"

# postinst: the one real, root-only thing worth doing at install time —
# see postinst's own header comment for why this replaces the earlier
# "nothing for a maintainer script to correctly do here" position (true
# before input-count support existed; no longer true — a fresh install's
# real user isn't in the `input` group by default on any mainstream
# distro, and evdev opening then fails PERMANENTLY-SILENTLY, see
# linux-collector::evdev_counter's own doc comment — a real, user-hit
# bug this closes at install time instead of leaving undiscoverable).
cp "$SCRIPT_DIR/postinst" "$STAGE/DEBIAN/postinst"
chmod 755 "$STAGE/DEBIAN/postinst"

cat > "$STAGE/usr/share/doc/growth-layer-agent/copyright" <<'EOF'
Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: growth-layer-agent
Source: internal (Growth Layer product monorepo, agent-core/crates/agent-bin)

Files: *
Copyright: 2026 Growth Layer
License: proprietary
 This package is an internal Growth Layer product component and is not
 licensed for external redistribution.
EOF

mkdir -p "$STAGE/usr/share/doc/growth-layer-agent"
cat > /tmp/growth-layer-agent-changelog <<EOF
growth-layer-agent ($VERSION) stable; urgency=low

  * See CROSS_PLATFORM_LIGHTWEIGHT_CLIENT_AUTOPILOT.md (AG-LNX-003) and
    the repository's git history for the real change log — this file
    exists to satisfy Debian packaging policy, not duplicate it.

 -- Growth Layer Packaging <packaging@growth-layer.local>  $(date -R)
EOF
gzip -9 -n -c /tmp/growth-layer-agent-changelog > "$STAGE/usr/share/doc/growth-layer-agent/changelog.gz"
rm /tmp/growth-layer-agent-changelog

# dpkg-shlibdeps needs a debian/control with a Source: stanza to run at
# all (it reads build-dependency-adjacent metadata from it), even
# though this isn't a real debhelper source package — see the module
# doc comment above.
mkdir -p "$STAGE/debian"
cat > "$STAGE/debian/control" <<EOF
Source: growth-layer-agent
Section: utils
Priority: optional
Maintainer: Growth Layer Packaging <packaging@growth-layer.local>

Package: growth-layer-agent
Architecture: amd64
Depends: \${shlibs:Depends}, \${misc:Depends}
Description: Growth Layer desktop agent
EOF

( cd "$STAGE" && dpkg-shlibdeps -Tdebian/substvars -O "usr/bin/growth-layer-agent" > debian/shlibdeps.out 2> debian/shlibdeps.err ) || true
SHLIBS_DEPENDS="$(grep -o 'shlibs:Depends=.*' "$STAGE/debian/shlibdeps.out" | sed 's/shlibs:Depends=//')"
if [ -z "$SHLIBS_DEPENDS" ]; then
    echo "error: dpkg-shlibdeps produced no Depends — see stderr below" >&2
    cat "$STAGE/debian/shlibdeps.err" >&2
    exit 1
fi
rm -rf "$STAGE/debian"

cat > "$STAGE/DEBIAN/control" <<EOF
Package: growth-layer-agent
Version: $VERSION
Section: utils
Priority: optional
Architecture: amd64
Maintainer: Growth Layer Packaging <packaging@growth-layer.local>
Depends: $SHLIBS_DEPENDS
Recommends: gnome-shell-extension-appindicator
Description: Growth Layer desktop agent
 Lightweight per-user desktop agent that collects activity signals for
 the Growth Layer product. Runs entirely in user space; never requires
 root at runtime. See AG-LNX-003 in
 CROSS_PLATFORM_LIGHTWEIGHT_CLIENT_AUTOPILOT.md.
 .
 Recommends gnome-shell-extension-appindicator: needed for the tray icon
 to appear at all on stock GNOME (Debian's default desktop does not
 bundle AppIndicator support the way Ubuntu's GNOME does) — a soft
 dependency since KDE/XFCE/Cinnamon/etc. already have native tray
 support and don't need it.
EOF

mkdir -p "$OUT_DIR"
DEB_PATH="$OUT_DIR/growth-layer-agent_${VERSION}_amd64.deb"
dpkg-deb --root-owner-group --build "$STAGE" "$DEB_PATH"

echo "built: $DEB_PATH"
if command -v lintian >/dev/null 2>&1; then
    lintian "$DEB_PATH" || true
fi
