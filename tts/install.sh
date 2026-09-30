#!/usr/bin/env bash
# Install the PixelPlus TTS sidecar (Pi 4/5 64-bit, or any Debian-ish x86_64/arm64 host).
#
#   sudo ./install.sh                      # venv + model (auto variant) + systemd service
#   sudo ./install.sh --variant fp16       # pick a model variant: auto | fp32 | fp16 | int8
#   sudo ./install.sh --no-service         # e.g. Docker: don't touch systemd
#   ./install.sh --prefix ~/pp-tts --data-dir ~/pp-data --no-service   # unprivileged dev install
#
# Re-running is safe: the venv is upgraded in place and model downloads resume and are
# skipped once verified (sha256).
set -euo pipefail

SRC="$(cd "$(dirname "$(readlink -f "$0")")" && pwd)"
PREFIX=/opt/pixelplus-tts
DATA_DIR=/var/lib/pixelplus
VARIANT=auto
MODELS=1
SERVICE=1
APT=1
MIRROR=""

usage() { sed -n '2,11p' "$0" | sed 's/^# \{0,1\}//'; exit "${1:-0}"; }
while [ $# -gt 0 ]; do
  case "$1" in
    --prefix) PREFIX="$2"; shift 2 ;;
    --data-dir) DATA_DIR="$2"; shift 2 ;;
    --variant) VARIANT="$2"; shift 2 ;;
    --models-mirror) MIRROR="$2"; shift 2 ;;
    --no-models) MODELS=0; shift ;;
    --no-service) SERVICE=0; shift ;;
    --no-apt) APT=0; shift ;;
    -h|--help) usage 0 ;;
    *) echo "unknown option: $1" >&2; usage 1 ;;
  esac
done
case "$VARIANT" in auto|fp32|fp16|int8) ;; *) echo "--variant must be auto, fp32, fp16 or int8" >&2; exit 1 ;; esac

log() { printf '\033[1;33m==>\033[0m %s\n' "$*"; }
IS_ROOT=0; [ "$(id -u)" = 0 ] && IS_ROOT=1

# --- board check -------------------------------------------------------------------
MEM_MB=$(awk '/MemTotal/ {print int($2/1024)}' /proc/meminfo 2>/dev/null || echo 4096)
ARCH=$(uname -m)
MODEL_NAME=$( { tr -d '\0' </proc/device-tree/model; } 2>/dev/null || true)
case "$ARCH" in
  aarch64|x86_64) ;;
  *) echo "PixelPlus TTS needs a 64-bit OS (aarch64 or x86_64); found $ARCH." >&2
     echo "On a Pi Zero 2 W / Pi 3 use browser rendering (Settings > DJ > TTS mode: browser)." >&2
     exit 1 ;;
esac
if echo "$MODEL_NAME" | grep -qE 'Zero 2|Pi 3'; then
  echo "Warning: $MODEL_NAME is too slow for on-device TTS (>5x slower than real time)." >&2
  echo "PixelPlus uses browser rendering on this board; installing anyway." >&2
fi

# --- system packages ---------------------------------------------------------------
if [ "$APT" = 1 ] && [ "$IS_ROOT" = 1 ] && command -v apt-get >/dev/null; then
  NEED=()
  command -v ffmpeg >/dev/null || NEED+=(ffmpeg)
  for py in python3.13 python3.12 python3.11 python3.10 python3; do
    if command -v $py >/dev/null; then
      $py -c 'import ensurepip, venv' 2>/dev/null || NEED+=("$(basename $py)-venv")
      break
    fi
  done
  if [ ${#NEED[@]} -gt 0 ]; then
    log "apt install ${NEED[*]}"
    DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends "${NEED[@]}" \
      || { apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends "${NEED[@]}"; }
  fi
fi

# --- python 3.10 - 3.13 --------------------------------------------------------------
PY=""
for c in python3.13 python3.12 python3.11 python3.10 python3; do
  if command -v $c >/dev/null && $c -c 'import sys; sys.exit(not (3,10) <= sys.version_info[:2] <= (3,13))'; then
    PY=$(command -v $c); break
  fi
done
[ -n "$PY" ] || { echo "Python 3.10 - 3.13 is required (the ONNX voice engine doesn't support 3.14 yet)." >&2; exit 1; }
log "using $($PY --version) at $PY"

# --- venv + package ----------------------------------------------------------------
mkdir -p "$PREFIX"
if [ ! -x "$PREFIX/venv/bin/python" ]; then
  log "creating $PREFIX/venv"
  $PY -m venv "$PREFIX/venv"
fi
VPY="$PREFIX/venv/bin/python"
"$VPY" -m pip install -q --upgrade pip wheel
EXTRA=""
command -v ffmpeg >/dev/null || EXTRA="[bundled-ffmpeg]"
log "installing pixelplus-tts$EXTRA"
# copy the source next to the venv so the unit's Documentation= path exists
if [ "$SRC" != "$PREFIX/src" ]; then
  rm -rf "$PREFIX/src"; mkdir -p "$PREFIX/src"
  cp -r "$SRC/pixelplus_tts" "$SRC/pyproject.toml" "$SRC/README.md" "$SRC/requirements.txt" "$PREFIX/src/"
fi
# Binary wheels for the heavy packages: compiling onnxruntime on a Pi takes hours.
"$VPY" -m pip install -q --upgrade --prefer-binary "$PREFIX/src$EXTRA"
# Optional Praat PSOLA prosody: wheels exist for x86_64 (and macOS), not for Linux aarch64.
if "$VPY" -m pip install -q --only-binary=:all: "praat-parselmouth>=0.4.3" 2>/dev/null; then
  log "praat-parselmouth installed (PSOLA energy prosody)"
else
  log "no praat-parselmouth wheel for this platform; using the built-in energy prosody"
fi
"$VPY" -c 'import pixelplus_tts, kokoro_onnx, onnxruntime; from pixelplus_tts import prosody; print("pixelplus-tts", pixelplus_tts.__version__, "onnxruntime", onnxruntime.__version__, "prosody", prosody.backend())'

# --- models ------------------------------------------------------------------------
MODELS_DIR="$DATA_DIR/tts/models"
mkdir -p "$MODELS_DIR" "$DATA_DIR/tts/cache"
if [ "$MODELS" = 1 ]; then
  log "downloading Kokoro model ($VARIANT; ${MEM_MB} MB RAM) into $MODELS_DIR"
  ARGS=(--models-dir "$MODELS_DIR" --variant "$VARIANT")
  [ -n "$MIRROR" ] && ARGS+=(--base-url "$MIRROR")
  PIXELPLUS_DATA_DIR="$DATA_DIR" "$VPY" -m pixelplus_tts download-models "${ARGS[@]}"
fi
# Its own unprivileged user (no polkit rights; security model in docs/BUILDING.md). It reads
# the music beds through group "pixelplus" (pixelplusd's) and owns only tts/.
TTS_USER=""
if [ "$IS_ROOT" = 1 ]; then
  if ! id pixelplus-tts >/dev/null 2>&1; then
    adduser --system --group --home /nonexistent --no-create-home \
      --shell /usr/sbin/nologin --gecos "PixelPlus TTS sidecar" pixelplus-tts >/dev/null
  fi
  TTS_USER=pixelplus-tts
  if getent group pixelplus >/dev/null 2>&1; then
    adduser pixelplus-tts pixelplus >/dev/null 2>&1 || true
  fi
  chown -R pixelplus-tts:pixelplus-tts "$DATA_DIR/tts"
fi

# --- systemd -----------------------------------------------------------------------
if [ "$SERVICE" = 1 ]; then
  if [ "$IS_ROOT" = 1 ] && command -v systemctl >/dev/null && [ -d /run/systemd/system ]; then
    UNIT=/etc/systemd/system/pixelplus-tts.service
    sed -e "s#/opt/pixelplus-tts#$PREFIX#g" -e "s#/var/lib/pixelplus#$DATA_DIR#g" "$SRC/pixelplus-tts.service" > "$UNIT"
    if [ -n "$TTS_USER" ]; then
      # run unprivileged as its own user; music beds are readable through group pixelplus
      EXTRA_GROUP=""
      getent group pixelplus >/dev/null 2>&1 && EXTRA_GROUP='\nSupplementaryGroups=pixelplus'
      sed -i "s#^\[Service\]\$#[Service]\nUser=$TTS_USER\nGroup=$TTS_USER$EXTRA_GROUP#" "$UNIT"
    fi
    if [ ! -f /etc/default/pixelplus-tts ]; then
      cat > /etc/default/pixelplus-tts <<EOF
# Overrides for pixelplus-tts.service (see $PREFIX/src/README.md)
# PIXELPLUS_TTS_MODEL=auto        # auto | fp32 | fp16 | int8
# PIXELPLUS_TTS_IDLE_MIN=10       # unload the model after N idle minutes (0 = never)
# PIXELPLUS_TTS_THREADS=3         # default: CPU cores - 1
# PIXELPLUS_TTS_CACHE_MB=200
EOF
      if [ "$VARIANT" != auto ]; then echo "PIXELPLUS_TTS_MODEL=$VARIANT" >> /etc/default/pixelplus-tts; fi
    fi
    systemctl daemon-reload
    systemctl enable pixelplus-tts.service >/dev/null
    systemctl restart pixelplus-tts.service
    log "pixelplus-tts.service running on 127.0.0.1:7081"
  else
    log "skipping systemd (not root or no systemd); run: $VPY -m pixelplus_tts serve"
  fi
fi

log "done. Try:  $VPY -m pixelplus_tts say nick \"Good evening and welcome to the show!\" -o /tmp/hello.mp3"
