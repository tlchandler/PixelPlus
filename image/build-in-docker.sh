#!/usr/bin/env bash
# Build the PixelPlus SD-card image inside Docker (wraps pi-gen's build-docker.sh).
#
#   image/build-in-docker.sh --deb dist/pixelplus_<ver>_arm64.deb [--release trixie|bookworm] ...
#
# Requirements (pi-gen needs to create loop devices and chroot into an arm64 rootfs):
#   * Docker with permission to run --privileged containers (not rootless Docker)
#   * on x86_64 hosts: qemu-user-static + binfmt_misc, e.g.
#       sudo apt install qemu-user-static binfmt-support
#     (GitHub-hosted ubuntu runners work; see .github/workflows/release.yml)
#   * ~25 GB free disk, 30-90 minutes
# All options are passed to image/build.sh (see `image/build.sh --help`).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

command -v docker >/dev/null 2>&1 || { echo "docker not found" >&2; exit 1; }
if ! docker info >/dev/null 2>&1 && ! sudo -n docker info >/dev/null 2>&1; then
    echo "cannot talk to the Docker daemon (is it running? are you in the docker group?)" >&2
    exit 1
fi
if [[ "$(uname -m)" != aarch64 && ! -e /proc/sys/fs/binfmt_misc/qemu-aarch64 ]]; then
    echo "warning: qemu-aarch64 binfmt handler not registered; pi-gen will try to register it" >&2
fi
exec "${HERE}/build.sh" --docker "$@"
