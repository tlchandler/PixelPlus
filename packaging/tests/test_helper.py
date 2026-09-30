"""Tests for packaging/bin/pixelplus-helper (the root helper pixelplusd starts via
``systemctl start pixelplus-helper@<verb>.service``). Runs the script unprivileged with
its paths redirected into a temp dir: ``python -m pytest packaging/tests``."""

import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
HELPER = os.path.join(HERE, "..", "bin", "pixelplus-helper")


@unittest.skipUnless(shutil.which("bash"), "bash required")
class HelperTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp()
        self.status = os.path.join(self.tmp, "run")
        os.makedirs(self.status)
        self.firstboot = os.path.join(self.tmp, "firstboot.py")
        self.calls = os.path.join(self.tmp, "calls.txt")
        with open(self.firstboot, "w") as f:
            f.write(
                "import sys\n"
                f"open({self.calls!r}, 'a').write(' '.join(sys.argv[1:]) + '\\n')\n"
                "sys.exit(3 if '--board' in sys.argv and 'broken' in sys.argv else 0)\n"
            )
        self.hosts = os.path.join(self.tmp, "hosts")
        self.env = dict(
            os.environ,
            PIXELPLUS_HELPER_STATUS_DIR=self.status,
            PIXELPLUS_HELPER_PRIVATE_DIR=os.path.join(self.tmp, "private"),
            PIXELPLUS_FIRSTBOOT=self.firstboot,
            PIXELPLUS_HOSTS_FILE=self.hosts,
        )

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def run_helper(self, instance):
        return subprocess.run(["bash", HELPER, instance], env=self.env, capture_output=True, text=True)

    def status_of(self, verb):
        with open(os.path.join(self.status, f"helper-{verb}.json")) as f:
            return json.load(f)

    def test_config_txt_reports_ok_and_passes_arguments(self):
        r = self.run_helper("config-txt:difftxlarge:1600")
        self.assertEqual(r.returncode, 0, r.stderr)
        st = self.status_of("config-txt")
        self.assertEqual(st["verb"], "config-txt")
        self.assertEqual(st["state"], "ok")
        self.assertIsInstance(st["updatedAt"], int)
        with open(self.calls) as f:
            self.assertIn("board-config --board difftxlarge --pixels 1600", f.read())

    def test_refresh_index_runs_apt_get_update_only(self):
        # PixelPlus images disable apt's daily timers: the update check needs this.
        apt = os.path.join(self.tmp, "apt-get")
        with open(apt, "w") as f:
            f.write(f"#!/bin/sh\necho \"$@\" >> {self.calls}\nexit ${{APT_FAIL:-0}}\n")
        os.chmod(apt, 0o755)
        self.env["PIXELPLUS_APT_GET"] = apt
        r = self.run_helper("refresh-index")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(self.status_of("refresh-index")["state"], "ok")
        with open(self.calls) as f:
            self.assertEqual(f.read().split(), ["update", "-q"])
        self.env["APT_FAIL"] = "100"
        r = self.run_helper("refresh-index")
        self.assertNotEqual(r.returncode, 0)
        self.assertEqual(self.status_of("refresh-index")["state"], "failed")
        self.assertNotEqual(self.run_helper("refresh-index:x").returncode, 0)

    def test_config_txt_failure_is_reported(self):
        r = self.run_helper("config-txt:broken")
        self.assertNotEqual(r.returncode, 0)
        self.assertEqual(self.status_of("config-txt")["state"], "failed")

    def test_rejects_bad_arguments(self):
        for inst, verb in [("config-txt:a;b", "config-txt"), ("config-txt:difftx:12x", "config-txt"),
                           ("wifi-country:USA", "wifi-country"), ("update:now", "update"),
                           ("a:b:c:d", "invalid"), ("frobnicate", "frobnicate")]:
            r = self.run_helper(inst)
            self.assertNotEqual(r.returncode, 0, inst)
            self.assertEqual(self.status_of(verb)["state"], "failed", inst)

    def test_status_messages_are_valid_json(self):
        r = self.run_helper('config-txt:x"y')
        self.assertNotEqual(r.returncode, 0)
        json.loads(open(os.path.join(self.status, "helper-config-txt.json")).read())

    def test_planted_symlinks_are_not_followed(self):
        victim = os.path.join(self.tmp, "victim")
        with open(victim, "w") as f:
            f.write("precious\n")
        os.chmod(victim, 0o600)
        # the unprivileged user owns /run/pixelplus: it could plant these
        os.symlink(victim, os.path.join(self.status, "helper-hosts.json"))
        os.symlink(victim, os.path.join(self.status, "helper-hosts.json.tmp"))
        r = self.run_helper("hosts")
        self.assertEqual(r.returncode, 0, r.stderr)
        with open(victim) as f:
            self.assertEqual(f.read(), "precious\n")
        self.assertEqual(os.stat(victim).st_mode & 0o777, 0o600)
        self.assertFalse(os.path.islink(os.path.join(self.status, "helper-hosts.json")))
        self.assertEqual(self.status_of("hosts")["state"], "ok")

    def test_hosts_rewrites_the_127_0_1_1_line(self):
        with open(self.hosts, "w") as f:
            f.write("127.0.0.1\tlocalhost\n127.0.1.1\told-name\n::1\tlocalhost\n")
        r = self.run_helper("hosts")
        self.assertEqual(r.returncode, 0, r.stderr)
        with open(self.hosts) as f:
            lines = f.read().splitlines()
        self.assertIn("127.0.0.1\tlocalhost", lines)
        self.assertNotIn("127.0.1.1\told-name", lines)
        self.assertEqual(len([ln for ln in lines if ln.startswith("127.0.1.1")]), 1)


if __name__ == "__main__":
    sys.exit(unittest.main())
