import numpy as np
import pytest

from pixelplus_tts.voices import (BASE_VOICE_IDS, blend_style, list_base_voices, load_presets, normalize_blend,
                                  normalize_voice, presets_as_dj_voices, resolve_voice, to_dj_voice, validate_eq)


def test_catalog_has_all_54_kokoro_v1_voices():
    base = list_base_voices()
    assert len(base) == 54 == len(set(BASE_VOICE_IDS))
    heart = next(v for v in base if v["id"] == "af_heart")
    assert heart == {"id": "af_heart", "name": "Heart", "language": "en-us", "gender": "female", "grade": "A"}
    george = next(v for v in base if v["id"] == "bm_george")
    assert george["language"] == "en-gb" and george["gender"] == "male"
    assert next(v for v in base if v["id"] == "zf_xiaobei")["language"] == "cmn"


def test_normalize_blend():
    assert normalize_blend({"am_echo": 3, "am_puck": 1}) == {"am_echo": 0.75, "am_puck": 0.25}
    assert normalize_blend({"am_echo": 1, "am_puck": 0, "am_fenrir": -1}) == {"am_echo": 1.0}
    with pytest.raises(ValueError):
        normalize_blend({"am_echo": 0})
    with pytest.raises(ValueError):
        normalize_blend({"../etc": 1})


def test_blend_style_is_weighted_average():
    styles = {"af_x": np.full((510, 1, 256), 1.0, np.float32), "af_y": np.full((510, 1, 256), 3.0, np.float32),
              "af_z": np.arange(510 * 256, dtype=np.float32).reshape(510, 1, 256)}
    out = blend_style({"af_x": 0.5, "af_y": 0.5}, styles.__getitem__)
    assert out.shape == (510, 1, 256) and out.dtype == np.float32
    assert np.allclose(out, 2.0)
    # unnormalized weights are normalized
    assert np.allclose(blend_style({"af_x": 1, "af_y": 3}, styles.__getitem__), 2.5)
    # single voice == that voice
    assert np.array_equal(blend_style({"af_z": 7}, styles.__getitem__), styles["af_z"])


def test_presets_match_fpp_voices():
    p = load_presets()
    assert set(p) == {"nick", "holly"}
    assert p["nick"]["blend"] == pytest.approx({"am_echo": 0.3, "am_fenrir": 0.3, "am_puck": 0.4})
    assert p["holly"]["blend"] == {"af_heart": 0.5, "af_kore": 0.5}
    assert p["holly"]["energy"]["stretch"] == 1.05 and p["nick"]["energy"]["max_lift"] == 9
    assert p["nick"]["speed"] == 1.05 and p["nick"]["default_energy"] == 0.4


def test_dj_voice_json_is_camel_case():
    nick = next(v for v in presets_as_dj_voices() if v["id"] == "nick")
    assert nick["name"] == "Nick" and nick["defaultEnergy"] == 0.4
    assert nick["energy"]["maxLift"] == 9 and "max_lift" not in nick["energy"]
    assert nick["eq"].startswith("equalizer=")
    # camelCase in, same internal form as snake_case
    again = normalize_voice(nick)
    assert again["energy"]["max_lift"] == 9 and again["default_energy"] == 0.4
    assert to_dj_voice(again)["energy"] == nick["energy"]


def test_custom_voice_without_energy_gets_defaults():
    v = normalize_voice({"id": "elf", "name": "Elf", "blend": {"af_sky": 1}})
    assert v["energy"]["lift"] > 0 and v["default_energy"] == 0.4 and v["speed"] == 1.0
    assert v["lang"] == "en-us"
    assert normalize_voice({"id": "g", "blend": {"bf_emma": 1}})["lang"] == "en-gb"


def test_resolve_voice():
    assert resolve_voice("male")["id"] == "nick"
    assert resolve_voice("She")["id"] == "holly"
    assert resolve_voice("af_bella")["blend"] == {"af_bella": 1.0}
    custom = {"elf": {"id": "elf", "name": "Jingle", "blend": {"af_sky": 1}}}
    assert resolve_voice("jingle", custom)["id"] == "elf"
    with pytest.raises(ValueError):
        resolve_voice("rudolph")
    with pytest.raises(ValueError):
        resolve_voice("")


def test_validate_eq():
    ok = "equalizer=f=150:t=q:w=1.0:g=2.5,equalizer=f=3200:t=q:w=1.2:g=2"
    assert validate_eq(ok) == ok
    assert validate_eq(None) is None
    for bad in ("amovie=/etc/passwd", "equalizer=f=1;rm -rf", "ametadata=mode=print:file=/tmp/x"):
        with pytest.raises(ValueError):
            validate_eq(bad)
