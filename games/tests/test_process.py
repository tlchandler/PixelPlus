"""End to end: ``python3 -m pixelplus_games`` as its own process against the fake pixelplusd.

    python3 -m unittest discover -s games/tests
"""

import os
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import unittest
import urllib.request
import uuid

import support  # noqa: F401  (import paths, quiet logs)
from support import GAMES, wait_for  # noqa: E402
from fake_pixelplus import FakePixelPlus  # noqa: E402
from pixelplus_games import ctl  # noqa: E402


def free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()
    return port


class ProcessTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp()
        self.sock = os.path.join(self.tmp, "run", "games.sock")
        self.port = free_port()
        self.fake = FakePixelPlus(prop_id="p" + uuid.uuid4().hex[:9], games={"port": self.port}).start()
        self.fake.overlay_enabled[self.fake.prop_id] = True   # left on by a crashed previous run
        env = dict(os.environ, PIXELPLUS_API=self.fake.base, PIXELPLUS_DATA_DIR=os.path.join(self.tmp, "data"),
                   PIXELPLUS_GAMES_SOCKET=self.sock, PYTHONUNBUFFERED="1")
        self.log = open(os.path.join(self.tmp, "games.log"), "w+")
        self.proc = subprocess.Popen([sys.executable, "-m", "pixelplus_games"], cwd=GAMES, env=env,
                                     stdout=self.log, stderr=subprocess.STDOUT)

    def tearDown(self):
        if self.proc.poll() is None:
            self.proc.kill()
            self.proc.wait()
        self.log.close()
        self.fake.stop()
        shutil.rmtree(self.tmp, ignore_errors=True)

    def output(self):
        self.log.seek(0)
        return self.log.read()

    def status(self):
        return ctl.send({"cmd": "status"}, timeout=2, path=self.sock)

    def test_runs_serves_follows_settings_and_stops(self):
        self.assertTrue(wait_for(lambda: self.status().get("enabled"), 10), self.output())
        self.assertTrue(os.path.isdir(os.path.join(self.tmp, "data", "games", "roms")))
        # the overlay a previous run left on is handed back to the show
        self.assertTrue(wait_for(lambda: self.fake.overlay_enabled[self.fake.prop_id] is False))
        s = self.status()
        self.assertEqual(s["model"], {"width": 80, "height": 40})
        self.assertIn("unavailable", s)
        self.assertTrue(wait_for(lambda: self.status().get("port") == self.port, 5), self.output())

        with urllib.request.urlopen("http://127.0.0.1:%d/" % self.port, timeout=3) as r:
            page = r.read().decode()
        self.assertIn("PRESS START TO BEGIN", page)
        self.assertIn("#F5A524", page)

        # live settings: turning games off (a {type:"show"} event) closes the controller port
        self.assertTrue(wait_for(lambda: self.fake.ws_clients, 5), self.output())
        self.fake.set_games({"enabled": False})
        self.assertTrue(wait_for(lambda: self.status().get("enabled") is False, 5))
        self.assertTrue(wait_for(lambda: self.status().get("port") is None, 5))

        self.proc.send_signal(signal.SIGTERM)
        self.assertEqual(self.proc.wait(10), 0, self.output())
        self.assertFalse(os.path.exists(self.sock))
        self.assertEqual(self.fake.unauthorized, 0)
        self.assertNotIn("Traceback", self.output())


if __name__ == "__main__":
    unittest.main()
