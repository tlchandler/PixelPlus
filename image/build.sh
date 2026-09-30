#!/usr/bin/env bash
# Build a PixelPlus Raspberry Pi OS Lite (64-bit) SD-card image with pi-gen.
#
#   image/build.sh --deb dist/pixelplus_<ver>_arm64.deb [options]
#
#   --release trixie|bookworm   Raspberry Pi OS base (default: trixie)
#   --deb FILE                  pixelplus arm64 .deb to install (repeatable; required)
#   --docker                    build inside Docker (pi-gen's build-docker.sh); this is what
#                               image/build-in-docker.sh does. Needs a privileged container,
#                               loop devices and binfmt_misc (qemu-user-static) on x86 hosts.
#   --no-tts                    leave out the Kokoro TTS sidecar (smaller image)
#   --no-tts-models             include the TTS venv but download voices at runtime
#   --version VER               PixelPlus version used in the image name (default: from the .deb)
#   --work DIR                  where pi-gen is checked out (default: image/work)
#   --out DIR                   where the finished image + os-list JSON go (default: image/deploy)
#   --continue                  reuse an existing pi-gen work tree (faster rebuilds; native only)
#   --pigen-ref REF             override the pinned pi-gen tag
#
# Output: <out>/pixelplus-<ver>-<release>-arm64.img.xz (+ .sha256, .info) and
#         <out>/os-list-<release>.json (Raspberry Pi Imager repository fragment).
#
# Native builds need a Debian/Raspberry Pi OS host (arm64 preferred) and root; see
# docs/BUILDING.md. Nothing here can run in an unprivileged container.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "${HERE}/.." && pwd)"

# pi-gen tags are "<date>-raspios-<release>-<arch>"; pinned for reproducibility.
PIGEN_REPO="https://github.com/RPi-Distro/pi-gen.git"
declare -A PIGEN_TAG=(
    [trixie]="2026-09-15-raspios-trixie-arm64"
    [bookworm]="2026-09-15-raspios-bookworm-arm64"
)

RELEASE=trixie
DEBS=()
DOCKER=0
TTS=1
TTS_MODELS=1
VERSION=""
WORK="${HERE}/work"
OUT="${HERE}/deploy"
CONTINUE=0
PIGEN_REF=""

log() { printf '\033[1;33m==>\033[0m %s\n' "$*"; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }
usage() { sed -n '2,24p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }

while [[ $# -gt 0 ]]; do
    case "$1" in
    --release) RELEASE="${2:?}"; shift 2 ;;
    --deb) DEBS+=("$(realpath "${2:?}")"); shift 2 ;;
    --docker) DOCKER=1; shift ;;
    --no-tts) TTS=0; shift ;;
    --no-tts-models) TTS_MODELS=0; shift ;;
    --version) VERSION="${2:?}"; shift 2 ;;
    --work) WORK="$(realpath -m "${2:?}")"; shift 2 ;;
    --out) OUT="$(realpath -m "${2:?}")"; shift 2 ;;
    --continue) CONTINUE=1; shift ;;
    --pigen-ref) PIGEN_REF="${2:?}"; shift 2 ;;
    -h | --help) usage; exit 0 ;;
    *) die "unknown option $1 (see --help)" ;;
    esac
done

[[ -n "${PIGEN_TAG[${RELEASE}]:-}" ]] || die "unsupported release '${RELEASE}' (trixie or bookworm)"
PIGEN_REF="${PIGEN_REF:-${PIGEN_TAG[${RELEASE}]}}"
[[ ${#DEBS[@]} -gt 0 ]] || die "--deb is required (build it with packaging/build-deb.sh --arch arm64)"
for d in "${DEBS[@]}"; do
    [[ -f "$d" ]] || die "no such file: $d"
    if command -v dpkg-deb >/dev/null 2>&1; then
        arch="$(dpkg-deb -f "$d" Architecture)"
        [[ "${arch}" == arm64 || "${arch}" == all ]] || die "$d is ${arch}, need arm64"
        if [[ -z "${VERSION}" && "$(dpkg-deb -f "$d" Package)" == pixelplus ]]; then
            VERSION="$(dpkg-deb -f "$d" Version)"
        fi
    fi
done
VERSION="${VERSION:-dev}"
SAFE_VERSION="${VERSION//[~+]/-}"
IMG_NAME="pixelplus-${SAFE_VERSION}-${RELEASE}-arm64"

# ---------------------------------------------------------------------------
# 1. pi-gen checkout at the pinned tag
# ---------------------------------------------------------------------------
PIGEN="${WORK}/pi-gen-${RELEASE}"
if [[ -d "${PIGEN}/.git" ]]; then
    log "updating pi-gen (${PIGEN_REF})"
    git -C "${PIGEN}" fetch --depth 1 origin "refs/tags/${PIGEN_REF}:refs/tags/${PIGEN_REF}" 2>/dev/null ||
        git -C "${PIGEN}" fetch --depth 1 origin "${PIGEN_REF}"
    git -C "${PIGEN}" checkout -q --force "${PIGEN_REF}"
    git -C "${PIGEN}" clean -qfdx -e work -e deploy
else
    log "cloning pi-gen ${PIGEN_REF}"
    mkdir -p "${WORK}"
    git clone -q --depth 1 --branch "${PIGEN_REF}" "${PIGEN_REPO}" "${PIGEN}"
fi

# ---------------------------------------------------------------------------
# 2. our stage + payload
# ---------------------------------------------------------------------------
STAGE="${PIGEN}/stage-pixelplus"
rm -rf "${STAGE}"
cp -a "${HERE}/stage-pixelplus" "${STAGE}"
rm -f "${STAGE}"/01-pixelplus/files/*.deb "${STAGE}"/01-pixelplus/files/*.minisig
cp "${DEBS[@]}" "${STAGE}/01-pixelplus/files/"
# Signatures next to the packages (signed releases) are kept for signed updates' rollback.
for d in "${DEBS[@]}"; do
    [[ -f "$d.minisig" ]] && cp "$d.minisig" "${STAGE}/01-pixelplus/files/"
done
if [[ "${TTS}" == 1 ]]; then
    [[ -f "${REPO}/tts/pyproject.toml" ]] || die "tts/pyproject.toml not found (use --no-tts)"
    rm -rf "${STAGE}/03-tts/files/tts"
    mkdir -p "${STAGE}/03-tts/files/tts"
    (cd "${REPO}/tts" && tar --exclude=__pycache__ --exclude=.venv --exclude=tests -cf - .) |
        tar -C "${STAGE}/03-tts/files/tts" -xf -
fi
# Only stage0-2 (= Raspberry Pi OS Lite) + ours; no stage2 "-lite" export.
touch "${PIGEN}/stage2/SKIP_IMAGES"

FIRST_USER_PASS="$(head -c 32 /dev/urandom | base64 | tr -dc 'A-Za-z0-9' | head -c 24)"
CONFIG="${PIGEN}/config"
cat >"${CONFIG}" <<EOF
# Generated by image/build.sh - do not edit (edit image/build.sh instead)
IMG_NAME='${IMG_NAME}'
PI_GEN_RELEASE='PixelPlus ${VERSION}'
RELEASE='${RELEASE}'
DEPLOY_COMPRESSION=xz
COMPRESSION_LEVEL=6
TARGET_HOSTNAME=pixelplus
LOCALE_DEFAULT=en_US.UTF-8
KEYBOARD_KEYMAP=us
KEYBOARD_LAYOUT='English (US)'
TIMEZONE_DEFAULT=Etc/UTC
# 'pi' exists but is locked (random password, no rename wizard); owners unlock it via
# ssh_password=/ssh_key= in pixelplus.txt or create their own user with Raspberry Pi Imager.
FIRST_USER_NAME=pi
FIRST_USER_PASS='${FIRST_USER_PASS}'
DISABLE_FIRST_BOOT_USER_RENAME=1
ENABLE_SSH=0
# Keep cloud-init (Trixie): Raspberry Pi Imager's "cloudinit-rpi" customisation needs it.
ENABLE_CLOUD_INIT=1
STAGE_LIST='stage0 stage1 stage2 stage-pixelplus'
export PIXELPLUS_VERSION='${VERSION}'
export PIXELPLUS_TTS_MODELS='${TTS_MODELS}'
export PIXELPLUS_TTS_VARIANT='int8'
EOF
if [[ "${CONTINUE}" == 1 ]]; then echo "CONTINUE=1" >>"${CONFIG}"; fi

# ---------------------------------------------------------------------------
# 3. build
# ---------------------------------------------------------------------------
cd "${PIGEN}"
if [[ "${DOCKER}" == 1 ]]; then
    log "building ${IMG_NAME} in Docker (privileged; takes 30-90 min)"
    CONTAINER_NAME="pigen_pixelplus_${RELEASE}" PRESERVE_CONTAINER=0 ./build-docker.sh -c "${CONFIG}"
else
    [[ "$(id -u)" == 0 ]] || die "native pi-gen builds must run as root (or use --docker)"
    log "building ${IMG_NAME} natively (takes 30-90 min)"
    ./build.sh -c "${CONFIG}"
fi

# ---------------------------------------------------------------------------
# 4. collect + Raspberry Pi Imager metadata
# ---------------------------------------------------------------------------
mkdir -p "${OUT}"
IMG_XZ="$(find "${PIGEN}/deploy" -maxdepth 1 -name "*${IMG_NAME}*.img.xz" -printf '%T@ %p\n' | sort -nr | head -n1 | cut -d' ' -f2-)"
[[ -n "${IMG_XZ}" ]] || die "no .img.xz produced in ${PIGEN}/deploy"
FINAL="${OUT}/${IMG_NAME}.img.xz"
cp "${IMG_XZ}" "${FINAL}"
INFO="$(find "${PIGEN}/deploy" -maxdepth 1 -name "*${IMG_NAME}*.info" | head -n1)"
[[ -n "${INFO}" ]] && cp "${INFO}" "${OUT}/${IMG_NAME}.info"
(cd "${OUT}" && sha256sum "$(basename "${FINAL}")" >"$(basename "${FINAL}").sha256")

log "writing Raspberry Pi Imager metadata"
python3 "${HERE}/rpi-imager/make-os-list.py" \
    --image "${FINAL}" --release "${RELEASE}" --version "${VERSION}" \
    --url "${PIXELPLUS_IMAGE_BASE_URL:-https://github.com/tlchandler/PixelPlus/releases/download/v${VERSION}}/$(basename "${FINAL}")" \
    --out "${OUT}/os-list-${RELEASE}.json"

log "done: ${FINAL}"
