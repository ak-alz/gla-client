#!/usr/bin/env bash
# Signs every .rpm in rpm/dist with rpm's native --addsign (embeds the
# signature in the RPM header itself — verified with `rpm --checksig`,
# no separate repository-metadata signature needed, unlike the .deb
# side; see ../repo/build_apt_repo.sh's doc comment for why that one
# signs the repo's Release file instead of each .deb individually).
#
# Requires a GPG secret key already present in the calling user's
# keyring (see ../keys/gen-signing-key.batch for how the demo key
# used during AG-LNX-003 development was generated) and a
# ~/.rpmmacros with %_gpg_name set to that key's identity — this
# script writes one if none exists, defaulting to the demo key's name.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GPG_NAME="${GPG_NAME:-Growth Layer Agent (Linux Packaging, self-generated)}"
DIST_DIR="${OUT_DIR:-$SCRIPT_DIR/dist}"

if [ ! -f "$HOME/.rpmmacros" ] || ! grep -q '_gpg_name' "$HOME/.rpmmacros"; then
    cat >> "$HOME/.rpmmacros" <<EOF
%_gpg_name $GPG_NAME
EOF
fi

shopt -s nullglob
RPMS=("$DIST_DIR"/*.rpm)
if [ ${#RPMS[@]} -eq 0 ]; then
    echo "error: no .rpm files found in $DIST_DIR (run build.sh first)" >&2
    exit 1
fi

rpm --addsign "${RPMS[@]}"
for f in "${RPMS[@]}"; do
    rpm --checksig "$f" | grep -v 'transaction lock' || true
done
