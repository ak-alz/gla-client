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

# AppIcon.icns, generated from the real brand-mark AppIcon.iconset
# (Growth-Layer-Brand-Assets-v1.0/app/macos/AppIcon.iconset, committed
# alongside this script) via `iconutil` -- a macOS-only tool, so this
# step, like the rest of this script, has not been exercised on real
# hardware (same honest limitation noted in the module header). Info.plist
# already declares CFBundleIconFile=AppIcon.icns; before this the
# reference was harmless-but-dangling (no design asset existed yet) and
# macOS fell back to a generic app icon -- that gap is what this closes.
if command -v iconutil >/dev/null 2>&1; then
    iconutil -c icns "$SCRIPT_DIR/AppIcon.iconset" -o "$CONTENTS/Resources/AppIcon.icns"
else
    echo "warning: iconutil not found (only ships with macOS) -- AppIcon.icns not generated, app will use a generic icon" >&2
fi

echo "built $APP_BUNDLE (unsigned, unnotarized -- see codesign_and_notarize.sh)"
