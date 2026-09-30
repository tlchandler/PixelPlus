"""The pi-gen stage installs the package and keeps it as the signed-update rollback copy."""
import os
import shutil
import stat
import subprocess
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
STAGE = os.path.join(HERE, "..", "stage-pixelplus", "01-pixelplus")


@unittest.skipUnless(shutil.which("bash"), "bash required")
class StageTests(unittest.TestCase):
    def test_installs_and_keeps_a_rollback_copy(self):
        with tempfile.TemporaryDirectory() as d:
            work = os.path.join(d, "stage")
            shutil.copytree(STAGE, work)
            os.makedirs(os.path.join(work, "files"), exist_ok=True)
            deb = os.path.join(work, "files", "pixelplus_1.2.3_arm64.deb")
            with open(deb, "w") as f:
                f.write("deb")
            with open(deb + ".minisig", "w") as f:
                f.write("sig")
            root = os.path.join(d, "rootfs")
            os.makedirs(root)
            log = os.path.join(d, "chroot.log")
            script = (
                f'on_chroot() {{ cat >> "{log}"; }}; export -f on_chroot; '
                f'ROOTFS_DIR="{root}" RELEASE=bookworm IMG_DATE=2026-09-30 PIXELPLUS_VERSION=1.2.3 '
                f'bash ./00-run.sh'
            )
            r = subprocess.run(["bash", "-c", script], cwd=work, capture_output=True, text=True)
            self.assertEqual(r.returncode, 0, r.stderr)
            rb = os.path.join(root, "var", "cache", "pixelplus", "rollback")
            self.assertEqual(sorted(os.listdir(rb)), ["pixelplus_1.2.3_arm64.deb", "pixelplus_1.2.3_arm64.deb.minisig"])
            self.assertEqual(stat.S_IMODE(os.stat(rb).st_mode), 0o700)
            self.assertIn("apt-get", open(log).read())
            self.assertIn("PIXELPLUS_IMAGE_VERSION=1.2.3", open(os.path.join(root, "etc", "pixelplus", "appliance")).read())


if __name__ == "__main__":
    unittest.main()
