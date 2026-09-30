"""Energy/hype prosody without Praat.

praat-parselmouth (Praat PSOLA, used by energy.add_energy) publishes no Linux aarch64 wheels
(checked 2026-09 on PyPI for 0.4.4 - 0.4.7, every CPython version), so on a Raspberry Pi it
would have to be compiled from source - which the image build refuses (binary wheels only).
Without it, lines keep their energy with a simpler method:

* the whole line is raised by the voice's energy pitch (in semitones) and, for hype lines,
  the punchline additionally by the lift, then slowed by the voice's stretch and swollen by
  its loudness boost - the same targets as the PSOLA path, but applied per segment
  (lead-in / punchline) instead of per pitch period, and without widening the pitch swings;
* pitch shifting uses ffmpeg's ``rubberband`` filter with formants preserved when ffmpeg
  has it (Debian / Raspberry Pi OS builds do), else a small phase vocoder + resampling in
  numpy (formants move with the pitch; fine for the few semitones used here).

``backend()`` reports which method is active ("psola", "rubberband" or "basic"); the TTS
server shows it in ``GET /health`` as ``prosody``.
"""
from __future__ import annotations

import functools
import subprocess
from typing import Mapping

import numpy as np

SAMPLE_RATE = 24000
_XFADE_S = 0.03


@functools.lru_cache(maxsize=1)
def have_parselmouth() -> bool:
    try:
        import parselmouth  # noqa: F401
    except ImportError:
        return False
    return True


@functools.lru_cache(maxsize=1)
def have_rubberband() -> bool:
    try:
        from .audio import ffmpeg
        out = subprocess.run([ffmpeg(), "-hide_banner", "-filters"], capture_output=True, timeout=20)
    except (RuntimeError, OSError, subprocess.SubprocessError):
        return False
    return any(line.split()[1:2] == ["rubberband"] for line in out.stdout.decode(errors="replace").splitlines()
               if len(line.split()) > 1)


def backend() -> str:
    if have_parselmouth():
        return "psola"
    return "rubberband" if have_rubberband() else "basic"


# ---------------------------------------------------------------------------
# pitch / tempo
# ---------------------------------------------------------------------------

def _stft_stretch(x: np.ndarray, rate: float, n_fft: int = 1024, hop: int = 256) -> np.ndarray:
    """Phase-vocoder time stretch: ``rate`` > 1 makes it shorter (faster)."""
    if len(x) < n_fft:
        x = np.pad(x, (0, n_fft - len(x)))
    win = np.hanning(n_fft).astype(np.float64)
    n_frames = 1 + (len(x) - n_fft) // hop
    frames = np.stack([x[i * hop:i * hop + n_fft] * win for i in range(n_frames)])
    spec = np.fft.rfft(frames, axis=1)
    steps = np.arange(0, n_frames - 1, rate)
    omega = 2 * np.pi * hop * np.arange(spec.shape[1]) / n_fft
    phase = np.angle(spec[0])
    out_len = n_fft + hop * len(steps)
    y = np.zeros(out_len)
    # Overlap-added squared Hann windows sum to a constant (1.5 at 75 % overlap); dividing by
    # the constant instead of the running sum keeps the (silent) edges from blowing up.
    norm = float(np.sum(win ** 2) / hop)
    for k, s in enumerate(steps):
        i = int(s)
        frac = s - i
        a, b = spec[i], spec[i + 1]
        mag = (1 - frac) * np.abs(a) + frac * np.abs(b)
        frame = np.fft.irfft(mag * np.exp(1j * phase), n=n_fft) * win
        y[k * hop:k * hop + n_fft] += frame
        dphi = np.angle(b) - np.angle(a) - omega
        dphi -= 2 * np.pi * np.round(dphi / (2 * np.pi))
        phase = phase + omega + dphi
    return y / norm


def _resample(x: np.ndarray, n: int) -> np.ndarray:
    if n <= 1 or len(x) < 2:
        return np.zeros(max(n, 0))
    return np.interp(np.linspace(0, len(x) - 1, n), np.arange(len(x)), x)


def shift_basic(x: np.ndarray, semis: float, tempo: float = 1.0) -> np.ndarray:
    """Pitch by ``semis`` semitones and speed by ``tempo`` (numpy only)."""
    x = np.asarray(x, dtype=np.float64)
    if abs(semis) < 1e-3 and abs(tempo - 1) < 1e-3:
        return x.astype(np.float32)
    ratio = 2 ** (semis / 12)
    target = max(1, int(round(len(x) / tempo)))
    stretched = _stft_stretch(x, tempo / ratio)
    return _resample(stretched, target).astype(np.float32)


def shift_rubberband(x: np.ndarray, semis: float, tempo: float = 1.0) -> np.ndarray:
    from .audio import ffmpeg
    ratio = 2 ** (semis / 12)
    fmt = ["-f", "f32le", "-ar", str(SAMPLE_RATE), "-ac", "1"]
    r = subprocess.run(
        [ffmpeg(), "-hide_banner", "-nostats", "-loglevel", "error", *fmt, "-i", "pipe:0",
         "-af", f"rubberband=pitch={ratio:.6f}:tempo={tempo:.6f}:formant=preserved",
         *fmt, "pipe:1"],
        input=np.asarray(x, dtype=np.float32).tobytes(), capture_output=True, timeout=120)
    if r.returncode != 0:
        raise RuntimeError("ffmpeg rubberband failed: " + r.stderr.decode(errors="replace")[-300:])
    return np.frombuffer(r.stdout, dtype=np.float32).copy()


def shift(x: np.ndarray, semis: float, tempo: float = 1.0) -> np.ndarray:
    if abs(semis) < 1e-3 and abs(tempo - 1) < 1e-3:
        return np.asarray(x, dtype=np.float32)
    if have_rubberband():
        try:
            return shift_rubberband(x, semis, tempo)
        except (RuntimeError, OSError, subprocess.SubprocessError):
            pass
    return shift_basic(x, semis, tempo)


# ---------------------------------------------------------------------------
# energy
# ---------------------------------------------------------------------------

def _join(a: np.ndarray, b: np.ndarray) -> np.ndarray:
    n = min(int(_XFADE_S * SAMPLE_RATE), len(a), len(b))
    if n <= 0:
        return np.concatenate([a, b])
    fade = np.linspace(0, 1, n, dtype=np.float32)
    mid = a[-n:] * (1 - fade) + b[:n] * fade
    return np.concatenate([a[:-n], mid, b[n:]]).astype(np.float32)


def add_energy(audio: np.ndarray, voice: Mapping, curve, base: float | None = None):
    """Same contract as energy.add_energy (flat: audio; hype: (audio, gain_db))."""
    from .energy import build_fn, hype_setting

    audio = np.asarray(audio, dtype=np.float32)
    times, levels = zip(*curve)
    peak = max(levels)
    if peak <= 0:
        return audio if base is None else (audio, None)
    pitch = hype_setting(voice, "pitch")
    if base is None:
        # flat line: overall raise by the energy's pitch
        return shift(audio, pitch * float(levels[0]))

    build = build_fn(curve, base)
    duration = len(audio) / SAMPLE_RATE
    grid = np.linspace(0, duration, 400)
    built = np.array([build(t) for t in grid])
    after = np.nonzero(built >= 0.5)[0]
    split_t = float(grid[after[0]]) if len(after) else duration
    split = int(split_t * SAMPLE_RATE)
    lead_semis = pitch * base
    # The PSOLA path measures the lead-in and punchline pitch and lands the punchline `lift`
    # above the lead-in, softly capped `ceiling` semitones above the voice's top notes. Without
    # a pitch track, assume both sit at the same level; the top notes are typically ~4 semitones
    # above the median, hence the cap.
    lift = min(hype_setting(voice, "lift") * peak, hype_setting(voice, "max_lift"),
               hype_setting(voice, "ceiling") + 4)
    stretch = 1 + (hype_setting(voice, "stretch") - 1) * peak

    lead = shift(audio[:split], lead_semis) if split > 0 else np.zeros(0, np.float32)
    punch = shift(audio[split:], lead_semis + lift, 1 / stretch) if split < len(audio) else np.zeros(0, np.float32)
    out = _join(lead, punch)

    # loudness swell on the new timeline (punchline stretched by `stretch`)
    boost = hype_setting(voice, "boost") * peak
    idx = np.arange(len(out)) / SAMPLE_RATE
    lead_len = len(lead) / SAMPLE_RATE
    orig_t = np.where(idx < lead_len, idx, split_t + (idx - lead_len) / stretch)
    gain_db = boost * np.interp(orig_t, grid, built)
    return out, gain_db
