"""Regression tests from the adversarial review: hostile phones on the internet, races in the
line and arcade turns, pixelplusd restarts / matrix changes mid-game, and the hot path.

    python3 -m unittest discover -s games/tests
"""

import asyncio
import base64
import json
import os
import stat
import struct
import tempfile
import threading
import time
import unittest
import uuid
from unittest import mock

import numpy as np

import support  # noqa: F401  (import paths, quiet logs)
from support import wait_for  # noqa: E402
from fake_pixelplus import FakePixelPlus  # noqa: E402
from test_server import FakeWS, HubTestCase, StubEngine, free_port, until  # noqa: E402
from pixelplus_games import audio, config, display, game, server, web  # noqa: E402
from pixelplus_games.api import PixelPlus  # noqa: E402
from pixelplus_games.display import OverlayModel, Scaler, frame_to_rgb  # noqa: E402
from pixelplus_games.libretro import Frame  # noqa: E402


def ws_frame(data, opcode=0x1, fin=True):
    if isinstance(data, str):
        data = data.encode()
    mask = os.urandom(4)
    n = len(data)
    first = (0x80 if fin else 0) | opcode
    if n < 126:
        head = struct.pack("!BB", first, 0x80 | n)
    else:
        head = struct.pack("!BBH", first, 0x80 | 126, n)
    return head + mask + bytes(b ^ mask[i & 3] for i, b in enumerate(data))


async def ws_open(port, headers=""):
    reader, writer = await asyncio.open_connection("127.0.0.1", port)
    key = base64.b64encode(os.urandom(16)).decode()
    writer.write(("GET /ws HTTP/1.1\r\nHost: x\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
                  "Sec-WebSocket-Key: %s\r\nSec-WebSocket-Version: 13\r\n%s\r\n" % (key, headers)).encode())
    head = await asyncio.wait_for(reader.readuntil(b"\r\n\r\n"), 3)
    return reader, writer, head


async def ws_recv(reader, timeout=3):
    h = await asyncio.wait_for(reader.readexactly(2), timeout)
    n = h[1] & 0x7F
    if n == 126:
        n = struct.unpack("!H", await reader.readexactly(2))[0]
    payload = await reader.readexactly(n)
    if h[0] & 0x0F == 0x8:
        return ("close", struct.unpack("!H", payload[:2])[0] if len(payload) >= 2 else None)
    return json.loads(payload)


class ScriptedWS(FakeWS):
    """A FakeWS whose recv() hands out ``msgs`` then None (the phone closed the page)."""

    def __init__(self, msgs):
        super().__init__()
        self.msgs = list(msgs)

    async def recv(self):
        return self.msgs.pop(0) if self.msgs else None


# --- the controller's HTTP/WebSocket server ----------------------------------------------


class WebServerTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.handled = []
        self.port = free_port()

        async def on_ws(ws):
            self.handled.append(ws)
            while True:
                text = await ws.recv()
                if text is None:
                    return
                ws.send_json({"echo": text})

        self.srv = web.Server(lambda: b"page", on_ws, max_per_ip=3)
        await self.srv.start(self.port, host="127.0.0.1")

    async def asyncTearDown(self):
        await self.srv.stop()

    async def test_stop_closes_open_websockets(self):
        # Python >= 3.12: Server.wait_closed() waits for every open connection, so a phone left on
        # the page would hang "games off" and shutdown forever unless stop() closes them.
        reader, writer, head = await ws_open(self.port)
        self.assertIn(b"101", head)
        writer.write(ws_frame("hi"))
        self.assertEqual(await ws_recv(reader), {"echo": "hi"})
        t = time.monotonic()
        await asyncio.wait_for(self.srv.stop(), 3)
        self.assertLess(time.monotonic() - t, 2)
        self.assertEqual(await asyncio.wait_for(reader.read(), 2), b"")   # the phone sees the close
        writer.close()

    async def test_silent_connections_time_out(self):
        with mock.patch.object(web, "WS_IDLE_TIMEOUT", 0.3):
            reader, writer, _ = await ws_open(self.port)
            msg = await ws_recv(reader, timeout=3)
            self.assertEqual(msg, ("close", 1001))
            writer.close()

    async def test_pings_keep_a_connection_alive(self):
        with mock.patch.object(web, "WS_IDLE_TIMEOUT", 0.4):
            reader, writer, _ = await ws_open(self.port)
            for _ in range(4):
                await asyncio.sleep(0.2)
                writer.write(ws_frame(b"", opcode=0x9))
                h = await asyncio.wait_for(reader.readexactly(2), 2)
                self.assertEqual(h[0] & 0x0F, 0xA)
            writer.write(ws_frame("still here"))
            self.assertEqual(await ws_recv(reader), {"echo": "still here"})
            writer.close()

    async def test_connections_per_address_are_capped(self):
        conns = [await ws_open(self.port) for _ in range(3)]
        reader, writer = await asyncio.open_connection("127.0.0.1", self.port)
        writer.write(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
        self.assertIn(b"429", await asyncio.wait_for(reader.read(), 3))
        writer.close()
        # visitors behind the tunnel are told apart by the forwarded address
        _, w2, head = await ws_open(self.port, "CF-Connecting-IP: 203.0.113.9\r\n")
        self.assertIn(b"101", head)
        w2.close()
        for _, w, _ in conns:
            w.close()
        # slots are given back
        self.assertTrue(await until(lambda: not self.srv.per_ip))

    async def test_control_frames_must_be_small_and_whole(self):
        reader, writer, _ = await ws_open(self.port)
        writer.write(ws_frame(b"x" * 200, opcode=0x9))   # ping payload > 125 bytes
        self.assertEqual(await ws_recv(reader), ("close", 1002))
        writer.close()

    async def test_fragmented_text_is_reassembled(self):
        reader, writer, _ = await ws_open(self.port)
        writer.write(ws_frame("hel", opcode=0x1, fin=False) + ws_frame(b"", opcode=0x9)
                     + ws_frame("lo", opcode=0x0, fin=True))
        h = await asyncio.wait_for(reader.readexactly(2), 2)
        self.assertEqual(h[0] & 0x0F, 0xA)   # the ping between fragments is answered
        self.assertEqual(await ws_recv(reader), {"echo": "hello"})
        writer.close()


class ClientAddressTests(unittest.TestCase):
    def test_forwarded_headers_only_from_a_local_proxy(self):
        h = {"cf-connecting-ip": "198.51.100.7", "x-forwarded-for": "192.0.2.1, 10.0.0.1"}
        self.assertEqual(web.client_address(("127.0.0.1", 5), h), "198.51.100.7")
        self.assertEqual(web.client_address(("192.168.1.20", 5), {"x-forwarded-for": "192.0.2.1, 10.0.0.1"}),
                         "192.0.2.1")
        # straight from the internet (port forward): headers are the visitor's own words
        self.assertEqual(web.client_address(("8.8.8.8", 5), h), "8.8.8.8")
        self.assertEqual(web.client_address(("127.0.0.1", 5), {"x-forwarded-for": "not an ip"}), "127.0.0.1")


# --- the hub ---------------------------------------------------------------------------------


class HostileMessageTests(HubTestCase):
    async def test_hostile_json_does_not_kill_the_connection(self):
        a, _ = self.connect()
        self.hub.press_start(a)
        deep = "[" * 5000 + "]" * 5000
        ws = ScriptedWS([json.dumps({"t": "hello", "id": a}),
                         '{"t":"in","b":Infinity}', '{"t":"in","b":1e999}', '{"t":"in","b":NaN}',
                         deep, "[1,2]", '"str"', '{"t":"in","b":true}', '{"t":"in","b":"256"}',
                         '{"t":"in","b":[1]}', '{"t":"in","b":1}'])
        await self.hub.handle_ws(ws)       # must not raise
        self.assertEqual(ws.sent[0], {"t": "welcome", "id": a})
        # the handler kept going to the last message; the page then closed, releasing the buttons
        self.assertEqual(self.hub.controls.buttons, 0)

    async def test_last_message_is_applied(self):
        a, _ = self.connect()
        self.hub.press_start(a)
        seen = []
        orig = self.hub.controls.set
        self.hub.controls.set = lambda b: (seen.append(b), orig(b))
        ws = ScriptedWS([json.dumps({"t": "hello", "id": a}), '{"t":"in","b":1e999}', '{"t":"in","b":129}'])
        await self.hub.handle_ws(ws)
        self.assertEqual(seen[:1], [129])

    async def test_weird_ids_get_a_fresh_one(self):
        for bad in ({"x": 1}, 12345678901234567890, "ab", "éééééééééééééééééééé"):
            ws = ScriptedWS([json.dumps({"t": "hello", "id": bad})])
            await self.hub.handle_ws(ws)
            self.assertEqual(len(ws.sent[0]["id"]), 32)

    async def test_flood_never_drops_a_button_release(self):
        a, _ = self.connect()
        self.hub.press_start(a)
        msgs = [json.dumps({"t": "hello", "id": a})]
        msgs += ['{"t":"ack"}'] * (server.MSG_BURST + 50)     # use up the budget
        msgs += ['{"t":"in","b":1}', '{"t":"in","b":0}']
        seen = []
        orig = self.hub.controls.set
        self.hub.controls.set = lambda b: (seen.append(b), orig(b))
        await self.hub.handle_ws(ScriptedWS(msgs))
        self.assertEqual(seen[:2], [1, 0])

    async def test_flooding_phone_is_disconnected(self):
        msgs = [json.dumps({"t": "hello", "id": uuid.uuid4().hex})] + ['{"t":"ack"}'] * (7 * server.MSG_BURST)
        ws = ScriptedWS(msgs)
        await self.hub.handle_ws(ws)
        self.assertTrue(ws.msgs)            # it stopped reading before the end

    async def test_non_hello_chatter_before_hello_is_cut_off(self):
        ws = ScriptedWS(['{"t":"start"}'] * 50)
        await self.hub.handle_ws(ws)
        self.assertTrue(ws.msgs)
        self.assertEqual(ws.sent, [])

    async def test_ping_is_answered(self):
        ws = ScriptedWS([json.dumps({"t": "hello", "id": ""}), '{"t":"ping"}'])
        await self.hub.handle_ws(ws)
        self.assertIn({"t": "pong"}, ws.sent)

    async def test_departed_players_are_bounded(self):
        with mock.patch.object(server, "MAX_GONE", 10):
            for _ in range(30):
                await self.hub.handle_ws(ScriptedWS([json.dumps({"t": "hello", "id": ""})]))
        self.assertLessEqual(len(self.hub.gone), 10)


class MarioRaceTests(HubTestCase):
    games = {"cooldownMinutes": 3}

    async def test_no_invite_or_test_pattern_while_a_turn_is_offered_or_starting(self):
        # An invite ends by switching the overlay off: started just before a game, it would switch
        # the overlay off under that game.
        self.fake.set_games({"cooldownMinutes": 0})
        await self.hub.refresh_show()
        a, _ = self.connect()
        b, _ = self.connect()
        self.hub.press_start(a)
        self.hub.press_start(b)
        self.engine.finish.set()
        self.assertTrue(await until(lambda: self.hub.active is None))
        self.engine.finish.clear()
        self.hub._offer_next()
        self.assertEqual(self.hub.offer[0], b)
        r = self.hub.start_invite({"force": True})
        self.assertFalse(r["ok"])
        self.assertIn("turn", r["error"])
        self.assertFalse(self.hub.start_test()["ok"])
        self.assertIsNone(self.hub.busy_reason)

    async def test_failed_game_starts_no_cooldown(self):
        class FailingEngine(StubEngine):
            def play(self, level, cfg, controls, stop, on_update):
                on_update({"phase": "error", "message": "The emulator core could not load smb.nes"})
                return 0

        self.hub.engine = self.engine = FailingEngine()
        a, wa = self.connect()
        self.hub.press_start(a)
        self.assertTrue(await until(lambda: self.hub.active is None))
        self.assertEqual(self.hub.cooldown_left(), 0)
        self.assertEqual(wa.last()["phase"], "error")
        # ... but a game that was played does
        self.hub.engine = self.engine = StubEngine()
        self.hub.press_start(a)
        self.assertTrue(await until(lambda: self.hub.game_view.get("phase") == "playing"))
        self.engine.finish.set()
        self.assertTrue(await until(lambda: self.hub.active is None))
        self.assertGreater(self.hub.cooldown_left(), 170)

    async def test_offer_accept_survives_a_line_that_changed(self):
        a, _ = self.connect()
        self.hub.offer = (a, time.monotonic() + 20)   # offered, but no longer in the list
        self.hub.press_start(a)                        # used to raise ValueError
        self.assertEqual(self.hub.active, a)

    async def test_pixelplusd_coming_back_reopens_the_overlay(self):
        calls = []
        self.engine.request_reopen = lambda: calls.append(1)
        self.fake.stop()
        self.assertFalse(await self.hub.refresh_show())
        self.assertEqual(calls, [])
        self.fake = FakePixelPlus(port=self.fake.port, prop_id=self.fake.prop_id,
                                  games={"port": self.hub.cfg.port}).start()
        self.assertTrue(await self.hub.refresh_show())
        self.assertEqual(calls, [1])

    async def test_matrix_resize_reopens_the_overlay(self):
        calls = []
        self.engine.request_reopen = lambda: calls.append(1)
        self.fake.width, self.fake.height = 64, 32
        self.fake.version += 1
        await self.hub.refresh_show()
        self.assertEqual(calls, [1])


class ArcadeRaceTests(HubTestCase):
    games = {"arcadeMode": True, "arcadeIdleSeconds": 0, "arcadeMinutes": 0}

    async def test_player_who_leaves_loses_the_turn_even_after_a_stopped_mario_game(self):
        # stop_event is Mario's; it stays set after an operator stop (or the switch to arcade mode).
        self.hub.stop_event.set()
        await self.hub._tick_once()
        self.assertTrue(await until(self.hub.arcade_running))
        a, _ = self.connect()
        b, wb = self.connect()
        self.hub.press_start(a)
        self.hub.press_start(b)
        self.assertEqual(self.hub.active, a)
        del self.hub.clients[a]
        self.hub.gone[a] = time.monotonic() - server.ABANDON_AFTER - 1
        await self.hub._tick_once()
        self.assertIsNone(self.hub.active)
        self.assertEqual(wb.last()["phase"], "yourturn")

    async def test_arcade_follows_the_matrix_to_another_prop(self):
        await self.hub._tick_once()
        self.assertTrue(await until(self.hub.arcade_running))
        a, _ = self.connect()
        b, _ = self.connect()
        self.hub.press_start(a)
        self.hub.press_start(b)
        old_prop = self.hub.arcade_cfg.prop_id
        # the operator picks another matrix prop (the fake renames its only matrix)
        self.fake.prop_id = "moved" + uuid.uuid4().hex[:6]
        self.fake.set_games({"matrixPropId": self.fake.prop_id})
        await self.hub.refresh_show()
        await self.hub._tick_once()          # asks the old arcade to close
        self.assertTrue(await until(lambda: not self.hub.arcade_running()))
        await self.hub._tick_once()          # and opens it on the new prop
        self.assertTrue(await until(self.hub.arcade_running))
        self.assertNotEqual(self.hub.arcade_cfg.prop_id, old_prop)
        self.assertEqual(self.engine.arcade_runs, 2)
        # the player kept the controller and the line was kept
        self.assertEqual(self.hub.active, a)
        self.assertEqual(self.hub.queue, [b])

    async def test_a_failing_arcade_is_not_restarted_every_second(self):
        class Crashing(StubEngine):
            def arcade(self, cfg, controls, stop, to_menu, on_update):
                self.arcade_runs += 1

        self.hub.arcade_stop.set()                     # close the arcade the set-up opened
        self.assertTrue(await until(lambda: not self.hub.arcade_running()))
        self.hub.engine = self.engine = Crashing()
        self.hub.arcade_started = -server.ARCADE_RETRY
        await self.hub._tick_once()
        self.assertTrue(await until(lambda: not self.hub.arcade_running()))
        for _ in range(3):
            await self.hub._tick_once()
        self.assertEqual(self.engine.arcade_runs, 1)


# --- the arcade loop -------------------------------------------------------------------------


class ArcadeLoopTests(unittest.TestCase):
    def test_turn_ending_on_the_list_does_not_bounce_the_next_pick(self):
        """end_turn() sets to_menu; if that happens while the list is up, the next player's pick
        must still start the game instead of returning straight to the list."""
        engine = game.Engine(api=None)
        stop, to_menu = threading.Event(), threading.Event()
        picks = ["/roms/a.nes"]
        seen = []

        class FakeSession:
            def __init__(self, *a, **k):
                self.model = mock.Mock(width=32, height=16)
                self.brightness = 100

            def __enter__(self):
                return self

            def __exit__(self, *exc):
                return False

            def text(self, lines):
                pass

        class FakeMenu:
            def __init__(self, roms):
                pass

            def run(self, session, controls, stop_, folder):
                if not picks:
                    stop.set()
                    return None
                to_menu.set()               # the previous player's turn ended meanwhile
                return picks.pop()

        def fake_run(session, core, scaler, controls, stop_ev, **kw):
            seen.append(stop_ev.is_set())
            return "stop"

        with mock.patch.object(game, "Session", FakeSession), \
                mock.patch.object(game.arcade, "Menu", FakeMenu), \
                mock.patch.object(game.arcade, "list_roms", lambda folder: ["/roms/a.nes"]), \
                mock.patch.object(engine, "_load", lambda cfg, path: mock.Mock()), \
                mock.patch.object(engine, "_run", fake_run):
            engine.arcade(mock.Mock(scale_mode="fit"), game.Controls(), stop, to_menu, lambda d: None)
        self.assertEqual(seen, [False])


# --- the overlay, reopened ------------------------------------------------------------------


class ReopenTests(unittest.TestCase):
    def setUp(self):
        self.fake = FakePixelPlus(prop_id="r" + uuid.uuid4().hex[:9], width=32, height=16).start()
        self.api = PixelPlus(self.fake.base)
        self.cfg = config.from_show(self.api.show())

    def tearDown(self):
        self.fake.stop()

    def test_wrong_size_frames_are_skipped(self):
        model = OverlayModel(self.api, self.cfg.matrix)
        model.open()
        try:
            model.write(np.full((10, 10, 3), 7, np.uint8))    # used to raise from the mmap
            self.assertEqual(self.fake.shm_header()[2] & 1, 0)
            model.write(np.full((16, 32, 3), 7, np.uint8))
            self.assertEqual(self.fake.shm_header()[2] & 1, 1)
        finally:
            model.close()

    def test_session_reopens_after_a_restart_or_resize(self):
        with game.Session(self.api, self.cfg, show="stop") as session:
            self.assertTrue(self.fake.overlay_enabled[self.fake.prop_id])
            # pixelplusd restarted: it forgot the overlay was on
            self.fake.overlay_enabled[self.fake.prop_id] = False
            self.fake.width, self.fake.height = 40, 20
            session.reopen.set()
            self.assertTrue(session.reopen_model())
            self.assertFalse(session.reopen.is_set())
            self.assertTrue(self.fake.overlay_enabled[self.fake.prop_id])
            self.assertEqual((session.model.width, session.model.height), (40, 20))
            session.model.write(np.full((20, 40, 3), 9, np.uint8))
            self.assertEqual(self.fake.shm_frame()[:3], bytes([9, 9, 9]))

    def test_engine_request_reaches_the_running_session(self):
        engine = game.Engine(self.api)
        engine.request_reopen()             # nothing running: nothing to do
        s = game.Session(self.api, self.cfg)
        engine.session = s
        engine.request_reopen()
        self.assertTrue(s.reopen.is_set())


# --- hot path ------------------------------------------------------------------------------


class ScaleFrameTests(unittest.TestCase):
    def test_same_picture_as_the_full_conversion(self):
        rng = np.random.default_rng(1)
        for fmt, bpp in ((0, 2), (1, 4), (2, 2)):
            for (w, h, pitch_extra) in ((256, 240, 0), (256, 224, 16)):
                pitch = (w + pitch_extra) * bpp
                data = rng.integers(0, 256, pitch * h, dtype=np.uint8).tobytes()
                frame = Frame(data, w, h, pitch, fmt)
                for size, mode, bright in (((80, 40), "fit", 100), ((33, 17), "stretch", 60), ((200, 90), "fit", 0)):
                    s = Scaler(*size, mode=mode, brightness=bright)
                    np.testing.assert_array_equal(s.scale_frame(frame), s.scale(frame_to_rgb(frame)))

    def test_resize(self):
        s = Scaler(80, 40)
        frame = Frame(bytes(256 * 240 * 4), 256, 240, 256 * 4, 1)
        self.assertEqual(s.scale_frame(frame).shape, (40, 80, 3))
        s.resize(64, 32)
        self.assertEqual(s.scale_frame(frame).shape, (32, 64, 3))


# --- audio ---------------------------------------------------------------------------------


class AudioTests(unittest.TestCase):
    def test_chatty_aplay_does_not_stall(self):
        """aplay writes a line per underrun to stderr; a pipe nobody reads fills up (64 KiB) and
        aplay then blocks, which stalled the game's sound and made stop() take seconds."""
        tmp = tempfile.mkdtemp()
        out = os.path.join(tmp, "out.raw")
        script = os.path.join(tmp, "aplay")
        with open(script, "w") as f:
            f.write("#!/bin/sh\npython3 -c \"import sys; sys.stderr.write('underrun!!! (at least 1 ms long)\\n' * 8000)"
                    "; sys.stderr.flush()\"\ncat > %s\n" % out)
        os.chmod(script, os.stat(script).st_mode | stat.S_IEXEC)
        env = dict(os.environ, PATH=tmp + os.pathsep + os.environ.get("PATH", ""))
        with mock.patch.dict(os.environ, env):
            a = audio.AudioOut(48000, "default", 100)
            a.start()
            chunk = bytes(3200)
            for _ in range(60):
                a.play(chunk)
                time.sleep(0.005)
            self.assertTrue(wait_for(lambda: os.path.exists(out) and os.path.getsize(out) >= 3200 * 40, 5))
            t = time.monotonic()
            a.stop()
            self.assertLess(time.monotonic() - t, 1.5)
        self.assertIn("underrun", a._last_error())

    def test_stop_with_a_stuck_sound_card(self):
        tmp = tempfile.mkdtemp()
        script = os.path.join(tmp, "aplay")
        with open(script, "w") as f:
            f.write("#!/bin/sh\nexec sleep 30\n")    # never reads its input
        os.chmod(script, os.stat(script).st_mode | stat.S_IEXEC)
        env = dict(os.environ, PATH=tmp + os.pathsep + os.environ.get("PATH", ""))
        with mock.patch.dict(os.environ, env):
            a = audio.AudioOut(48000, "default", 100)
            a.start()
            for _ in range(40):
                a.play(bytes(16384))
            time.sleep(0.3)
            t = time.monotonic()
            a.stop()
            self.assertLess(time.monotonic() - t, 8)
            self.assertIsNone(a._proc)


if __name__ == "__main__":
    unittest.main()
