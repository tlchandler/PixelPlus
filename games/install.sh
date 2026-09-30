#!/bin/sh
# Install the PixelPlus games sidecar (pixelplus-games.service). Safe to run more than once.
#
#   sudo ./install.sh [--prefix DIR] [--data-dir DIR] [--no-apt] [--no-service]
#
#   --prefix DIR    where the program is installed      (default /usr/lib/pixelplus/games)
#   --data-dir DIR  PixelPlus data directory            (default /var/lib/pixelplus)
#   --no-apt        don't install the Debian packages   (e.g. already in the image)
#   --no-service    install files only; don't enable/start the systemd service
#
# The service runs as the unprivileged "pixelplus" user, like pixelplusd and exactly as the
# pixelplus Debian package installs it (packaging/systemd/pixelplus-games.service is the same
# file as ./pixelplus-games.service). On a system with the package the program and unit are
# already installed; this script then only refreshes the program files and enables the unit.
set -eu

PREFIX=/usr/lib/pixelplus/games
DATA_DIR=/var/lib/pixelplus
APT=1
SERVICE=1
SERVICE_USER=pixelplus
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

# The service user (normally created by the pixelplus package's postinst).
if ! getent passwd "$SERVICE_USER" >/dev/null 2>&1; then
    say "Creating the system user $SERVICE_USER"
    adduser --system --group --home "$DATA_DIR" --no-create-home \
        --shell /usr/sbin/nologin --gecos "PixelPlus daemon" "$SERVICE_USER" >/dev/null
fi
# Game sound goes through aplay: the sound card belongs to group "audio".
if getent group audio >/dev/null 2>&1; then
    adduser "$SERVICE_USER" audio >/dev/null 2>&1 || true
fi

if [ "$(cd "$PREFIX" 2>/dev/null && pwd)" != "$SRC" ]; then
    say "Installing to $PREFIX"
    mkdir -p "$PREFIX"
    rm -rf "$PREFIX/pixelplus_games"
    cp -r "$SRC/pixelplus_games" "$PREFIX/"
    cp "$SRC/README.md" "$SRC/requirements.txt" "$PREFIX/"
    find "$PREFIX" -name __pycache__ -type d -prune -exec rm -rf {} +
    chmod -R u=rwX,go=rX "$PREFIX"
fi

# ROMs are uploaded from the web UI (Settings > Games) into this folder; smb.nes is Super Mario Bros.
say "ROM folder: $DATA_DIR/games/roms"
install -d -o "$SERVICE_USER" -g "$SERVICE_USER" -m 0750 "$DATA_DIR" "$DATA_DIR/games" "$DATA_DIR/games/roms"

# /run/pixelplus holds the control socket (games.sock) pixelplusd talks to. The package's
# tmpfiles.d entry creates it; without the package, add an equivalent one.
if [ ! -f /usr/lib/tmpfiles.d/pixelplus.conf ] && [ ! -f /etc/tmpfiles.d/pixelplus.conf ]; then
    say "Adding /etc/tmpfiles.d/pixelplus.conf (/run/pixelplus)"
    mkdir -p /etc/tmpfiles.d
    printf 'd /run/pixelplus 0775 %s %s -\n' "$SERVICE_USER" "$SERVICE_USER" >/etc/tmpfiles.d/pixelplus.conf
fi
if command -v systemd-tmpfiles >/dev/null 2>&1; then
    systemd-tmpfiles --create /usr/lib/tmpfiles.d/pixelplus.conf /etc/tmpfiles.d/pixelplus.conf 2>/dev/null || true
fi
if [ ! -d /run/pixelplus ]; then
    install -d -o "$SERVICE_USER" -g "$SERVICE_USER" -m 0775 /run/pixelplus
fi

if ! python3 -c 'import sys; sys.path.insert(0, sys.argv[1]); from pixelplus_games.libretro import find_core; sys.exit(0 if find_core() else 1)' "$PREFIX"; then
    echo "WARNING: no libretro NES core found; install libretro-nestopia (or set PIXELPLUS_NES_CORE)." >&2
fi

if [ "$SERVICE" = 1 ] && command -v systemctl >/dev/null 2>&1; then
    PACKAGED=""
    for u in /usr/lib/systemd/system/pixelplus-games.service /lib/systemd/system/pixelplus-games.service; do
        [ -f "$u" ] && PACKAGED="$u" && break
    done
    if [ -n "$PACKAGED" ] && [ "$PREFIX" = /usr/lib/pixelplus/games ] && [ "$DATA_DIR" = /var/lib/pixelplus ]; then
        say "Using the packaged $PACKAGED"
        rm -f /etc/systemd/system/pixelplus-games.service   # an old override from earlier versions
    else
        say "Installing /etc/systemd/system/pixelplus-games.service"
        sed -e "s|/usr/lib/pixelplus/games|$PREFIX|g" \
            -e "s|/var/lib/pixelplus|$DATA_DIR|g" \
            "$SRC/pixelplus-games.service" >/etc/systemd/system/pixelplus-games.service
    fi
    systemctl daemon-reload
    systemctl enable pixelplus-games.service >/dev/null
    systemctl restart pixelplus-games.service
    say "Running as $SERVICE_USER. Logs: journalctl -u pixelplus-games -f"
fi

say "Done. Next: in PixelPlus open Settings > Games, upload your ROM(s), pick the matrix prop,"
say "press 'Test pattern', then enable games and play a round from your phone."
