"""The fpp-voices script format -> DjLine dicts.

    # comment (hash + space), blank lines ignored
    nick: Good evening, and welcome to the show!
    holly!: Now sit back, relax, and *enjoy the show!*     (! = hype, energy 1)
    holly!!: Merry Christmas, everybody!                    (!! = extra hype, 1.5)
    [pause 1.0]                                             (seconds; also 1.5s / 500ms)

Lines come out as {voice, text, pauseMs, energy?}. `pauseMs` is the silence after
a line; a `[pause]` adds to the previous line's pauseMs (a pause before any line
becomes a text-less line). `*asterisks*` stay in the text; the renderer uses them
to pick the hype punchline. Semantics are shared with web/src/lib/tts-browser/script.ts.
"""
from __future__ import annotations

import re
from typing import Any, Callable

PAUSE_RE = re.compile(r"^\[pause\s+(\d+(?:\.\d+)?|\.\d+)\s*(ms|s)?\s*\]$", re.I)
VOICE_RE = re.compile(r"^([A-Za-z0-9_][A-Za-z0-9_ .'\-]*?)\s*(!*)$")


class ScriptError(ValueError):
    def __init__(self, line: int, message: str):
        super().__init__(f"line {line}: {message}")
        self.line = line


def parse_script(text: str, resolve: Callable[[str], str] | None = None) -> list[dict[str, Any]]:
    """`resolve(name) -> voice id` maps names/aliases (nick, male, holly, af_heart…)
    and raises ValueError for unknown voices; default keeps the name lowercased."""
    lines: list[dict[str, Any]] = []
    for n, raw in enumerate(text.splitlines(), 1):
        line = raw.strip()
        if not line or line == "#" or line.startswith("# "):
            continue
        m = PAUSE_RE.match(line)
        if m:
            ms = round(float(m.group(1)) * (1 if (m.group(2) or "s").lower() == "ms" else 1000))
            if lines:
                lines[-1]["pauseMs"] += ms
            else:
                lines.append({"voice": "", "text": "", "pauseMs": ms})
            continue
        if ":" not in line:
            raise ScriptError(n, "expected 'voice: text' or '[pause N]'")
        who, say = line.split(":", 1)
        vm = VOICE_RE.match(who.strip())
        if not vm:
            raise ScriptError(n, f"bad voice name {who.strip()!r}")
        name, bangs = vm.group(1).strip(), len(vm.group(2))
        say = say.strip()
        if not say:
            raise ScriptError(n, "nothing to say")
        try:
            voice = resolve(name) if resolve else name.lower()
        except ValueError as e:
            raise ScriptError(n, str(e)) from None
        item: dict[str, Any] = {"voice": voice, "text": say, "pauseMs": 0}
        if bangs:
            item["energy"] = 1.0 if bangs == 1 else 1.5
        lines.append(item)
    return lines


def format_script(lines: list[dict[str, Any]], names: dict[str, str] | None = None) -> str:
    """DjLines -> script text (inverse of parse_script for default gaps)."""
    out = []
    for ln in lines:
        if ln.get("text"):
            e = ln.get("energy")
            bang = "" if e is None or e < 1 else ("!!" if e >= 1.5 else "!")
            vid = ln["voice"] if isinstance(ln["voice"], str) else ln["voice"].get("id", "voice")
            out.append(f"{(names or {}).get(vid, vid)}{bang}: {ln['text']}")
        if ln.get("pauseMs"):
            out.append(f"[pause {ln['pauseMs'] / 1000:g}]")
    return "\n".join(out) + "\n"
