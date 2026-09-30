"""The real libretro frontend and game sessions, with a tiny homebrew ROM made on the spot.

Skipped when no NES core is installed (``apt-get install libretro-nestopia``).
No commercial ROM is needed or used.

    python3 -m unittest discover -s games/tests
"""

import os
import shutil
import tempfile
import threading
import unittest
import uuid

import support  # noqa: F401  (import paths, quiet logs)
from support import wait_for  # noqa: E402
from fake_pixelplus import FakePixelPlus  # noqa: E402
from pixelplus_games import config, game  # noqa: E402
from pixelplus_games.api import PixelPlus  # noqa: E402
from pixelplus_games.game import Controls, Engine  # noqa: E402
from pixelplus_games.libretro import JOYPAD_A, find_core  # noqa: E402

COUNTER = 0x10   # the ROM increments this RAM byte forever


def homebrew_rom():
    """A 24 KiB NROM image: reset -> SEI; CLD; loop: INC $10; JMP loop. NMI/IRQ -> RTI."""
    prg = bytearray(16 * 1024)
    code = bytes([0x78, 0xD8, 0xEE, COUNTER, 0x00, 0x4C, 0x02, 0xC0, 0x40])
    prg[:len(code)] = code
    prg[0x3FFA:0x4000] = bytes([0x08, 0xC0, 0x00, 0xC0, 0x08, 0xC0])   # NMI, RESET, IRQ vectors
    return b"NES\x1a" + bytes([1, 1, 0, 0]) + bytes(8) + bytes(prg) + bytes(8 * 1024)


CORE = find_core(config.core_path())
_ENGINE = None   # libretro cores keep global state: one Core (so one Engine) per process


@unittest.skipUnless(CORE, "no libretro NES core installed")
class EmulatorTests(unittest.TestCase):
    def setUp(self):
        global _ENGINE
        self.tmp = tempfile.mkdtemp()
        self.saved_env = dict(os.environ)
        os.environ["PIXELPLUS_DATA_DIR"] = self.tmp
        os.makedirs(config.rom_dir())
        self.fake = FakePixelPlus(prop_id="e" + uuid.uuid4().hex[:9], width=32, height=16).start()
        self.api = PixelPlus(self.fake.base)
        if _ENGINE is None:
            _ENGINE = Engine(self.api)
        self.engine = _ENGINE
        self.engine.api = self.api
        self.saved = game.BANNER_SECONDS, game.RESULT_SECONDS
        game.BANNER_SECONDS, game.RESULT_SECONDS = 0.1, 0.1

    def tearDown(self):
        game.BANNER_SECONDS, game.RESULT_SECONDS = self.saved
        self.fake.stop()
        os.environ.clear()
        os.environ.update(self.saved_env)
        shutil.rmtree(self.tmp, ignore_errors=True)

    def cfg(self, **games):
        self.fake.set_games(games)
        return config.from_show(self.api.show())

    def write_rom(self, name):
        with open(os.path.join(config.rom_dir(), name), "wb") as f:
            f.write(homebrew_rom())

    def test_core_runs_the_rom(self):
        self.write_rom("counter.nes")
        core = self.engine._load(self.cfg(), os.path.join(config.rom_dir(), "counter.nes"))
        core.reset()
        core.run()
        before = core.ram[COUNTER]
        for _ in range(3):
            core.run()
        self.assertNotEqual(core.ram[COUNTER], before)
        self.assertIsNotNone(core.frame)
        self.assertGreater(core.fps, 50)
        state = core.serialize()
        self.assertTrue(state)
        self.assertTrue(core.unserialize(state))

    def test_ready(self):
        cfg = self.cfg()
        self.assertIn("Super Mario Bros.", self.engine.ready(cfg))
        self.assertIn("arcade", self.engine.ready(cfg, arcade_mode=True))
        self.write_rom("smb.nes")
        self.assertIsNone(self.engine.ready(cfg))
        self.assertIsNone(self.engine.ready(cfg, arcade_mode=True))

    def test_timed_game_pauses_and_resumes_the_show(self):
        # Not Super Mario Bros., so the level select can't land, but the session must still run,
        # show frames, count down and hand everything back.
        self.write_rom("smb.nes")
        cfg = self.cfg(gameSeconds=10)
        updates = []
        stop = threading.Event()
        t = threading.Thread(target=self.engine.play, args=("1-1", cfg, Controls(), stop, updates.append))
        t.start()
        self.assertTrue(wait_for(lambda: any(u["phase"] == "playing" for u in updates), 10))
        self.assertEqual(self.fake.player_state, "paused")
        self.assertTrue(self.fake.overlay_enabled[self.fake.prop_id])
        self.assertTrue(wait_for(lambda: self.fake.shm_header()[2] & 1, 2))   # frames are arriving
        stop.set()
        t.join(10)
        self.assertFalse(t.is_alive())
        phases = [u["phase"] for u in updates]
        self.assertEqual(phases[0], "starting")
        self.assertEqual(phases[-1], "over")
        self.assertNotIn("error", phases)
        self.assertEqual(self.fake.player_state, "playing")
        self.assertFalse(self.fake.overlay_enabled[self.fake.prop_id])
        self.assertEqual(self.fake.commands(), ["pause", "resume"])

    def test_arcade_menu_pick_and_exit(self):
        self.write_rom("counter.nes")
        self.write_rom("Another Game (U) [!].nes")
        cfg = self.cfg(arcadeMode=True)
        controls = Controls()
        stop, to_menu = threading.Event(), threading.Event()
        updates = []
        t = threading.Thread(target=self.engine.arcade, args=(cfg, controls, stop, to_menu, updates.append))
        t.start()
        try:
            self.assertTrue(wait_for(lambda: updates and updates[-1]["phase"] == "menu", 5))
            self.assertEqual(self.fake.player_state, "idle")     # the arcade stopped the show
            self.assertTrue(wait_for(lambda: any(self.fake.shm_frame()), 2))   # the game list is drawn
            controls.set(1 << JOYPAD_A)                          # play the highlighted game
            self.assertTrue(wait_for(lambda: updates[-1]["phase"] == "arcade", 5))
            self.assertEqual(updates[-1]["game"], "ANOTHER GAME")
            controls.set(0)
            to_menu.set()                                        # the turn ended: back to the list
            self.assertTrue(wait_for(lambda: updates[-1]["phase"] == "menu", 5))
        finally:
            stop.set()
            t.join(10)
        self.assertFalse(t.is_alive())
        self.assertFalse(self.fake.overlay_enabled[self.fake.prop_id])
        self.assertEqual(self.fake.commands(), ["stop"])          # and did not restart it


if __name__ == "__main__":
    unittest.main()
