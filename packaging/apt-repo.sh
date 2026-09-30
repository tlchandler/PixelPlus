#!/usr/bin/env bash
# Build (or update) a signed apt repository from pixelplus .deb files.
#
#   packaging/apt-repo.sh --repo DIR [--suite stable|beta] [--key KEYID] DEB...
#
# Layout (one suite per update channel, component "main"; packages are pooled and a
# suite lists the ones added with it):
#   DIR/pool/<suite>/p/pixelplus/pixelplus_<ver>_<arch>.deb
#   DIR/dists/<suite>/main/binary-{arm64,amd64}/Packages{,.gz}
#   DIR/dists/<suite>/{Release,Release.gpg,InRelease}
#   DIR/pixelplus-archive-keyring.gpg     (public key for clients)
# Stable releases go into both suites (beta users get them too); pre-releases into beta.
#
# Serve DIR over HTTPS (GitHub Pages, S3, nginx). Clients:
#   curl -fsSL https://REPO/pixelplus-archive-keyring.gpg | sudo tee /usr/share/keyrings/pixelplus.gpg >/dev/null
#   echo "deb [signed-by=/usr/share/keyrings/pixelplus.gpg] https://REPO stable main" | sudo tee /etc/apt/sources.list.d/pixelplus.list
# (Settings -> Updates -> Channel switches the suite with the helper verb update-channel.)
# Signed over-the-air updates (pixelplus-<channel>.json, packaging/release-index.py) don't
# need apt at all; this repository is for hand installs and `apt upgrade`.
# PixelPlus images use this for Settings -> Updates (pixelplus-helper@update).
# Needs: dpkg-dev (dpkg-scanpackages), apt-utils (apt-ftparchive), gnupg.
set -euo pipefail

REPO=""
KEY=""
SUITE="stable"
DEBS=()
while [[ $# -gt 0 ]]; do
    case "$1" in
    --repo) REPO="${2:?}"; shift 2 ;;
    --key) KEY="${2:?}"; shift 2 ;;
    --suite) SUITE="${2:?}"; shift 2 ;;
    -h | --help) sed -n '2,25p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) DEBS+=("$1"); shift ;;
    esac
done
[[ -n "${REPO}" ]] || { echo "--repo is required" >&2; exit 2; }
[[ "${SUITE}" =~ ^(stable|beta)$ ]] || { echo "--suite must be stable or beta" >&2; exit 2; }
for t in dpkg-scanpackages apt-ftparchive; do
    command -v "$t" >/dev/null 2>&1 || { echo "missing $t (apt install dpkg-dev apt-utils)" >&2; exit 1; }
done

mkdir -p "${REPO}/pool/${SUITE}/p/pixelplus"
for d in "${DEBS[@]}"; do
    cp -v "$d" "${REPO}/pool/${SUITE}/p/pixelplus/"
done

cd "${REPO}"
# Older layouts kept everything in pool/main (stable).
[[ -d pool/main && "${SUITE}" == stable ]] && pools=(pool/main "pool/${SUITE}") || pools=("pool/${SUITE}")
for arch in arm64 amd64; do
    dir="dists/${SUITE}/main/binary-${arch}"
    mkdir -p "${dir}"
    : >"${dir}/Packages"
    for p in "${pools[@]}"; do
        dpkg-scanpackages --multiversion --arch "${arch}" "${p}/" >>"${dir}/Packages"
    done
    gzip -9kf "${dir}/Packages"
done

apt-ftparchive \
    -o APT::FTPArchive::Release::Origin=PixelPlus \
    -o APT::FTPArchive::Release::Label=PixelPlus \
    -o APT::FTPArchive::Release::Suite="${SUITE}" \
    -o APT::FTPArchive::Release::Codename="${SUITE}" \
    -o APT::FTPArchive::Release::Architectures="arm64 amd64" \
    -o APT::FTPArchive::Release::Components=main \
    release "dists/${SUITE}" >"dists/${SUITE}/Release"

if [[ -n "${KEY}" ]]; then
    gpg --batch --yes --default-key "${KEY}" -abs -o "dists/${SUITE}/Release.gpg" "dists/${SUITE}/Release"
    gpg --batch --yes --default-key "${KEY}" --clearsign -o "dists/${SUITE}/InRelease" "dists/${SUITE}/Release"
    gpg --batch --yes --export "${KEY}" >pixelplus-archive-keyring.gpg
else
    echo "warning: unsigned repository (use --key KEYID); apt will refuse it without [trusted=yes]" >&2
fi
echo "repository ${SUITE} updated in ${REPO}"
