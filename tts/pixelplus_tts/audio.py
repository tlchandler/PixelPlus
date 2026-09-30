"""ffmpeg processing (radio chain, loudness, encoding, music bed) on in-memory audio.

All intermediate audio is float32 numpy piped through ffmpeg (no temp files).
Output is 44.1 kHz stereo, like fpp-voices.
"""
from __future__ import annotations

import functools
import json
import re
import shutil
import subprocess
from typing import Sequence

import numpy as np

OUT_RATE = 44100
LEAD_IN = "adelay=250|250"  # so the player never clips the first word
TAIL_S = 0.6


@functools.lru_cache(maxsize=1)
def ffmpeg() -> str:
    """System ffmpeg (apt, hardware-tuned on Pi OS) first, else imageio-ffmpeg's bundle."""
    exe = shutil.which("ffmpeg")
    if exe:
        return exe
    try:
        import imageio_ffmpeg
        return imageio_ffmpeg.get_ffmpeg_exe()
    except (ImportError, RuntimeError):
        raise RuntimeError("ffmpeg not found: apt install ffmpeg (or pip install imageio-ffmpeg)") from None


def _run(args: Sequence[str], data: bytes | None) -> subprocess.CompletedProcess:
    extra = ["-nostdin"] if data is None else []
    r = subprocess.run([ffmpeg(), "-hide_banner", "-nostats", *extra, *args],
                       input=data, capture_output=True)
    if r.returncode != 0:
        raise RuntimeError("ffmpeg failed: " + r.stderr.decode(errors="replace")[-600:])
    return r


def _in_args(rate: int, channels: int) -> list[str]:
    return ["-f", "f32le", "-ar", str(rate), "-ac", str(channels), "-i", "pipe:0"]


def as_2d(a: np.ndarray) -> np.ndarray:
    return a[:, None] if a.ndim == 1 else a


def process(audio: np.ndarray, rate: int, filters: str, out_rate: int = OUT_RATE, out_channels: int = 2) -> np.ndarray:
    """Run float audio through an ffmpeg filter chain, return float32 (n, out_channels)."""
    a = as_2d(np.ascontiguousarray(audio, dtype=np.float32))
    r = _run([*_in_args(rate, a.shape[1]), "-af", filters or "anull", "-ar", str(out_rate),
              "-ac", str(out_channels), "-f", "f32le", "pipe:1"], a.tobytes())
    return np.frombuffer(r.stdout, dtype=np.float32).reshape(-1, out_channels).copy()


def radio_fx(eq: str | None, energy: float = 0.0) -> str:
    """Broadcast-style chain from fpp-voices: rumble filter, voice EQ, presence for
    energetic lines, de-esser, compressor."""
    return ",".join(filter(None, [
        "highpass=f=75",
        eq,
        f"equalizer=f=3500:t=q:w=1.5:g={1.5 * energy:.1f}" if energy > 0 else None,
        "deesser=i=0.4",
        "acompressor=threshold=-21dB:ratio=3.5:attack=4:release=90:makeup=3",
    ]))


def measure(audio: np.ndarray, rate: int = OUT_RATE) -> dict[str, float] | None:
    """EBU R128 integrated loudness etc. via loudnorm's analysis pass.
    -> {input_i, input_tp, input_lra, input_thresh} or None for silence."""
    a = as_2d(np.ascontiguousarray(audio, dtype=np.float32))
    if not len(a):
        return None
    r = _run([*_in_args(rate, a.shape[1]), "-af", "loudnorm=print_format=json", "-f", "null", "-"], a.tobytes())
    m = re.search(r"\{[^{}]*\"input_i\"[^{}]*\}", r.stderr.decode(errors="replace"))
    if not m:
        return None
    try:
        vals = {k: float(v) for k, v in json.loads(m.group(0)).items() if k.startswith("input_")}
    except ValueError:
        return None
    return None if vals.get("input_i", -99) < -70 else vals


def level_filter(audio: np.ndarray, target_lufs: float, rate: int = OUT_RATE) -> str:
    """Linear loudness normalization: gain to target + true-peak-safe limiter
    (-1.5 dBFS). Linear (not loudnorm's dynamic mode) so the hype swell survives."""
    m = measure(audio, rate)
    gain = 0.0 if m is None else target_lufs - m["input_i"]
    return f"volume={gain:.2f}dB,alimiter=limit=0.84:attack=2:release=50:level=false"


def encode(audio: np.ndarray, fmt: str, rate: int = OUT_RATE, filters: str = "anull") -> bytes:
    codec = {"mp3": ["-c:a", "libmp3lame", "-b:a", "192k", "-f", "mp3"],
             "ogg": ["-c:a", "libvorbis", "-q:a", "6", "-f", "ogg"],
             "wav": ["-c:a", "pcm_s16le", "-f", "wav"]}[fmt]
    a = as_2d(np.ascontiguousarray(audio, dtype=np.float32))
    r = _run([*_in_args(rate, a.shape[1]), "-af", filters, "-ar", str(OUT_RATE), "-ac", "2",
              "-map_metadata", "-1", *codec, "pipe:1"], a.tobytes())
    return r.stdout


def decode(path: str, rate: int = OUT_RATE) -> np.ndarray:
    r = _run(["-i", path, "-vn", "-ar", str(rate), "-ac", "2", "-f", "f32le", "pipe:1"], None)
    return np.frombuffer(r.stdout, dtype=np.float32).reshape(-1, 2).copy()


def silence(seconds: float, rate: int = OUT_RATE) -> np.ndarray:
    return np.zeros((max(0, int(round(seconds * rate))), 2), dtype=np.float32)


def apply_gain_envelope(clip: np.ndarray, gain_db: np.ndarray) -> np.ndarray:
    """Stretch a per-sample dB envelope over the clip and apply it (fpp-voices)."""
    env = np.interp(np.linspace(0, 1, len(clip)), np.linspace(0, 1, len(gain_db)), gain_db)
    return (clip * (10 ** (env / 20))[:, None]).astype(np.float32)


# ------------------------------------------------------------- music bed ----

def speech_activity(speech: np.ndarray, rate: int = OUT_RATE, win_s: float = 0.02,
                    threshold_db: float = -45.0) -> np.ndarray:
    """Per-window 0/1 speech activity from RMS."""
    mono = as_2d(speech).mean(axis=1)
    win = max(1, int(rate * win_s))
    n = len(mono) // win
    if n == 0:
        return np.zeros(0)
    rms = np.sqrt(np.mean(mono[: n * win].reshape(n, win) ** 2, axis=1) + 1e-12)
    return (20 * np.log10(rms) > threshold_db).astype(np.float64)


def duck_envelope(active: np.ndarray, duck_db: float, win_s: float = 0.02,
                  attack_s: float = 0.15, release_s: float = 0.6, hold_s: float = 0.35) -> np.ndarray:
    """Per-window bed gain in dB: 0 when no speech, -duck_db under speech, with
    look-ahead attack (the bed dips *before* the first word), hold across short
    gaps between words, and a slow release."""
    n = len(active)
    if n == 0:
        return np.zeros(0)
    hold = int(round(hold_s / win_s))
    target = active.copy()
    # hold: fill gaps shorter than hold_s
    idx = np.flatnonzero(active)
    for a, b in zip(idx[:-1], idx[1:]):
        if 1 < b - a <= hold:
            target[a:b] = 1
    # look-ahead: start ducking attack_s before speech
    look = int(round(attack_s / win_s))
    starts = np.flatnonzero(np.diff(np.concatenate([[0], target])) > 0)
    for s in starts:
        target[max(0, s - look):s] = np.maximum(target[max(0, s - look):s],
                                                np.linspace(0, 1, s - max(0, s - look) + 1)[:-1])
    # release: one-pole smoothing on the way back up
    rel = np.exp(-win_s / max(release_s, 1e-3))
    out = np.empty(n)
    g = 0.0
    for i, t in enumerate(target):
        g = t if t >= g else t + (g - t) * rel
        out[i] = g
    return -duck_db * out


def mix_music_bed(speech: np.ndarray, bed: np.ndarray, duck_db: float = 12.0, bed_gain_db: float = -6.0,
                  target_lufs: float = -16.0, intro_s: float = 1.5, outro_s: float = 2.5,
                  rate: int = OUT_RATE) -> np.ndarray:
    """Speech over a (looped) music bed: bed at target+bed_gain_db LUFS, ducked by
    duck_db under speech, intro before the first word and a faded outro."""
    speech = as_2d(speech)
    if speech.shape[1] == 1:
        speech = np.repeat(speech, 2, axis=1)
    lead = int(intro_s * rate)
    total = lead + len(speech) + int(outro_s * rate)
    if len(bed) == 0:
        raise ValueError("music bed is empty")
    reps = int(np.ceil(total / len(bed)))
    bed = np.tile(bed, (reps, 1))[:total]
    m = measure(bed, rate)
    if m is not None:
        bed = bed * 10 ** ((target_lufs + bed_gain_db - m["input_i"]) / 20)
    voice = np.concatenate([np.zeros((lead, 2), np.float32), speech, np.zeros((total - lead - len(speech), 2), np.float32)])
    win_s = 0.02
    act = speech_activity(voice, rate, win_s)
    env_db = duck_envelope(act, duck_db, win_s)
    win = int(rate * win_s)
    t_env = (np.arange(len(env_db)) + 0.5) * win
    gain = 10 ** (np.interp(np.arange(total), t_env, env_db) / 20)
    fade = np.ones(total)
    fi = min(int(0.3 * rate), total)
    fo = min(int(outro_s * rate), total)
    fade[:fi] = np.linspace(0, 1, fi)
    if fo:
        fade[total - fo:] = np.minimum(fade[total - fo:], np.linspace(1, 0, fo))
    return (voice + bed * (gain * fade)[:, None]).astype(np.float32)
