#!/usr/bin/env bash
# Install what cross-compiling pixelplusd/pixelplus for arm64 on an amd64 Debian/Ubuntu host
# needs: the aarch64 GCC toolchain plus arm64 ALSA headers (libasound2-dev:arm64) for the
# daemon's audio output (cpal, cargo feature "audio"). Used by CI and release workflows;
# run with sudo. Afterwards build with the environment printed at the end (packaging/build-deb.sh
# sets it by itself).
#
# Ubuntu serves arm64 packages from ports.ubuntu.com, so existing sources are pinned to the
# host architecture and a ports source is added; Debian mirrors carry arm64 already.
set -euo pipefail

[[ "$(id -u)" == 0 ]] || { echo "run as root (sudo $0)" >&2; exit 1; }
export DEBIAN_FRONTEND=noninteractive
host_arch="$(dpkg --print-architecture)"
dpkg --add-architecture arm64

# shellcheck source=/dev/null
. /etc/os-release
if [[ "${ID}" == ubuntu ]]; then
    codename="${VERSION_CODENAME:?}"
    if [[ -f /etc/apt/sources.list.d/ubuntu.sources ]]; then
        # deb822 (24.04+): add "Architectures: <host>" to stanzas that don't have one
        if ! grep -q '^Architectures:' /etc/apt/sources.list.d/ubuntu.sources; then
            sed -i "s/^Types: deb\$/Types: deb\nArchitectures: ${host_arch}/" /etc/apt/sources.list.d/ubuntu.sources
        fi
    fi
    if [[ -f /etc/apt/sources.list ]]; then
        sed -i -E "s/^deb (http|https|mirror)/deb [arch=${host_arch}] \1/" /etc/apt/sources.list
    fi
    cat >/etc/apt/sources.list.d/pixelplus-arm64-ports.list <<PORTS
deb [arch=arm64] http://ports.ubuntu.com/ubuntu-ports ${codename} main universe
deb [arch=arm64] http://ports.ubuntu.com/ubuntu-ports ${codename}-updates main universe
deb [arch=arm64] http://ports.ubuntu.com/ubuntu-ports ${codename}-security main universe
PORTS
fi

# Third-party sources without an arch pin may 404 for arm64; that must not stop us.
apt-get update -q || echo "warning: some package sources failed to update (see above); continuing" >&2
apt-get install -y -q --no-install-recommends \
    gcc-aarch64-linux-gnu libc6-dev-arm64-cross pkg-config libasound2-dev:arm64

cat <<'ENV'
arm64 cross dependencies installed. Build environment:
  CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc
  CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc
  AR_aarch64_unknown_linux_gnu=aarch64-linux-gnu-ar
  PKG_CONFIG_ALLOW_CROSS=1
  PKG_CONFIG_LIBDIR=/usr/lib/aarch64-linux-gnu/pkgconfig:/usr/share/pkgconfig
ENV
