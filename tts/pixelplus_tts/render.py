"""Dialog rendering: request validation, the fpp-voices per-line chain, joining,
pauses, loudness, optional music bed, encoding and an on-disk result cache."""
from __future__ import annotations

import hashlib
import json
import os
import re
import threading
import time
from dataclasses import dataclass, field
from typing import Any, Mapping

import numpy as np

from . import __version__
from . import audio as au
from .engine import SAMPLE_RATE, Engine
from .pronounce import builtin_pronunciations, compile_rules, merge
from .voices import resolve_voice, validate_eq, voice_key

DEFAULT_GAP_MS = 350  # between lines when a line's pauseMs is 0 (fpp-voices GAP_SECONDS)
DEFAULT_LUFS = -16.0
FORMATS = {"mp3": "audio/mpeg", "wav": "audio/wav", "ogg": "audio/ogg"}
MAX_LINES = 200
MAX_TEXT = 2000
MAX_TOTAL_TEXT = 12000  # a clip of ~12 minutes of speech; keeps one request from holding the renderer for hours
MAX_PLACEHOLDER = 300   # characters per placeholder value
PLACEHOLDER_RE = re.compile(r"\{([A-Za-z][A-Za-z0-9_]*)\}")


class BadRequest(ValueError):
    def __init__(self, message: str, code: str = "bad_request"):
        super().__init__(message)
        self.code = code


@dataclass
class RenderResult:
    data: bytes
    content_type: str
    duration_ms: int
    loudness_lufs: float | None
    render_ms: int
    cached: bool = False
    warnings: list[str] = field(default_factory=list)


# ------------------------------------------------------------------ cache ----

class Cache:
    def __init__(self, directory: str, max_mb: int = 200):
        self.dir = directory
        self.max_bytes = max_mb * 1024 * 1024
        self._lock = threading.Lock()

    def _path(self, key: str, ext: str) -> str:
        return os.path.join(self.dir, key[:2], f"{key}.{ext}")

    def get(self, key: str, ext: str) -> tuple[bytes, dict] | None:
        p = self._path(key, ext)
        try:
            with open(p, "rb") as f:
                data = f.read()
            with open(p + ".json", encoding="utf-8") as f:
                meta = json.load(f)
            os.utime(p)  # LRU
            return data, meta
        except (OSError, ValueError):
            return None

    def put(self, key: str, ext: str, data: bytes, meta: dict) -> None:
        if self.max_bytes <= 0:
            return
        p = self._path(key, ext)
        try:
            os.makedirs(os.path.dirname(p), exist_ok=True)
            tmp = f"{p}.{os.getpid()}.tmp"
            with open(tmp, "wb") as f:
                f.write(data)
            with open(p + ".json", "w", encoding="utf-8") as f:
                json.dump(meta, f)
            os.replace(tmp, p)
        except OSError:
            return
        self.prune()

    def prune(self) -> None:
        with self._lock:
            files = []
            now = time.time()
            for root, _, names in os.walk(self.dir):
                for n in names:
                    if n.endswith(".tmp"):  # left by a render that was killed mid-write
                        p = os.path.join(root, n)
                        try:
                            if now - os.stat(p).st_mtime > 3600:
                                os.remove(p)
                        except OSError:
                            pass
                        continue
                    if n.endswith((".mp3", ".wav", ".ogg")):
                        p = os.path.join(root, n)
                        try:
                            st = os.stat(p)
                        except OSError:
                            continue
                        files.append((st.st_mtime, st.st_size, p))
            total = sum(s for _, s, _ in files)
            for _, size, p in sorted(files):
                if total <= self.max_bytes:
                    break
                for q in (p, p + ".json"):
                    try:
                        os.remove(q)
                    except OSError:
                        pass
                total -= size


# ------------------------------------------------------------- validation ----

def substitute_placeholders(text: str, values: Mapping[str, Any]) -> tuple[str, list[str]]:
    """{nextSong} -> values["nextSong"]. Unresolved names are spoken without braces."""
    missing: list[str] = []

    def rep(m: re.Match) -> str:
        name = m.group(1)
        v = values.get(name)
        if isinstance(v, bool):
            v = "yes" if v else "no"
        if isinstance(v, (str, int, float)):
            return str(v)[:MAX_PLACEHOLDER]
        missing.append(name)
        return re.sub(r"(?<=[a-z])(?=[A-Z])", " ", name)

    return PLACEHOLDER_RE.sub(rep, text), missing


def _num(v: Any, name: str, lo: float, hi: float, default: float) -> float:
    if v is None:
        return default
    try:
        f = float(v)
    except (TypeError, ValueError):
        raise BadRequest(f"{name} must be a number") from None
    if not (lo <= f <= hi):
        raise BadRequest(f"{name} must be between {lo} and {hi}")
    return f


@dataclass
class Job:
    lines: list[dict[str, Any]]  # {voice(internal), text, pause_ms, energy}
    speed: float
    rules_pairs: list[tuple[str, str]]
    fmt: str
    lufs: float
    fx: bool
    bed: dict[str, Any] | None
    warnings: list[str]

    def cache_key(self, variant: str) -> str:
        blob = json.dumps({
            "v": __version__, "model": variant, "speed": self.speed, "fmt": self.fmt,
            "lufs": self.lufs, "fx": self.fx, "bed": self.bed, "rules": sorted(self.rules_pairs),
            "lines": [[voice_key(ln["voice"]), ln["text"], ln["pause_ms"], ln["energy"]] for ln in self.lines],
        }, sort_keys=True, ensure_ascii=False)
        return hashlib.sha256(blob.encode()).hexdigest()


def build_job(req: Mapping[str, Any], *, known_base: set[str] | None = None, data_dir: str = "/var/lib/pixelplus",
              allow_any_path: bool = False) -> Job:
    if not isinstance(req, Mapping):
        raise BadRequest("body must be a JSON object")
    raw_lines = req.get("lines")
    if not isinstance(raw_lines, list) or not raw_lines:
        raise BadRequest("lines must be a non-empty array")
    if len(raw_lines) > MAX_LINES:
        raise BadRequest(f"at most {MAX_LINES} lines")
    custom: dict[str, Mapping[str, Any]] = {}
    for v in req.get("voices") or []:
        if isinstance(v, Mapping) and v.get("id"):
            custom[str(v["id"])] = v
    placeholders = req.get("placeholders") or {}
    if not isinstance(placeholders, Mapping):
        raise BadRequest("placeholders must be an object")
    warnings: list[str] = []
    lines = []
    total_text = 0
    for i, ln in enumerate(raw_lines):
        if not isinstance(ln, Mapping):
            raise BadRequest(f"lines[{i}] must be an object")
        text = ln.get("text")
        text = text.strip() if isinstance(text, str) else ""
        text, missing = substitute_placeholders(text, placeholders)
        text = text.strip()
        # checked after the placeholders are filled in: their values count too
        if len(text) > MAX_TEXT:
            raise BadRequest(f"lines[{i}].text is longer than {MAX_TEXT} characters")
        total_text += len(text)
        if total_text > MAX_TOTAL_TEXT:
            raise BadRequest(f"the clip is longer than {MAX_TOTAL_TEXT} characters in all", "too_large")
        for name in missing:
            warnings.append(f"lines[{i}]: placeholder {{{name}}} has no value")
        pause = int(_num(ln.get("pauseMs"), f"lines[{i}].pauseMs", 0, 60000, 0))
        voice = None
        if text:
            try:
                voice = resolve_voice(ln.get("voice"), custom, known_base)
                voice["eq"] = validate_eq(voice.get("eq"))
            except ValueError as e:
                raise BadRequest(f"lines[{i}].voice: {e}", "unknown_voice") from None
            if known_base:
                missing_v = [b for b in voice["blend"] if b not in known_base]
                if missing_v:
                    raise BadRequest(f"lines[{i}].voice: unknown Kokoro voice(s) {', '.join(missing_v)}", "unknown_voice")
        e = ln.get("energy")
        energy = None if e is None else _num(e, f"lines[{i}].energy", 0, 2, 0.4)
        if text or pause:
            lines.append({"voice": voice, "text": text, "pause_ms": pause, "energy": energy})
    if not any(ln["text"] for ln in lines):
        raise BadRequest("nothing to say")
    fmt = str(req.get("format") or "mp3").lower()
    if fmt not in FORMATS:
        raise BadRequest(f"format must be one of {', '.join(FORMATS)}")
    prons = req.get("pronunciations") or []
    if not isinstance(prons, list) or not all(isinstance(p, Mapping) and "word" in p and "say" in p for p in prons):
        raise BadRequest("pronunciations must be an array of {word, say}")
    builtin = builtin_pronunciations() if req.get("builtinPronunciations", True) else []
    bed = None
    mb = req.get("musicBed")
    if mb:
        if not isinstance(mb, Mapping) or not mb.get("path"):
            raise BadRequest("musicBed must be {path, duckDb?, gainDb?, introMs?, outroMs?}")
        path = os.path.realpath(str(mb["path"]))
        root = os.path.realpath(data_dir)
        if not allow_any_path and not path.startswith(root + os.sep):
            raise BadRequest(f"musicBed.path must be under {root}", "forbidden_path")
        if not os.path.isfile(path):
            raise BadRequest(f"musicBed.path not found: {mb['path']}", "not_found")
        st = os.stat(path)
        bed = {"path": path, "mtime": st.st_mtime, "size": st.st_size,
               "duckDb": _num(mb.get("duckDb"), "musicBed.duckDb", 0, 40, 12),
               "gainDb": _num(mb.get("gainDb"), "musicBed.gainDb", -40, 6, -6),
               "introMs": _num(mb.get("introMs"), "musicBed.introMs", 0, 30000, 1500),
               "outroMs": _num(mb.get("outroMs"), "musicBed.outroMs", 0, 30000, 2500)}
    return Job(
        lines=lines,
        speed=_num(req.get("speed"), "speed", 0.5, 2.0, 1.0),
        rules_pairs=merge(builtin, prons),
        fmt=fmt,
        lufs=_num(req.get("loudnessLufs"), "loudnessLufs", -40, -6, DEFAULT_LUFS),
        fx=req.get("fx", True) not in (False, 0, "false", "0", "no", "off"),
        bed=bed,
        warnings=warnings,
    )


# -------------------------------------------------------------- rendering ----

def render_job(engine: Engine, job: Job, progress=None) -> tuple[np.ndarray, list[str]]:
    """-> (44.1 kHz stereo float32 mix before the final lead-in/tail, warnings)."""
    rules = compile_rules(job.rules_pairs)
    pieces: list[np.ndarray] = []
    n = len(job.lines)
    for i, ln in enumerate(job.lines):
        if ln["text"]:
            v = ln["voice"]
            energy = ln["energy"] if ln["energy"] is not None else v.get("default_energy", 0)
            speed = float(v.get("speed", 1.0)) * job.speed
            audio, gain_db = engine.synthesize(ln["text"], v, rules, speed, energy)
            clip = au.process(audio, SAMPLE_RATE, au.radio_fx(v.get("eq"), energy) if job.fx else "anull")
            if job.fx:
                if gain_db is not None:
                    # swell into the punchline (after compression, so it isn't squashed)
                    clip = au.apply_gain_envelope(clip, gain_db)
                # hype lines sit a touch louder than the rest of the banter
                clip = au.process(clip, au.OUT_RATE, au.level_filter(clip, job.lufs + min(energy, 1.5)))
            pieces.append(clip)
        # gap after this line: its pauseMs, else the default gap between lines
        gap = ln["pause_ms"] if ln["pause_ms"] > 0 else (DEFAULT_GAP_MS if i < n - 1 and ln["text"] else 0)
        if gap:
            pieces.append(au.silence(gap / 1000))
        if progress:
            progress((i + 1) / n)
    joined = np.concatenate(pieces) if pieces else au.silence(0.1)
    if job.fx:
        joined = au.process(joined, au.OUT_RATE, au.level_filter(joined, job.lufs))
    if job.bed:
        # decode only what the mix can use (it loops a shorter bed): an hour-long file would
        # otherwise become ~1.3 GB of float samples
        need = len(joined) / au.OUT_RATE + (job.bed["introMs"] + job.bed["outroMs"]) / 1000 + 1
        bed = au.decode(job.bed["path"], max_seconds=need)
        joined = au.mix_music_bed(joined, bed, job.bed["duckDb"], job.bed["gainDb"], job.lufs,
                                  job.bed["introMs"] / 1000, job.bed["outroMs"] / 1000)
        joined = au.process(joined, au.OUT_RATE, au.level_filter(joined, job.lufs))
    return joined, job.warnings


def lookup(cache: Cache | None, job: Job, variant: str) -> RenderResult | None:
    """A cached result for this job, if any (cheap; no model needed)."""
    if not cache:
        return None
    t = time.time()
    hit = cache.get(job.cache_key(variant), job.fmt)
    if not hit:
        return None
    data, meta = hit
    return RenderResult(data, FORMATS[job.fmt], meta.get("durationMs", 0), meta.get("loudnessLufs"),
                        int((time.time() - t) * 1000), True, job.warnings)


def render(engine: Engine, job: Job, cache: Cache | None = None, use_cache: bool = True) -> RenderResult:
    t = time.time()
    key = job.cache_key(engine.variant)
    hit = lookup(cache, job, engine.variant) if use_cache else None
    if hit:
        return hit
    mix, warnings = render_job(engine, job)
    tail = 0.1 if job.bed else au.TAIL_S
    filters = f"apad=pad_dur={tail}" if job.bed else f"{au.LEAD_IN},apad=pad_dur={tail}"
    data = au.encode(mix, job.fmt, au.OUT_RATE, filters)
    lead = 0 if job.bed else 0.25
    duration_ms = int(round((len(mix) / au.OUT_RATE + lead + tail) * 1000))
    m = au.measure(mix) if job.fx else None
    lufs = round(m["input_i"], 1) if m else None
    res = RenderResult(data, FORMATS[job.fmt], duration_ms, lufs, int((time.time() - t) * 1000), False, warnings)
    if cache:
        cache.put(key, job.fmt, data, {"durationMs": duration_ms, "loudnessLufs": lufs})
    return res


AUDITION_TEXT = "Hi, I'm {name}! Welcome to the show. Now sit back, relax, and enjoy the lights!"


def audition_request(req: Mapping[str, Any]) -> dict[str, Any]:
    """{voice, text?, energy?, speed?, format?} -> a /render request."""
    if not isinstance(req, Mapping) or not req.get("voice"):
        raise BadRequest("voice is required")
    voice = req["voice"]
    text = str(req.get("text") or "").strip()
    if not text:
        name = voice.get("name") if isinstance(voice, Mapping) else None
        if not name:
            try:
                name = resolve_voice(voice)["name"]
            except ValueError as e:
                raise BadRequest(str(e), "unknown_voice") from None
        text = AUDITION_TEXT.format(name=name)
    line: dict[str, Any] = {"voice": voice, "text": text[:400], "pauseMs": 0}
    if req.get("energy") is not None:
        line["energy"] = req["energy"]
    return {"lines": [line], "speed": req.get("speed", 1.0), "format": req.get("format", "mp3"),
            "pronunciations": req.get("pronunciations") or [], "voices": req.get("voices") or []}
