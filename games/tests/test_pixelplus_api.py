"""The PixelPlus client, overlay output, invites and sessions, against the fake pixelplusd.

    python3 -m unittest discover -s games/tests
"""

import struct
import unittest
import urllib.error
import urllib.request
import uuid

import numpy as np

import support  # noqa: F401  (import paths, quiet logs)
from support import wait_for  # noqa: E402
from fake_pixelplus import FakePixelPlus  # noqa: E402
from pixelplus_games import config, invite  # noqa: E402
from pixelplus_games.api import ApiError, PixelPlus  # noqa: E402
from pixelplus_games.display import OverlayModel  # noqa: E402
from pixelplus_games.game import Session  # noqa: E402


class FakeTestCase(unittest.TestCase):
    shm = True

    def setUp(self):
        self.fake = FakePixelPlus(prop_id="t" + uuid.uuid4().hex[:9], shm=self.shm).start()
        self.api = PixelPlus(self.fake.base)
        self.cfg = config.from_show(self.api.show())

    def tearDown(self):
        self.fake.stop()


class ClientTests(FakeTestCase):
    def test_every_request_says_it_is_local(self):
        self.api.player()
        self.api.pause()
        self.assertEqual(self.fake.unauthorized, 0)
        self.assertEqual(self.fake.forbidden, 0)
        # the old constant header is not enough
        req = urllib.request.Request(self.fake.base + "/api/v1/show", headers={"X-PixelPlus-Local": "1"})
        with self.assertRaises(urllib.error.HTTPError) as ctx:
            urllib.request.urlopen(req, timeout=3)
        self.assertEqual(ctx.exception.code, 401)
        self.fake.unauthorized = 0
        # without the header, a protected daemon refuses
        with self.assertRaises(urllib.error.HTTPError) as ctx:
            urllib.request.urlopen(self.fake.base + "/api/v1/show", timeout=3)
        self.assertEqual(ctx.exception.code, 401)
        self.assertEqual(self.fake.unauthorized, 1)

    def test_new_token_after_a_daemon_restart_is_picked_up(self):
        self.api.player()
        with open(self.fake.token_file, "w") as f:
            f.write("rotated-token-0123456789")
        self.fake.token = "rotated-token-0123456789"
        self.assertIsNotNone(self.api.player())
        self.assertTrue(self.api.pause())

    def test_error_envelope_is_surfaced(self):
        with self.assertRaises(ApiError) as ctx:
            self.api.request("GET", "/nope")
        self.assertEqual(ctx.exception.status, 404)
        self.assertEqual(ctx.exception.code, "not_found")
        self.assertIn("No route for GET /api/v1/nope", str(ctx.exception))

    def test_show_and_player(self):
        show = self.api.show()
        self.assertEqual(show["settings"]["games"]["matrixPropId"], self.fake.prop_id)
        self.assertEqual(self.cfg.matrix.width, 80)
        self.assertEqual(self.cfg.audio_device, "null")
        self.assertEqual(self.api.player_state(), "playing")
        self.assertTrue(self.api.pause())
        self.assertEqual(self.api.player_state(), "paused")
        self.assertTrue(self.api.resume())
        self.assertTrue(self.api.stop())
        self.assertEqual(self.api.player_state(), "idle")
        self.assertEqual(self.fake.commands(), ["pause", "resume", "stop"])

    def test_unreachable_daemon(self):
        api = PixelPlus("http://127.0.0.1:9")
        self.assertIsNone(api.player())
        self.assertEqual(api.player_state(), "unknown")
        self.assertFalse(api.pause())
        with self.assertRaises(ApiError):
            api.show()

    def test_ws_url(self):
        self.assertEqual(PixelPlus("http://127.0.0.1:8080").ws_url(), "ws://127.0.0.1:8080/api/v1/ws")
        self.assertEqual(PixelPlus("https://pp.local").ws_url(), "wss://pp.local/api/v1/ws")
        self.assertTrue(PixelPlus("http://localhost").is_local())
        self.assertFalse(PixelPlus("http://10.1.2.3").is_local())


class SharedMemoryTests(FakeTestCase):
    def test_frames_go_through_shared_memory(self):
        model = OverlayModel(self.api, self.cfg.matrix)
        model.open()
        self.assertEqual(model.transport, "shared memory")
        self.assertEqual((model.width, model.height), (80, 40))
        img = np.zeros((40, 80, 3), np.uint8)
        img[0, 0] = (255, 0, 0)       # top-left
        img[39, 79] = (0, 0, 255)     # bottom-right
        model.write(img)
        w, h, flags = self.fake.shm_header()
        self.assertEqual((w, h), (80, 40))
        self.assertEqual(flags & 1, 1)          # "new frame" bit set for pixelplusd
        frame = self.fake.shm_frame()
        self.assertEqual(frame[:3], b"\xff\x00\x00")      # row-major RGB from the top-left
        self.assertEqual(frame[-3:], b"\x00\x00\xff")
        # pixelplusd clears the bit after copying; the next frame sets it again
        with open(self.fake.shm_path(), "r+b") as f:
            f.seek(8)
            f.write(struct.pack("=I", 0))
        model.blank()
        self.assertEqual(self.fake.shm_header()[2] & 1, 1)
        self.assertEqual(self.fake.shm_frame(), bytes(80 * 40 * 3))
        model.close()
        self.assertEqual(self.fake.frames, 0)   # nothing went over HTTP

    def test_enable_disable(self):
        model = OverlayModel(self.api, self.cfg.matrix)
        model.enable()
        self.assertTrue(self.fake.overlay_enabled[self.fake.prop_id])
        model.disable()
        self.assertFalse(self.fake.overlay_enabled[self.fake.prop_id])


class HttpFallbackTests(FakeTestCase):
    shm = False  # the daemon offers no shared memory: frames are PUT instead

    def test_frames_go_over_http(self):
        model = OverlayModel(self.api, self.cfg.matrix)
        model.open()
        self.assertEqual(model.transport, "HTTP")
        self.assertEqual((model.width, model.height), (80, 40))  # taken from the show's matrix info
        img = np.full((40, 80, 3), 7, np.uint8)
        model.write(img)
        self.assertTrue(wait_for(lambda: self.fake.frames >= 1))
        model.blank()
        model.close()  # the last frame (the blank) is still delivered
        self.assertEqual(self.fake.last_frame, bytes(80 * 40 * 3))

    def test_remote_daemon_uses_http(self):
        api = PixelPlus(self.fake.base.replace("127.0.0.1", "localhost"))
        self.assertTrue(api.is_local())
        remote = PixelPlus(self.fake.base)
        remote.is_local = lambda: False
        model = OverlayModel(remote, self.cfg.matrix)
        model.open()
        self.assertEqual(model.transport, "HTTP")
        model.close()


class InviteTests(FakeTestCase):
    def setUp(self):
        super().setUp()
        self.saved = invite.ON_SECONDS, invite.OFF_SECONDS, invite.TEST_PATTERN_SECONDS
        invite.ON_SECONDS, invite.OFF_SECONDS, invite.TEST_PATTERN_SECONDS = 0.05, 0.01, 0.05

    def tearDown(self):
        invite.ON_SECONDS, invite.OFF_SECONDS, invite.TEST_PATTERN_SECONDS = self.saved
        super().tearDown()

    def test_text_invite_enables_then_releases_the_matrix(self):
        seen = []
        orig = OverlayModel.write

        def spy(model, rgb):
            seen.append((rgb.copy(), self.fake.overlay_enabled.get(self.fake.prop_id)))
            orig(model, rgb)

        OverlayModel.write = spy
        try:
            invite.show(self.api, self.cfg, "mario.example.com", flashes=2, style="text")
        finally:
            OverlayModel.write = orig
        self.assertTrue(any(img.any() and on for img, on in seen))   # the URL was lit while enabled
        self.assertFalse(self.fake.overlay_enabled[self.fake.prop_id])  # handed back to the show
        self.assertEqual(self.fake.shm_frame(), bytes(80 * 40 * 3))   # and left blank
        self.assertEqual(self.fake.commands(), [])                   # the show keeps playing

    def test_qr_invite(self):
        m = invite.qr_mask("https://mario.example.com", 40)
        self.assertIsNotNone(m)
        self.assertLessEqual(m.shape[0], 40)
        img = invite._qr_frame(80, 40, "https://mario.example.com", (255, 0, 0), 1.0)
        self.assertTrue((img == 255).any())
        invite.show(self.api, self.cfg, "mario.example.com", flashes=1, style="qr")
        self.assertFalse(self.fake.overlay_enabled[self.fake.prop_id])

    def test_test_pattern(self):
        frames = []
        orig = OverlayModel.write
        OverlayModel.write = lambda m, rgb: (frames.append(rgb.copy()), orig(m, rgb))
        try:
            invite.test_pattern(self.api, self.cfg)
        finally:
            OverlayModel.write = orig
        pattern = frames[0]
        self.assertEqual(tuple(pattern[0, 0]), (255, 0, 0))    # red top-left
        self.assertEqual(tuple(pattern[0, 79]), (0, 255, 0))   # green top-right
        self.assertEqual(tuple(pattern[39, 40]), (0, 0, 255))  # blue border
        self.assertFalse(self.fake.overlay_enabled[self.fake.prop_id])


class SessionTests(FakeTestCase):
    def test_pause_and_resume_the_show(self):
        with Session(self.api, self.cfg, show="pause") as s:
            self.assertTrue(self.fake.overlay_enabled[self.fake.prop_id])
            self.assertEqual(self.fake.player_state, "paused")
            s.text([("WORLD", (255, 255, 255)), ("4-2", (255, 200, 0))])
            self.assertTrue(any(self.fake.shm_frame()))
        self.assertEqual(self.fake.player_state, "playing")
        self.assertFalse(self.fake.overlay_enabled[self.fake.prop_id])
        self.assertEqual(self.fake.commands(), ["pause", "resume"])

    def test_no_pause_when_disabled_or_not_playing(self):
        cfg = config.from_show(dict(self.api.show(), settings={"games": dict(
            self.api.show()["settings"]["games"], pauseShow=False)}))
        with Session(self.api, cfg, show="pause"):
            pass
        self.fake.player_state = "idle"
        with Session(self.api, self.cfg, show="pause"):
            pass
        self.assertEqual(self.fake.commands(), [])

    def test_operator_stop_during_a_game_is_respected(self):
        with Session(self.api, self.cfg, show="pause"):
            self.fake.player_state = "idle"   # someone pressed Stop in the UI meanwhile
        self.assertEqual(self.fake.commands(), ["pause"])

    def test_arcade_stops_the_show(self):
        with Session(self.api, self.cfg, show="stop"):
            self.assertEqual(self.fake.player_state, "idle")
        self.assertEqual(self.fake.commands(), ["stop"])
        self.assertEqual(self.fake.player_state, "idle")   # not restarted

    def test_no_matrix(self):
        cfg = config.from_show({})
        with self.assertRaises(RuntimeError):
            Session(self.api, cfg)


if __name__ == "__main__":
    unittest.main()
