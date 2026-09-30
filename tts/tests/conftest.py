import os
import shutil

import numpy as np
import pytest


def have_ffmpeg() -> bool:
    if shutil.which("ffmpeg"):
        return True
    try:
        import imageio_ffmpeg
        return bool(imageio_ffmpeg.get_ffmpeg_exe())
    except Exception:
        return False


needs_ffmpeg = pytest.mark.skipif(not have_ffmpeg(), reason="ffmpeg not available")


class FakeEngine:
    """Stands in for the Kokoro engine: a 220 Hz tone, 60 ms per character."""
    variant = "fp32"
    available = True
    loaded = False
    threads = 1

    def __init__(self):
        self.calls = []

    def base_voice_ids(self):
        from pixelplus_tts.voices import BASE_VOICE_IDS
        return set(BASE_VOICE_IDS)

    def synthesize(self, text, voice, rules, speed=None, energy=0.0):
        self.calls.append({"text": text, "voice": voice["id"], "speed": speed, "energy": energy})
        n = int(24000 * 0.06 * max(len(text), 1) / (speed or 1))
        t = np.arange(n) / 24000
        return (0.3 * np.sin(2 * np.pi * 220 * t)).astype(np.float32), None

    def load(self):
        self.loaded = True

    def unload(self):
        was, self.loaded = self.loaded, False
        return was


@pytest.fixture
def fake_engine():
    return FakeEngine()


def models_dir() -> str:
    return os.environ.get("PIXELPLUS_TTS_MODELS_DIR") or os.path.join(
        os.environ.get("PIXELPLUS_DATA_DIR", "/var/lib/pixelplus"), "tts", "models")
