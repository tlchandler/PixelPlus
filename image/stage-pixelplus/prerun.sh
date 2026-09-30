#!/bin/bash -e
# PixelPlus stage: builds on stage2 (Raspberry Pi OS Lite) - see image/build.sh.
if [ ! -d "${ROOTFS_DIR}" ]; then
	copy_previous
fi
