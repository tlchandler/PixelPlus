"""The games sidecar runs as the unprivileged ``pixelplus`` user everywhere: the unit that
install.sh installs must be the one the Debian package ships."""

import os
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.join(HERE, "..", "..")


def read(*parts):
    with open(os.path.join(ROOT, *parts), encoding="utf-8") as f:
        return f.read()


class UnitTests(unittest.TestCase):
    def test_games_unit_matches_the_package(self):
        self.assertEqual(read("games", "pixelplus-games.service"),
                         read("packaging", "systemd", "pixelplus-games.service"))

    def test_runs_unprivileged_with_the_shared_runtime_dir(self):
        unit = read("games", "pixelplus-games.service")
        self.assertIn("\nUser=pixelplus\n", unit)
        self.assertIn("\nGroup=pixelplus\n", unit)
        self.assertIn("/run/pixelplus", unit)
        self.assertIn("PIXELPLUS_GAMES_SOCKET=/run/pixelplus/games.sock", unit)
        self.assertIn("WorkingDirectory=/usr/lib/pixelplus/games", unit)
        tmpfiles = read("packaging", "tmpfiles", "pixelplus.conf")
        self.assertIn("d /run/pixelplus 0775 pixelplus pixelplus -", tmpfiles)

    def test_installer_uses_the_service_user(self):
        sh = read("games", "install.sh")
        self.assertIn("SERVICE_USER=pixelplus", sh)
        self.assertIn('install -d -o "$SERVICE_USER"', sh)


if __name__ == "__main__":
    unittest.main()
