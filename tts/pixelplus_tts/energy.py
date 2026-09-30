"""Energy / hype prosody, ported from fpp-voices (dj_voice.py) unchanged in behavior.

Energy levels: 0 calm, 0.4 normal DJ (default), 1 hype, 1.5 extra hype.

Hype lines *build to the end*: the lead-in stays at the voice's normal DJ level,
then the punchline (text in *asterisks*, else the last phrase) is lifted a few
semitones above the lead-in (speech naturally drifts down), its pitch swings
widen, it swells ~2.5 dB louder and (Holly) slows slightly. A soft pitch ceiling
keeps short punchlines ("…everybody!") excited rather than shrieking.

The pure helpers (find_emphasis, plan_line, energy_curve, punch_lift,
soft_ceiling, shape_pitch) are unit tested; add_energy() runs Praat PSOLA via
praat-parselmouth when it is installed (optional: it has no Linux aarch64 wheels), else the
simpler segment-wise method in prosody.py.
"""
from __future__ import annotations

import re
from typing import Callable, Mapping, Sequence

import numpy as np

from .voices import FALLBACK_ENERGY

SAMPLE_RATE = 24000
Curve = list[tuple[float, float]]


def hype_setting(voice: Mapping, key: str) -> float:
    return float(voice.get("energy", {}).get(key, FALLBACK_ENERGY[key]))


def find_emphasis(text: str) -> tuple[str, str, str]:
    """Split a hype line into (lead-in, punchline, tail). Words wrapped in *asterisks*
    are the punchline; otherwise it's the last phrase: "Sit back, relax, *and enjoy the show!*"."""
    m = re.search(r"\*([^*]+)\*", text)
    if m:
        return (text[:m.start()].replace("*", ""), m.group(1), text[m.end():].replace("*", ""))
    sentences = re.split(r"(?<=[.!?])\s+", text.strip())
    lead = " ".join(sentences[:-1])
    last = sentences[-1]
    clauses = re.split(r"(?<=[,;:—])\s+", last)
    punch = clauses[-1]
    lead = " ".join(filter(None, [lead] + clauses[:-1]))
    words = punch.split()
    if not lead and len(words) > 2:
        # one phrase with no commas: build into the second half of it
        half = len(words) // 2
        lead, punch = " ".join(words[:half]), " ".join(words[half:])
    return lead, punch, ""


def plan_line(voice: Mapping, energy: float, speed: float | None = None) -> tuple[float, float, bool]:
    """-> (base energy, synthesis speed, is_hype). `speed` overrides the voice's own
    speed (fpp-voices --speed). Hype = energy above the voice's default level."""
    base = min(energy, float(voice.get("default_energy", 0)))
    spd = (speed or float(voice.get("speed", 1.0))) * (1 + (hype_setting(voice, "speed") - 1) * base)
    return base, spd, energy > base


def energy_curve(phonemes: Sequence[str], duration: float, base: float, energy: float) -> Curve:
    """Energy over time for a hype line: normal level through the lead-in, build into
    the punchline (placed by phoneme count), ease back for any tail."""
    total = len(" ".join(filter(None, phonemes)))
    t0 = duration * len(phonemes[0]) / total
    t1 = duration * (len(phonemes[0]) + len(phonemes[1]) + 1) / total if phonemes[2] else duration
    curve = [(0.0, base), (max(t0 - 0.25, 0.0), base), (t0 + 0.2, energy), (t1, energy)]
    if phonemes[2]:
        curve.append((min(t1 + 0.25, duration), base))
    return [p for n, p in enumerate(curve) if n == 0 or p[0] > curve[n - 1][0]]


def punch_lift(lead_semis: float | None, punch_semis: float | None, voice: Mapping, peak: float) -> float:
    """Semitones to raise the punchline so it lands `lift*peak` above the lead-in,
    never negative and capped at max_lift."""
    if lead_semis is None or punch_semis is None:
        return 0.0
    target = hype_setting(voice, "lift") * peak
    return float(min(max(0.0, lead_semis - punch_semis + target), hype_setting(voice, "max_lift")))


def soft_ceiling(f: float, ceiling: float) -> float:
    """Ease pitches above the ceiling back towards it (4th-root compression)."""
    return ceiling * (f / ceiling) ** 0.25 if f > ceiling else f


def shape_pitch(f: float, mean: float, e: float, voice: Mapping, base: float | None,
                lift_semis: float, ceiling: float) -> float:
    """New pitch for one PitchTier point: overall raise, widened swings, punchline
    lift, soft ceiling."""
    shift = 2 ** ((hype_setting(voice, "pitch") * min(e, base if base is not None else e) + lift_semis) / 12)
    widen = 1 + (hype_setting(voice, "range") - 1) * e
    return soft_ceiling(mean * shift * (f / mean) ** widen, ceiling)


def build_fn(curve: Curve, base: float) -> Callable[[float], float]:
    times, levels = zip(*curve)
    peak = max(levels)
    return lambda t: (float(np.interp(t, times, levels)) - base) / (peak - base)


def add_energy(audio: np.ndarray, voice: Mapping, curve: Curve, base: float | None = None):
    """Praat PSOLA: lift the pitch, widen its swings and (hype lines) lift + stretch
    the punchline. Flat (base None) returns audio; hype returns (audio, gain_db)."""
    times, levels = zip(*curve)
    if max(levels) <= 0:
        return audio if base is None else (audio, None)
    from . import prosody
    if not prosody.have_parselmouth():
        # No Praat (no aarch64 wheels): segment-wise pitch/tempo/loudness instead.
        return prosody.add_energy(audio, voice, curve, base)
    import parselmouth
    from parselmouth.praat import call
    energy_at = lambda t: float(np.interp(t, times, levels))  # noqa: E731
    snd = parselmouth.Sound(audio.astype(np.float64), SAMPLE_RATE)
    manip = call(snd, "To Manipulation", 0.01, 60, 500)
    tier = call(manip, "Extract pitch tier")
    mean = call(tier, "Get mean (curve)", 0, 0)
    if not mean or np.isnan(mean):
        return audio if base is None else (audio, None)
    points = [(call(tier, "Get time from index", i), call(tier, "Get value at index", i))
              for i in range(1, call(tier, "Get number of points") + 1)]
    lift = lambda t: 0.0  # noqa: E731
    peak = max(levels)
    if base is not None:
        build = build_fn(curve, base)
        semis = lambda pts: float(np.median([12 * np.log2(f) for _, f in pts])) if pts else None  # noqa: E731
        lead = semis([p for p in points if build(p[0]) == 0])
        punch = semis([p for p in points if build(p[0]) == 1])
        reset = punch_lift(lead, punch, voice, peak)
        lift = lambda t: reset * build(t)  # noqa: E731
    ceiling = np.percentile([f for _, f in points], 95) * 2 ** (hype_setting(voice, "ceiling") / 12)
    new_tier = call("Create PitchTier", "hype", 0, snd.duration)
    for t, f in points:
        call(new_tier, "Add point", t, shape_pitch(f, mean, energy_at(t), voice, base, lift(t), ceiling))
    call([new_tier, manip], "Replace pitch tier")
    if base is None:
        return call(manip, "Get resynthesis (overlap-add)").values[0].astype(np.float32)

    # Slow the punchline down a touch and swell its loudness.
    stretch = 1 + (hype_setting(voice, "stretch") - 1) * peak
    dtier = call("Create DurationTier", "hype", 0, snd.duration)
    for t in times:
        call(dtier, "Add point", t, 1 + (stretch - 1) * build(t))
    call([dtier, manip], "Replace duration tier")
    out = call(manip, "Get resynthesis (overlap-add)").values[0].astype(np.float32)
    # map the loudness build onto the new (stretched) timeline
    grid = np.linspace(0, snd.duration, 2000)
    new_grid = np.concatenate([[0], np.cumsum(np.diff(grid) * (1 + (stretch - 1) * np.array([build(t) for t in grid[1:]])))])
    boost = hype_setting(voice, "boost") * peak
    gain_db = np.interp(np.arange(len(out)) / SAMPLE_RATE, new_grid, [boost * build(t) for t in grid])
    return out, gain_db
