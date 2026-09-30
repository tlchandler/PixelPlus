"""Kokoro model files: variants, per-device choice, and a resumable, verified download.

Hashes were computed from the official kokoro-onnx release assets
(github.com/thewh1teagle/kokoro-onnx/releases/tag/model-files-v1.0).
"""
from __future__ import annotations

import hashlib
import os
import shutil
import subprocess
import sys
import time
import urllib.error
import urllib.request
from dataclasses import dataclass

BASE_URL = "https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0"


@dataclass(frozen=True)
class ModelFile:
    name: str
    size: int
    sha256: str


MODEL_FILES = {
    "fp32": ModelFile("kokoro-v1.0.onnx", 325532387,
                      "7d5df8ecf7d4b1878015a32686053fd0eebe2bc377234608764cc0ef3636a6c5"),
    "fp16": ModelFile("kokoro-v1.0.fp16.onnx", 177464787,
                      "c1610a859f3bdea01107e73e50100685af38fff88f5cd8e5c56df109ec880204"),
    "int8": ModelFile("kokoro-v1.0.int8.onnx", 92361271,
                      "6e742170d309016e5891a994e1ce1559c702a2ccd0075e67ef7157974f6406cb"),
}
VOICES_FILE = ModelFile("voices-v1.0.bin", 28214398,
                        "bca610b8308e8d99f32e6fe4197e7ec01679264efed0cac9140fe9c29f1fbf7d")
VARIANTS = tuple(MODEL_FILES)


def mem_total_mb() -> int:
    try:
        with open("/proc/meminfo") as f:
            for line in f:
                if line.startswith("MemTotal:"):
                    return int(line.split()[1]) // 1024
    except OSError:
        pass
    return 4096


def auto_variant(mem_mb: int | None = None) -> str:
    """fp32 is the reference quality and (measured) the fastest portable choice;
    peak RSS while rendering is ~700 MB. Only boards with < 1.5 GB RAM (Pi 4 1GB)
    get int8: ~190 MB resident / ~530 MB peak, but ~3x slower on CPU."""
    mem_mb = mem_total_mb() if mem_mb is None else mem_mb
    return "int8" if mem_mb < 1536 else "fp32"


def resolve_variant(variant: str, models_dir: str | None = None) -> str:
    """Turn "auto" into a concrete variant. If the preferred file isn't present but
    another variant is, use what's installed rather than failing."""
    if variant in MODEL_FILES:
        return variant
    preferred = auto_variant()
    if models_dir and not os.path.exists(os.path.join(models_dir, MODEL_FILES[preferred].name)):
        for v in ("fp32", "fp16", "int8"):
            if os.path.exists(os.path.join(models_dir, MODEL_FILES[v].name)):
                return v
    return preferred


def model_paths(models_dir: str, variant: str) -> tuple[str, str]:
    return (os.path.join(models_dir, MODEL_FILES[variant].name),
            os.path.join(models_dir, VOICES_FILE.name))


def models_present(models_dir: str, variant: str) -> bool:
    return all(os.path.isfile(p) for p in model_paths(models_dir, variant))


def sha256_file(path: str) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def download(mf: ModelFile, dest_dir: str, base_url: str = BASE_URL, retries: int = 5,
             quiet: bool = False) -> str:
    """Download with HTTP Range resume (into <name>.part) and sha256 verification.
    Already-present, verified files are skipped."""
    os.makedirs(dest_dir, exist_ok=True)
    final = os.path.join(dest_dir, mf.name)
    if os.path.isfile(final) and os.path.getsize(final) == mf.size:
        if sha256_file(final) == mf.sha256:
            if not quiet:
                print(f"{mf.name}: already present, verified")
            return final
        os.remove(final)
    part = final + ".part"
    url = f"{base_url}/{mf.name}"
    for attempt in range(1, retries + 1):
        have = os.path.getsize(part) if os.path.exists(part) else 0
        if have > mf.size:
            os.remove(part)
            have = 0
        if have < mf.size and shutil.which("curl") and not os.environ.get("PIXELPLUS_TTS_NO_CURL"):
            # curl: resumable (-C -), honors proxies/CA bundles the same way the rest of the OS does
            r = subprocess.run(["curl", "-fL", "--retry", "3", "--connect-timeout", "30", "-C", "-",
                                *(["-sS"] if quiet else ["-#"]), "-o", part, url])
            if r.returncode not in (0, 33):  # 33: server can't resume -> retry below from scratch
                if attempt == retries:
                    raise RuntimeError(f"download of {mf.name} failed (curl exit {r.returncode})")
                print(f"{mf.name}: curl exit {r.returncode}; retrying ({attempt}/{retries})", file=sys.stderr)
                if r.returncode == 33 and os.path.exists(part):
                    os.remove(part)
                time.sleep(min(2 ** attempt, 30))
                continue
        elif have < mf.size:
            req = urllib.request.Request(url, headers={"User-Agent": "pixelplus-tts"})
            if have:
                req.add_header("Range", f"bytes={have}-")
            try:
                with urllib.request.urlopen(req, timeout=60) as r:
                    if have and r.status != 206:  # server ignored Range: start over
                        have = 0
                    with open(part, "ab" if have else "wb") as out:
                        done, last = have, 0.0
                        while True:
                            chunk = r.read(1 << 20)
                            if not chunk:
                                break
                            out.write(chunk)
                            done += len(chunk)
                            if not quiet and time.time() - last > 2:
                                last = time.time()
                                print(f"{mf.name}: {done * 100 // mf.size}% "
                                      f"({done >> 20}/{mf.size >> 20} MB)", flush=True)
            except (urllib.error.URLError, OSError, TimeoutError) as e:
                if attempt == retries:
                    raise RuntimeError(f"download of {mf.name} failed: {e}") from e
                print(f"{mf.name}: {e}; retrying ({attempt}/{retries})", file=sys.stderr)
                time.sleep(min(2 ** attempt, 30))
                continue
        if os.path.getsize(part) != mf.size:
            if attempt == retries:
                raise RuntimeError(f"{mf.name}: incomplete download")
            continue
        digest = sha256_file(part)
        if digest != mf.sha256:
            os.remove(part)
            raise RuntimeError(f"{mf.name}: sha256 mismatch ({digest}); corrupt file removed")
        os.replace(part, final)
        if not quiet:
            print(f"{mf.name}: downloaded and verified")
        return final
    raise RuntimeError(f"download of {mf.name} failed")


def ensure_models(models_dir: str, variant: str = "auto", **kw) -> str:
    variant = auto_variant() if variant == "auto" else variant
    if variant not in MODEL_FILES:
        raise ValueError(f"unknown model variant {variant!r}; choose from {', '.join(VARIANTS)}")
    download(VOICES_FILE, models_dir, **kw)
    download(MODEL_FILES[variant], models_dir, **kw)
    return variant
