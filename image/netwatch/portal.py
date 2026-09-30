"""Captive-portal HTTP server for the PixelPlus setup hotspot (stdlib only).

Routes
------
``GET  /``                  the Wi-Fi picker page (portal/index.html)
``GET  /api/status``        {state, hostname, hotspotSsid, lastError, scanAgeS, stay, countries, country}
``GET  /api/scan[?rescan=1]`` [{ssid, signal, secure, security, wpa3Only, enterprise}]
``POST /api/connect``       {ssid, password, hidden, country} -> {ok, hostname, url} | {ok:false, error}
``POST /api/stay``          keep the hotspot and use PixelPlus without Wi-Fi -> {ok, url}

Anything else - including the OS "am I online?" probes (Apple ``/hotspot-detect.html``,
Android ``/generate_204``, Windows ``/connecttest.txt``, Firefox ``/canonical.html``,
...) and any foreign Host header - gets a ``302`` to ``http://10.42.0.1/``, which makes
phones pop up the sign-in sheet.
"""

from __future__ import annotations

import json
import logging
import os
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Dict, List, Optional
from urllib.parse import parse_qs, urlsplit

LOG = logging.getLogger("pixelplus-portal")
HERE = os.path.dirname(os.path.abspath(__file__))
STATIC_DIRS = [os.path.join(HERE, "portal"), "/usr/lib/pixelplus/netwatch/portal"]
PORTAL_ADDR = "10.42.0.1"
MAX_BODY = 8192

COMMON_COUNTRIES = [
    ("US", "United States"), ("CA", "Canada"), ("GB", "United Kingdom"), ("IE", "Ireland"),
    ("AU", "Australia"), ("NZ", "New Zealand"), ("DE", "Germany"), ("FR", "France"),
    ("NL", "Netherlands"), ("BE", "Belgium"), ("ES", "Spain"), ("IT", "Italy"),
    ("SE", "Sweden"), ("NO", "Norway"), ("DK", "Denmark"), ("FI", "Finland"),
    ("CH", "Switzerland"), ("AT", "Austria"), ("PL", "Poland"), ("MX", "Mexico"),
    ("BR", "Brazil"), ("ZA", "South Africa"), ("JP", "Japan"), ("IN", "India"),
]


def load_countries(path: str = "/usr/share/zoneinfo/iso3166.tab") -> List[List[str]]:
    try:
        with open(path, encoding="utf-8") as f:
            rows = [ln.rstrip("\n").split("\t", 1) for ln in f if ln.strip() and not ln.startswith("#")]
        rows = [r for r in rows if len(r) == 2]
        return sorted(rows, key=lambda r: r[1])
    except OSError:
        return [list(c) for c in sorted(COMMON_COUNTRIES, key=lambda r: r[1])]


def current_country() -> Optional[str]:
    """Regulatory domain already configured (cmdline.txt, as set by raspi-config / Imager)."""
    for path in ("/proc/cmdline",):
        try:
            with open(path, encoding="ascii", errors="replace") as f:
                for tok in f.read().split():
                    if tok.startswith("cfg80211.ieee80211_regdom="):
                        return tok.split("=", 1)[1][:2].upper()
        except OSError:
            pass
    return None


def read_static(name: str) -> Optional[bytes]:
    for d in STATIC_DIRS:
        p = os.path.join(d, name)
        if os.path.isfile(p):
            with open(p, "rb") as f:
                return f.read()
    return None


class Handler(BaseHTTPRequestHandler):
    server_version = "PixelPlusSetup/1"
    protocol_version = "HTTP/1.1"

    # quieter logs
    def log_message(self, fmt, *args):  # noqa: D401
        LOG.debug("%s %s", self.address_string(), fmt % args)

    @property
    def ctl(self):
        return self.server.ctl  # type: ignore[attr-defined]

    def _host_is_ours(self) -> bool:
        host = (self.headers.get("Host") or "").split(":")[0].lower()
        ours = {PORTAL_ADDR, "localhost", "127.0.0.1", "pixelplus.setup"}
        hn = getattr(self.ctl, "hostname", "")
        if hn:
            ours |= {hn.lower(), f"{hn.lower()}.local"}
        extra = getattr(self.server, "extra_hosts", ())
        return host in ours or host in extra

    def _send(self, code: int, body: bytes, ctype: str, extra: Optional[Dict[str, str]] = None) -> None:
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store, no-cache, must-revalidate")
        self.send_header("Pragma", "no-cache")
        self.send_header("Connection", "close")
        for k, v in (extra or {}).items():
            self.send_header(k, v)
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(body)

    def _json(self, obj, code: int = 200) -> None:
        self._send(code, json.dumps(obj).encode("utf-8"), "application/json; charset=utf-8")

    def _redirect(self) -> None:
        url = f"http://{getattr(self.server, 'public_host', PORTAL_ADDR)}/"
        body = (
            f'<!doctype html><html><head><meta http-equiv="refresh" content="0; url={url}"></head>'
            f'<body><a href="{url}">PixelPlus setup</a></body></html>'
        ).encode()
        self._send(302, body, "text/html; charset=utf-8", {"Location": url})

    def do_HEAD(self):  # noqa: N802
        self.do_GET()

    def do_GET(self):  # noqa: N802
        parts = urlsplit(self.path)
        path = parts.path
        if not self._host_is_ours():
            return self._redirect()
        if path in ("/", "/index.html", "/setup"):
            body = read_static("index.html")
            if body is None:
                return self._send(500, b"setup page missing", "text/plain")
            return self._send(200, body, "text/html; charset=utf-8")
        if path == "/api/status":
            st = dict(self.ctl.portal_status())
            st["countries"] = self.server.countries  # type: ignore[attr-defined]
            st["country"] = current_country()
            return self._json(st)
        if path == "/api/scan":
            rescan = parse_qs(parts.query).get("rescan", ["0"])[0] in ("1", "true", "yes")
            return self._json(self.ctl.portal_scan(rescan))
        if path in ("/favicon.ico", "/favicon.svg"):
            body = read_static("favicon.svg")
            if body:
                return self._send(200, body, "image/svg+xml")
            return self._send(404, b"", "text/plain")
        return self._redirect()

    def do_POST(self):  # noqa: N802
        path = urlsplit(self.path).path
        if not self._host_is_ours():
            return self._redirect()
        try:
            n = int(self.headers.get("Content-Length") or 0)
        except ValueError:
            n = 0
        if n > MAX_BODY:
            return self._json({"ok": False, "error": "request too large"}, 413)
        raw = self.rfile.read(n) if n else b""
        try:
            data = json.loads(raw.decode("utf-8") or "{}")
            if not isinstance(data, dict):
                raise ValueError
        except (ValueError, UnicodeDecodeError):
            return self._json({"ok": False, "error": "bad request"}, 400)
        if path == "/api/connect":
            ssid = data.get("ssid")
            pw = data.get("password") or ""
            if not isinstance(ssid, str) or not ssid or len(ssid.encode()) > 32 or any(ord(c) < 32 for c in ssid):
                return self._json({"ok": False, "error": "Please choose a network."}, 400)
            if not isinstance(pw, str) or (pw and not (8 <= len(pw.encode()) <= 63 or (len(pw) == 64 and all(c in "0123456789abcdefABCDEF" for c in pw)))):
                return self._json({"ok": False, "error": "Wi-Fi passwords are 8 to 63 characters."}, 400)
            if any(ord(c) < 32 for c in pw):
                return self._json({"ok": False, "error": "The password contains invalid characters."}, 400)
            country = data.get("country")
            if country is not None and (not isinstance(country, str) or len(country) != 2 or not country.isalpha()):
                country = None
            res = self.ctl.portal_connect(ssid, pw, bool(data.get("hidden")), country)
            return self._json(res, 200 if res.get("ok") else 409)
        if path == "/api/stay":
            return self._json(self.ctl.portal_stay())
        return self._json({"ok": False, "error": "not found"}, 404)


class PortalServer(ThreadingHTTPServer):
    daemon_threads = True
    allow_reuse_address = True

    def __init__(self, addr, ctl, public_host: str = PORTAL_ADDR, extra_hosts=()):
        self.ctl = ctl
        self.public_host = public_host
        self.extra_hosts = set(extra_hosts)
        self.countries = load_countries()
        super().__init__(addr, Handler)


# ---------------------------------------------------------------------------
# development / test mode
# ---------------------------------------------------------------------------

class DemoController:
    """Fake netwatch used by ``netwatch.py --portal-only`` and the unit tests."""

    hostname = "pixelplus"

    def __init__(self):
        self.connected: List[Dict] = []
        self.stayed = False
        self.last_error: Optional[str] = None
        self.nets = [
            {"ssid": "Chandler Home", "signal": 82, "secure": True, "security": "WPA2", "wpa3Only": False, "enterprise": False},
            {"ssid": "Chandler Home 5G", "signal": 64, "secure": True, "security": "WPA2 WPA3", "wpa3Only": False, "enterprise": False},
            {"ssid": "Garage Lights", "signal": 41, "secure": True, "security": "WPA3", "wpa3Only": True, "enterprise": False},
            {"ssid": "xfinitywifi", "signal": 30, "secure": False, "security": "", "wpa3Only": False, "enterprise": False},
            {"ssid": "Neighbor's Wi-Fi ✨", "signal": 12, "secure": True, "security": "WPA2", "wpa3Only": False, "enterprise": False},
        ]

    def portal_status(self) -> Dict:
        return {"state": "hotspot", "hostname": self.hostname, "hotspotSsid": "PixelPlus-3F2A",
                "lastError": self.last_error, "scanAgeS": 4, "stay": self.stayed}

    def portal_scan(self, rescan: bool) -> List[Dict]:
        return list(self.nets)

    def portal_connect(self, ssid, password, hidden, country) -> Dict:
        self.connected.append({"ssid": ssid, "password": password, "hidden": hidden, "country": country})
        return {"ok": True, "hostname": self.hostname, "url": f"http://{self.hostname}.local/"}

    def portal_stay(self) -> Dict:
        self.stayed = True
        return {"ok": True, "url": f"http://{PORTAL_ADDR}/"}


def serve_demo(host: str, port: int) -> int:
    srv = PortalServer((host, port), DemoController(), public_host=f"{host}:{port}", extra_hosts={host})
    print(f"PixelPlus setup page (demo data): http://{host}:{port}/")
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        pass
    return 0
