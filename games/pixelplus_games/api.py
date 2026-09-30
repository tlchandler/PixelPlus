"""Small client for pixelplusd's HTTP API (``/api/v1``).

The sidecar runs next to pixelplusd and talks to it over loopback.  Every
request carries ``X-PixelPlus-Local: 1``, which pixelplusd honours only for
connections from 127.0.0.1 / ::1 so that a local service needs no session
even when the UI is password protected.
"""

import json
import logging
import urllib.error
import urllib.parse
import urllib.request

log = logging.getLogger("pixelplus_games.api")

LOCAL_HEADER = "X-PixelPlus-Local"
PREFIX = "/api/v1"


class ApiError(Exception):
    """pixelplusd answered with an error, or could not be reached."""

    def __init__(self, message, status=None, code=None):
        super().__init__(message)
        self.status = status
        self.code = code


class PixelPlus:
    def __init__(self, base="http://127.0.0.1"):
        self.base = base.rstrip("/")

    def is_local(self):
        host = urllib.parse.urlparse(self.base).hostname or ""
        return host in ("127.0.0.1", "localhost", "::1")

    def ws_url(self):
        u = urllib.parse.urlparse(self.base)
        scheme = "wss" if u.scheme == "https" else "ws"
        return "%s://%s%s/ws" % (scheme, u.netloc, PREFIX)

    def request(self, method, path, body=None, content_type="application/json", timeout=5):
        """Send a request to ``/api/v1<path>``; return decoded JSON (or text / None).

        Raises :class:`ApiError` on HTTP errors and connection problems.
        """
        data = None
        if body is not None:
            data = body if isinstance(body, (bytes, bytearray)) else json.dumps(body).encode()
        req = urllib.request.Request(self.base + PREFIX + path, data=data, method=method)
        req.add_header(LOCAL_HEADER, "1")
        req.add_header("Accept", "application/json")
        if data is not None:
            req.add_header("Content-Type", content_type)
        try:
            with urllib.request.urlopen(req, timeout=timeout) as r:
                raw = r.read()
        except urllib.error.HTTPError as e:
            message, code = _describe(e)
            raise ApiError(message, status=e.code, code=code) from None
        except (OSError, ValueError) as e:
            raise ApiError("pixelplusd is not reachable at %s: %s" % (self.base, e)) from None
        if not raw:
            return None
        try:
            return json.loads(raw)
        except ValueError:
            return raw.decode(errors="replace")

    # --- show & player --------------------------------------------------------

    def show(self):
        """The full Show document. Raises ApiError."""
        s = self.request("GET", "/show", timeout=10)
        if not isinstance(s, dict):
            raise ApiError("GET /show did not return a show")
        return s

    def player(self):
        """PlayerStatus, or None if pixelplusd is unreachable."""
        try:
            p = self.request("GET", "/player")
            return p if isinstance(p, dict) else None
        except ApiError as e:
            log.warning("Could not read the player state: %s", e)
            return None

    def player_state(self):
        """'idle' | 'playing' | 'paused' | 'testing' | 'effect', or 'unknown'."""
        return (self.player() or {}).get("state", "unknown")

    def _player_cmd(self, what, body=None):
        try:
            self.request("POST", "/player/" + what, body if body is not None else {})
            log.info("Player: %s", what)
            return True
        except ApiError as e:
            log.warning("Player %s failed: %s", what, e)
            return False

    def pause(self):
        return self._player_cmd("pause")

    def resume(self):
        return self._player_cmd("resume")

    def stop(self):
        return self._player_cmd("stop", {"fade": False})

    # --- overlays (ARCHITECTURE.md section 10) -----------------------------------

    def _overlay(self, prop_id):
        return "/overlay/" + urllib.parse.quote(prop_id, safe="")

    def overlay_open(self, prop_id):
        """Ask for the prop's shared-memory buffer: {shm, width, height}. Raises ApiError."""
        r = self.request("POST", self._overlay(prop_id) + "/open", {})
        if not isinstance(r, dict) or "shm" not in r:
            raise ApiError("overlay open returned no shared memory path")
        return r

    def overlay_enable(self, prop_id, enabled):
        try:
            self.request("POST", self._overlay(prop_id), {"enabled": bool(enabled)})
            return True
        except ApiError as e:
            log.warning("Could not %s the overlay on %s: %s", "enable" if enabled else "disable", prop_id, e)
            return False

    def overlay_frame(self, prop_id, rgb):
        """HTTP fallback: one raw RGB frame (width*height*3 bytes)."""
        try:
            self.request("PUT", self._overlay(prop_id) + "/frame", bytes(rgb),
                         "application/octet-stream", timeout=2)
            return True
        except ApiError as e:
            log.debug("Frame upload to %s failed: %s", prop_id, e)
            return False


def _describe(e):
    """(message, code) from an HTTPError carrying PixelPlus's error envelope."""
    try:
        body = json.loads(e.read() or b"{}")
    except (ValueError, OSError):
        body = {}
    err = body.get("error") if isinstance(body, dict) else None
    if isinstance(err, dict) and err.get("message"):
        return "%s (HTTP %d)" % (err["message"], e.code), err.get("code")
    return "HTTP %d %s" % (e.code, e.reason), None
