"""Kokoro model manager: lazy load, one render at a time, unload when idle."""
from __future__ import annotations

import ctypes
import gc
import logging
import threading
import time
from typing import Any, Mapping

import numpy as np

from . import energy as en
from .models import model_paths, models_present, resolve_variant
from .pronounce import Rule, to_phonemes
from .voices import blend_style

log = logging.getLogger("pixelplus_tts")
SAMPLE_RATE = en.SAMPLE_RATE


class ModelMissing(RuntimeError):
    pass


def _malloc_trim() -> None:
    """Give freed heap back to the OS so an unloaded model really frees RAM."""
    try:
        ctypes.CDLL("libc.so.6").malloc_trim(0)
    except (OSError, AttributeError):
        pass


class Engine:
    def __init__(self, models_dir: str, variant: str = "auto", threads: int = 0,
                 idle_minutes: float = 10.0):
        self.models_dir = models_dir
        self.variant = resolve_variant(variant, models_dir)
        self.threads = threads
        self.idle_seconds = idle_minutes * 60
        self._k = None
        self._lock = threading.RLock()  # held for load + each synthesis
        self._last_used = 0.0
        self._styles: dict[str, np.ndarray] = {}
        self.load_seconds: float | None = None
        if self.idle_seconds > 0:
            threading.Thread(target=self._idle_loop, daemon=True, name="tts-idle").start()

    # ---------------------------------------------------------------- state --
    @property
    def loaded(self) -> bool:
        return self._k is not None

    @property
    def available(self) -> bool:
        return models_present(self.models_dir, self.variant)

    def voices_path(self) -> str:
        return model_paths(self.models_dir, self.variant)[1]

    def load(self):
        with self._lock:
            if self._k is None:
                if not self.available:
                    raise ModelMissing(
                        f"Kokoro model files not found in {self.models_dir} (variant {self.variant}). "
                        "Run: python -m pixelplus_tts download-models")
                t = time.time()
                import onnxruntime as rt
                from kokoro_onnx import Kokoro
                model, voices = model_paths(self.models_dir, self.variant)
                so = rt.SessionOptions()
                if self.threads:
                    so.intra_op_num_threads = self.threads
                    so.inter_op_num_threads = 1
                sess = rt.InferenceSession(model, so, providers=["CPUExecutionProvider"])
                if hasattr(Kokoro, "from_session"):
                    self._k = Kokoro.from_session(sess, voices)
                else:  # kokoro-onnx < 0.5
                    del sess
                    self._k = Kokoro(model, voices)
                self.load_seconds = time.time() - t
                log.info("loaded kokoro %s in %.1fs (%d threads)", self.variant, self.load_seconds, self.threads)
            self._last_used = time.time()
            return self._k

    def unload(self) -> bool:
        with self._lock:
            if self._k is None:
                return False
            self._k = None
            self._styles.clear()
            gc.collect()
            _malloc_trim()
            log.info("unloaded kokoro model")
            return True

    def _idle_loop(self) -> None:
        while True:
            time.sleep(min(30.0, max(self.idle_seconds / 4, 1.0)))
            if self._k is not None and time.time() - self._last_used > self.idle_seconds:
                if self._lock.acquire(blocking=False):
                    try:
                        if time.time() - self._last_used > self.idle_seconds:
                            self.unload()
                    finally:
                        self._lock.release()

    def base_voice_ids(self) -> set[str]:
        k = self._k
        if k is not None:
            return set(k.get_voices())
        from .voices import voices_in_file
        return voices_in_file(self.voices_path())

    # ------------------------------------------------------------ synthesis --
    def style(self, blend: Mapping[str, float]) -> np.ndarray:
        k = self.load()
        key = repr(sorted(blend.items()))
        if key not in self._styles:
            known = set(k.get_voices())
            missing = [v for v in blend if v not in known]
            if missing:
                raise ValueError(f"unknown Kokoro voice(s): {', '.join(missing)}")
            self._styles[key] = blend_style(blend, k.get_voice_style)
        return self._styles[key]

    def phonemize(self, text: str, lang: str) -> str:
        return self.load().tokenizer.phonemize(text, lang)

    def synthesize(self, text: str, voice: Mapping[str, Any], rules: list[Rule],
                   speed: float | None = None, energy: float = 0.0):
        """One line -> (audio float32 @24 kHz, gain_db envelope or None). Mirrors
        fpp-voices synthesize(): whole line in one pass, energy via PSOLA."""
        with self._lock:
            k = self.load()
            lang = voice.get("lang", "en-us")
            base, spd, hype = en.plan_line(voice, energy, speed)
            spd = float(min(max(spd, 0.5), 2.0))
            style = self.style(voice["blend"])
            parts = en.find_emphasis(text) if hype else (text.replace("*", ""), "", "")
            ph = lambda s: k.tokenizer.phonemize(s, lang)  # noqa: E731
            phonemes = [to_phonemes(p, rules, ph) if p.strip() else "" for p in parts]
            joined = " ".join(filter(None, phonemes))
            if not joined.strip():
                raise ValueError(f"nothing to say in {text!r}")
            audio, _ = k.create(joined, voice=style, speed=spd, lang=lang, is_phonemes=True)
            self._last_used = time.time()
        audio = np.asarray(audio, dtype=np.float32)
        if not hype or not phonemes[1]:
            return en.add_energy(audio, voice, [(0, energy)]), None
        curve = en.energy_curve(phonemes, len(audio) / SAMPLE_RATE, base, energy)
        return en.add_energy(audio, voice, curve, base)
