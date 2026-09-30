#!/bin/bash -e
# PixelPlus system tuning: boot config, SD-card friendly logging, swap, CPU governor,
# fast boot, pixelplus.txt template, locked default user.

BOOT="${ROOTFS_DIR}/boot/firmware"

# --- config.txt / cmdline.txt (build time only; at runtime PixelPlus never edits cmdline.txt)
sed -i -e 's/^camera_auto_detect=1/camera_auto_detect=0/' \
	-e 's/^display_auto_detect=1/display_auto_detect=0/' "${BOOT}/config.txt"
if ! grep -q "PixelPlus base settings" "${BOOT}/config.txt"; then
	cat files/config-pixelplus.txt >> "${BOOT}/config.txt"
fi
if ! grep -q "cpufreq.default_governor=" "${BOOT}/cmdline.txt"; then
	# Constant CPU clock = stable pixel timing and sync. Single line file: append in place.
	sed -i '1 s/$/ cpufreq.default_governor=performance/' "${BOOT}/cmdline.txt"
fi

# --- board settings include file (filled on first boot by pixelplus-firstboot)
if [ ! -s "${BOOT}/pixelplus.conf" ]; then
	cat > "${BOOT}/pixelplus.conf" <<CONF
# pixelplus.conf - PixelPlus board settings, included from config.txt.
# Empty until the first boot detects the board (see /var/log/pixelplus-firstboot.log).
CONF
fi

# --- the user-editable settings file
install -m 0644 "${ROOTFS_DIR}/usr/share/pixelplus/pixelplus.txt.template" "${BOOT}/pixelplus.txt"

# --- modules
install -d "${ROOTFS_DIR}/etc/modules-load.d"
echo "i2c-dev" > "${ROOTFS_DIR}/etc/modules-load.d/pixelplus.conf"

# --- logging: volatile journal, no rsyslog
install -D -m 0644 files/journald-pixelplus.conf "${ROOTFS_DIR}/etc/systemd/journald.conf.d/50-pixelplus.conf"

# --- swap: zram only
if [ -d "${ROOTFS_DIR}/etc/rpi" ] || [ -e "${ROOTFS_DIR}/usr/lib/systemd/system-generators/rpi-swap-generator" ]; then
	install -D -m 0644 files/rpi-swap-pixelplus.conf "${ROOTFS_DIR}/etc/rpi/swap.conf.d/50-pixelplus.conf"
fi

# --- console banner + motd
install -D -m 0644 files/issue-pixelplus "${ROOTFS_DIR}/etc/issue.d/pixelplus.issue"
install -m 0644 files/motd "${ROOTFS_DIR}/etc/motd"

# --- Wi-Fi radio on by default (hotspot fallback needs it; the kernel's world
#     regulatory domain applies until a country is set in pixelplus.txt/Imager/portal)
install -d "${ROOTFS_DIR}/var/lib/NetworkManager"
cat > "${ROOTFS_DIR}/var/lib/NetworkManager/NetworkManager.state" <<STATE
[main]
NetworkingEnabled=true
WirelessEnabled=true
WWANEnabled=false
STATE

install -m 0644 files/zramswap "${ROOTFS_DIR}/tmp/pixelplus-zramswap"

on_chroot <<'CHROOT'
set -e
# SD-card friendliness and boot speed
apt-get purge -y rsyslog dphys-swapfile triggerhappy >/dev/null 2>&1 || true
if ! dpkg -s rpi-swap >/dev/null 2>&1; then
	# Bookworm: zram-tools instead of a swap file
	apt-get install -y --no-install-recommends zram-tools
	install -m 0644 /tmp/pixelplus-zramswap /etc/default/zramswap
fi
rm -f /tmp/pixelplus-zramswap

# Units that only cost boot time or SD writes on an appliance. apt timers are off so a
# background upgrade never runs during a show (updates: Settings -> Updates in the UI).
for u in apt-daily.timer apt-daily-upgrade.timer man-db.timer e2scrub_reap.service \
	NetworkManager-wait-online.service ModemManager.service raspi-config.service \
	keyboard-setup.service; do
	systemctl disable "$u" >/dev/null 2>&1 || true
done
# raspi-config.service would switch the governor back to ondemand; it is disabled above.

# Default login user: locked until the owner sets ssh_password/ssh_key in pixelplus.txt
# or creates a user with Raspberry Pi Imager.
if id -u pi >/dev/null 2>&1; then
	passwd -l pi >/dev/null
fi

systemctl enable pixelplusd.service pixelplus-firstboot.service pixelplus-reapply.path \
	pixelplus-netwatch.service avahi-daemon.service NetworkManager.service
systemctl enable pixelplus-games.service pixelplus-tts.service >/dev/null 2>&1 || true
CHROOT
