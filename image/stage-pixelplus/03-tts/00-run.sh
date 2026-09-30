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
# Binary wheels only: compiling onnxruntime under qemu would take hours. Every package in
# requirements.txt has manylinux aarch64 wheels for Bookworm (3.11) and Trixie (3.13).
# If one is missing for a new Python, skip TTS rather than failing the whole image:
# the DJ then renders speech in the browser.
if ! /opt/pixelplus-tts/venv/bin/pip install --no-cache-dir --prefer-binary --only-binary=:all: \
	-r /tmp/pixelplus-tts/requirements.txt; then
	echo "stage-pixelplus: WARNING: TTS wheels unavailable for \$(python3 --version); TTS left out of this image" >&2
	rm -rf /opt/pixelplus-tts /tmp/pixelplus-tts
	exit 0
fi
# praat-parselmouth (optional PSOLA prosody) has no aarch64 wheels; try anyway for the future.
/opt/pixelplus-tts/venv/bin/pip install --no-cache-dir --only-binary=:all: "praat-parselmouth>=0.4.3" \
	|| echo "stage-pixelplus: no praat-parselmouth wheel; TTS uses its built-in energy prosody"
/opt/pixelplus-tts/venv/bin/pip install --no-cache-dir --no-deps /tmp/pixelplus-tts
/opt/pixelplus-tts/venv/bin/python -c 'import kokoro_onnx, onnxruntime, pixelplus_tts; from pixelplus_tts import prosody; print("TTS ok, prosody:", prosody.backend())'
if [ "${PIXELPLUS_TTS_MODELS:-1}" = "1" ]; then
	install -d -o pixelplus -g pixelplus /var/lib/pixelplus/tts/models
	PIXELPLUS_DATA_DIR=/var/lib/pixelplus /opt/pixelplus-tts/venv/bin/python -m pixelplus_tts \
		download-models --models-dir /var/lib/pixelplus/tts/models --variant "${PIXELPLUS_TTS_VARIANT:-int8}"
	chown -R pixelplus:pixelplus /var/lib/pixelplus/tts
fi
find /opt/pixelplus-tts -name __pycache__ -type d -prune -exec rm -rf {} +
rm -rf /tmp/pixelplus-tts
CHROOT
