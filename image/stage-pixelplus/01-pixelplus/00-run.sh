#!/bin/bash -e
# Install the pixelplus .deb(s) copied here by image/build.sh.

# Mark this system as a PixelPlus appliance BEFORE the package is installed, so its
# postinst links the hotspot/NetworkManager config and the appliance-only units run.
install -d -m 0755 "${ROOTFS_DIR}/etc/pixelplus"
cat > "${ROOTFS_DIR}/etc/pixelplus/appliance" <<APPLIANCE
# This file marks a PixelPlus SD-card image. Removing it disables pixelplus.txt
# handling and the setup hotspot (pixelplus-firstboot / -netwatch services).
PIXELPLUS_IMAGE_VERSION=${PIXELPLUS_VERSION:-dev}
PIXELPLUS_IMAGE_RELEASE=${RELEASE}
PIXELPLUS_IMAGE_DATE=${IMG_DATE}
APPLIANCE

shopt -s nullglob
debs=(files/*.deb)
if [ ${#debs[@]} -eq 0 ]; then
	echo "stage-pixelplus: no .deb in $(pwd)/files - run image/build.sh with --deb" >&2
	exit 1
fi
install -d "${ROOTFS_DIR}/tmp/pixelplus-debs"
cp "${debs[@]}" "${ROOTFS_DIR}/tmp/pixelplus-debs/"

# Keep the installed package (and its signature, when the release is signed) as the
# version signed updates can go back to (F15): the first over-the-air update then
# needs neither apt nor dpkg-repack to keep a rollback copy.
install -d -m 0755 "${ROOTFS_DIR}/var/cache/pixelplus"
install -d -m 0700 "${ROOTFS_DIR}/var/cache/pixelplus/rollback"
for d in "${debs[@]}"; do
	install -m 0600 "$d" "${ROOTFS_DIR}/var/cache/pixelplus/rollback/"
	[ -f "$d.minisig" ] && install -m 0600 "$d.minisig" "${ROOTFS_DIR}/var/cache/pixelplus/rollback/"
done

on_chroot <<'CHROOT'
set -e
apt-get -o Acquire::Retries=3 install -y --no-install-recommends /tmp/pixelplus-debs/*.deb
rm -rf /tmp/pixelplus-debs
CHROOT
