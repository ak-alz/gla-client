#!/usr/bin/env bash
# Builds growth-layer-agent-<version>-1.<dist>.x86_64.rpm from the
# already-built release binary. Run on Linux only (uses rpmbuild).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
AGENT_CORE_DIR="$(cd "$SCRIPT_DIR/../../.." && pwd)"
VERSION="${1:-$(grep -m1 '^version' "$AGENT_CORE_DIR/crates/agent-bin/Cargo.toml" | sed -E 's/version = "(.*)"/\1/')}"
TARGET_DIR="${CARGO_TARGET_DIR:-$AGENT_CORE_DIR/target}"
BIN_PATH="${AGENT_BIN_PATH:-$TARGET_DIR/release/growth-layer-agent}"
OUT_DIR="${OUT_DIR:-$SCRIPT_DIR/dist}"

if [ ! -x "$BIN_PATH" ]; then
    echo "error: release binary not found at $BIN_PATH (build with: cargo build --release -p agent-bin)" >&2
    exit 1
fi

RPMBUILD_ROOT="$(mktemp -d)"
trap 'rm -rf "$RPMBUILD_ROOT"' EXIT
mkdir -p "$RPMBUILD_ROOT"/{BUILD,RPMS,SOURCES,SPECS,SRPMS,BUILDROOT}

rpmbuild \
    --define "_topdir $RPMBUILD_ROOT" \
    --define "_agent_version $VERSION" \
    --define "_agent_bin_path $BIN_PATH" \
    --define "_agent_core_dir $AGENT_CORE_DIR" \
    -bb "$SCRIPT_DIR/growth-layer-agent.spec"

mkdir -p "$OUT_DIR"
find "$RPMBUILD_ROOT/RPMS" -name '*.rpm' -exec cp {} "$OUT_DIR/" \;
find "$OUT_DIR" -name "growth-layer-agent-${VERSION}-1*.rpm" -printf 'built: %p\n'
