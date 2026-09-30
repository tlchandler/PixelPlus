import json
import threading
import urllib.error
import urllib.request

import numpy as np
import pytest

from conftest import needs_ffmpeg
from pixelplus_tts import audio as au
from pixelplus_tts.render import BadRequest, Cache, audition_request, build_job, render, render_job, \
    substitute_placeholders


def test_placeholders():
    text, missing = substitute_placeholders("Up next, {nextSong}! {daysUntilChristmas} days to go. {showName}",
                                            {"nextSong": "Feliz Navidad", "daysUntilChristmas": 12})
    assert text == "Up next, Feliz Navidad! 12 days to go. show Name"
    assert missing == ["showName"]


def test_build_job_validation():
    with pytest.raises(BadRequest):
        build_job({"lines": []})
    with pytest.raises(BadRequest) as e:
        build_job({"lines": [{"voice": "rudolph", "text": "hi"}]})
    assert e.value.code == "unknown_voice"
    with pytest.raises(BadRequest):
        build_job({"lines": [{"voice": "nick", "text": "hi"}], "speed": 5})
    with pytest.raises(BadRequest):
        build_job({"lines": [{"voice": "nick", "text": "hi"}], "format": "flac"})
    with pytest.raises(BadRequest):
        build_job({"lines": [{"voice": {"id": "x", "blend": {"af_sky": 1}, "eq": "amovie=/etc/passwd"}, "text": "hi"}]})
    with pytest.raises(BadRequest) as e:
        build_job({"lines": [{"voice": "nick", "text": "hi"}], "musicBed": {"path": "/etc/passwd"}},
                  data_dir="/var/lib/pixelplus")
    assert e.value.code == "forbidden_path"


def test_build_job_resolves_voices_and_pronunciations():
    elf = {"id": "elf", "name": "Elf", "blend": {"af_sky": 2, "af_bella": 2}}
    job = build_job({"lines": [{"voice": "elf", "text": "Hi {name}", "pauseMs": 500, "energy": 1},
                               {"voice": {"id": "inline", "blend": {"am_puck": 1}}, "text": "Yo"}],
                     "voices": [elf], "placeholders": {"name": "Noel"},
                     "pronunciations": [{"word": "Noel", "say": "No well"}]})
    assert job.lines[0]["voice"]["blend"] == {"af_sky": 0.5, "af_bella": 0.5}
    assert job.lines[0]["text"] == "Hi Noel" and job.lines[0]["pause_ms"] == 500 and job.lines[0]["energy"] == 1
    assert job.lines[1]["voice"]["id"] == "inline" and job.lines[1]["energy"] is None
    assert dict(job.rules_pairs)["Noel"] == "No well"
    assert job.fmt == "mp3" and job.lufs == -16 and job.speed == 1.0
    no_builtin = build_job({"lines": [{"voice": "nick", "text": "x"}], "builtinPronunciations": False})
    assert no_builtin.rules_pairs == []


def test_cache_key_changes_with_content():
    a = build_job({"lines": [{"voice": "nick", "text": "hi"}]})
    b = build_job({"lines": [{"voice": "nick", "text": "hi!"}]})
    c = build_job({"lines": [{"voice": "nick", "text": "hi"}]})
    assert a.cache_key("fp32") == c.cache_key("fp32") != b.cache_key("fp32")
    assert a.cache_key("fp32") != a.cache_key("int8")


def test_audition_request():
    req = audition_request({"voice": "holly"})
    assert req["lines"][0]["text"].startswith("Hi, I'm Holly!")
    req = audition_request({"voice": {"id": "x", "name": "Jingle", "blend": {"af_sky": 1}}, "text": "Test"})
    assert req["lines"][0]["text"] == "Test"
    with pytest.raises(BadRequest):
        audition_request({})


@needs_ffmpeg
def test_render_job_pauses_and_energy_defaults(fake_engine):
    job = build_job({"lines": [
        {"voice": "", "text": "", "pauseMs": 400},          # leading pause
        {"voice": "nick", "text": "abcde"},                  # default gap follows
        {"voice": "holly", "text": "abcde", "pauseMs": 1000, "energy": 1.5},
        {"voice": "af_heart", "text": "abcde"},              # last line: no gap
    ], "fx": False, "speed": 1.0})
    mix, _ = render_job(fake_engine, job)
    assert [c["energy"] for c in fake_engine.calls] == [0.4, 1.5, 0.4]
    assert fake_engine.calls[0]["speed"] == pytest.approx(1.05)
    assert fake_engine.calls[2]["speed"] == pytest.approx(1.0)
    speech = sum(round(0.06 * 5 / s * 24000) for s in (1.05, 1.05, 1.0)) / 24000
    expected = 0.4 + 0.35 + 1.0 + speech
    assert len(mix) / 44100 == pytest.approx(expected, abs=0.01)
    # leading pause is silent
    assert np.abs(mix[: int(0.39 * 44100)]).max() == 0


@needs_ffmpeg
def test_render_hits_target_loudness_and_caches(tmp_path, fake_engine):
    cache = Cache(str(tmp_path), 10)
    job = build_job({"lines": [{"voice": "nick", "text": "x" * 40}], "format": "wav", "loudnessLufs": -18})
    res = render(fake_engine, job, cache)
    assert res.content_type == "audio/wav" and res.data[:4] == b"RIFF"
    assert res.loudness_lufs == pytest.approx(-18, abs=0.6)
    assert res.duration_ms == pytest.approx(0.25 * 1000 + 40 * 60 / 1.05 + 600, abs=40)
    again = render(fake_engine, job, cache)
    assert again.cached and again.data == res.data and len(fake_engine.calls) == 1


def test_duck_envelope():
    active = np.array([0] * 20 + [1] * 10 + [0] * 5 + [1] * 10 + [0] * 60, dtype=float)
    env = au.duck_envelope(active, 12.0, win_s=0.02, attack_s=0.1, release_s=0.3, hold_s=0.2)
    assert env[0] == 0 and env[25] == -12
    assert env[32] == -12  # short gap between words is held
    assert -12 < env[17] < 0  # look-ahead ramp before speech
    assert env[-1] > -0.5  # released after speech
    assert np.all(np.diff(env[46:]) >= -1e-9)  # release only goes up


@needs_ffmpeg
def test_music_bed_mix(tmp_path):
    rate = 44100
    speech = np.zeros((rate * 2, 2), np.float32)
    speech[rate // 2: rate] = 0.3
    bed = (0.1 * np.sin(2 * np.pi * 440 * np.arange(rate) / rate)).astype(np.float32)[:, None].repeat(2, 1)
    out = au.mix_music_bed(speech, bed, duck_db=12, bed_gain_db=-6, target_lufs=-16, intro_s=1.0, outro_s=1.0)
    assert len(out) == rate * 4
    lead = int(rate * 1.0)

    def bed_rms(a, b):
        return float(np.sqrt(np.mean((out[a:b] - np.pad(speech, ((lead, rate), (0, 0)))[a:b]) ** 2)))
    quiet = bed_rms(int(rate * 0.5), int(rate * 0.9))
    ducked = bed_rms(lead + int(rate * 0.6), lead + int(rate * 0.9))
    assert 20 * np.log10(ducked / quiet) == pytest.approx(-12, abs=1.5)


@needs_ffmpeg
def test_http_server_end_to_end(tmp_path, fake_engine):
    from http.server import ThreadingHTTPServer

    from pixelplus_tts.config import Config
    from pixelplus_tts.server import App, make_handler
    cfg = Config(data_dir=str(tmp_path), idle_minutes=0, port=0)
    app = App(cfg)
    app.engine = fake_engine
    httpd = ThreadingHTTPServer(("127.0.0.1", 0), make_handler(app))
    threading.Thread(target=httpd.serve_forever, daemon=True).start()
    base = f"http://127.0.0.1:{httpd.server_address[1]}"

    def call(path, body=None):
        req = urllib.request.Request(base + path, data=None if body is None else json.dumps(body).encode(),
                                     headers={"Content-Type": "application/json"})
        with urllib.request.urlopen(req) as r:
            return r.status, dict(r.headers), r.read()

    try:
        _, _, b = call("/health")
        h = json.loads(b)
        assert h["ok"] and h["device"] == "cpu" and h["modelVariant"] == "fp32" and "modelLoaded" in h
        _, _, b = call("/voices")
        v = json.loads(b)
        assert len(v["base"]) == 54 and {p["id"] for p in v["presets"]} == {"nick", "holly"}
        assert v["presets"][0]["defaultEnergy"] == 0.4
        _, _, b = call("/parse", {"script": "nick!: hi\n[pause 1]\nf: yo"})
        assert json.loads(b)["lines"] == [{"voice": "nick", "text": "hi", "pauseMs": 1000, "energy": 1.0},
                                          {"voice": "holly", "text": "yo", "pauseMs": 0}]
        status, headers, b = call("/render", {"lines": [{"voice": "nick", "text": "Up next {nextSong}"}],
                                              "placeholders": {}})
        assert status == 200 and headers["Content-Type"] == "audio/mpeg" and len(b) > 1000
        assert int(headers["X-Duration-Ms"]) > 1000 and headers["X-Cache"] == "miss"
        assert "nextSong" in headers["X-Warnings"]
        status, headers, b = call("/audition", {"voice": "af_heart"})
        assert status == 200 and headers["X-Cache"] == "miss"
        _, headers, _ = call("/audition", {"voice": "af_heart"})
        assert headers["X-Cache"] == "hit"
        with pytest.raises(urllib.error.HTTPError) as e:
            call("/render", {"lines": [{"voice": "rudolph", "text": "hi"}]})
        assert e.value.code == 400
        assert json.loads(e.value.read())["error"]["code"] == "unknown_voice"
        with pytest.raises(urllib.error.HTTPError) as e:
            call("/parse", {"script": "no voice here"})
        assert json.loads(e.value.read())["error"]["code"] == "parse_error"
    finally:
        httpd.shutdown()
