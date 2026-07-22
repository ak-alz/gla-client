#!/usr/bin/env bash
# Builds a distro-agnostic growth-layer-agent-<version>-linux-x86_64.tar.gz
# from the already-built release binary — no package manager involved at
# all, so it installs the same way on any Linux (Arch included, where
# .deb/.rpm cannot be used at all: pacman is Arch's native package
# manager, and `dpkg -i`/`rpm -i` on Arch report every dependency as
# "not installed" regardless of what's actually on the system, since
# their own package databases were never populated there — see
# ../deb/build.sh and ../rpm/growth-layer-agent.spec for the package-
# manager-native alternatives this complements, not replaces).
#
# Run on Linux only.
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

PKG_NAME="growth-layer-agent-${VERSION}-linux-x86_64"
ROOT="$STAGE/$PKG_NAME"
mkdir -p "$ROOT/bin" "$ROOT/icons"

cp "$BIN_PATH" "$ROOT/bin/growth-layer-agent"
chmod 755 "$ROOT/bin/growth-layer-agent"
strip "$ROOT/bin/growth-layer-agent"

cp -r "$ICONS_DIR" "$ROOT/icons/hicolor"
cp "$SCRIPT_DIR/growth-layer-agent.desktop" "$ROOT/growth-layer-agent.desktop"
cp "$SCRIPT_DIR/install.sh" "$ROOT/install.sh"
cp "$SCRIPT_DIR/uninstall.sh" "$ROOT/uninstall.sh"
chmod 755 "$ROOT/install.sh" "$ROOT/uninstall.sh"

mkdir -p "$OUT_DIR"
TAR_PATH="$OUT_DIR/${PKG_NAME}.tar.gz"
tar -czf "$TAR_PATH" -C "$STAGE" "$PKG_NAME"

echo "built: $TAR_PATH"
