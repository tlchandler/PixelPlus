"""Unit tests that need neither pixelplusd, an emulator core nor a ROM.

    python3 -m unittest discover -s games/tests
"""

import os
import tempfile
import time
import unittest

import numpy as np

import support  # noqa: F401,E402  (import paths, quiet logs)
from pixelplus_games import arcade, config, font, smb  # noqa: E402
from pixelplus_games.display import Scaler  # noqa: E402


class LevelTests(unittest.TestCase):
    def test_parse_levels(self):
        self.assertEqual(len(smb.parse_levels("all")), 32)
        self.assertEqual(smb.parse_levels(" 1-1, 4-2 ,9-9, 1-1"), ["1-1", "4-2"])
        self.assertEqual(len(smb.parse_levels("nonsense")), 32)

    def test_area_numbers(self):
        # worlds with a pipe-intro area before level 2 shift later levels by one
        self.assertEqual(smb.area_number(1, 1), 0)
        self.assertEqual(smb.area_number(1, 2), 2)
        self.assertEqual(smb.area_number(1, 4), 4)
        self.assertEqual(smb.area_number(3, 2), 1)
        self.assertEqual(smb.area_number(7, 3), 3)
        self.assertEqual(smb.area_number(8, 4), 3)

    def test_score_includes_the_unstored_ones_digit(self):
        # RAM $07DD-$07E2 holds millions..tens; 000040 there is 400 points on screen
        class FakeCore:
            ram = bytearray(0x800)
        core = FakeCore()
        core.ram[smb.PLAYER_SCORE_DISPLAY:smb.PLAYER_SCORE_DISPLAY + 6] = bytes([0, 0, 0, 0, 4, 0])
        self.assertEqual(smb.SMB(core).score(), 400)
        core.ram[smb.PLAYER_SCORE_DISPLAY:smb.PLAYER_SCORE_DISPLAY + 6] = bytes([9, 9, 9, 9, 9, 5])
        self.assertEqual(smb.SMB(core).score(), 9999950)


class ScalerTests(unittest.TestCase):
    def test_fit_is_centred(self):
        img = np.full((240, 256, 3), 200, np.uint8)
        out = Scaler(80, 40, mode="fit").scale(img)
        cols = np.where(out[..., 0].any(axis=0))[0]
        rows = np.where(out[..., 0].any(axis=1))[0]
        self.assertEqual(cols.min(), 79 - cols.max())  # equal margins left and right
        self.assertEqual((rows.min(), rows.max()), (0, 39))

    def test_stretch_fills(self):
        out = Scaler(80, 40, mode="stretch").scale(np.full((240, 256, 3), 90, np.uint8))
        self.assertTrue((out == 90).all())

    def test_brightness(self):
        out = Scaler(80, 40, mode="stretch", brightness=50).scale(np.full((240, 256, 3), 200, np.uint8))
        self.assertTrue((out == 100).all())

    def test_map_point_centre(self):
        s = Scaler(80, 40, crop=(0, 0, 256, 240), mode="stretch")
        x, y = s.map_point(128, 120)
        self.assertAlmostEqual(x, 40, delta=0.01)
        self.assertAlmostEqual(y, 20, delta=0.01)

    def test_overscan_trimmed_frames(self):
        out = Scaler(80, 40).scale(np.zeros((224, 256, 3), np.uint8))
        self.assertEqual(out.shape, (40, 80, 3))


class FontTests(unittest.TestCase):
    def test_centred_text(self):
        img = np.zeros((40, 80, 3), np.uint8)
        font.draw_lines_centered(img, [("WORLD", (255, 255, 255)), ("4-2", (255, 200, 0))])
        cols = np.where(img.any(axis=(0, 2)))[0]
        rows = np.where(img.any(axis=(1, 2)))[0]
        self.assertLessEqual(abs(cols.min() - (79 - cols.max())), 1)
        self.assertLessEqual(abs(rows.min() - (39 - rows.max())), 1)

    def test_every_glyph_is_3x5(self):
        for ch, rows in font._GLYPHS.items():
            self.assertEqual(len(rows), 5, ch)
            self.assertTrue(all(len(r) == 3 for r in rows), ch)

    def test_clipping(self):
        img = np.zeros((5, 5, 3), np.uint8)
        font.draw(img, font.text_mask("HELLO"), -3, 2, (1, 1, 1))  # must not raise


class ArcadeMenuTests(unittest.TestCase):
    def test_display_name(self):
        self.assertEqual(arcade.display_name("/x/Legend of Zelda, The (U) (PRG1) [!].nes"), "LEGEND OF ZELDA, THE")
        self.assertEqual(arcade.display_name("mega_man_2.nes"), "MEGA MAN 2")
        self.assertEqual(arcade.display_name("/var/lib/pixelplus/games/roms/smb.nes"), "SUPER MARIO BROS.")

    def test_list_roms_filters(self):
        with tempfile.TemporaryDirectory() as d:
            for n, data in (("b.nes", b"NES\x1a" + bytes(40)), ("a.NES", b"NES\x1a" + bytes(40)),
                            ("readme.txt", b"hi"), ("tiny.nes", b"NES")):
                with open(os.path.join(d, n), "wb") as f:
                    f.write(data)
            self.assertEqual([os.path.basename(p) for p in arcade.list_roms(d)], ["a.NES", "b.nes"])

    def test_scrolling_and_scrollbar(self):
        m = arcade.Menu(["/g/%02d.nes" % i for i in range(20)])
        m.index = 19
        img = m.render(80, 40)
        self.assertGreater(m.top, 0)                   # scrolled to keep the selection visible
        self.assertTrue(img[:, 79].any())               # scroll bar drawn in the last column
        thumb = np.where(img[:, 79].max(axis=1) == 255)[0]
        self.assertGreater(thumb.min(), 20)             # thumb at the bottom for the last item

    def test_marquee_bounds(self):
        for t in np.linspace(0, 30, 300):
            self.assertTrue(0 <= arcade.Menu._marquee(40, t) <= 40)


class ConfigTests(unittest.TestCase):
    def show(self, games=None, props=None, audio=None):
        props = props if props is not None else [
            {"id": "arch1", "name": "Arch", "kind": "arch", "pixelCount": 50},
            {"id": "mx1", "name": "Big Matrix", "kind": "matrix", "pixelCount": 3200,
             "matrix": {"width": 80, "height": 40, "pixelMap": []}},
        ]
        settings = {"games": games if games is not None else {}}
        if audio is not None:
            settings["audio"] = audio
        return {"version": 3, "props": props, "settings": settings}

    def test_defaults_match_game_settings_default(self):
        cfg = config.from_show({})
        self.assertFalse(cfg.enabled)
        self.assertEqual((cfg.port, cfg.game_seconds, cfg.cooldown_minutes), (8088, 60, 5))
        self.assertEqual(cfg.play_window, "duringShow")
        self.assertTrue(cfg.pause_show and cfg.santa_hat)
        self.assertEqual((cfg.arcade_minutes, cfg.arcade_idle_seconds), (0, 600))
        self.assertEqual((cfg.invite_every_minutes, cfg.invite_style, cfg.invite_flashes), (5, "text", 3))
        self.assertEqual(cfg.invite_color, (255, 0, 0))
        self.assertEqual((cfg.scale_mode, cfg.output_fps, cfg.brightness, cfg.volume), ("fit", 40, 100, 80))
        self.assertEqual(cfg.crop, (8, 32, 256, 224))
        self.assertEqual(cfg.turn_timeout, 20)
        self.assertEqual(cfg.audio_device, "default")
        self.assertIsNone(cfg.matrix)
        self.assertTrue(cfg.matrix_problem)

    def test_camel_case_settings(self):
        cfg = config.from_show(self.show({
            "enabled": True, "matrixPropId": "mx1", "port": 9000, "gameSeconds": 90, "cooldownMinutes": 2,
            "levels": "1-1,4-2", "playWindow": "anytime", "pauseShow": False, "santaHat": False,
            "arcadeMode": True, "arcadeMinutes": 3, "arcadeIdleSeconds": 30, "publicUrl": " mario.example.com ",
            "inviteEveryMinutes": 7, "inviteStyle": "alternate", "inviteFlashes": 4, "inviteColor": "#00FF80",
            "scaleMode": "stretch", "outputFps": 20, "brightness": 50, "volume": 30, "crop": [0, 8, 256, 232],
        }, audio={"device": "hw:1,0", "volume": 70}))
        self.assertTrue(cfg.enabled and cfg.arcade_mode)
        self.assertEqual(cfg.matrix, config.Matrix("mx1", "Big Matrix", 80, 40))
        self.assertEqual(cfg.prop_id, "mx1")
        self.assertEqual((cfg.port, cfg.game_seconds, cfg.cooldown_minutes, cfg.levels), (9000, 90, 2, "1-1,4-2"))
        self.assertEqual(cfg.play_window, "anytime")
        self.assertFalse(cfg.pause_show or cfg.santa_hat)
        self.assertEqual((cfg.arcade_minutes, cfg.arcade_idle_seconds), (3, 30))
        self.assertEqual(cfg.public_url, "mario.example.com")
        self.assertEqual((cfg.invite_every_minutes, cfg.invite_style, cfg.invite_flashes), (7, "alternate", 4))
        self.assertEqual(cfg.invite_color, (0, 255, 128))
        self.assertEqual((cfg.scale_mode, cfg.output_fps, cfg.brightness, cfg.volume), ("stretch", 20, 50, 30))
        self.assertEqual(cfg.crop, (0, 8, 256, 232))
        self.assertEqual(cfg.audio_device, "hw:1,0")

    def test_values_are_clamped(self):
        cfg = config.from_show(self.show({"gameSeconds": 1, "port": 99999, "inviteFlashes": 50, "outputFps": 60,
                                          "brightness": 400, "crop": [200, 8, 100, 232], "inviteStyle": "sparkles",
                                          "inviteColor": "red", "volume": "loud"}))
        self.assertEqual(cfg.game_seconds, 10)
        self.assertEqual(cfg.port, 65535)
        self.assertEqual(cfg.invite_flashes, 10)
        self.assertEqual(cfg.output_fps, 40)
        self.assertEqual(cfg.brightness, 100)
        self.assertEqual(cfg.crop, config.DEFAULT_CROP)   # right < left: fall back
        self.assertEqual(cfg.invite_style, "text")
        self.assertEqual(cfg.invite_color, (255, 0, 0))
        self.assertEqual(cfg.volume, 80)

    def test_matrix_selection(self):
        # no prop chosen, exactly one matrix prop: use it
        self.assertEqual(config.from_show(self.show()).prop_id, "mx1")
        # the chosen prop is gone
        cfg = config.from_show(self.show({"matrixPropId": "nope"}))
        self.assertIsNone(cfg.matrix)
        self.assertIn("no longer exists", cfg.matrix_problem)
        # the chosen prop has no matrix geometry
        cfg = config.from_show(self.show({"matrixPropId": "arch1"}))
        self.assertIsNone(cfg.matrix)
        self.assertIn("no matrix", cfg.matrix_problem)
        # two matrices and none chosen: ask the user
        two = [{"id": "a", "matrix": {"width": 8, "height": 8}}, {"id": "b", "matrix": {"width": 8, "height": 8}}]
        self.assertIsNone(config.from_show(self.show(props=two)).matrix)

    def test_invite_style_names(self):
        self.assertEqual(config.normalize_style("both"), "alternate")
        self.assertEqual(config.normalize_style("QR"), "qr")
        self.assertEqual(config.normalize_style(None, "qr"), "qr")
        self.assertEqual(config.parse_color("#f80"), (255, 136, 0))

    def test_paths_follow_the_environment(self):
        old = dict(os.environ)
        try:
            os.environ["PIXELPLUS_DATA_DIR"] = "/srv/pp"
            os.environ.pop("PIXELPLUS_API", None)
            os.environ["PIXELPLUS_HTTP_PORT"] = "8080"
            os.environ["PIXELPLUS_NES_CORE_OPTIONS"] = "a=1; b = two ;junk"
            self.assertEqual(config.rom_dir(), "/srv/pp/games/roms")
            self.assertEqual(config.smb_rom_path(), "/srv/pp/games/roms/smb.nes")
            self.assertEqual(config.api_base(), "http://127.0.0.1:8080")
            self.assertEqual(config.core_options(), {"a": "1", "b": "two"})
            os.environ["PIXELPLUS_API"] = "http://10.0.0.5:81/"
            self.assertEqual(config.api_base(), "http://10.0.0.5:81")
            del os.environ["PIXELPLUS_DATA_DIR"]
            self.assertEqual(config.rom_dir(), "/var/lib/pixelplus/games/roms")
        finally:
            os.environ.clear()
            os.environ.update(old)


class ControlsTests(unittest.TestCase):
    def test_reset_restarts_the_idle_clock(self):
        # A new arcade turn must not inherit the idle time of the last player: set(0) on
        # already-released buttons is not a change, so it would leave the old timestamp.
        from pixelplus_games.game import Controls
        c = Controls()
        c.changed -= 3600
        c.set(0)
        self.assertLess(c.changed, time.monotonic() - 3000)
        c.reset()
        self.assertGreater(c.changed, time.monotonic() - 1)
        self.assertEqual(c.buttons, 0)


if __name__ == "__main__":
    unittest.main()
