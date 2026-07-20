#!/usr/bin/env bash
# AG-MAC-002 — real Developer ID signing + notarization flow, using
# Apple's own documented tools (`codesign`, `xcrun notarytool`,
# `xcrun stapler`) -- NOT a custom or guessed signing mechanism. Written
# and never run (no macOS hardware, no Apple Developer Program
# membership available in this environment -- both are real,
# organizational prerequisites, not something this task can obtain).
#
# Requires, supplied by whoever runs this on real hardware with a real
# Apple Developer account:
#   DEVELOPER_ID_APPLICATION   -- e.g. "Developer ID Application: Your Org (TEAMID)"
#   NOTARYTOOL_KEYCHAIN_PROFILE -- a profile stored via:
#     xcrun notarytool store-credentials <profile-name> \
#       --apple-id <id> --team-id <TEAMID> --password <app-specific-password>
#
# Usage: codesign_and_notarize.sh <path-to-GrowthLayerAgent.app>
set -euo pipefail

APP_BUNDLE="${1:?usage: codesign_and_notarize.sh <path-to-GrowthLayerAgent.app>}"
: "${DEVELOPER_ID_APPLICATION:?set DEVELOPER_ID_APPLICATION to your real Developer ID Application identity}"
: "${NOTARYTOOL_KEYCHAIN_PROFILE:?set NOTARYTOOL_KEYCHAIN_PROFILE to a profile created via 'xcrun notarytool store-credentials'}"

# Hardened runtime is required for notarization (Apple's own
# requirement, not optional) -- the entitlements file only requests
# what the capability matrix (AG-MAC-001) says this agent actually
# needs: no JIT, no unsigned-executable-memory, no library-validation
# disable. Accessibility/input-monitoring are TCC prompts at runtime,
# not entitlements, so nothing extra is declared here.
ENTITLEMENTS="$(dirname "${BASH_SOURCE[0]}")/entitlements.plist"

echo "codesigning $APP_BUNDLE ..."
codesign --force --deep --options runtime \
    --entitlements "$ENTITLEMENTS" \
    --sign "$DEVELOPER_ID_APPLICATION" \
    "$APP_BUNDLE"

codesign --verify --deep --strict --verbose=2 "$APP_BUNDLE"

ZIP_PATH="${APP_BUNDLE%.app}.zip"
ditto -c -k --keepParent "$APP_BUNDLE" "$ZIP_PATH"

echo "submitting for notarization (this blocks until Apple responds) ..."
xcrun notarytool submit "$ZIP_PATH" \
    --keychain-profile "$NOTARYTOOL_KEYCHAIN_PROFILE" \
    --wait

echo "stapling notarization ticket ..."
xcrun stapler staple "$APP_BUNDLE"

echo "done: $APP_BUNDLE is signed and notarized"
