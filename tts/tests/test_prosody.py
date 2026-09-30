"""Energy without Praat (prosody.py): praat-parselmouth has no Linux aarch64 wheels."""
import numpy as np
import pytest

from pixelplus_tts import energy, prosody
from pixelplus_tts.energy import SAMPLE_RATE

from conftest import needs_ffmpeg


def tone(freq=200.0, seconds=1.0):
    t = np.arange(int(SAMPLE_RATE * seconds)) / SAMPLE_RATE
    return (0.4 * np.sin(2 * np.pi * freq * t)).astype(np.float32)


def dominant(x):
    spec = np.abs(np.fft.rfft(x * np.hanning(len(x))))
    return np.argmax(spec) * SAMPLE_RATE / len(x)


def test_basic_shift_changes_pitch_not_length():
    x = tone(200)
    y = prosody.shift_basic(x, 2.0)
    assert abs(len(y) - len(x)) <= len(x) * 0.01
    assert dominant(y[2000:-2000]) == pytest.approx(200 * 2 ** (2 / 12), rel=0.03)
    assert np.isfinite(y).all() and np.abs(y).max() < 1.0


def test_basic_tempo_changes_length():
    y = prosody.shift_basic(tone(200), 0.0, tempo=0.8)
    assert len(y) == pytest.approx(SAMPLE_RATE / 0.8, rel=0.01)
    assert dominant(y[2000:-2000]) == pytest.approx(200, rel=0.03)


def test_no_change_is_identity():
    x = tone()
    assert np.array_equal(prosody.shift_basic(x, 0.0), x)


def test_fallback_energy_contract(monkeypatch):
    monkeypatch.setattr(prosody, "have_parselmouth", lambda: False)
    monkeypatch.setattr(prosody, "have_rubberband", lambda: False)
    voice = {"energy": {"pitch": 2, "lift": 3, "stretch": 1.1, "boost": 2.5, "max_lift": 12, "ceiling": 3}}
    x = tone(200, 2.0)
    # flat: same length, raised pitch
    flat = energy.add_energy(x, voice, [(0, 1.0)])
    assert isinstance(flat, np.ndarray) and len(flat) == pytest.approx(len(x), rel=0.01)
    assert dominant(flat[2000:-2000]) > 215
    # nothing to do at energy 0
    assert energy.add_energy(x, voice, [(0, 0.0)]) is x
    # hype: builds into the punchline (second half): higher, longer, louder
    curve = energy.energy_curve(["aaaa", "bbbb", ""], 2.0, 0.4, 1.0)
    out, gain = energy.add_energy(x, voice, curve, 0.4)
    assert len(out) > len(x) * 1.02
    assert len(gain) == len(out)
    assert gain[: len(out) // 4].max() < 0.5 and gain[-100:].min() > 2.0
    lead, punch = out[2000:SAMPLE_RATE - 3000], out[-SAMPLE_RATE + 3000:-2000]
    assert dominant(punch) > dominant(lead) * 1.1


def test_backend_names():
    assert prosody.backend() in ("psola", "rubberband", "basic")


@needs_ffmpeg
def test_rubberband_when_available():
    if not prosody.have_rubberband():
        pytest.skip("ffmpeg without the rubberband filter")
    y = prosody.shift_rubberband(tone(200), 2.0)
    assert dominant(y[2000:-2000]) == pytest.approx(200 * 2 ** (2 / 12), rel=0.03)


def test_fallback_tail_after_the_punchline_eases_back(monkeypatch):
    """"Up next, *Feliz Navidad!* Enjoy." - PSOLA lifts only the punchline and eases the tail back
    to the lead-in level; the fallback used to lift and slow everything after the punchline."""
    monkeypatch.setattr(prosody, "have_parselmouth", lambda: False)
    monkeypatch.setattr(prosody, "have_rubberband", lambda: False)
    voice = {"energy": {"pitch": 0, "lift": 4, "stretch": 1.2, "boost": 3, "max_lift": 12, "ceiling": 3}}
    x = tone(200, 3.0)
    curve = energy.energy_curve(["aaaa", "bbbb", "cccc"], 3.0, 0.4, 1.0)   # lead, punch, tail: 1 s each
    out, gain = energy.add_energy(x, voice, curve, 0.4)
    # only the middle second is stretched
    assert len(x) * 1.02 < len(out) < len(x) * 1.1
    tail = out[-SAMPLE_RATE // 2:-2000]
    punch_mid = out[int(1.4 * SAMPLE_RATE):int(1.8 * SAMPLE_RATE)]
    assert dominant(punch_mid) > 200 * 1.2
    assert dominant(tail) == pytest.approx(200, rel=0.03)   # back at the lead-in pitch
    assert gain[-200:].max() < 0.5                           # and loudness
    assert len(gain) == len(out)


def test_fallback_is_bounded_for_extreme_settings(monkeypatch):
    monkeypatch.setattr(prosody, "have_parselmouth", lambda: False)
    monkeypatch.setattr(prosody, "have_rubberband", lambda: False)
    voice = {"energy": {"pitch": 500, "lift": 1000, "stretch": 1000, "boost": 3, "max_lift": 1000, "ceiling": 1000}}
    x = tone(200, 1.0)
    curve = energy.energy_curve(["aaaa", "bbbb", ""], 1.0, 0.4, 1.0)
    out, _ = energy.add_energy(x, voice, curve, 0.4)
    assert len(out) < len(x) * 6 and np.isfinite(out).all()
    y = prosody.shift_basic(x, 1e6, tempo=1e-9)
    assert len(y) < len(x) * 6
