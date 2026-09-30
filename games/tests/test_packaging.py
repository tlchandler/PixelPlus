"""The games sidecar runs as its own unprivileged ``pixelplus-games`` user everywhere (group
``pixelplus-overlay`` shared with pixelplusd): the unit that install.sh installs must be the
one the Debian package ships."""

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

    def test_runs_as_its_own_user_seeing_only_its_folder(self):
        unit = read("games", "pixelplus-games.service")
        self.assertIn("\nUser=pixelplus-games\n", unit)
        self.assertIn("\nGroup=pixelplus-overlay\n", unit)
        self.assertIn("PIXELPLUS_GAMES_SOCKET=/run/pixelplus-games/games.sock", unit)
        self.assertIn("RuntimeDirectory=pixelplus-games", unit)
        self.assertIn("TemporaryFileSystem=/var/lib/pixelplus:ro", unit)
        self.assertIn("BindPaths=/var/lib/pixelplus/games", unit)
        self.assertIn("WorkingDirectory=/usr/lib/pixelplus/games", unit)
        daemon = read("packaging", "systemd", "pixelplusd.service")
        self.assertIn("PIXELPLUS_GAMES_SOCKET=/run/pixelplus-games/games.sock", daemon)
        tmpfiles = read("packaging", "tmpfiles", "pixelplus.conf")
        self.assertIn("d /run/pixelplus 0775 pixelplus pixelplus -", tmpfiles)
        self.assertIn("d /var/lib/pixelplus/games 2770 pixelplus pixelplus-overlay -", tmpfiles)
        postinst = read("packaging", "debian", "postinst")
        self.assertIn("pixelplus-games", postinst)
        self.assertIn("adduser \"${SERVICE_USER}\" pixelplus-overlay", postinst)

    def test_installer_uses_the_service_user(self):
        sh = read("games", "install.sh")
        self.assertIn("SERVICE_USER=pixelplus-games", sh)
        self.assertIn("SIDECAR_GROUP=pixelplus-overlay", sh)
        self.assertIn('-g "$SIDECAR_GROUP" -m 2770', sh)


if __name__ == "__main__":
    unittest.main()
