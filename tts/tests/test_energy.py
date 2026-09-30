import pytest

from pixelplus_tts.energy import (energy_curve, find_emphasis, hype_setting, plan_line, punch_lift, shape_pitch,
                                  soft_ceiling)
from pixelplus_tts.voices import load_presets

NICK = load_presets()["nick"]
HOLLY = load_presets()["holly"]


@pytest.mark.parametrize("text,expected", [
    ("Sit back, relax, and enjoy the show!", ("Sit back, relax,", "and enjoy the show!", "")),
    ("Sit back, *relax*, and enjoy the show.", ("Sit back, ", "relax", ", and enjoy the show.")),
    ("Welcome! Merry Christmas, everybody!", ("Welcome! Merry Christmas,", "everybody!", "")),
    ("Enjoy the show tonight everybody", ("Enjoy the", "show tonight everybody", "")),
    ("Go team", ("", "Go team", "")),
    ("First sentence. Second one here now.", ("First sentence.", "Second one here now.", "")),
])
def test_find_emphasis(text, expected):
    assert find_emphasis(text) == expected


def test_hype_setting_fallbacks():
    assert hype_setting(NICK, "lift") == 3
    assert hype_setting({"energy": {}}, "max_lift") == 12
    assert hype_setting({}, "range") == 1


def test_plan_line():
    # normal DJ line: base = energy, not hype
    base, spd, hype = plan_line(HOLLY, 0.4)
    assert (base, hype) == (0.4, False)
    assert spd == pytest.approx(1.05 * (1 + 0.1 * 0.4))
    # hype: base stays at the voice default, speed only follows the base
    base, spd, hype = plan_line(HOLLY, 1.5)
    assert (base, hype) == (0.4, True) and spd == pytest.approx(1.05 * 1.04)
    # calm below default
    assert plan_line(NICK, 0.0) == (0.0, 1.05, False)
    # explicit speed replaces the voice speed
    assert plan_line(NICK, 1.0, speed=1.2)[1] == pytest.approx(1.2)


def test_energy_curve_builds_into_punchline():
    ph = ["aaaaaaaaa", "bbbbbbbbb", ""]  # 9 + 1 + 9 chars = 19
    c = energy_curve(ph, 3.8, 0.4, 1.0)
    t0 = 3.8 * 9 / 19
    assert c == [(0.0, 0.4), (pytest.approx(t0 - 0.25), 0.4), (pytest.approx(t0 + 0.2), 1.0), (3.8, 1.0)]
    # a tail eases back to base
    c = energy_curve(["aaaa", "bbbb", "cccc"], 2.0, 0.4, 1.5)
    assert c[-1] == (pytest.approx(2.0 * (4 + 4 + 1) / 14 + 0.25), 0.4)
    assert [lvl for _, lvl in c] == [0.4, 0.4, 1.5, 1.5, 0.4]


def test_energy_curve_short_lead_drops_duplicate_times():
    c = energy_curve(["", "bbbbbbbb", ""], 1.0, 0.4, 1.0)
    times = [t for t, _ in c]
    assert times == sorted(set(times))
    assert c[0] == (0.0, 0.4)


def test_punch_lift():
    # punchline sank 4 semitones below the lead-in; target lift 3*1.0 -> +7
    assert punch_lift(50.0, 46.0, NICK, 1.0) == pytest.approx(7.0)
    # capped at max_lift (Nick 9)
    assert punch_lift(50.0, 40.0, NICK, 1.5) == 9
    # already far above: never lowered
    assert punch_lift(40.0, 50.0, NICK, 1.0) == 0
    assert punch_lift(None, 50.0, NICK, 1.0) == 0


def test_soft_ceiling():
    assert soft_ceiling(200.0, 300.0) == 200.0
    assert soft_ceiling(300.0 * 16, 300.0) == pytest.approx(600.0)  # 4th-root compression


def test_shape_pitch():
    v = {"energy": {"pitch": 2, "range": 1.5}}
    # energy 0 leaves the pitch alone
    assert shape_pitch(150.0, 120.0, 0.0, v, None, 0.0, 1e9) == pytest.approx(150.0)
    # flat energy 1: +2 semitones and 1.5x wider swings around the mean
    expect = 120 * 2 ** (2 / 12) * (150 / 120) ** 1.5
    assert shape_pitch(150.0, 120.0, 1.0, v, None, 0.0, 1e9) == pytest.approx(expect)
    # hype line: overall raise limited to the base level, plus the punchline lift
    expect = 120 * 2 ** ((2 * 0.4 + 3) / 12) * (150 / 120) ** 1.5
    assert shape_pitch(150.0, 120.0, 1.0, v, 0.4, 3.0, 1e9) == pytest.approx(expect)
