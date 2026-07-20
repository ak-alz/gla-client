#!/usr/bin/env bash
# AG-MAC-002 — assembles GrowthLayerAgent.app from an already-built
# universal (or single-arch) growth-layer-agent binary. Written and
# never run on a real Mac (no macOS hardware in this environment, same
# honest limitation as AG-MAC-001's collector skeleton) -- this is the
# real, standard .app bundle layout (Apple's own documented structure),
# not a guess, but it has not been exercised end to end.
#
# Usage: build_app_bundle.sh <path-to-growth-layer-agent-binary> <output-dir>
set -euo pipefail

BINARY_PATH="${1:?usage: build_app_bundle.sh <binary_path> <output_dir>}"
OUTPUT_DIR="${2:?usage: build_app_bundle.sh <binary_path> <output_dir>}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

APP_BUNDLE="$OUTPUT_DIR/GrowthLayerAgent.app"
CONTENTS="$APP_BUNDLE/Contents"

rm -rf "$APP_BUNDLE"
mkdir -p "$CONTENTS/MacOS" "$CONTENTS/Resources" "$CONTENTS/Library/LaunchAgents"

cp "$BINARY_PATH" "$CONTENTS/MacOS/growth-layer-agent"
chmod +x "$CONTENTS/MacOS/growth-layer-agent"
cp "$SCRIPT_DIR/Info.plist" "$CONTENTS/Info.plist"
cp "$SCRIPT_DIR/com.growthlayer.agent.plist" "$CONTENTS/Library/LaunchAgents/com.growthlayer.agent.plist"

# AppIcon.icns is deliberately not generated here -- no design asset
# exists yet in this repo; Info.plist's CFBundleIconFile reference is
# harmless without it (macOS falls back to a generic app icon).

echo "built $APP_BUNDLE (unsigned, unnotarized -- see codesign_and_notarize.sh)"
