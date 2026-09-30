"""Regression tests from the adversarial review: request limits, loopback-only service,
voice validation, bounded resources."""
import http.client
import json
import os
import socket
import threading
from http.server import ThreadingHTTPServer

import numpy as np
import pytest

from pixelplus_tts import audio as au
from pixelplus_tts import engine as eng
from pixelplus_tts import server
from pixelplus_tts.config import Config
from pixelplus_tts.render import MAX_TOTAL_TEXT, BadRequest, build_job, substitute_placeholders
from pixelplus_tts.voices import normalize_voice, validate_eq

from conftest import FakeEngine, needs_ffmpeg


@pytest.fixture
def svc(tmp_path):
    cfg = Config(data_dir=str(tmp_path), idle_minutes=0, cache_mb=0)
    app = server.App(cfg)
    app.engine = FakeEngine()
    httpd = ThreadingHTTPServer(("127.0.0.1", 0), server.make_handler(app))
    httpd.daemon_threads = True
    t = threading.Thread(target=httpd.serve_forever, daemon=True)
    t.start()
    yield httpd.server_address[1]
    httpd.shutdown()
    httpd.server_close()


def req(port, method, path, body=None, headers=None):
    c = http.client.HTTPConnection("127.0.0.1", port, timeout=10)
    data = body if isinstance(body, (bytes, type(None))) else json.dumps(body).encode()
    c.request(method, path, body=data, headers=headers or {})
    r = c.getresponse()
    out = r.status, r.read()
    c.close()
    return out


def raw(port, data, timeout=5):
    s = socket.create_connection(("127.0.0.1", port), timeout=timeout)
    s.sendall(data)
    chunks = []
    try:
        while True:
            b = s.recv(65536)
            if not b:
                break
            chunks.append(b)
    except socket.timeout:
        pass
    s.close()
    return b"".join(chunks)


# ------------------------------------------------------------------ server --

def test_health_from_loopback(svc):
    status, body = req(svc, "GET", "/health")
    assert status == 200 and json.loads(body)["ok"]


def test_foreign_host_header_is_refused(svc):
    """DNS rebinding: a website whose name resolves to 127.0.0.1 reaches the service with its own Host."""
    status, body = req(svc, "GET", "/health", headers={"Host": "evil.example:7081"})
    assert status == 403
    assert req(svc, "GET", "/health", headers={"Host": "localhost:7081"})[0] == 200
    assert req(svc, "GET", "/health", headers={"Host": "[::1]:7081"})[0] == 200


def test_browser_origin_is_refused(svc):
    """A web page in a browser on the same machine could POST renders (CSRF)."""
    status, _ = req(svc, "POST", "/parse", {"script": "nick: hi"}, {"Origin": "https://evil.example"})
    assert status == 403


def test_negative_content_length_does_not_read_to_eof(svc):
    out = raw(svc, b"POST /parse HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: -1\r\n\r\n{}", timeout=5)
    assert out.startswith(b"HTTP/1.1 400")


def test_oversized_body_closes_the_connection(svc):
    """The unread body must not be parsed as the next request on a keep-alive connection."""
    smuggled = b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"
    head = ("POST /parse HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: %d\r\n\r\n" % (server.MAX_BODY + 1)).encode()
    out = raw(svc, head + smuggled, timeout=5)
    assert out.count(b"HTTP/1.1 ") == 1 and b"too_large" in out


def test_deeply_nested_json_is_a_400(svc):
    status, body = req(svc, "POST", "/parse", b"[" * 100000 + b"]" * 100000)
    assert status == 400


def test_serve_refuses_non_loopback(monkeypatch, tmp_path):
    monkeypatch.delenv("PIXELPLUS_TTS_ALLOW_REMOTE", raising=False)
    with pytest.raises(SystemExit):
        server.serve(Config(host="0.0.0.0", port=0, data_dir=str(tmp_path), idle_minutes=0))
    assert server.is_loopback("127.0.0.1") and server.is_loopback("::1") and server.is_loopback("localhost")
    assert not server.is_loopback("192.168.1.2")


# ------------------------------------------------------------------ render --

def test_placeholder_values_count_toward_the_text_limit():
    with pytest.raises(BadRequest):
        build_job({"lines": [{"voice": "nick", "text": "{x} " * 20}], "placeholders": {"x": "la " * 100}})
    # ... and are capped individually, non-scalars are not spoken
    text, missing = substitute_placeholders("{a} {b}", {"a": "y" * 1000, "b": {"nested": 1}})
    assert len(text) < 400 and missing == ["b"]


def test_total_text_is_bounded():
    lines = [{"voice": "nick", "text": "x" * 1900} for _ in range(MAX_TOTAL_TEXT // 1900 + 1)]
    with pytest.raises(BadRequest) as e:
        build_job({"lines": lines})
    assert e.value.code == "too_large"


def test_fx_false_string():
    assert build_job({"lines": [{"voice": "nick", "text": "hi"}], "fx": "false"}).fx is False


def test_music_bed_symlink_out_of_the_data_dir(tmp_path):
    data = tmp_path / "data"
    data.mkdir()
    outside = tmp_path / "secret.mp3"
    outside.write_bytes(b"x")
    (data / "bed.mp3").symlink_to(outside)
    with pytest.raises(BadRequest) as e:
        build_job({"lines": [{"voice": "nick", "text": "hi"}], "musicBed": {"path": str(data / "bed.mp3")}},
                  data_dir=str(data))
    assert e.value.code == "forbidden_path"
    with pytest.raises(BadRequest):
        build_job({"lines": [{"voice": "nick", "text": "hi"}], "musicBed": {"path": str(data) + "/../secret.mp3"}},
                  data_dir=str(data))


@needs_ffmpeg
def test_music_bed_decode_is_bounded(tmp_path):
    p = tmp_path / "long.wav"
    tone = (0.1 * np.sin(np.arange(44100 * 20) / 10)).astype(np.float32)
    p.write_bytes(au.encode(np.stack([tone, tone], 1), "wav"))
    assert len(au.decode(str(p), max_seconds=2.0)) == pytest.approx(2 * 44100, abs=100)


# ------------------------------------------------------------------ voices --

@pytest.mark.parametrize("field", [{"speed": float("nan")}, {"speed": float("inf")},
                                   {"energy": {"lift": float("nan")}}, {"defaultEnergy": float("inf")}])
def test_non_finite_voice_numbers_are_rejected(field):
    with pytest.raises(ValueError):
        normalize_voice({"id": "x", "blend": {"af_heart": 1}, **field})


def test_voice_numbers_are_clamped():
    v = normalize_voice({"id": "x", "blend": {"af_heart": 1}, "speed": 0.0001,
                         "energy": {"lift": 1000, "stretch": 0, "maxLift": -5}, "defaultEnergy": 50})
    assert v["speed"] == 0.25 and v["default_energy"] == 2.0
    assert v["energy"]["lift"] == 24 and v["energy"]["stretch"] == 0.25 and v["energy"]["max_lift"] == 0


def test_voice_lang_is_a_language_code():
    assert normalize_voice({"blend": {"af_heart": 1}, "lang": "en-gb"})["lang"] == "en-gb"
    for bad in ("en us; rm -rf", "--help", "x" * 50):
        with pytest.raises(ValueError):
            normalize_voice({"blend": {"af_heart": 1}, "lang": bad})


def test_eq_is_bounded():
    with pytest.raises(ValueError):
        validate_eq("equalizer=f=100:t=q:w=1:g=1," * 200)
    with pytest.raises(ValueError):
        validate_eq(["equalizer"])
    for bad in ("equalizer=f=1[out];[out]amovie=/etc/passwd", "amovie=/etc/passwd", "volume='1'", "aeval=exprs=0"):
        with pytest.raises(ValueError):
            validate_eq(bad)


# ------------------------------------------------------------------ engine --

def test_blended_styles_cache_is_bounded(tmp_path):
    e = eng.Engine(str(tmp_path), "fp32", 1, idle_minutes=0)

    class K:
        def get_voices(self):
            return ["af_heart", "af_kore"]

        def get_voice_style(self, vid):
            return np.ones((510, 1, 256), np.float32)

    e._k = K()
    for i in range(eng.MAX_STYLES + 20):
        e.style({"af_heart": 1.0, "af_kore": (i + 1) / 1000})
    assert len(e._styles) == eng.MAX_STYLES
