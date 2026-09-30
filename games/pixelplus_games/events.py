"""Listen to pixelplusd's WebSocket (``/api/v1/ws``) for live updates.

Two server messages matter here (ARCHITECTURE.md section 8.1):

* ``{"type": "show", "data": {"version": N}}`` - the show changed; refetch
  ``/show`` (the games settings may have changed).
* ``{"type": "status", "data": PlayerStatus}`` - the player state, pushed
  every 250 ms while playing and every 2 s when idle.

A minimal RFC 6455 client on asyncio streams, standard library only.  When the
socket is unavailable the sidecar polls the HTTP API instead, so this is an
optimisation, never a requirement.
"""

import asyncio
import base64
import json
import logging
import os
import struct
import urllib.parse

from .api import LOCAL_HEADER

log = logging.getLogger("pixelplus_games.events")

MAX_MESSAGE = 4 * 1024 * 1024   # preview frames are never subscribed to, but be generous
HANDSHAKE_TIMEOUT = 5
IDLE_TIMEOUT = 30               # pixelplusd sends status at least every 2 s


class EventStream:
    """Connects, reconnects with backoff, and calls ``on_message(type, data)``.

    ``connected`` tells whether live updates are currently flowing.
    """

    def __init__(self, url, on_message):
        self.url = url
        self.on_message = on_message
        self.connected = False
        self._task = None

    def start(self):
        self._task = asyncio.ensure_future(self._run())

    async def stop(self):
        if self._task:
            self._task.cancel()
            try:
                await self._task
            except (asyncio.CancelledError, Exception):
                pass
            self._task = None
        self.connected = False

    async def _run(self):
        delay = 1.0
        while True:
            try:
                await self._session()
                delay = 1.0
            except asyncio.CancelledError:
                raise
            except Exception as e:
                log.debug("Event stream unavailable (%s); retrying in %.0f s", e, delay)
            if self.connected:
                log.info("Event stream from pixelplusd closed")
            self.connected = False
            await asyncio.sleep(delay)
            delay = min(delay * 2, 30.0)

    async def _session(self):
        u = urllib.parse.urlparse(self.url)
        if u.scheme not in ("ws", "wss"):
            raise ValueError("not a ws:// URL: %s" % self.url)
        port = u.port or (443 if u.scheme == "wss" else 80)
        reader, writer = await asyncio.wait_for(
            asyncio.open_connection(u.hostname, port, ssl=(u.scheme == "wss") or None), HANDSHAKE_TIMEOUT)
        try:
            key = base64.b64encode(os.urandom(16)).decode()
            path = (u.path or "/") + ("?" + u.query if u.query else "")
            writer.write(("GET %s HTTP/1.1\r\nHost: %s\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
                          "Sec-WebSocket-Key: %s\r\nSec-WebSocket-Version: 13\r\n%s: 1\r\n\r\n"
                          % (path, u.netloc, key, LOCAL_HEADER)).encode())
            await writer.drain()
            head = await asyncio.wait_for(reader.readuntil(b"\r\n\r\n"), HANDSHAKE_TIMEOUT)
            status = head.split(b"\r\n", 1)[0]
            if b" 101 " not in status + b" ":
                raise ConnectionError("upgrade refused: %s" % status.decode(errors="replace"))
            self.connected = True
            log.info("Receiving live updates from %s", self.url)
            while True:
                opcode, payload = await asyncio.wait_for(_read_frame(reader), IDLE_TIMEOUT)
                if opcode == 0x8:
                    _write_frame(writer, 0x8, payload[:2])
                    return
                if opcode == 0x9:
                    _write_frame(writer, 0xA, payload)
                    continue
                if opcode != 0x1:
                    continue  # binary (preview) frames and pongs are not for us
                try:
                    msg = json.loads(payload.decode("utf-8"))
                except (UnicodeDecodeError, ValueError):
                    continue
                if isinstance(msg, dict) and isinstance(msg.get("type"), str):
                    try:
                        self.on_message(msg["type"], msg.get("data"))
                    except Exception:
                        log.exception("Handling %s event failed", msg.get("type"))
        finally:
            try:
                writer.close()
            except (OSError, RuntimeError):
                pass


async def _read_frame(reader):
    """Next complete message as (opcode, payload); continuation frames are joined."""
    buf = b""
    first_opcode = None
    while True:
        h = await reader.readexactly(2)
        fin, opcode = h[0] & 0x80, h[0] & 0x0F
        n = h[1] & 0x7F
        if n == 126:
            n = struct.unpack("!H", await reader.readexactly(2))[0]
        elif n == 127:
            n = struct.unpack("!Q", await reader.readexactly(8))[0]
        mask = await reader.readexactly(4) if h[1] & 0x80 else None
        if n + len(buf) > MAX_MESSAGE:
            raise ConnectionError("message too large")
        payload = await reader.readexactly(n)
        if mask:
            payload = bytes(b ^ mask[i & 3] for i, b in enumerate(payload))
        if opcode >= 0x8:          # control frames may arrive between fragments
            return opcode, payload
        if opcode:
            first_opcode = opcode
        buf += payload
        if fin:
            return first_opcode or 0x1, buf


def _write_frame(writer, opcode, payload):
    """Clients must mask what they send."""
    mask = os.urandom(4)
    n = len(payload)
    header = struct.pack("!BB", 0x80 | opcode, 0x80 | n) if n < 126 else \
        struct.pack("!BBH", 0x80 | opcode, 0x80 | 126, n)
    writer.write(header + mask + bytes(b ^ mask[i & 3] for i, b in enumerate(payload)))
