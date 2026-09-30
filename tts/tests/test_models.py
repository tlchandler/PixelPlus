import hashlib
import os
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import pytest

from pixelplus_tts.models import ModelFile, auto_variant, download, resolve_variant

PAYLOAD = os.urandom(300_000)


class RangeHandler(BaseHTTPRequestHandler):
    requests = []

    def log_message(self, *a):
        pass

    def do_GET(self):
        rng = self.headers.get("Range")
        RangeHandler.requests.append(rng)
        start = int(rng.split("=")[1].split("-")[0]) if rng else 0
        body = PAYLOAD[start:]
        self.send_response(206 if rng else 200)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


@pytest.fixture
def server(monkeypatch):
    monkeypatch.setenv("PIXELPLUS_TTS_NO_CURL", "1")
    for k in ("http_proxy", "HTTP_PROXY", "https_proxy", "HTTPS_PROXY", "all_proxy", "ALL_PROXY"):
        monkeypatch.delenv(k, raising=False)
    monkeypatch.setenv("no_proxy", "*")
    httpd = ThreadingHTTPServer(("127.0.0.1", 0), RangeHandler)
    threading.Thread(target=httpd.serve_forever, daemon=True).start()
    RangeHandler.requests = []
    yield f"http://127.0.0.1:{httpd.server_address[1]}"
    httpd.shutdown()


def test_download_resumes_and_verifies(tmp_path, server):
    mf = ModelFile("m.onnx", len(PAYLOAD), hashlib.sha256(PAYLOAD).hexdigest())
    (tmp_path / "m.onnx.part").write_bytes(PAYLOAD[:100_000])
    path = download(mf, str(tmp_path), base_url=server, quiet=True)
    assert open(path, "rb").read() == PAYLOAD
    assert RangeHandler.requests == ["bytes=100000-"]
    assert not (tmp_path / "m.onnx.part").exists()
    # second call: verified, no request
    download(mf, str(tmp_path), base_url=server, quiet=True)
    assert len(RangeHandler.requests) == 1


def test_download_rejects_bad_hash(tmp_path, server):
    mf = ModelFile("m.onnx", len(PAYLOAD), "0" * 64)
    with pytest.raises(RuntimeError, match="sha256"):
        download(mf, str(tmp_path), base_url=server, quiet=True)
    assert not (tmp_path / "m.onnx").exists() and not (tmp_path / "m.onnx.part").exists()


def test_variant_choice(tmp_path):
    assert auto_variant(1024) == "int8"   # Pi 4 1GB
    assert auto_variant(1900) == "fp32"   # Pi 4 2GB
    assert auto_variant(8000) == "fp32"
    assert resolve_variant("fp16") == "fp16"
    # auto falls back to whatever is installed
    (tmp_path / "kokoro-v1.0.int8.onnx").write_bytes(b"x")
    if auto_variant() == "fp32":
        assert resolve_variant("auto", str(tmp_path)) == "int8"
