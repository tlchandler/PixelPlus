"""Tiny asyncio HTTP/1.1 + WebSocket (RFC 6455) server, standard library only."""

import asyncio
import base64
import hashlib
import ipaddress
import json
import logging
import struct

log = logging.getLogger("pixelplus_games.web")

WS_GUID = b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11"
MAX_HEADER = 8192
MAX_WS_MESSAGE = 4096
SEND_BUFFER_LIMIT = 256 * 1024
# A phone that shows no sign of life for this long is gone (the page pings every 20 s;
# background tabs may only run timers once a minute). Without
# it, idle sockets opened from the internet would pile up until max_connections is reached.
WS_IDLE_TIMEOUT = 120
MAX_PER_IP = 16            # concurrent connections from one address (generous: mobile CGNAT)

SECURITY_HEADERS = (
    "X-Content-Type-Options: nosniff\r\n"
    "Referrer-Policy: no-referrer\r\n"
    "Cache-Control: no-cache\r\n"
)


class WebSocket:
    def __init__(self, reader, writer, headers, peer):
        self.reader = reader
        self.writer = writer
        self.headers = headers
        self.peer = peer
        self.closed = False

    async def recv(self, idle_timeout=None):
        """Return the next text message, or None when the connection ends.

        ``idle_timeout``: close the connection when no frame at all (pings included) arrives
        for this many seconds (default WS_IDLE_TIMEOUT).
        """
        buf = b""
        idle = WS_IDLE_TIMEOUT if idle_timeout is None else idle_timeout
        while True:
            try:
                try:
                    h = await asyncio.wait_for(self.reader.readexactly(2), idle)
                except asyncio.TimeoutError:
                    await self.close(1001)
                    return None
                opcode = h[0] & 0x0F
                fin = h[0] & 0x80
                masked = h[1] & 0x80
                n = h[1] & 0x7F
                if n == 126:
                    n = struct.unpack("!H", await self.reader.readexactly(2))[0]
                elif n == 127:
                    n = struct.unpack("!Q", await self.reader.readexactly(8))[0]
                if not masked or n > MAX_WS_MESSAGE or len(buf) + n > MAX_WS_MESSAGE \
                        or (opcode >= 0x8 and (n > 125 or not fin)):
                    await self.close(1009 if n > MAX_WS_MESSAGE else 1002)
                    return None
                # The rest of a frame must follow promptly (no trickling a frame in byte by byte).
                mask, payload = await asyncio.wait_for(self._read_body(n), 10)
            except (asyncio.IncompleteReadError, asyncio.TimeoutError, ConnectionError, OSError):
                self.closed = True
                return None
            if opcode == 0x8:  # close
                await self.close()
                return None
            if opcode == 0x9:  # ping
                self._send_frame(0xA, payload)
                continue
            if opcode == 0xA:  # pong
                continue
            if opcode in (0x1, 0x2, 0x0):
                buf += payload
                if fin:
                    try:
                        return buf.decode("utf-8")
                    except UnicodeDecodeError:
                        await self.close(1007)
                        return None
                continue
            await self.close(1002)
            return None

    async def _read_body(self, n):
        mask = await self.reader.readexactly(4)
        data = await self.reader.readexactly(n)
        if not n:
            return mask, b""
        # unmask in one go: XOR with the 4-byte key repeated over the payload
        key = (mask * (n // 4 + 1))[:n]
        return mask, (int.from_bytes(data, "little") ^ int.from_bytes(key, "little")).to_bytes(n, "little")

    def _send_frame(self, opcode, payload):
        if self.closed:
            return
        n = len(payload)
        if n < 126:
            header = struct.pack("!BB", 0x80 | opcode, n)
        elif n < 65536:
            header = struct.pack("!BBH", 0x80 | opcode, 126, n)
        else:
            header = struct.pack("!BBQ", 0x80 | opcode, 127, n)
        try:
            if self.writer.transport.get_write_buffer_size() > SEND_BUFFER_LIMIT:
                # a client that stopped reading; don't let it eat memory
                self.closed = True
                self.writer.close()
                return
            self.writer.write(header + payload)
        except (ConnectionError, OSError, RuntimeError):
            self.closed = True

    def send_json(self, obj):
        self._send_frame(0x1, json.dumps(obj, separators=(",", ":")).encode())

    async def close(self, code=1000):
        if not self.closed:
            self._send_frame(0x8, struct.pack("!H", code))
            self.closed = True
        try:
            self.writer.close()
        except (ConnectionError, OSError, RuntimeError):
            pass


async def _read_request(reader):
    data = await reader.readuntil(b"\r\n\r\n")
    if len(data) > MAX_HEADER:
        raise ValueError("header too large")
    lines = data.decode("latin-1").split("\r\n")
    method, target, _version = lines[0].split(" ", 2)
    headers = {}
    for line in lines[1:]:
        if ":" in line:
            k, v = line.split(":", 1)
            headers[k.strip().lower()] = v.strip()
    return method, target, headers


def _response(writer, status, body=b"", ctype="text/plain; charset=utf-8", extra=""):
    if isinstance(body, str):
        body = body.encode()
    writer.write(("HTTP/1.1 %s\r\nContent-Type: %s\r\nContent-Length: %d\r\n%s%sConnection: close\r\n\r\n"
                  % (status, ctype, len(body), SECURITY_HEADERS, extra)).encode() + body)


def client_address(peer, headers):
    """The visitor's address. Behind a local reverse proxy / Cloudflare Tunnel every connection
    comes from the proxy, so the forwarded address is used - but only when the peer is loopback
    or on the LAN, since anyone on the internet could send those headers."""
    ip = peer[0] if isinstance(peer, (tuple, list)) and peer else ""
    try:
        addr = ipaddress.ip_address(ip)
        trusted = addr.is_loopback or addr.is_private
    except ValueError:
        trusted = False
    if trusted:
        fwd = headers.get("cf-connecting-ip") or headers.get("x-forwarded-for", "").split(",")[0]
        fwd = fwd.strip()[:64]
        if fwd:
            try:
                return str(ipaddress.ip_address(fwd))
            except ValueError:
                pass
    return ip or "?"


class Server:
    """Routes: GET / -> page(), GET /ws -> websocket handler, GET /healthz."""

    def __init__(self, page, on_websocket, max_connections=300, max_per_ip=MAX_PER_IP):
        self.page = page
        self.on_websocket = on_websocket
        self.max_connections = max_connections
        self.max_per_ip = max_per_ip
        self.connections = 0
        self.per_ip = {}
        self._writers = set()
        self._server = None

    async def start(self, port, host=None):
        self._server = await asyncio.start_server(self._handle, host=host, port=port,
                                                  limit=MAX_HEADER * 2, reuse_address=True)
        log.info("Controller page listening on port %d", port)

    async def stop(self):
        """Stop listening and drop every open connection (phones reconnect when it's back).

        Open connections must be closed here: since Python 3.12 ``wait_closed`` waits for them,
        and a WebSocket stays open for as long as the phone is on the page."""
        if self._server:
            self._server.close()
            for w in list(self._writers):
                try:
                    w.close()
                except (ConnectionError, OSError, RuntimeError):
                    pass
            try:
                await asyncio.wait_for(self._server.wait_closed(), 5)
            except asyncio.TimeoutError:
                log.warning("Controller connections did not close in time")
            self._server = None

    async def _handle(self, reader, writer):
        peer = writer.get_extra_info("peername")
        self.connections += 1
        self._writers.add(writer)
        ip = None
        try:
            if self.connections > self.max_connections:
                _response(writer, "503 Service Unavailable", "Busy")
                return
            try:
                method, target, headers = await asyncio.wait_for(_read_request(reader), 10)
            except (asyncio.TimeoutError, asyncio.IncompleteReadError, asyncio.LimitOverrunError,
                    ValueError, ConnectionError, OSError):
                return
            ip = client_address(peer, headers)
            self.per_ip[ip] = self.per_ip.get(ip, 0) + 1
            if self.per_ip[ip] > self.max_per_ip:
                _response(writer, "429 Too Many Requests", "Too many connections")
                return
            path = target.split("?", 1)[0]
            if method != "GET":
                _response(writer, "405 Method Not Allowed", "Method not allowed", extra="Allow: GET\r\n")
            elif path == "/ws" and headers.get("upgrade", "").lower() == "websocket":
                key = headers.get("sec-websocket-key", "")
                if not key or len(key) > 64:
                    _response(writer, "400 Bad Request", "Bad request")
                    return
                accept = base64.b64encode(hashlib.sha1(key.encode() + WS_GUID).digest()).decode()
                writer.write(("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n"
                              "Connection: Upgrade\r\nSec-WebSocket-Accept: %s\r\n\r\n" % accept).encode())
                ws = WebSocket(reader, writer, headers, ip)
                try:
                    await self.on_websocket(ws)
                except Exception:
                    log.exception("WebSocket handler failed")
                finally:
                    await ws.close()
                return
            elif path in ("/", "/index.html"):
                _response(writer, "200 OK", self.page(), "text/html; charset=utf-8",
                          extra="Content-Security-Policy: default-src 'self'; style-src 'self' 'unsafe-inline'; "
                                "script-src 'self' 'unsafe-inline'; connect-src 'self' ws: wss:; img-src 'self' data:; "
                                "frame-ancestors 'none'\r\n")
            elif path == "/favicon.ico":
                _response(writer, "204 No Content", "")
            elif path == "/healthz":
                _response(writer, "200 OK", "ok")
            else:
                _response(writer, "404 Not Found", "Not found")
            try:
                await asyncio.wait_for(writer.drain(), 10)
            except (asyncio.TimeoutError, ConnectionError, OSError):
                pass
        finally:
            self.connections -= 1
            self._writers.discard(writer)
            if ip is not None:
                n = self.per_ip.get(ip, 1) - 1
                if n > 0:
                    self.per_ip[ip] = n
                else:
                    self.per_ip.pop(ip, None)
            try:
                writer.close()
            except (ConnectionError, OSError, RuntimeError):
                pass
