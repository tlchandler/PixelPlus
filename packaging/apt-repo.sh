#!/usr/bin/env bash
# Build (or update) a signed apt repository from pixelplus .deb files.
#
#   packaging/apt-repo.sh --repo DIR [--key KEYID] DEB...
#
# Layout (flat "dists" repo, one suite "stable", component "main"):
#   DIR/pool/main/p/pixelplus/pixelplus_<ver>_<arch>.deb
#   DIR/dists/stable/main/binary-{arm64,amd64}/Packages{,.gz}
#   DIR/dists/stable/{Release,Release.gpg,InRelease}
#   DIR/pixelplus-archive-keyring.gpg     (public key for clients)
#
# Serve DIR over HTTPS (GitHub Pages, S3, nginx). Clients:
#   curl -fsSL https://REPO/pixelplus-archive-keyring.gpg | sudo tee /usr/share/keyrings/pixelplus.gpg >/dev/null
#   echo "deb [signed-by=/usr/share/keyrings/pixelplus.gpg] https://REPO stable main" | sudo tee /etc/apt/sources.list.d/pixelplus.list
# PixelPlus images use this for Settings -> Updates (pixelplus-helper@update).
# Needs: dpkg-dev (dpkg-scanpackages), apt-utils (apt-ftparchive), gnupg.
set -euo pipefail

REPO=""
KEY=""
DEBS=()
while [[ $# -gt 0 ]]; do
    case "$1" in
    --repo) REPO="${2:?}"; shift 2 ;;
    --key) KEY="${2:?}"; shift 2 ;;
    -h | --help) sed -n '2,17p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) DEBS+=("$1"); shift ;;
    esac
done
[[ -n "${REPO}" ]] || { echo "--repo is required" >&2; exit 2; }
for t in dpkg-scanpackages apt-ftparchive; do
    command -v "$t" >/dev/null 2>&1 || { echo "missing $t (apt install dpkg-dev apt-utils)" >&2; exit 1; }
done

mkdir -p "${REPO}/pool/main/p/pixelplus"
for d in "${DEBS[@]}"; do
    cp -v "$d" "${REPO}/pool/main/p/pixelplus/"
done

cd "${REPO}"
for arch in arm64 amd64; do
    dir="dists/stable/main/binary-${arch}"
    mkdir -p "${dir}"
    dpkg-scanpackages --multiversion --arch "${arch}" pool/ >"${dir}/Packages"
    gzip -9kf "${dir}/Packages"
done

apt-ftparchive \
    -o APT::FTPArchive::Release::Origin=PixelPlus \
    -o APT::FTPArchive::Release::Label=PixelPlus \
    -o APT::FTPArchive::Release::Suite=stable \
    -o APT::FTPArchive::Release::Codename=stable \
    -o APT::FTPArchive::Release::Architectures="arm64 amd64" \
    -o APT::FTPArchive::Release::Components=main \
    release dists/stable >dists/stable/Release

if [[ -n "${KEY}" ]]; then
    gpg --batch --yes --default-key "${KEY}" -abs -o dists/stable/Release.gpg dists/stable/Release
    gpg --batch --yes --default-key "${KEY}" --clearsign -o dists/stable/InRelease dists/stable/Release
    gpg --batch --yes --export "${KEY}" >pixelplus-archive-keyring.gpg
else
    echo "warning: unsigned repository (use --key KEYID); apt will refuse it without [trusted=yes]" >&2
fi
echo "repository updated in ${REPO}"
