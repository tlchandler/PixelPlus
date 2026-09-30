"""Voice catalog (all Kokoro v1.0 base voices), DjVoice presets and blending.

A *voice* in a request is either a Kokoro base voice id ("af_heart") or a DjVoice
object (camelCase, as in pixelplus-core's model.rs; the snake_case fpp-voices
voices.json form is accepted too):

    {id, name, description, blend: {am_echo: .3, am_fenrir: .3, am_puck: .4},
     speed, lang, eq?, defaultEnergy, energy: {pitch, range, speed, stretch,
     boost, lift, ceiling, maxLift}}
"""
from __future__ import annotations

import json
import os
import re
from importlib import resources
from typing import Any, Mapping

import numpy as np

# Grades are the overall grades published by hexgrad (Kokoro-82M VOICES.md) for
# English voices; other languages are best-effort in Kokoro and left ungraded.
_EN = {
    # id: (name, grade)
    "af_heart": ("Heart", "A"), "af_alloy": ("Alloy", "C"), "af_aoede": ("Aoede", "C+"),
    "af_bella": ("Bella", "A-"), "af_jessica": ("Jessica", "D"), "af_kore": ("Kore", "C+"),
    "af_nicole": ("Nicole", "B-"), "af_nova": ("Nova", "C"), "af_river": ("River", "D"),
    "af_sarah": ("Sarah", "C+"), "af_sky": ("Sky", "C-"),
    "am_adam": ("Adam", "F+"), "am_echo": ("Echo", "D"), "am_eric": ("Eric", "D"),
    "am_fenrir": ("Fenrir", "C+"), "am_liam": ("Liam", "D"), "am_michael": ("Michael", "C+"),
    "am_onyx": ("Onyx", "D"), "am_puck": ("Puck", "C+"), "am_santa": ("Santa", "D-"),
    "bf_alice": ("Alice", "D"), "bf_emma": ("Emma", "B-"), "bf_isabella": ("Isabella", "C"),
    "bf_lily": ("Lily", "D"), "bm_daniel": ("Daniel", "D"), "bm_fable": ("Fable", "C"),
    "bm_george": ("George", "C"), "bm_lewis": ("Lewis", "D+"),
}
_OTHER = [
    "ef_dora", "em_alex", "em_santa", "ff_siwis", "hf_alpha", "hf_beta", "hm_omega", "hm_psi",
    "if_sara", "im_nicola", "jf_alpha", "jf_gongitsune", "jf_nezumi", "jf_tebukuro", "jm_kumo",
    "pf_dora", "pm_alex", "pm_santa", "zf_xiaobei", "zf_xiaoni", "zf_xiaoxiao", "zf_xiaoyi",
    "zm_yunjian", "zm_yunxi", "zm_yunxia", "zm_yunyang",
]
# First letter of a Kokoro voice id -> espeak language code used by kokoro-onnx.
LANG_BY_PREFIX = {"a": "en-us", "b": "en-gb", "e": "es", "f": "fr-fr", "h": "hi", "i": "it",
                  "j": "ja", "p": "pt-br", "z": "cmn"}

# Neutral-but-lively hype tuning for base voices and custom blends that don't set
# their own (between the tested Nick and Holly settings).
DEFAULT_ENERGY = {"pitch": 1.5, "range": 1.5, "speed": 1.0, "stretch": 1.0, "boost": 3.0,
                  "lift": 2.5, "ceiling": 3.0, "max_lift": 8.0}
# What fpp-voices used when a key is missing from a voice's energy block.
FALLBACK_ENERGY = {"pitch": 0, "range": 1, "speed": 1, "stretch": 1, "boost": 0, "lift": 0,
                   "ceiling": 3, "max_lift": 12}
ENERGY_KEYS = tuple(FALLBACK_ENERGY)
_SAFE_ID = re.compile(r"^[a-z]{2}_[a-z]+$")


def base_voice_info(vid: str) -> dict[str, Any]:
    name, grade = _EN.get(vid, (vid.split("_", 1)[-1].capitalize(), None))
    info: dict[str, Any] = {
        "id": vid, "name": name, "language": LANG_BY_PREFIX.get(vid[0], "en-us"),
        "gender": "female" if vid[1:2] == "f" else "male",
    }
    if grade:
        info["grade"] = grade
    return info


BASE_VOICE_IDS: tuple[str, ...] = tuple(list(_EN) + _OTHER)


def list_base_voices(available: set[str] | None = None) -> list[dict[str, Any]]:
    """All Kokoro v1.0 base voices; `available` (keys of voices-v1.0.bin) adds any
    ids this catalog doesn't know about."""
    ids = list(BASE_VOICE_IDS)
    if available:
        ids += sorted(v for v in available if v not in BASE_VOICE_IDS and _SAFE_ID.match(v))
    return [base_voice_info(v) for v in ids]


def voices_in_file(voices_bin: str) -> set[str]:
    """Voice ids in voices-v1.0.bin (an .npz) without loading the arrays."""
    import zipfile
    try:
        with zipfile.ZipFile(voices_bin) as z:
            return {n[:-4] for n in z.namelist() if n.endswith(".npy")}
    except (OSError, zipfile.BadZipFile):
        return set()


# ---------------------------------------------------------------- DjVoice ----

def _camel_to_snake(key: str) -> str:
    return re.sub(r"(?<!^)([A-Z])", r"_\1", key).lower()


def normalize_voice(v: Mapping[str, Any], vid: str | None = None) -> dict[str, Any]:
    """Accept camelCase (DjVoice) or snake_case (voices.json) and return the internal
    form: {id, name, description, blend (weights normalized to 1), speed, lang, eq,
    default_energy, energy (snake_case keys, floats)}."""
    if not isinstance(v, Mapping):
        raise ValueError("voice must be a base voice id or a DjVoice object")
    vid = str(v.get("id") or vid or "custom")
    blend_in = v.get("blend")
    if not isinstance(blend_in, Mapping) or not blend_in:
        raise ValueError(f"voice {vid!r}: blend must be a non-empty object of voiceId -> weight")
    blend = normalize_blend(blend_in)
    energy_in = v.get("energy") or {}
    if not isinstance(energy_in, Mapping):
        raise ValueError(f"voice {vid!r}: energy must be an object")
    energy = dict(DEFAULT_ENERGY) if not energy_in else {}
    for k, val in energy_in.items():
        key = _camel_to_snake(str(k))
        if key in ENERGY_KEYS:
            energy[key] = float(val)
    de = v.get("defaultEnergy", v.get("default_energy", 0.4))
    first = next(iter(blend))
    return {
        "id": vid,
        "name": str(v.get("name") or vid.capitalize()),
        "description": str(v.get("description") or ""),
        "blend": blend,
        "speed": float(v.get("speed") or 1.0),
        "lang": str(v.get("lang") or LANG_BY_PREFIX.get(first[0], "en-us")),
        "eq": v.get("eq") or None,
        "default_energy": float(0.4 if de is None else de),
        "energy": energy,
    }


def normalize_blend(blend: Mapping[str, Any]) -> dict[str, float]:
    """Drop non-positive weights and scale the rest to sum to 1."""
    out: dict[str, float] = {}
    for k, w in blend.items():
        try:
            w = float(w)
        except (TypeError, ValueError):
            raise ValueError(f"blend weight for {k!r} must be a number") from None
        if w > 0 and np.isfinite(w):
            if not _SAFE_ID.match(str(k)):
                raise ValueError(f"unknown base voice {k!r}")
            out[str(k)] = out.get(str(k), 0.0) + w
    total = sum(out.values())
    if total <= 0:
        raise ValueError("blend needs at least one voice with a positive weight")
    return {k: w / total for k, w in out.items()}


def base_as_voice(vid: str) -> dict[str, Any]:
    info = base_voice_info(vid)
    return normalize_voice({"id": vid, "name": info["name"], "blend": {vid: 1.0},
                            "speed": 1.0, "lang": info["language"]})


def blend_style(blend: Mapping[str, float], get_style) -> np.ndarray:
    """Weighted average of Kokoro style tensors (each (510, 1, 256)). Weights are
    renormalized, so {a: 3, b: 1} == {a: .75, b: .25}."""
    weights = normalize_blend(blend)
    style = None
    for vid, w in weights.items():
        s = np.asarray(get_style(vid), dtype=np.float32) * np.float32(w)
        style = s if style is None else style + s
    return style


def to_dj_voice(v: Mapping[str, Any]) -> dict[str, Any]:
    """Internal form -> DjVoice JSON (camelCase)."""
    e = {("maxLift" if k == "max_lift" else k): val for k, val in v["energy"].items()}
    out = {"id": v["id"], "name": v["name"], "description": v["description"],
           "blend": v["blend"], "speed": v["speed"], "lang": v["lang"],
           "defaultEnergy": v["default_energy"], "energy": e}
    if v.get("eq"):
        out["eq"] = v["eq"]
    return out


def _load_presets_raw() -> dict[str, Any]:
    with resources.files("pixelplus_tts").joinpath("data/voices.json").open(encoding="utf-8") as f:
        return json.load(f)


def load_presets() -> dict[str, dict[str, Any]]:
    """Nick and Holly (from fpp-voices voices.json), keyed by id."""
    out = {}
    for vid, raw in _load_presets_raw().items():
        v = normalize_voice({**raw, "id": vid, "name": vid.capitalize()})
        v["aliases"] = [a.lower() for a in raw.get("aliases", [])]
        out[vid] = v
    return out


def resolve_voice(ref: Any, custom: Mapping[str, Mapping[str, Any]] | None = None,
                  known_base: set[str] | None = None) -> dict[str, Any]:
    """Voice reference (object, preset id/alias/name, custom voice id, or base id) ->
    internal voice dict."""
    if isinstance(ref, Mapping):
        return normalize_voice(ref)
    if not isinstance(ref, str) or not ref.strip():
        raise ValueError("voice is required")
    key = ref.strip()
    low = key.lower()
    for vid, v in (custom or {}).items():
        if low in (vid.lower(), str(v.get("name", "")).lower()):
            return normalize_voice(v, vid)
    for vid, v in load_presets().items():
        if low == vid or low in v["aliases"] or low == v["name"].lower():
            return v
    bases = known_base or set(BASE_VOICE_IDS)
    if low in bases:
        return base_as_voice(low)
    raise ValueError(f"unknown voice {ref!r} (use nick, holly, a custom voice or a Kokoro id like af_heart)")


# ffmpeg filters a voice's `eq` may use (it's user editable, so keep it to audio EQ).
_EQ_FILTERS = {"equalizer", "bass", "treble", "highpass", "lowpass", "highshelf", "lowshelf",
               "bandpass", "bandreject", "volume", "acompressor", "deesser", "aecho"}
_EQ_PART = re.compile(r"^([a-z]+)(=[A-Za-z0-9_.:=\-]*)?$")


def validate_eq(eq: str | None) -> str | None:
    if not eq:
        return None
    parts = [p.strip() for p in str(eq).split(",") if p.strip()]
    for p in parts:
        m = _EQ_PART.match(p)
        if not m or m.group(1) not in _EQ_FILTERS:
            raise ValueError(f"eq filter {p!r} not allowed (allowed: {', '.join(sorted(_EQ_FILTERS))})")
    return ",".join(parts)


def voice_key(v: Mapping[str, Any]) -> str:
    """Stable JSON of the parts of a voice that affect audio (for cache keys)."""
    return json.dumps({k: v.get(k) for k in ("blend", "speed", "lang", "eq", "default_energy", "energy")},
                      sort_keys=True)


def presets_as_dj_voices() -> list[dict[str, Any]]:
    return [to_dj_voice(v) for v in load_presets().values()]


def data_path(name: str) -> str:
    return os.fspath(resources.files("pixelplus_tts").joinpath("data", name))
