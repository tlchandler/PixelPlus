"""Command line: python -m pixelplus_tts <command> ...

  serve            run the HTTP service (127.0.0.1:7081)
  say              make a clip, like fpp-voices' ./say (default command)
  parse FILE       print a script as DjLine JSON
  voices           list voices
  download-models  fetch + verify the Kokoro model into the models dir
  bench            measure load time, real-time factor and memory

Examples:
  python -m pixelplus_tts say nick "Good evening and welcome to the show!"
  python -m pixelplus_tts say holly --hype "Sit back, relax, and enjoy the show!" -o hype.mp3
  python -m pixelplus_tts say --script show_intro.txt -o show_intro.mp3
"""
from __future__ import annotations

import argparse
import json
import logging
import os
import re
import resource
import sys
import time

from . import __version__
from .config import Config

COMMANDS = ("serve", "say", "parse", "voices", "download-models", "bench")


def _common(p: argparse.ArgumentParser) -> None:
    p.add_argument("--models-dir", help="model directory (default $PIXELPLUS_DATA_DIR/tts/models)")
    p.add_argument("--variant", choices=["auto", "fp32", "fp16", "int8"], help="model variant (default auto)")
    p.add_argument("--threads", type=int, help="onnxruntime threads (default cores-1)")


def _cfg(a) -> Config:
    cfg = Config.from_env()
    if getattr(a, "models_dir", None):
        cfg.models_dir = a.models_dir
    if getattr(a, "variant", None):
        cfg.variant = a.variant
    if getattr(a, "threads", None):
        cfg.threads = a.threads
    return cfg


def _slug(text: str) -> str:
    return re.sub(r"[^a-z0-9]+", "_", text.lower()).strip("_")[:48] or "clip"


def cmd_serve(argv: list[str]) -> None:
    p = argparse.ArgumentParser(prog="pixelplus_tts serve")
    _common(p)
    p.add_argument("--host")
    p.add_argument("--port", type=int)
    p.add_argument("--idle-minutes", type=float, help="unload model after N idle minutes (0 = never)")
    p.add_argument("--cache-dir")
    p.add_argument("-v", "--verbose", action="store_true")
    a = p.parse_args(argv)
    cfg = _cfg(a)
    for k in ("host", "port", "idle_minutes", "cache_dir"):
        if getattr(a, k) is not None:
            setattr(cfg, k, getattr(a, k))
    logging.basicConfig(level=logging.DEBUG if a.verbose else logging.INFO,
                        format="%(asctime)s %(levelname)s %(message)s")
    from .server import serve
    serve(cfg)


def cmd_say(argv: list[str]) -> None:
    p = argparse.ArgumentParser(prog="pixelplus_tts say", description="Create DJ-style voice clips.")
    p.add_argument("voice", nargs="?", help="nick / male, holly / female, or a Kokoro id like af_heart")
    p.add_argument("text", nargs="*", help="what to say (or pipe it in on stdin)")
    p.add_argument("-o", "--output", help="output file (.mp3, .wav or .ogg)")
    p.add_argument("-s", "--script", help="text file of 'voice: line' entries")
    p.add_argument("--speed", type=float, help="speaking speed (default from the voice, ~1.05)")
    p.add_argument("--energy", type=float, help="0 calm, 0.4 normal DJ (default), 1 hype, 1.5 extra hype")
    p.add_argument("--hype", action="store_const", const=1.0, dest="energy", help="same as --energy 1")
    p.add_argument("--loudness", type=float, default=-16, help="target LUFS (default -16)")
    p.add_argument("--no-fx", action="store_true", help="skip the radio processing")
    p.add_argument("--pronunciations", help="extra pronunciations file (word = say)")
    p.add_argument("--music-bed", help="audio file to play under the voice (ducked)")
    p.add_argument("--list", action="store_true", help="list voices")
    _common(p)
    a = p.parse_intermixed_args(argv)
    if a.list:
        return cmd_voices([])
    from .engine import Engine
    from .pronounce import parse_pronunciations_txt
    from .render import build_job, render
    from .script import parse_script
    from .voices import resolve_voice

    if a.script:
        with open(a.script, encoding="utf-8") as f:
            try:
                lines = parse_script(f.read(), lambda n: resolve_voice(n)["id"])
            except ValueError as e:
                sys.exit(f"{a.script}: {e}")
        default_name = os.path.splitext(os.path.basename(a.script))[0]
    else:
        if not a.voice:
            p.error("give a voice and some text, or --script FILE")
        try:
            vid = resolve_voice(a.voice)["id"]
        except ValueError as e:
            sys.exit(str(e))
        text = " ".join(a.text) or sys.stdin.read()
        if not text.strip():
            p.error("no text given")
        lines = [{"voice": vid, "text": text.strip(), "pauseMs": 0}]
        default_name = f"{vid}_{_slug(text)}"
    if a.energy is not None:
        for ln in lines:
            ln.setdefault("energy", a.energy)
    prons = []
    if a.pronunciations:
        with open(a.pronunciations, encoding="utf-8") as f:
            prons = [{"word": w, "say": s} for w, s in parse_pronunciations_txt(f.read())]
    out = a.output or os.path.join("outputs", default_name + ".mp3")
    fmt = os.path.splitext(out)[1].lstrip(".").lower() or "mp3"
    req = {"lines": lines, "format": fmt, "loudnessLufs": a.loudness, "fx": not a.no_fx, "pronunciations": prons}
    if a.music_bed:
        req["musicBed"] = {"path": a.music_bed}
    try:
        job = build_job(req, allow_any_path=True)
    except ValueError as e:
        sys.exit(str(e))
    if a.speed:  # fpp-voices: --speed replaces the voice's own speed
        for ln in job.lines:
            if ln["voice"]:
                ln["voice"]["speed"] = a.speed
    cfg = _cfg(a)
    engine = Engine(cfg.models_dir, cfg.variant, cfg.threads, idle_minutes=0)
    if not engine.available:
        sys.exit(f"Voice model not found in {cfg.models_dir}. Run: python -m pixelplus_tts download-models")
    res = render(engine, job, cache=None)
    os.makedirs(os.path.dirname(os.path.abspath(out)), exist_ok=True)
    with open(out, "wb") as f:
        f.write(res.data)
    for w in res.warnings:
        print("warning:", w, file=sys.stderr)
    print(out)


def cmd_parse(argv: list[str]) -> None:
    p = argparse.ArgumentParser(prog="pixelplus_tts parse")
    p.add_argument("file", help="script file ('-' for stdin)")
    a = p.parse_args(argv)
    from .script import parse_script
    from .voices import resolve_voice
    text = sys.stdin.read() if a.file == "-" else open(a.file, encoding="utf-8").read()
    try:
        print(json.dumps(parse_script(text, lambda n: resolve_voice(n)["id"]), indent=2, ensure_ascii=False))
    except ValueError as e:
        sys.exit(str(e))


def cmd_voices(argv: list[str]) -> None:
    from .voices import list_base_voices, load_presets
    for vid, v in load_presets().items():
        print(f"{vid:8s} ({', '.join(v['aliases'])}): {v['description']}")
    print()
    for b in list_base_voices():
        print(f"  {b['id']:14s} {b['name']:12s} {b['language']:6s} {b['gender']:6s} {b.get('grade', '')}")


def cmd_download(argv: list[str]) -> None:
    p = argparse.ArgumentParser(prog="pixelplus_tts download-models")
    p.add_argument("--models-dir")
    p.add_argument("--variant", default="auto", choices=["auto", "fp32", "fp16", "int8"])
    p.add_argument("--base-url", help="mirror URL (default: kokoro-onnx GitHub release)")
    a = p.parse_args(argv)
    from .models import BASE_URL, ensure_models
    cfg = _cfg(a)
    try:
        v = ensure_models(cfg.models_dir, a.variant, base_url=a.base_url or BASE_URL)
    except (RuntimeError, ValueError) as e:
        sys.exit(str(e))
    print(f"model {v} ready in {cfg.models_dir}")


def _rss_mb() -> int:
    try:
        with open("/proc/self/status") as f:
            for line in f:
                if line.startswith("VmRSS"):
                    return int(line.split()[1]) // 1024
    except OSError:
        pass
    return 0


def cmd_bench(argv: list[str]) -> None:
    p = argparse.ArgumentParser(prog="pixelplus_tts bench")
    _common(p)
    a = p.parse_args(argv)
    from .engine import Engine
    from .render import build_job, render
    from .voices import resolve_voice
    cfg = _cfg(a)
    engine = Engine(cfg.models_dir, cfg.variant, cfg.threads, idle_minutes=0)
    print(f"pixelplus-tts {__version__}  model={engine.variant}  threads={engine.threads}  rss={_rss_mb()}MB")
    t = time.time()
    engine.load()
    print(f"load: {time.time() - t:.1f}s  rss={_rss_mb()}MB")
    text = ("Good evening, and welcome to the Christmas light show! Tonight we've got twenty-four songs "
            "and over forty thousand lights, all dancing to the music. Now sit back, relax, and enjoy the show!")
    engine.synthesize("Warm up.", resolve_voice("nick"), [])
    for voice, energy in (("nick", None), ("holly", 1.0)):
        line = {"voice": voice, "text": text, "pauseMs": 0}
        if energy is not None:
            line["energy"] = energy
        job = build_job({"lines": [line], "format": "mp3"})
        t = time.time()
        res = render(engine, job, cache=None)
        dt = time.time() - t
        audio_s = res.duration_ms / 1000
        print(f"{voice:6s} energy={energy or 'default':7}  render={dt:.2f}s  audio={audio_s:.1f}s  "
              f"RTF={dt / audio_s:.2f}  lufs={res.loudness_lufs}")
    print(f"peak rss: {resource.getrusage(resource.RUSAGE_SELF).ru_maxrss // 1024}MB")
    engine.unload()
    print(f"after unload: rss={_rss_mb()}MB")


def main(argv: list[str] | None = None) -> None:
    argv = list(sys.argv[1:] if argv is None else argv)
    if argv and argv[0] in ("-V", "--version"):
        print(__version__)
        return
    if argv and argv[0] in ("-h", "--help"):
        print(__doc__)
        return
    cmd = argv.pop(0) if argv and argv[0] in COMMANDS else "say"
    {"serve": cmd_serve, "say": cmd_say, "parse": cmd_parse, "voices": cmd_voices,
     "download-models": cmd_download, "bench": cmd_bench}[cmd](argv)


if __name__ == "__main__":
    main()
