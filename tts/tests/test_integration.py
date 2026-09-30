"""Real Kokoro renders. Skipped unless the model is downloaded
(PIXELPLUS_TTS_MODELS_DIR or $PIXELPLUS_DATA_DIR/tts/models)."""
import time

import numpy as np
import pytest

from conftest import models_dir, needs_ffmpeg
from pixelplus_tts.models import models_present, resolve_variant

MODELS = models_dir()
VARIANT = resolve_variant("auto", MODELS)
pytestmark = [needs_ffmpeg, pytest.mark.skipif(not models_present(MODELS, VARIANT),
                                               reason=f"Kokoro model not downloaded in {MODELS}")]


@pytest.fixture(scope="module")
def engine():
    from pixelplus_tts.engine import Engine
    return Engine(MODELS, VARIANT, threads=0, idle_minutes=0)


def median_semitones(audio, sr, t0, t1):
    import parselmouth
    snd = parselmouth.Sound(audio.astype(np.float64), sr).extract_part(t0, t1)
    f = snd.to_pitch().selected_array["frequency"]
    f = f[f > 0]
    return float(np.median(12 * np.log2(f)))


def test_blended_voice_render(engine):
    from pixelplus_tts.render import build_job, render
    job = build_job({"lines": [{"voice": "nick", "text": "Good evening, and welcome to the show!"},
                               {"voice": "holly", "text": "Merry Christmas, everybody!", "energy": 1.5}],
                     "format": "mp3"})
    t = time.time()
    res = render(engine, job)
    dt = time.time() - t
    assert res.data[:3] == b"ID3" or res.data[:2] == b"\xff\xfb" or len(res.data) > 10000
    assert 3000 < res.duration_ms < 10000
    assert res.loudness_lufs == pytest.approx(-16, abs=1.0)
    print(f"\nrendered {res.duration_ms} ms in {dt:.2f}s (RTF {dt * 1000 / res.duration_ms:.2f}) model={VARIANT}")


def test_hype_lifts_the_punchline(engine):
    from pixelplus_tts.pronounce import compile_rules
    from pixelplus_tts.voices import resolve_voice
    nick = resolve_voice("nick")
    text = "Now sit back, relax, and enjoy the show!"
    flat, g0 = engine.synthesize(text, nick, compile_rules([]), 1.05, 0.4)
    hype, g1 = engine.synthesize(text, nick, compile_rules([]), 1.05, 1.0)
    assert g0 is None and g1 is not None and g1.max() == pytest.approx(3.0, abs=0.01)  # boost*peak
    sr = 24000
    # punchline (last ~40%) vs lead-in (first ~35%): hype lands higher relative to its lead-in
    d_flat = median_semitones(flat, sr, 0.6 * len(flat) / sr, len(flat) / sr) - median_semitones(flat, sr, 0, 0.35 * len(flat) / sr)
    d_hype = median_semitones(hype, sr, 0.6 * len(hype) / sr, len(hype) / sr) - median_semitones(hype, sr, 0, 0.35 * len(hype) / sr)
    assert d_hype > d_flat + 1.0


def test_ipa_pronunciation_render(engine):
    from pixelplus_tts.pronounce import builtin_pronunciations, compile_rules
    from pixelplus_tts.voices import resolve_voice
    audio, _ = engine.synthesize("Feliz Navidad from Tchaikovsky!", resolve_voice("holly"),
                                 compile_rules(builtin_pronunciations()), 1.05, 0.4)
    assert len(audio) > 24000


def test_unload_frees_model(engine):
    engine.load()
    assert engine.loaded
    assert engine.unload() and not engine.loaded
