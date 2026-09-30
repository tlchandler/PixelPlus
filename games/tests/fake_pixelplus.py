"""A stand-in for pixelplusd's HTTP API, for running the games sidecar off a real player.

Implements only what the sidecar uses, and creates the overlay's shared-memory
buffer the way pixelplusd does, so the whole sidecar runs on any Linux box:

    python3 games/tests/fake_pixelplus.py 18080 [WIDTH HEIGHT]
    cd games && PIXELPLUS_API=http://127.0.0.1:18080 PIXELPLUS_DATA_DIR=/tmp/pp \\
        PIXELPLUS_GAMES_SOCKET=/tmp/pp/games.sock python3 -m pixelplus_games

Like pixelplusd with a password set, it answers 401 unless a request carries
``X-PixelPlus-Local: <token>`` with the token it wrote to a file (exported as
``PIXELPLUS_LOCAL_TOKEN_FILE`` when started), and 403 for POST/PUT without
``X-PixelPlus-Request: 1``.  Test helpers live under ``/fake/``:

    GET  /fake/state              everything recorded so far
    POST /fake/player/<state>     pretend the scheduler changed the show state
    POST /fake/games              merge a JSON patch into settings.games (bumps the version)
"""

import base64
import copy
import hashlib
import json
import os
import re
import signal
import socket
import struct
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

WS_GUID = b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11"
LOCAL_HEADER = "X-PixelPlus-Local"
REQUEST_HEADER = "X-PixelPlus-Request"


def default_games(prop_id, port=8088):
    """settings.games as pixelplusd serialises GameSettings (camelCase)."""
    return {
        "enabled": True, "matrixPropId": prop_id, "port": port, "gameSeconds": 60,
        "cooldownMinutes": 5, "levels": "", "playWindow": "duringShow", "pauseShow": True,
        "santaHat": True, "arcadeMode": False, "arcadeMinutes": 0, "arcadeIdleSeconds": 600,
        "publicUrl": "mario.example.com", "inviteEveryMinutes": 0, "inviteStyle": "text",
        "inviteFlashes": 3, "inviteColor": "#ff0000", "scaleMode": "fit", "outputFps": 40,
        "brightness": 100, "volume": 80, "crop": [8, 32, 256, 224],
    }


class FakePixelPlus:
    """The fake daemon. ``start()`` serves on a background thread (port 0 = any free port)."""

    def __init__(self, port=0, width=80, height=40, prop_id="matrix01", shm=True, games=None,
                 require_local=True, host="127.0.0.1"):
        self.width, self.height = width, height
        self.prop_id = prop_id
        self.shm_enabled = shm          # False: /overlay/:id/open answers 404 (HTTP fallback)
        self.require_local = require_local
        self.lock = threading.Lock()
        self.version = 1
        self.player_state = "playing"
        self.overlay_enabled = {}
        self.frames = 0                 # HTTP frames received
        self.last_frame = None
        self.calls = []                 # (method, path) in order
        self.unauthorized = 0
        self.forbidden = 0
        # Like pixelplusd: a fresh random token per run, in a file the sidecar reads.
        self.token = base64.b16encode(os.urandom(32)).decode().lower()
        self.token_dir = __import__("tempfile").mkdtemp(prefix="fake-pp-")
        self.token_file = os.path.join(self.token_dir, "local-token")
        with open(self.token_file, "w") as f:
            f.write(self.token)
        self.games = default_games(prop_id)
        if games:
            self.games.update(games)
        self.ws_clients = []
        self.ws_lock = threading.Lock()
        self.httpd = ThreadingHTTPServer((host, port), self._handler())
        self.httpd.daemon_threads = True
        self.port = self.httpd.server_address[1]
        self.base = "http://%s:%d" % (host, self.port)
        self._thread = None

    # --- lifecycle -------------------------------------------------------------

    def _start_status_ticker(self):
        """Like pixelplusd: a status message every 2 s to WebSocket clients."""
        self._ticking = threading.Event()

        def tick():
            while not self._ticking.wait(2):
                if self.ws_clients:
                    self.broadcast({"type": "status", "data": self.player()})

        threading.Thread(target=tick, name="fake-status", daemon=True).start()

    def start(self):
        # The sidecar (in-process or a child process started afterwards) reads this.
        os.environ["PIXELPLUS_LOCAL_TOKEN_FILE"] = self.token_file
        self._start_status_ticker()
        self._thread = threading.Thread(target=self.httpd.serve_forever, args=(0.05,), name="fake-pixelplus",
                                        daemon=True)
        self._thread.start()
        return self

    def stop(self):
        __import__("shutil").rmtree(self.token_dir, ignore_errors=True)
        if getattr(self, "_ticking", None):
            self._ticking.set()
        for c in list(self.ws_clients):
            try:
                c.shutdown(socket.SHUT_RDWR)
                c.close()
            except OSError:
                pass
        self.httpd.shutdown()
        self.httpd.server_close()
        try:
            os.unlink(self.shm_path())
        except OSError:
            pass

    # --- state -------------------------------------------------------------------

    def shm_path(self):
        return "/dev/shm/pixelplus-overlay-" + self.prop_id

    def shm_header(self):
        with open(self.shm_path(), "rb") as f:
            return struct.unpack("=III", f.read(12))

    def shm_frame(self):
        with open(self.shm_path(), "rb") as f:
            data = f.read()
        return data[12:12 + self.width * self.height * 3]

    def show(self):
        n = self.width * self.height
        return {
            "version": self.version,
            "name": "Test Show",
            "props": [
                {"id": "arch000001", "name": "Left Arch", "kind": "arch", "pixelCount": 50, "channelStart": 0,
                 "channelsPerPixel": 3, "segments": [], "groupIds": []},
                {"id": self.prop_id, "name": "Matrix", "kind": "matrix", "pixelCount": n, "channelStart": 150,
                 "channelsPerPixel": 3, "segments": [], "groupIds": [],
                 "matrix": {"width": self.width, "height": self.height, "pixelMap": list(range(n))}},
            ],
            "settings": {
                "audio": {"device": "null", "volume": 80, "normalize": True, "targetLufs": -16.0},
                "games": copy.deepcopy(self.games),
            },
        }

    def set_games(self, patch):
        with self.lock:
            self.games.update(patch)
            self.version += 1
        self.broadcast({"type": "show", "data": {"version": self.version}})

    def set_player(self, state):
        self.player_state = state
        self.broadcast({"type": "status", "data": self.player()})

    def player(self):
        return {"state": self.player_state, "posMs": 0, "durationMs": 0, "volume": 80, "brightness": 100, "fps": 40}

    def state(self):
        return {"version": self.version, "player": self.player_state, "overlayEnabled": self.overlay_enabled,
                "frames": self.frames, "calls": ["%s %s" % c for c in self.calls],
                "unauthorized": self.unauthorized, "wsClients": len(self.ws_clients)}

    def commands(self):
        """Player commands received, e.g. ['pause', 'resume']."""
        return [p.rsplit("/", 1)[1] for m, p in self.calls if p.startswith("/api/v1/player/") and m == "POST"]

    def broadcast(self, msg):
        data = json.dumps(msg).encode()
        header = struct.pack("!BB", 0x81, len(data)) if len(data) < 126 else \
            struct.pack("!BBH", 0x81, 126, len(data))
        with self.ws_lock:
            for c in list(self.ws_clients):
                try:
                    c.sendall(header + data)
                except OSError:
                    if c in self.ws_clients:
                        self.ws_clients.remove(c)

    # --- HTTP ----------------------------------------------------------------------

    def _handler(self):
        fake = self

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *a):
                pass

            def _json(self, obj, code=200):
                body = json.dumps(obj).encode()
                self.send_response(code)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def _error(self, code, kind, message):
                self._json({"error": {"code": kind, "message": message}}, code)

            def _body(self):
                n = int(self.headers.get("Content-Length") or 0)
                return self.rfile.read(n) if n else b""

            def _authorized(self):
                if self.path.startswith("/fake/"):
                    return True
                if self.command in ("POST", "PUT", "DELETE") and self.headers.get(REQUEST_HEADER) != "1":
                    fake.forbidden += 1
                    self._error(403, "csrf", "missing X-PixelPlus-Request")
                    return False
                if not fake.require_local:
                    return True
                if self.headers.get(LOCAL_HEADER) == fake.token:
                    return True
                fake.unauthorized += 1
                self._error(401, "unauthorized", "Sign in first")
                return False

            def _route(self, method):
                path = self.path.split("?", 1)[0]
                fake.calls.append((method, path))
                if not self._authorized():
                    self._body()
                    return
                body = self._body() if method in ("POST", "PUT") else b""
                getattr(self, "_" + method.lower())(path, body)

            def do_GET(self):
                self._route("GET")

            def do_POST(self):
                self._route("POST")

            def do_PUT(self):
                self._route("PUT")

            def _get(self, path, body):
                if path == "/api/v1/show":
                    return self._json(fake.show())
                if path == "/api/v1/player":
                    return self._json(fake.player())
                if path == "/api/v1/ws" and self.headers.get("Upgrade", "").lower() == "websocket":
                    return self._websocket()
                if path == "/fake/state":
                    return self._json(fake.state())
                self._error(404, "not_found", "No route for GET %s" % path)

            def _post(self, path, body):
                m = re.match(r"^/api/v1/player/(pause|resume|stop)$", path)
                if m:
                    cmd = m.group(1)
                    if cmd == "pause" and fake.player_state == "playing":
                        fake.player_state = "paused"
                    elif cmd == "resume" and fake.player_state == "paused":
                        fake.player_state = "playing"
                    elif cmd == "stop":
                        fake.player_state = "idle"
                    self._json({"ok": True})
                    fake.broadcast({"type": "status", "data": fake.player()})
                    return
                m = re.match(r"^/api/v1/overlay/([^/]+)(/open)?$", path)
                if m:
                    if m.group(1) != fake.prop_id:
                        return self._error(404, "not_found", "No such prop")
                    if m.group(2):
                        if not fake.shm_enabled:
                            return self._error(404, "not_found", "No shared memory here")
                        with open(fake.shm_path(), "wb") as f:
                            f.write(struct.pack("=III", fake.width, fake.height, 0)
                                    + bytes(fake.width * fake.height * 3))
                        return self._json({"shm": fake.shm_path(), "width": fake.width, "height": fake.height})
                    try:
                        req = json.loads(body or b"{}")
                    except ValueError:
                        return self._error(400, "bad_request", "Invalid JSON")
                    fake.overlay_enabled[m.group(1)] = bool(req.get("enabled"))
                    return self._json({"ok": True})
                if path.startswith("/fake/player/"):
                    fake.set_player(path.rsplit("/", 1)[1])
                    return self._json({"ok": True})
                if path == "/fake/games":
                    fake.set_games(json.loads(body or b"{}"))
                    return self._json({"ok": True, "version": fake.version})
                self._error(404, "not_found", "No route for POST %s" % path)

            def _put(self, path, body):
                m = re.match(r"^/api/v1/overlay/([^/]+)/frame$", path)
                if m and m.group(1) == fake.prop_id:
                    if len(body) != fake.width * fake.height * 3:
                        return self._error(400, "bad_request", "Frame must be %d bytes" % (fake.width * fake.height * 3))
                    fake.frames += 1
                    fake.last_frame = body
                    return self._json({"ok": True})
                self._error(404, "not_found", "No route for PUT %s" % path)

            def _websocket(self):
                key = self.headers.get("Sec-WebSocket-Key", "")
                accept = base64.b64encode(hashlib.sha1(key.encode() + WS_GUID).digest()).decode()
                self.send_response(101, "Switching Protocols")
                self.send_header("Upgrade", "websocket")
                self.send_header("Connection", "Upgrade")
                self.send_header("Sec-WebSocket-Accept", accept)
                self.end_headers()
                self.wfile.flush()
                sock = self.connection
                fake.ws_clients.append(sock)
                fake.broadcast({"type": "status", "data": fake.player()})
                try:
                    while sock.recv(1024):  # ignore whatever the client sends until it leaves
                        pass
                except OSError:
                    pass
                finally:
                    if sock in fake.ws_clients:
                        fake.ws_clients.remove(sock)
                    self.close_connection = True

        return Handler


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 18080
    w = int(sys.argv[2]) if len(sys.argv) > 2 else 80
    h = int(sys.argv[3]) if len(sys.argv) > 3 else 40
    fake = FakePixelPlus(port=port, width=w, height=h)

    def terminate(*_):
        raise KeyboardInterrupt

    signal.signal(signal.SIGTERM, terminate)  # clean up the shared memory on kill too
    fake._start_status_ticker()
    print("Fake pixelplusd on %s (matrix prop %r, %dx%d, shm %s)" % (fake.base, fake.prop_id, w, h, fake.shm_path()))
    try:
        fake.httpd.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        fake.stop()
