#!/bin/bash -e
# Optional: Kokoro TTS sidecar venv (+ models) for Pi 4/5. image/build.sh copies the
# tts/ sources to files/tts when TTS is enabled (default); otherwise this is a no-op.
if [ ! -f files/tts/pyproject.toml ]; then
	echo "stage-pixelplus: TTS not included in this image"
	exit 0
fi
rm -rf "${ROOTFS_DIR}/tmp/pixelplus-tts"
cp -a files/tts "${ROOTFS_DIR}/tmp/pixelplus-tts"

on_chroot <<CHROOT
set -e
python3 -m venv /opt/pixelplus-tts/venv
/opt/pixelplus-tts/venv/bin/pip install --no-cache-dir --upgrade pip
# Binary wheels only: compiling onnxruntime/parselmouth under qemu would take hours.
/opt/pixelplus-tts/venv/bin/pip install --no-cache-dir --prefer-binary --only-binary=:all: \
	-r /tmp/pixelplus-tts/requirements.txt
/opt/pixelplus-tts/venv/bin/pip install --no-cache-dir --no-deps /tmp/pixelplus-tts
if [ "${PIXELPLUS_TTS_MODELS:-1}" = "1" ]; then
	install -d -o pixelplus -g pixelplus /var/lib/pixelplus/tts/models
	PIXELPLUS_DATA_DIR=/var/lib/pixelplus /opt/pixelplus-tts/venv/bin/python -m pixelplus_tts \
		download-models --models-dir /var/lib/pixelplus/tts/models --variant "${PIXELPLUS_TTS_VARIANT:-int8}"
	chown -R pixelplus:pixelplus /var/lib/pixelplus/tts
fi
find /opt/pixelplus-tts -name __pycache__ -type d -prune -exec rm -rf {} +
rm -rf /tmp/pixelplus-tts
CHROOT
