#!/usr/bin/env bash
# Build the "pixelplus" Debian package (arm64 for Raspberry Pi, amd64 for PCs).
#
#   packaging/build-deb.sh [--arch arm64|amd64] [--version X.Y.Z] [--out DIR]
#                          [--bin-dir DIR] [--web-dir DIR] [--skip-web]
#
#   --arch      target architecture (default: the build host's dpkg architecture)
#   --version   package version (default: workspace version from Cargo.toml, plus
#               "~git<date>.<sha>" when not building a tagged commit)
#   --out       output directory (default: dist/)
#   --bin-dir   use prebuilt pixelplusd + pixelplus binaries from DIR (skip cargo)
#   --web-dir   use a prebuilt web UI from DIR (default: build web/ -> web/build)
#   --skip-web  package without the web UI (development only)
#
# Cross-compiling for arm64 on an x86_64 host, in order of preference:
#   1. `cross` (https://github.com/cross-rs/cross; needs Docker/Podman):
#        cargo install cross --locked
#      glibc of the cross image (2.31) is older than Raspberry Pi OS Bookworm (2.36)
#      and Trixie (2.41), so the binaries run on both.
#   2. Debian/Ubuntu cross toolchain (the script sets the linker and CC_aarch64_unknown_linux_gnu
#      that zstd-sys needs):
#        sudo apt install gcc-aarch64-linux-gnu && rustup target add aarch64-unknown-linux-gnu
#      Build on the OLDEST distribution you target (Bookworm) - glibc is forward compatible only.
#   3. Native build on a Pi 4/5 (slow but simple).
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PKG="${REPO}/packaging"

ARCH=""
VERSION=""
OUT="${REPO}/dist"
BIN_DIR=""
WEB_DIR=""
SKIP_WEB=0

usage() { sed -n '2,24p' "$0" | sed 's/^# \{0,1\}//'; }

while [[ $# -gt 0 ]]; do
    case "$1" in
    --arch) ARCH="${2:?}"; shift 2 ;;
    --version) VERSION="${2:?}"; shift 2 ;;
    --out) OUT="${2:?}"; shift 2 ;;
    --bin-dir) BIN_DIR="${2:?}"; shift 2 ;;
    --web-dir) WEB_DIR="${2:?}"; shift 2 ;;
    --skip-web) SKIP_WEB=1; shift ;;
    -h | --help) usage; exit 0 ;;
    *) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
    esac
done

log() { printf '\033[1;33m==>\033[0m %s\n' "$*"; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

host_arch() {
    if command -v dpkg >/dev/null 2>&1; then dpkg --print-architecture; else
        case "$(uname -m)" in aarch64 | arm64) echo arm64 ;; x86_64) echo amd64 ;; *) uname -m ;; esac
    fi
}

ARCH="${ARCH:-$(host_arch)}"
case "${ARCH}" in
arm64) TRIPLE=aarch64-unknown-linux-gnu ;;
amd64) TRIPLE=x86_64-unknown-linux-gnu ;;
*) die "unsupported architecture ${ARCH} (use arm64 or amd64)" ;;
esac

if [[ -z "${VERSION}" ]]; then
    VERSION="$(sed -n '/^\[workspace.package\]/,/^\[/{s/^version *= *"\(.*\)"/\1/p}' "${REPO}/Cargo.toml" | head -n1)"
    [[ -n "${VERSION}" ]] || die "cannot read the workspace version from Cargo.toml"
    if git -C "${REPO}" rev-parse --git-dir >/dev/null 2>&1; then
        if ! git -C "${REPO}" describe --exact-match --tags >/dev/null 2>&1; then
            VERSION="${VERSION}~git$(git -C "${REPO}" log -1 --format=%cd --date=format:%Y%m%d).$(git -C "${REPO}" rev-parse --short HEAD)"
        fi
    fi
fi
[[ "${VERSION}" =~ ^[0-9][A-Za-z0-9.+~-]*$ ]] || die "invalid Debian version '${VERSION}'"

# ---------------------------------------------------------------------------
# 1. binaries
# ---------------------------------------------------------------------------
find_bin() { # dir name... -> first existing
    local dir="$1"; shift
    local n
    for n in "$@"; do
        if [[ -x "${dir}/${n}" ]]; then echo "${dir}/${n}"; return 0; fi
    done
    return 1
}

if [[ -z "${BIN_DIR}" ]]; then
    CARGO_ARGS=(build --release --locked -p pixelplus-daemon -p pixelplus-cli)
    if [[ "${ARCH}" == "$(host_arch)" ]]; then
        log "cargo build (native ${ARCH})"
        (cd "${REPO}" && cargo "${CARGO_ARGS[@]}")
        BIN_DIR="${REPO}/target/release"
    elif command -v cross >/dev/null 2>&1; then
        log "cross build --target ${TRIPLE}"
        (cd "${REPO}" && cross "${CARGO_ARGS[@]}" --target "${TRIPLE}")
        BIN_DIR="${REPO}/target/${TRIPLE}/release"
    elif command -v aarch64-linux-gnu-gcc >/dev/null 2>&1 && [[ "${ARCH}" == arm64 ]]; then
        log "cargo build --target ${TRIPLE} (gcc-aarch64-linux-gnu)"
        rustup target add "${TRIPLE}" >/dev/null 2>&1 || true
        # CC_* is needed by C dependencies (zstd-sys) built through the cc crate.
        (cd "${REPO}" && CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
            CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc \
            AR_aarch64_unknown_linux_gnu=aarch64-linux-gnu-ar \
            cargo "${CARGO_ARGS[@]}" --target "${TRIPLE}")
        BIN_DIR="${REPO}/target/${TRIPLE}/release"
    else
        die "no cross toolchain for ${ARCH}: install 'cross' or gcc-aarch64-linux-gnu (see header)"
    fi
fi
DAEMON_BIN="$(find_bin "${BIN_DIR}" pixelplusd)" || die "pixelplusd not found in ${BIN_DIR}"
# The CLI crate is pixelplus-cli; its binary should be named "pixelplus" ([[bin]] name).
CLI_BIN="$(find_bin "${BIN_DIR}" pixelplus pixelplus-cli)" || die "pixelplus CLI not found in ${BIN_DIR}"

check_elf_arch() {
    local f="$1" want
    command -v file >/dev/null 2>&1 || return 0
    case "${ARCH}" in arm64) want="aarch64|ARM aarch64" ;; amd64) want="x86-64" ;; esac
    if file -b "$f" | grep -q "ELF" && ! file -b "$f" | grep -Eq "${want}"; then
        die "$f is not an ${ARCH} binary: $(file -b "$f")"
    fi
}
check_elf_arch "${DAEMON_BIN}"
check_elf_arch "${CLI_BIN}"

# ---------------------------------------------------------------------------
# 2. web UI (SvelteKit static build)
# ---------------------------------------------------------------------------
if [[ "${SKIP_WEB}" == 0 && -z "${WEB_DIR}" ]]; then
    [[ -f "${REPO}/web/package.json" ]] || die "web/package.json missing (use --skip-web)"
    log "building web UI (pnpm)"
    (cd "${REPO}/web" && pnpm install --frozen-lockfile && pnpm build)
    WEB_DIR="${REPO}/web/build"
fi
if [[ "${SKIP_WEB}" == 0 ]]; then
    [[ -f "${WEB_DIR}/index.html" ]] || die "no index.html in ${WEB_DIR}"
fi

# ---------------------------------------------------------------------------
# 3. stage the package tree
# ---------------------------------------------------------------------------
STAGE="$(mktemp -d)"
trap 'rm -rf "${STAGE}"' EXIT
R="${STAGE}/root"
log "staging ${ARCH} ${VERSION}"

inst() { # mode src dst
    install -D -m "$1" "$2" "${R}$3"
}

inst 0755 "${DAEMON_BIN}" /usr/bin/pixelplusd
inst 0755 "${CLI_BIN}" /usr/bin/pixelplus
# (binaries are already stripped: [profile.release] strip = true)

if [[ "${SKIP_WEB}" == 0 ]]; then
    install -d "${R}/usr/share/pixelplus/web"
    cp -a "${WEB_DIR}/." "${R}/usr/share/pixelplus/web/"
fi

# pixelplus.txt applier + setup hotspot (image/)
for f in firstboot.py pptxt.py nmconn.py; do
    inst 0644 "${REPO}/image/firstboot/${f}" "/usr/lib/pixelplus/firstboot/${f}"
done
chmod 0755 "${R}/usr/lib/pixelplus/firstboot/firstboot.py"
for f in netwatch.py portal.py; do
    inst 0644 "${REPO}/image/netwatch/${f}" "/usr/lib/pixelplus/netwatch/${f}"
done
chmod 0755 "${R}/usr/lib/pixelplus/netwatch/netwatch.py"
for f in "${REPO}"/image/netwatch/portal/*; do
    inst 0644 "$f" "/usr/lib/pixelplus/netwatch/portal/$(basename "$f")"
done
inst 0644 "${REPO}/image/boot/pixelplus.txt" /usr/share/pixelplus/pixelplus.txt.template
install -d "${R}/usr/sbin"
ln -s ../lib/pixelplus/firstboot/firstboot.py "${R}/usr/sbin/pixelplus-firstboot"

# games sidecar (python, system interpreter; see games/README.md)
if [[ -d "${REPO}/games/pixelplus_games" ]]; then
    install -d "${R}/usr/lib/pixelplus/games"
    cp -a "${REPO}/games/pixelplus_games" "${R}/usr/lib/pixelplus/games/"
    for f in README.md requirements.txt; do
        [[ -f "${REPO}/games/$f" ]] && inst 0644 "${REPO}/games/$f" "/usr/lib/pixelplus/games/$f"
    done
    find "${R}/usr/lib/pixelplus/games" \( -name __pycache__ -o -name tests \) -type d -prune -exec rm -rf {} +
fi

# DPI overlays (Raspberry Pi only; postinst copies them to /boot/firmware/overlays)
if [[ "${ARCH}" == arm64 ]]; then
    command -v dtc >/dev/null 2>&1 || die "dtc not found (sudo apt install device-tree-compiler)"
    for dts in "${REPO}"/crates/pixelplus-output/overlays/*.dts; do
        name="$(basename "${dts}" .dts)"
        install -d "${R}/usr/lib/pixelplus/overlays"
        dtc -q -@ -I dts -O dtb -o "${R}/usr/lib/pixelplus/overlays/${name}.dtbo" "${dts}"
    done
fi

# helpers
inst 0755 "${PKG}/bin/pixelplus-helper" /usr/lib/pixelplus/pixelplus-helper
inst 0755 "${PKG}/bin/tts-capable" /usr/lib/pixelplus/tts-capable

# system integration
for u in "${PKG}"/systemd/*; do
    inst 0644 "$u" "/usr/lib/systemd/system/$(basename "$u")"
done
inst 0644 "${PKG}/avahi/pixelplus.service" /etc/avahi/services/pixelplus.service
inst 0644 "${PKG}/polkit/50-pixelplus.rules" /usr/share/polkit-1/rules.d/50-pixelplus.rules
inst 0644 "${PKG}/sysctl/60-pixelplus.conf" /usr/lib/sysctl.d/60-pixelplus.conf
inst 0644 "${PKG}/udev/60-pixelplus.rules" /usr/lib/udev/rules.d/60-pixelplus.rules
inst 0644 "${PKG}/tmpfiles/pixelplus.conf" /usr/lib/tmpfiles.d/pixelplus.conf
inst 0644 "${PKG}/logrotate/pixelplus" /etc/logrotate.d/pixelplus
inst 0644 "${PKG}/default/pixelplus" /etc/default/pixelplus
inst 0644 "${PKG}/appliance/nm-dnsmasq-portal.conf" /usr/share/pixelplus/appliance/nm-dnsmasq-portal.conf
inst 0644 "${PKG}/appliance/nm-pixelplus.conf" /usr/share/pixelplus/appliance/nm-pixelplus.conf
inst 0644 "${REPO}/LICENSE" /usr/share/doc/pixelplus/copyright

# DEBIAN/
install -d "${R}/DEBIAN"
for s in postinst prerm postrm; do
    install -m 0755 "${PKG}/debian/${s}" "${R}/DEBIAN/${s}"
done
{
    cat "${PKG}/debian/conffiles"
    echo /etc/avahi/services/pixelplus.service
    echo /etc/logrotate.d/pixelplus
} >"${R}/DEBIAN/conffiles"
SIZE_KB="$(du -sk --exclude=DEBIAN "${R}" | cut -f1)"
sed -e "s/@VERSION@/${VERSION}/" -e "s/@ARCH@/${ARCH}/" -e "s/@SIZE@/${SIZE_KB}/" \
    "${PKG}/debian/control.in" >"${R}/DEBIAN/control"
(cd "${R}" && find . -path ./DEBIAN -prune -o -type f -print0 | sort -z |
    xargs -0 md5sum | sed 's|  \./|  |') >"${R}/DEBIAN/md5sums"

mkdir -p "${OUT}"
DEB="${OUT}/pixelplus_${VERSION}_${ARCH}.deb"
dpkg-deb --root-owner-group -Zxz --build "${R}" "${DEB}" >/dev/null
log "built ${DEB} ($(du -h "${DEB}" | cut -f1))"
