import pytest

from pixelplus_tts.script import ScriptError, format_script, parse_script
from pixelplus_tts.voices import resolve_voice

SHOW_INTRO = """\
# Two-DJ banter.  Format:   voice: line
#
nick: Good evening, and welcome to the show!
holly: You're tuned in to 88.7 FM.
[pause 1.0]
nick!: Now sit back, relax, and enjoy the show!
holly!!: Merry Christmas, everybody!
"""


def resolve(name):
    return resolve_voice(name)["id"]


def test_fpp_voices_format():
    lines = parse_script(SHOW_INTRO, resolve)
    assert [ln["voice"] for ln in lines] == ["nick", "holly", "nick", "holly"]
    assert lines[0] == {"voice": "nick", "text": "Good evening, and welcome to the show!", "pauseMs": 0}
    assert lines[1]["pauseMs"] == 1000  # [pause] attaches to the previous line
    assert "energy" not in lines[0]
    assert lines[2]["energy"] == 1.0
    assert lines[3]["energy"] == 1.5


def test_aliases_and_base_voices():
    lines = parse_script("male: hi\nF: hello\nAF_HEART: hey\nHolly: yo", resolve)
    assert [ln["voice"] for ln in lines] == ["nick", "holly", "af_heart", "holly"]


def test_pause_units_and_leading_pause():
    lines = parse_script("[pause 0.5]\nnick: a\n[pause 250ms]\n[pause 1.5s]\n[Pause .5]", resolve)
    assert lines[0] == {"voice": "", "text": "", "pauseMs": 500}
    assert lines[1]["pauseMs"] == 250 + 1500 + 500


def test_colons_and_asterisks_in_text():
    (ln,) = parse_script("holly!: Doors at 5:30, *enjoy the show!*", resolve)
    assert ln["text"] == "Doors at 5:30, *enjoy the show!*"
    assert ln["energy"] == 1.0


def test_three_bangs_is_extra_hype():
    assert parse_script("nick!!!: go", resolve)[0]["energy"] == 1.5


def test_hash_without_space_is_not_comment():
    # "#1" style text is only a comment with "# " (hash + space), like fpp-voices
    with pytest.raises(ScriptError):
        parse_script("#1 hit", resolve)


def test_errors_carry_line_numbers():
    with pytest.raises(ScriptError) as e:
        parse_script("nick: ok\n\nthis line has no voice", resolve)
    assert e.value.line == 3
    with pytest.raises(ScriptError) as e:
        parse_script("nick: ok\nrudolph: hi", resolve)
    assert e.value.line == 2 and "unknown voice" in str(e.value)
    with pytest.raises(ScriptError):
        parse_script("nick:   ", resolve)


def test_default_resolver_lowercases():
    assert parse_script("Custom Voice: hi")[0]["voice"] == "custom voice"


def test_format_roundtrip():
    lines = parse_script(SHOW_INTRO, resolve)
    assert parse_script(format_script(lines), resolve) == lines
