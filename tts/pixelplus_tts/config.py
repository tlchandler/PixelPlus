"""Runtime configuration (environment variables with sensible Pi defaults)."""
from __future__ import annotations

import os
from dataclasses import dataclass, field

DATA_DIR_DEFAULT = "/var/lib/pixelplus"


def _env_float(name: str, default: float) -> float:
    try:
        return float(os.environ.get(name, default))
    except ValueError:
        return default


def default_threads() -> int:
    """Leave one core free for pixelplusd's realtime output thread."""
    return max(1, (os.cpu_count() or 1) - 1)


@dataclass
class Config:
    host: str = "127.0.0.1"
    port: int = 7081
    data_dir: str = DATA_DIR_DEFAULT
    models_dir: str = ""
    cache_dir: str = ""
    variant: str = "auto"  # auto | fp32 | fp16 | int8
    idle_minutes: float = 10.0  # unload the model after this long unused (0 = never)
    threads: int = field(default_factory=default_threads)
    cache_mb: int = 200
    allow_any_path: bool = False  # music beds must live under data_dir unless set

    def __post_init__(self) -> None:
        self.models_dir = self.models_dir or os.path.join(self.data_dir, "tts", "models")
        self.cache_dir = self.cache_dir or os.path.join(self.data_dir, "tts", "cache")

    @classmethod
    def from_env(cls) -> "Config":
        e = os.environ
        return cls(
            host=e.get("PIXELPLUS_TTS_HOST", "127.0.0.1"),
            port=int(e.get("PIXELPLUS_TTS_PORT", "7081")),
            data_dir=e.get("PIXELPLUS_DATA_DIR", DATA_DIR_DEFAULT),
            models_dir=e.get("PIXELPLUS_TTS_MODELS_DIR", ""),
            cache_dir=e.get("PIXELPLUS_TTS_CACHE_DIR", ""),
            variant=e.get("PIXELPLUS_TTS_MODEL", "auto"),
            idle_minutes=_env_float("PIXELPLUS_TTS_IDLE_MIN", 10.0),
            threads=int(e.get("PIXELPLUS_TTS_THREADS", "0")) or default_threads(),
            cache_mb=int(e.get("PIXELPLUS_TTS_CACHE_MB", "200")),
            allow_any_path=e.get("PIXELPLUS_TTS_ALLOW_ANY_PATH", "") in ("1", "true", "yes"),
        )
