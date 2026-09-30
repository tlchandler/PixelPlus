"""Pronunciation fixes, ported from fpp-voices.

* Matching is whole-word (no letter/digit/underscore on either side).
* Entries containing a capital letter are case-sensitive ("LED" doesn't change
  "she led the way"); all-lowercase entries match any case.
* Longer phrases are matched first ("Feliz Navidad" wins over "Navidad").
* A replacement written between slashes (/noʊˈɛl/) is exact IPA; it's spliced
  into the phoneme stream instead of being spelled.
"""
from __future__ import annotations

import re
from typing import Iterable, Mapping

Rule = tuple["re.Pattern[str]", str]
IPA_MARK = "\x00"


def parse_pronunciations_txt(text: str) -> list[tuple[str, str]]:
    """`word = replacement` lines; comments start with "# " (or are a bare "#")."""
    out = []
    for line in text.splitlines():
        line = line.strip()
        if not line or line == "#" or line.startswith("# "):
            continue
        if "=" in line:
            src, dst = (s.strip() for s in line.split("=", 1))
            if src and dst:
                out.append((src, dst))
    return out


def builtin_pronunciations() -> list[tuple[str, str]]:
    from importlib import resources
    txt = resources.files("pixelplus_tts").joinpath("data/pronunciations.txt").read_text(encoding="utf-8")
    return parse_pronunciations_txt(txt)


def merge(builtin: Iterable[tuple[str, str]], user: Iterable[Mapping[str, str] | tuple[str, str]]) -> list[tuple[str, str]]:
    """User entries override built-ins with the same word (exact spelling)."""
    table: dict[str, str] = {}
    for w, s in builtin:
        table[w] = s
    for item in user:
        w, s = (item["word"], item["say"]) if isinstance(item, Mapping) else item
        w, s = str(w).strip(), str(s).strip()
        if w and s:
            table[w] = s
    return list(table.items())


def compile_rules(pairs: Iterable[tuple[str, str]]) -> list[Rule]:
    pairs = sorted(pairs, key=lambda r: -len(r[0]))
    return [(re.compile(r"(?<!\w)" + re.escape(src) + r"(?!\w)", 0 if src != src.lower() else re.I), dst)
            for src, dst in pairs]


def is_ipa(say: str) -> bool:
    return len(say) >= 2 and say.startswith("/") and say.endswith("/")


def apply_pronunciations(text: str, rules: list[Rule]) -> tuple[str, list[str]]:
    """Plain replacements are applied to the text. A /ipa/ replacement becomes a
    placeholder "\\x00<n>\\x00" and its phonemes are returned in the list."""
    ipa: list[str] = []
    for pattern, replacement in rules:
        if is_ipa(replacement):
            if not pattern.search(text):
                continue
            ipa.append(replacement[1:-1])
            replacement = f"{IPA_MARK}{len(ipa) - 1}{IPA_MARK}"
        text = pattern.sub(lambda m, r=replacement: r, text)
    return text, ipa


def to_phonemes(text: str, rules: list[Rule], phonemize) -> str:
    """Text -> phoneme string, with IPA overrides spliced in. `phonemize(str) -> str`."""
    text, ipa = apply_pronunciations(text, rules)
    parts = text.split(IPA_MARK)  # even indexes: text, odd indexes: IPA slot numbers
    return " ".join(ipa[int(p)] if i % 2 else phonemize(p)
                    for i, p in enumerate(parts) if p.strip())
