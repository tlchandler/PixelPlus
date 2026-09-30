"""Loopback HTTP service (stdlib only). See tts/README.md for the API."""
from __future__ import annotations

import json
import logging
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any

from . import __version__
from .config import Config
from .engine import Engine, ModelMissing
from .pronounce import builtin_pronunciations
from .render import BadRequest, Cache, audition_request, build_job, lookup, render
from .script import ScriptError, parse_script
from .voices import list_base_voices, presets_as_dj_voices, resolve_voice

log = logging.getLogger("pixelplus_tts")
MAX_BODY = 1 << 20


class App:
    def __init__(self, cfg: Config):
        self.cfg = cfg
        self.engine = Engine(cfg.models_dir, cfg.variant, cfg.threads, cfg.idle_minutes)
        self.cache = Cache(cfg.cache_dir, cfg.cache_mb)
        self.started = time.time()
        self._renders = 0
        self._render_lock = threading.Lock()  # one render at a time (CPU; don't starve the lights)
        self._queue_lock = threading.Lock()
        self._waiting = 0
        self.max_queue = 4

    def known_base(self) -> set[str] | None:
        ids = self.engine.base_voice_ids()
        return ids or None

    # ------------------------------------------------------------- routes --
    def health(self) -> dict[str, Any]:
        return {"ok": True, "version": __version__, "modelLoaded": self.engine.loaded,
                "modelAvailable": self.engine.available, "device": "cpu",
                "modelVariant": self.engine.variant, "threads": self.engine.threads,
                "idleUnloadMinutes": self.cfg.idle_minutes, "renders": self._renders, "queued": self._waiting,
                "uptimeS": int(time.time() - self.started)}

    def voices(self) -> dict[str, Any]:
        return {"base": list_base_voices(self.engine.base_voice_ids()), "presets": presets_as_dj_voices()}

    def parse(self, body: dict) -> dict[str, Any]:
        script = body.get("script")
        if not isinstance(script, str):
            raise BadRequest("script (string) is required")
        custom = {str(v["id"]): v for v in body.get("voices") or [] if isinstance(v, dict) and v.get("id")}
        base = self.known_base()

        def resolve(name: str) -> str:
            return resolve_voice(name, custom, base)["id"]

        try:
            return {"lines": parse_script(script, resolve)}
        except ScriptError as e:
            raise BadRequest(str(e), "parse_error") from None

    def do_render(self, body: dict, audition: bool = False):
        if audition:
            body = audition_request(body)
        job = build_job(body, known_base=self.known_base(), data_dir=self.cfg.data_dir,
                        allow_any_path=self.cfg.allow_any_path)
        use_cache = audition or body.get("cache", True) is not False
        hit = lookup(self.cache, job, self.engine.variant) if use_cache else None
        if hit:  # served without waiting for a running render
            return hit
        with self._queue_lock:
            if self._waiting >= self.max_queue:
                raise BadRequest("renderer is busy, try again shortly", "busy")
            self._waiting += 1
        try:
            with self._render_lock:
                res = render(self.engine, job, self.cache, use_cache=use_cache)
        finally:
            with self._queue_lock:
                self._waiting -= 1
        self._renders += 1
        return res


def make_handler(app: App):
    class Handler(BaseHTTPRequestHandler):
        server_version = f"pixelplus-tts/{__version__}"
        protocol_version = "HTTP/1.1"

        def log_message(self, fmt, *args):  # route to logging
            log.debug("%s " + fmt, self.address_string(), *args)

        def _send(self, status: int, body: bytes, ctype: str, headers: dict[str, str] | None = None):
            self.send_response(status)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Cache-Control", "no-store")
            for k, v in (headers or {}).items():
                self.send_header(k, v)
            self.end_headers()
            if self.command != "HEAD":
                self.wfile.write(body)

        def _json(self, status: int, obj: Any):
            self._send(status, json.dumps(obj, ensure_ascii=False).encode(), "application/json; charset=utf-8")

        def _error(self, status: int, code: str, message: str):
            self._json(status, {"error": {"code": code, "message": message}})

        def _body(self) -> dict:
            n = int(self.headers.get("Content-Length") or 0)
            if n > MAX_BODY:
                raise BadRequest("request body too large", "too_large")
            raw = self.rfile.read(n) if n else b"{}"
            try:
                obj = json.loads(raw or b"{}")
            except ValueError:
                raise BadRequest("body is not valid JSON") from None
            if not isinstance(obj, dict):
                raise BadRequest("body must be a JSON object")
            return obj

        def do_GET(self):
            path = self.path.split("?", 1)[0].rstrip("/") or "/"
            try:
                if path == "/health":
                    return self._json(200, app.health())
                if path == "/voices":
                    return self._json(200, app.voices())
                if path == "/pronunciations":
                    return self._json(200, {"builtin": [{"word": w, "say": s} for w, s in builtin_pronunciations()]})
                self._error(404, "not_found", f"no route {path}")
            except Exception as e:  # pragma: no cover
                log.exception("GET %s failed", path)
                self._error(500, "internal", str(e))

        do_HEAD = do_GET

        def do_POST(self):
            path = self.path.split("?", 1)[0].rstrip("/")
            try:
                body = self._body()
                if path in ("/render", "/audition"):
                    res = app.do_render(body, audition=path == "/audition")
                    headers = {"X-Duration-Ms": str(res.duration_ms), "X-Render-Ms": str(res.render_ms),
                               "X-Cache": "hit" if res.cached else "miss",
                               "X-Model-Variant": app.engine.variant}
                    if res.loudness_lufs is not None:
                        headers["X-Loudness-Lufs"] = f"{res.loudness_lufs:.1f}"
                    if res.warnings:
                        headers["X-Warnings"] = json.dumps(res.warnings, ensure_ascii=True)
                    return self._send(200, res.data, res.content_type, headers)
                if path == "/parse":
                    return self._json(200, app.parse(body))
                if path == "/warmup":
                    t = time.time()
                    app.engine.load()
                    return self._json(200, {"ok": True, "modelLoaded": True, "loadMs": int((time.time() - t) * 1000)})
                if path == "/unload":
                    return self._json(200, {"ok": True, "unloaded": app.engine.unload()})
                self._error(404, "not_found", f"no route {path}")
            except BadRequest as e:
                self._error(503 if e.code == "busy" else 400, e.code, str(e))
            except ModelMissing as e:
                self._error(503, "model_missing", str(e))
            except ValueError as e:
                self._error(400, "bad_request", str(e))
            except Exception as e:
                log.exception("POST %s failed", path)
                self._error(500, "render_failed", str(e))

    return Handler


def serve(cfg: Config) -> None:
    app = App(cfg)
    httpd = ThreadingHTTPServer((cfg.host, cfg.port), make_handler(app))
    httpd.daemon_threads = True
    log.info("pixelplus-tts %s on http://%s:%d (model %s, %s)", __version__, cfg.host, cfg.port,
             app.engine.variant, "present" if app.engine.available else "MISSING")
    try:
        httpd.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        httpd.server_close()
