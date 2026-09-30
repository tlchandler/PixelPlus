"""Tests for packaging/release-index.py (signed-update index, F15)."""
import importlib.util
import json
import os
import shutil
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("release_index", os.path.join(HERE, "..", "release-index.py"))
ri = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ri)


class ReleaseIndexTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp()

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def deb(self, name, body=b"x"):
        p = os.path.join(self.tmp, name)
        with open(p, "wb") as f:
            f.write(body)
        return p

    def test_index_lists_every_arch_with_hash_and_url(self):
        a = self.deb("pixelplus_1.2.3_arm64.deb", b"arm")
        b = self.deb("pixelplus_1.2.3_amd64.deb", b"amd")
        out = os.path.join(self.tmp, "idx.json")
        ri.main(["--channel", "stable", "--version", "1.2.3", "--base-url",
                 "https://example.com/rel/", "--out", out, a, b])
        idx = json.load(open(out))
        self.assertEqual((idx["v"], idx["channel"], idx["version"]), (1, "stable", "1.2.3"))
        self.assertEqual([f["arch"] for f in idx["files"]], ["amd64", "arm64"])
        arm = [f for f in idx["files"] if f["arch"] == "arm64"][0]
        self.assertEqual(arm["size"], 3)
        self.assertEqual(arm["url"], "https://example.com/rel/pixelplus_1.2.3_arm64.deb")
        self.assertEqual(len(arm["sha256"]), 64)
        self.assertEqual((idx["protoMin"], idx["protoMax"], idx["formatVersion"]), (2, 2, 1))

    def test_refuses_mismatches(self):
        a = self.deb("pixelplus_1.2.3_arm64.deb")
        for args in [("beta", "1.2.4", "https://x", [a]),       # wrong version
                     ("nightly", "1.2.3", "https://x", [a]),     # unknown channel
                     ("stable", "1.2.3", "http://x", [a]),       # not https
                     ("stable", "1.2.3; rm", "https://x", [a]),  # bad version
                     ("stable", "1.2.3", "https://x", [])]:      # nothing
            with self.assertRaises(ValueError, msg=str(args)):
                ri.build(*args)
        with self.assertRaises(ValueError):
            ri.build("stable", "1.2.3", "https://x", [self.deb("pixelplus_1.2.3_armhf.deb")])

    def test_committed_fixture_matches_the_script(self):
        # crates/pixelplus-daemon parses this very file (services/updates.rs tests).
        fx = os.path.join(HERE, "fixtures")
        idx = ri.build("stable", "9.9.9", "https://github.com/tlchandler/PixelPlus/releases/download/v9.9.9",
                       [os.path.join(fx, "minisign", "pixelplus_9.9.9_arm64.deb")], date="2026-09-30T00:00:00Z")
        self.assertEqual(idx, json.load(open(os.path.join(fx, "pixelplus-stable.json"))))


if __name__ == "__main__":
    unittest.main()
