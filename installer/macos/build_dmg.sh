#!/usr/bin/env bash
# AG-MAC-002 — packages a signed+notarized GrowthLayerAgent.app into a
# distributable .dmg using `hdiutil` (Apple's own built-in tool, not a
# third-party packager). Written and never run on real hardware.
#
# Usage: build_dmg.sh <path-to-GrowthLayerAgent.app> <output-dmg-path>
set -euo pipefail

APP_BUNDLE="${1:?usage: build_dmg.sh <path-to-GrowthLayerAgent.app> <output_dmg_path>}"
OUTPUT_DMG="${2:?usage: build_dmg.sh <path-to-GrowthLayerAgent.app> <output_dmg_path>}"

STAGING_DIR="$(mktemp -d)"
trap 'rm -rf "$STAGING_DIR"' EXIT

cp -R "$APP_BUNDLE" "$STAGING_DIR/"
# A drag-to-install /Applications symlink -- the standard macOS
# distribution convention (Finder shows "drag this to that").
ln -s /Applications "$STAGING_DIR/Applications"

rm -f "$OUTPUT_DMG"
hdiutil create -volname "Growth Layer Agent" \
    -srcfolder "$STAGING_DIR" \
    -ov -format UDZO \
    "$OUTPUT_DMG"

echo "built $OUTPUT_DMG"
echo "note: notarize the .app BEFORE building the dmg (this script does not itself notarize the dmg image -- Gatekeeper checks the stapled ticket inside the .app)"
