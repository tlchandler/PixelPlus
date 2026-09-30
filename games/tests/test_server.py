"""The hub (line, turns, cooldown, invites), the control socket, the phone WebSocket and live
updates from pixelplusd, against the fake pixelplusd with a stub emulator.

    python3 -m unittest discover -s games/tests
"""

import asyncio
import dataclasses
import base64
import json
import os
import socket
import struct
import tempfile
import threading
import time
import unittest
import uuid

import support  # noqa: F401  (import paths, quiet logs)
from fake_pixelplus import FakePixelPlus  # noqa: E402
from pixelplus_games import ctl, invite, server  # noqa: E402
from pixelplus_games.api import PixelPlus  # noqa: E402
from pixelplus_games.events import EventStream  # noqa: E402


class StubEngine:
    """Stands in for the emulator: a 'game' reports progress until stopped or ``finish`` is set."""

    def __init__(self):
        self.reason = None
        self.finish = threading.Event()
        self.played = []
        self.arcade_runs = 0

    def ready(self, cfg, arcade_mode=False):
        return self.reason

    def play(self, level, cfg, controls, stop, on_update):
        self.played.append(level)
        on_update({"phase": "playing", "level": level, "remaining": cfg.game_seconds, "score": 0})
        while not stop.is_set() and not self.finish.is_set():
            time.sleep(0.01)
        on_update({"phase": "over", "score": 1250, "level": level})
        return 1250

    def arcade(self, cfg, controls, stop, to_menu, on_update):
        self.arcade_runs += 1
        on_update({"phase": "menu"})
        while not stop.is_set():
            time.sleep(0.01)


class FakeWS:
    def __init__(self):
        self.sent = []
        self.closed = False
        self.close_code = None

    def send_json(self, obj):
        self.sent.append(obj)

    async def close(self, code=1000):
        self.closed = True
        self.close_code = code

    def last(self):
        states = [m for m in self.sent if m.get("t") == "state"]
        return states[-1] if states else None


def free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()
    return port


async def until(cond, timeout=3.0):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        if cond():
            return True
        await asyncio.sleep(0.01)
    return bool(cond())


class HubTestCase(unittest.IsolatedAsyncioTestCase):
    games = {}

    async def asyncSetUp(self):
        self.fake = FakePixelPlus(prop_id="h" + uuid.uuid4().hex[:9],
                                  games=dict({"port": free_port()}, **self.games)).start()
        self.hub = server.Hub(asyncio.get_running_loop(), PixelPlus(self.fake.base))
        self.hub.engine = self.engine = StubEngine()
        self.assertTrue(await self.hub.refresh_show())
        await self.hub._tick_once()   # player state, controller port, arcade

    async def asyncTearDown(self):
        self.hub.stop_event.set()
        self.hub.arcade_stop.set()
        self.engine.finish.set()
        for t in (self.hub.game_thread, self.hub.arcade_thread):
            if t is not None:
                t.join(2)
        if self.hub.server:
            await self.hub.server.stop()
        self.fake.stop()

    def connect(self, cid=None, ip=None):
        """A phone joins: returns (id, ws) after the hello/welcome exchange."""
        cid = cid or uuid.uuid4().hex
        ws = FakeWS()
        if ip:
            ws.peer = ip
        c = server.Client(cid, ws)
        self.hub.clients[cid] = c
        self.hub.ips[cid] = c.ip
        self.hub.push(cid)
        return cid, ws


class VisitorLimitTests(HubTestCase):
    games = {"cooldownMinutes": 0, "maxQueuePerVisitor": 2}

    async def test_one_address_can_hold_only_its_share_of_the_line(self):
        self.assertEqual(self.hub.cfg.max_queue_per_visitor, 2)
        a, _ = self.connect(ip="203.0.113.7")
        b, wb = self.connect(ip="203.0.113.7")
        c, wc = self.connect(ip="203.0.113.7")
        d, wd = self.connect(ip="198.51.100.1")
        self.hub.press_start(a)                 # playing
        self.hub.press_start(b)                 # in line
        self.assertEqual(wb.last()["phase"], "queued")
        self.hub.press_start(c)                 # a third phone from the same address
        self.assertEqual(wc.last()["phase"], "limited")
        self.assertIn("2 phones", wc.last()["message"])
        self.assertNotIn(c, self.hub.queue)
        self.hub.press_start(d)                 # another visitor is not affected
        self.assertEqual(wd.last()["phase"], "queued")
        # once one of them leaves the line, the third phone may join
        self.hub.queue.remove(b)
        self.hub.press_start(c)
        self.assertEqual(wc.last()["phase"], "queued")

    async def test_unknown_address_and_no_limit(self):
        a, _ = self.connect()
        b, wb = self.connect()
        c, wc = self.connect()
        for cid in (a, b, c):
            self.hub.press_start(cid)
        self.assertEqual(wc.last()["phase"], "queued")
        self.hub.cfg = dataclasses.replace(self.hub.cfg, max_queue_per_visitor=0)
        self.assertFalse(self.hub.visitor_limit_reached(c))


class MarioFlowTests(HubTestCase):
    games = {"cooldownMinutes": 0}

    async def test_play_queue_and_offer(self):
        await self.hub._tick_once()
        a, wa = self.connect()
        b, wb = self.connect()
        self.assertEqual(wa.last()["phase"], "idle")
        self.hub.press_start(a)
        self.assertEqual(self.hub.active, a)
        self.hub.press_start(b)
        self.assertEqual(wb.last()["phase"], "queued")
        self.assertEqual(wb.last()["position"], 1)
        self.assertTrue(await until(lambda: wa.last()["phase"] == "playing"))
        self.assertEqual(wb.last()["phase"], "queued")
        # the level is one of the configured ones
        self.assertIn(self.engine.played[0], [f"{w}-{lv}" for w in range(1, 9) for lv in range(1, 5)])

        self.engine.finish.set()
        self.assertTrue(await until(lambda: self.hub.active is None))
        self.assertEqual(wa.last()["phase"], "over")
        self.assertEqual(wa.last()["score"], 1250)
        self.engine.finish.clear()

        # no cooldown: the next in line is offered the turn and has turn_timeout seconds to take it
        self.hub._offer_next()
        self.assertEqual(wb.last()["phase"], "yourturn")
        self.assertGreater(wb.last()["timeout"], 15)
        self.hub.press_start(b)
        self.assertEqual(self.hub.active, b)
        self.assertEqual(self.hub.queue, [])

    async def test_missed_turn(self):
        a, wa = self.connect()
        b, wb = self.connect()
        self.hub.press_start(a)
        self.hub.press_start(b)
        self.engine.finish.set()
        self.assertTrue(await until(lambda: self.hub.active is None))
        self.hub._offer_next()
        self.assertEqual(self.hub.offer[0], b)
        self.hub.offer = (b, time.monotonic() - 1)   # 20 s passed without START
        self.hub._offer_next()
        self.assertIsNone(self.hub.offer)
        self.assertNotIn(b, self.hub.queue)
        self.assertEqual(wb.last()["phase"], "idle")

    async def test_leave_the_line(self):
        a, _ = self.connect()
        b, wb = self.connect()
        self.hub.press_start(a)
        self.hub.press_start(b)
        self.assertIn(b, self.hub.queue)
        ws = FakeWS()
        msgs = [json.dumps({"t": "hello", "id": b}), json.dumps({"t": "leave"}), None]

        async def recv():
            return msgs.pop(0)

        ws.recv = recv
        await self.hub.handle_ws(ws)
        self.assertNotIn(b, self.hub.queue)
        self.assertTrue(wb.closed)                 # the older page was told it was replaced...
        self.assertEqual(wb.close_code, server.REPLACED)
        self.assertEqual(ws.sent[0], {"t": "welcome", "id": b})

    async def test_operator_stop(self):
        a, wa = self.connect()
        self.hub.press_start(a)
        self.assertTrue(await until(lambda: wa.last()["phase"] == "playing"))
        self.assertTrue(self.hub.stop_game())
        self.assertTrue(await until(lambda: self.hub.active is None))
        self.assertEqual(wa.last()["phase"], "over")

    async def test_player_who_leaves_loses_the_game(self):
        a, wa = self.connect()
        self.hub.press_start(a)
        del self.hub.clients[a]
        self.hub.gone[a] = time.monotonic() - server.ABANDON_AFTER - 1
        await self.hub._tick_once()
        self.assertTrue(await until(lambda: self.hub.active is None))

    async def test_buttons_only_from_the_player(self):
        a, _ = self.connect()
        b, _ = self.connect()
        self.hub.press_start(a)
        for cid, bits in ((b, 0x100), (a, 0x80 | 0xFFFF0000)):
            ws = FakeWS()
            msgs = [json.dumps({"t": "hello", "id": cid}), json.dumps({"t": "in", "b": bits}), None]

            async def recv(msgs=msgs):
                return msgs.pop(0)

            ws.recv = recv
            await self.hub.handle_ws(ws)
            if cid == b:
                self.assertEqual(self.hub.controls.buttons, 0)     # a watcher can't steer
        # the player's handler ended (page closed): buttons are released
        self.assertEqual(self.hub.controls.buttons, 0)


class CooldownTests(HubTestCase):
    games = {"cooldownMinutes": 2}

    async def test_cooldown_clears_the_line(self):
        a, wa = self.connect()
        b, wb = self.connect()
        self.hub.press_start(a)
        self.hub.press_start(b)
        self.engine.finish.set()
        self.assertTrue(await until(lambda: self.hub.active is None))
        self.assertEqual(self.hub.queue, [])
        self.assertEqual(wb.last()["phase"], "cooldown")
        self.assertGreater(wb.last()["wait"], 110)
        self.assertEqual(self.hub.status()["cooldownS"], wb.last()["wait"])
        # no invites during the cooldown, unless forced
        self.assertFalse(self.hub.start_invite({})["ok"])
        self.hub.press_start(b)
        self.assertIsNone(self.hub.active)


class AvailabilityTests(HubTestCase):
    async def test_play_window(self):
        a, wa = self.connect()
        self.fake.player_state = "idle"
        await self.hub._tick_once()
        self.assertEqual(wa.last()["phase"], "closed")
        self.assertIn("taking a break", wa.last()["message"])
        # inside a schedule window (idle look between playlists) counts as show time
        self.hub._set_player_status({"state": "effect", "scheduleEntry": {"id": "e", "name": "Nightly"}})
        self.hub.push(a)
        self.assertEqual(wa.last()["phase"], "idle")
        # "anytime"
        self.fake.set_games({"playWindow": "anytime"})
        await self.hub.refresh_show()
        self.hub._set_player_status({"state": "idle"})
        self.hub.push(a)
        self.assertEqual(wa.last()["phase"], "idle")

    async def test_engine_not_ready(self):
        a, wa = self.connect()
        self.engine.reason = "No Super Mario Bros. ROM has been uploaded"
        self.hub.push(a)
        self.assertEqual(wa.last(), dict(wa.last(), phase="closed", message=self.engine.reason))
        self.hub.press_start(a)
        self.assertIsNone(self.hub.active)

    async def test_disabled(self):
        self.fake.set_games({"enabled": False})
        await self.hub.refresh_show()
        self.assertEqual(self.hub.unavailable_reason(), "Games are turned off right now.")
        self.assertFalse(self.hub.start_invite({})["ok"])

    async def test_controller_port_follows_the_setting(self):
        port = self.hub.cfg.port
        await self.hub._tick_once()
        self.assertEqual(self.hub.server_port, port)
        reader, writer = await asyncio.open_connection("127.0.0.1", port)
        writer.write(b"GET /healthz HTTP/1.1\r\nHost: x\r\n\r\n")
        self.assertIn(b"200 OK", await reader.read())
        writer.close()
        self.fake.set_games({"enabled": False})
        await self.hub.refresh_show()
        await self.hub._tick_once()
        self.assertIsNone(self.hub.server_port)
        with self.assertRaises(OSError):
            socket.create_connection(("127.0.0.1", port), timeout=1).close()

    async def test_settings_change_disables_old_matrix_overlay(self):
        other = self.fake.prop_id
        self.fake.set_games({"matrixPropId": ""})    # still resolves: the only matrix prop
        await self.hub.refresh_show()
        self.assertEqual(self.hub.cfg.prop_id, other)


class ArcadeTests(HubTestCase):
    games = {"arcadeMode": True, "arcadeIdleSeconds": 30, "arcadeMinutes": 1}

    async def test_turns(self):
        await self.hub._tick_once()
        self.assertTrue(await until(self.hub.arcade_running))
        a, wa = self.connect()
        b, wb = self.connect()
        self.hub.press_start(a)
        self.assertEqual(self.hub.active, a)
        self.assertTrue(await until(lambda: wa.last().get("phase") == "menu"))
        self.assertEqual(wa.last()["remaining"], 60)
        self.hub.press_start(b)
        self.assertEqual(wb.last()["phase"], "queued")
        status = self.hub.status()
        self.assertTrue(status["arcade"] and status["running"])
        self.assertEqual(status["player"], {"phase": "menu", "remaining": 60})

        # idle for longer than arcadeIdleSeconds: the turn ends and B is offered the controller
        self.hub.controls.changed -= 31
        await self.hub._tick_once()
        self.assertIsNone(self.hub.active)
        self.assertTrue(self.hub.to_menu.is_set())
        self.assertEqual(wa.last()["phase"], "over")
        self.assertIn("nothing was pressed", wa.last()["message"])
        self.assertEqual(wb.last()["phase"], "yourturn")
        self.hub.press_start(b)
        self.assertEqual(self.hub.active, b)
        # the idle clock started again with B
        self.assertLess(time.monotonic() - self.hub.controls.changed, 1)

        # turn length
        self.hub.turn_started -= 61
        await self.hub._tick_once()
        self.assertIsNone(self.hub.active)
        self.assertIn("Time's up", wb.last()["message"])

    async def test_keeps_the_show_stopped_and_closes(self):
        await self.hub._tick_once()
        self.assertTrue(await until(self.hub.arcade_running))
        self.assertFalse(self.hub.start_invite({"force": True})["ok"])   # the matrix is the arcade's
        self.hub.last_show_stop -= server.KEEP_STOPPED_EVERY + 1
        self.fake.player_state = "playing"                               # the scheduler started a show
        self.hub._set_player_status({"state": "playing"})
        await self.hub._tick_once()
        self.assertTrue(await until(lambda: "stop" in self.fake.commands()))
        # turning arcade mode off closes it
        self.fake.set_games({"arcadeMode": False})
        await self.hub.refresh_show()
        await self.hub._tick_once()
        self.assertTrue(await until(lambda: not self.hub.arcade_running()))


class ControlSocketTests(HubTestCase):
    games = {"publicUrl": "play.example.com"}

    async def asyncSetUp(self):
        await super().asyncSetUp()
        self.tmp = tempfile.TemporaryDirectory()
        self.path = os.path.join(self.tmp.name, "games.sock")
        self.control = await asyncio.start_unix_server(self.hub.handle_control, path=self.path)
        self.saved = invite.ON_SECONDS, invite.OFF_SECONDS, invite.TEST_PATTERN_SECONDS
        invite.ON_SECONDS, invite.OFF_SECONDS, invite.TEST_PATTERN_SECONDS = 0.05, 0.01, 0.1

    async def asyncTearDown(self):
        invite.ON_SECONDS, invite.OFF_SECONDS, invite.TEST_PATTERN_SECONDS = self.saved
        self.control.close()
        await super().asyncTearDown()
        self.tmp.cleanup()

    async def ask(self, req, raw=None):
        reader, writer = await asyncio.open_unix_connection(self.path)
        writer.write(raw if raw is not None else (json.dumps(req) + "\n").encode())
        await writer.drain()
        line = await asyncio.wait_for(reader.readline(), 5)
        writer.close()
        return json.loads(line)

    async def test_status(self):
        await self.hub._tick_once()
        s = await self.ask({"cmd": "status"})
        self.assertEqual(s, dict(s, ok=True, enabled=True, running=False, arcade=False, queueLength=0,
                                 cooldownS=0, model={"width": 80, "height": 40}, propId=self.fake.prop_id))
        self.assertNotIn("player", s)
        self.assertNotIn("lastError", s)
        a, _ = self.connect()
        self.hub.press_start(a)
        await until(lambda: self.hub.game_view.get("phase") == "playing")
        s = await self.ask({"cmd": "status"})
        self.assertTrue(s["running"])
        self.assertEqual(s["player"]["phase"], "playing")
        self.assertIn("level", s["player"])
        self.assertNotIn(a, json.dumps(s))   # player ids are secrets (they let a phone reclaim a turn)

    async def test_last_error(self):
        self.hub._game_update("nobody", {"phase": "error", "message": "The emulator core could not load smb.nes"})
        s = await self.ask({"cmd": "status"})
        self.assertEqual(s["lastError"], "The emulator core could not load smb.nes")

    async def test_invite(self):
        r = await self.ask({"cmd": "invite", "flashes": 1, "style": "both"})
        self.assertEqual(r, {"ok": True})
        self.assertEqual(self.hub.busy_reason, "invite")
        r = await self.ask({"cmd": "invite"})
        self.assertFalse(r["ok"])
        self.assertIn("busy", r["error"])
        self.assertTrue(await until(lambda: self.hub.busy_reason is None))
        self.assertFalse(self.fake.overlay_enabled[self.fake.prop_id])
        self.assertIn("POST /api/v1/overlay/%s/open" % self.fake.prop_id, self.fake.state()["calls"])

    async def test_invite_needs_a_url(self):
        self.fake.set_games({"publicUrl": ""})
        await self.ask({"cmd": "reload"})
        r = await self.ask({"cmd": "invite"})
        self.assertFalse(r["ok"])
        self.assertIn("Public URL", r["error"])

    async def test_test_pattern_and_stop(self):
        self.assertEqual(await self.ask({"cmd": "test"}), {"ok": True})
        self.assertFalse((await self.ask({"cmd": "test"}))["ok"])
        self.assertTrue(await until(lambda: self.hub.busy_reason is None))
        self.assertEqual(await self.ask({"cmd": "stop"}), {"ok": True, "stopped": False})

    async def test_bad_requests(self):
        self.assertFalse((await self.ask({"cmd": "dance"}))["ok"])
        self.assertFalse((await self.ask(None, raw=b"not json\n"))["ok"])
        self.assertFalse((await self.ask(None, raw=b"[1, 2]\n"))["ok"])

    async def test_ctl_client(self):
        reply = await asyncio.get_running_loop().run_in_executor(None, lambda: ctl.send({"cmd": "status"},
                                                                                           path=self.path))
        self.assertTrue(reply["ok"])
        missing = ctl.send({"cmd": "status"}, path=self.path + ".missing")
        self.assertFalse(missing["ok"])


class LiveUpdateTests(HubTestCase):
    async def test_show_events_trigger_a_refetch(self):
        stream = EventStream(self.hub.api.ws_url(), self.hub.on_event)
        self.hub.events = stream
        stream.start()
        try:
            self.assertTrue(await until(lambda: stream.connected and self.fake.ws_clients))
            self.fake.set_player("paused")
            self.assertTrue(await until(lambda: self.hub.player_state == "paused"))
            fetches = self.fake.state()["calls"].count("GET /api/v1/show")
            self.fake.set_games({"gameSeconds": 45})
            self.assertTrue(await until(lambda: self.hub.cfg.game_seconds == 45))
            self.assertEqual(self.fake.state()["calls"].count("GET /api/v1/show"), fetches + 1)
        finally:
            await stream.stop()
        self.assertFalse(stream.connected)

    async def test_stream_reconnects_quietly_when_unavailable(self):
        stream = EventStream("ws://127.0.0.1:9/api/v1/ws", lambda *a: None)
        stream.start()
        await asyncio.sleep(0.1)
        self.assertFalse(stream.connected)
        await stream.stop()


class ControllerPageTests(HubTestCase):
    """The real HTTP/WebSocket server with a raw client, like a phone would connect."""

    async def test_page_and_websocket(self):
        await self.hub._tick_once()
        port = self.hub.server_port
        reader, writer = await asyncio.open_connection("127.0.0.1", port)
        writer.write(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
        page = await reader.read()
        writer.close()
        self.assertIn(b"200 OK", page)
        self.assertIn(b"PRESS START", page)
        self.assertIn(b"Content-Security-Policy", page)

        reader, writer = await asyncio.open_connection("127.0.0.1", port)
        key = base64.b64encode(os.urandom(16)).decode()
        writer.write(("GET /ws HTTP/1.1\r\nHost: x\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
                      "Sec-WebSocket-Key: %s\r\nSec-WebSocket-Version: 13\r\n\r\n" % key).encode())
        head = await reader.readuntil(b"\r\n\r\n")
        self.assertIn(b"101", head)

        def frame(obj):
            data = json.dumps(obj).encode()
            mask = os.urandom(4)
            return struct.pack("!BB", 0x81, 0x80 | len(data)) + mask + bytes(
                b ^ mask[i & 3] for i, b in enumerate(data))

        async def recv():
            h = await reader.readexactly(2)
            return json.loads(await reader.readexactly(h[1] & 0x7F))

        writer.write(frame({"t": "hello", "id": ""}))
        welcome = await recv()
        self.assertEqual(welcome["t"], "welcome")
        self.assertEqual(len(welcome["id"]), 32)   # a fresh id when the phone had none
        state = await recv()
        self.assertEqual((state["t"], state["phase"], state["seconds"]), ("state", "idle", 60))
        writer.write(frame({"t": "start"}))
        state = await recv()
        self.assertIn(state["phase"], ("starting", "playing"))
        self.assertEqual(self.hub.active, welcome["id"])
        writer.close()


if __name__ == "__main__":
    unittest.main()
