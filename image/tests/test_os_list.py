import json
import lzma
import os
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
TOOL = os.path.join(HERE, "..", "rpi-imager", "make-os-list.py")


class OsListTests(unittest.TestCase):
    def test_describe_and_merge(self):
        with tempfile.TemporaryDirectory() as d:
            raw = os.urandom(100_000) + b"\0" * 900_000
            img = os.path.join(d, "x.img.xz")
            with open(img, "wb") as f:
                f.write(lzma.compress(raw))
            frags = []
            for rel in ("bookworm", "trixie"):
                out = os.path.join(d, f"{rel}.json")
                subprocess.run([sys.executable, TOOL, "--image", img, "--release", rel, "--version", "1.2.3",
                                "--url", f"https://example.com/{rel}.img.xz", "--release-date", "2026-10-01",
                                "--out", out], check=True, capture_output=True)
                frags.append(out)
            repo = os.path.join(d, "repo.json")
            subprocess.run([sys.executable, TOOL, "--merge", *frags, "--out", repo], check=True, capture_output=True)
            doc = json.load(open(repo))
            self.assertEqual([e["init_format"] for e in doc["os_list"]], ["cloudinit-rpi", "systemd"])
            e = doc["os_list"][0]
            self.assertEqual(e["extract_size"], len(raw))
            self.assertEqual(len(e["extract_sha256"]), 64)
            self.assertIn("pi3-64bit", e["devices"])
            self.assertTrue(doc["imager"]["devices"])
            schema = os.environ.get("RPI_IMAGER_SCHEMA")
            if schema:
                import jsonschema

                jsonschema.validate(doc, json.load(open(schema)))


if __name__ == "__main__":
    unittest.main()
