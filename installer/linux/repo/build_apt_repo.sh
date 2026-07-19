#!/usr/bin/env bash
# Builds a minimal local APT repository from every .deb in
# ../deb/dist and GPG-signs its Release file (into both InRelease and
# a detached Release.gpg) — this is the actual, standard way apt
# verifies package authenticity: it trusts the repository's signed
# Release file (whose recorded checksums cover Packages/Packages.gz),
# not a signature on each individual .deb. That's why this script
# signs the REPO, whereas ../rpm/sign.sh signs each .rpm file
# directly with rpm's own native --addsign — two different, both
# textbook-correct mechanisms for the two package formats, not an
# inconsistency between them.
set -euxo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEB_DIST="${DEB_DIST:-$SCRIPT_DIR/../deb/dist}"
REPO_OUT="${REPO_OUT:-$SCRIPT_DIR/dist/apt}"
GPG_NAME="${GPG_NAME:-Growth Layer Agent (Linux Packaging, self-generated)}"

shopt -s nullglob
DEBS=("$DEB_DIST"/*.deb)
if [ ${#DEBS[@]} -eq 0 ]; then
    echo "error: no .deb files found in $DEB_DIST (run ../deb/build.sh first)" >&2
    exit 1
fi

rm -rf "$REPO_OUT"
mkdir -p "$REPO_OUT/pool/main" "$REPO_OUT/dists/stable/main/binary-amd64"
cp "${DEBS[@]}" "$REPO_OUT/pool/main/"

cd "$REPO_OUT"
dpkg-scanpackages --arch amd64 pool/main > dists/stable/main/binary-amd64/Packages
gzip -9 -n -k -f dists/stable/main/binary-amd64/Packages

cd "$REPO_OUT/dists/stable"
apt-ftparchive \
    -o APT::FTPArchive::Release::Origin="Growth Layer" \
    -o APT::FTPArchive::Release::Suite="stable" \
    -o APT::FTPArchive::Release::Codename="stable" \
    -o APT::FTPArchive::Release::Architectures="amd64" \
    -o APT::FTPArchive::Release::Components="main" \
    release . > Release

gpg --default-key "$GPG_NAME" --clearsign -o InRelease Release
gpg --default-key "$GPG_NAME" -abs -o Release.gpg Release

echo "built apt repo at: $REPO_OUT"
