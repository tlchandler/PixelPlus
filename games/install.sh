#!/bin/sh
# Install the PixelPlus games sidecar (pixelplus-games.service). Safe to run more than once.
#
#   sudo ./install.sh [--prefix DIR] [--data-dir DIR] [--no-apt] [--no-service]
#
#   --prefix DIR    where the program is installed      (default /usr/lib/pixelplus/games)
#   --data-dir DIR  PixelPlus data directory            (default /var/lib/pixelplus)
#   --no-apt        don't install the Debian packages   (e.g. already in the image)
#   --no-service    install files only; don't enable/start the systemd service
set -eu

PREFIX=/usr/lib/pixelplus/games
DATA_DIR=/var/lib/pixelplus
APT=1
SERVICE=1
while [ $# -gt 0 ]; do
    case "$1" in
        --prefix) PREFIX="${2:?--prefix needs a directory}"; shift 2 ;;
        --data-dir) DATA_DIR="${2:?--data-dir needs a directory}"; shift 2 ;;
        --no-apt) APT=0; shift ;;
        --no-service) SERVICE=0; shift ;;
        -h|--help) sed -n '2,9p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "Unknown option: $1 (see --help)" >&2; exit 2 ;;
    esac
done

if [ "$(id -u)" -ne 0 ]; then
    echo "Please run as root: sudo $0 $*" >&2
    exit 1
fi

SRC="$(cd "$(dirname "$0")" && pwd)"
say() { printf '\033[1;33m==>\033[0m %s\n' "$*"; }

if [ "$APT" = 1 ]; then
    # NES emulator core, numpy for scaling, qrcode for the QR invite, aplay for sound.
    say "Installing packages: libretro-nestopia python3-numpy python3-qrcode alsa-utils"
    apt-get update -q
    DEBIAN_FRONTEND=noninteractive apt-get install -y -q --no-install-recommends \
        libretro-nestopia python3-numpy python3-qrcode alsa-utils
fi

if [ "$(cd "$PREFIX" 2>/dev/null && pwd)" != "$SRC" ]; then
    say "Installing to $PREFIX"
    mkdir -p "$PREFIX"
    rm -rf "$PREFIX/pixelplus_games"
    cp -r "$SRC/pixelplus_games" "$PREFIX/"
    cp "$SRC/README.md" "$SRC/requirements.txt" "$SRC/pixelplus-games.service" "$PREFIX/"
    find "$PREFIX" -name __pycache__ -type d -prune -exec rm -rf {} +
fi

# ROMs are uploaded from the web UI (Settings > Games) into this folder; smb.nes is Super Mario Bros.
say "ROM folder: $DATA_DIR/games/roms"
mkdir -p "$DATA_DIR/games/roms"

if ! python3 -c 'import sys; sys.path.insert(0, sys.argv[1]); from pixelplus_games.libretro import find_core; sys.exit(0 if find_core() else 1)' "$PREFIX"; then
    echo "WARNING: no libretro NES core found; install libretro-nestopia (or set PIXELPLUS_NES_CORE)." >&2
fi

if [ "$SERVICE" = 1 ] && command -v systemctl >/dev/null 2>&1; then
    say "Installing pixelplus-games.service"
    sed -e "s|^WorkingDirectory=.*|WorkingDirectory=$PREFIX|" \
        -e "s|^Environment=PIXELPLUS_DATA_DIR=.*|Environment=PIXELPLUS_DATA_DIR=$DATA_DIR|" \
        -e "s|^ReadWritePaths=.*|ReadWritePaths=-$DATA_DIR/games|" \
        -e "s|^Documentation=.*|Documentation=file://$PREFIX/README.md|" \
        "$SRC/pixelplus-games.service" > /etc/systemd/system/pixelplus-games.service
    systemctl daemon-reload
    systemctl enable pixelplus-games.service >/dev/null
    systemctl restart pixelplus-games.service
    say "Running. Logs: journalctl -u pixelplus-games -f"
fi

say "Done. Next: in PixelPlus open Settings > Games, upload your ROM(s), pick the matrix prop,"
say "press 'Test pattern', then enable games and play a round from your phone."
