import json
import os
import shutil
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "firstboot"))

import firstboot  # noqa: E402
import nmconn  # noqa: E402
import pptxt  # noqa: E402

TEMPLATE = os.path.join(HERE, "..", "boot", "pixelplus.txt")


class RecordingSys(firstboot.Sys):
    """Executes nothing; records commands; answers the PixelPlus CLI with canned output."""

    def __init__(self, board_json=None, fragment="dtoverlay=pixelplus-dpi-4\n"):
        super().__init__(dry_run=False)
        self.board_json = board_json
        self.fragment = fragment

    def run(self, argv, timeout=60, input_text=None):
        self.commands.append(list(argv) + ([f"<stdin:{input_text!r}>"] if input_text else []))
        return True

    def output(self, argv, timeout=30):
        self.commands.append(list(argv))
        if argv[1:3] == ["--json", "detect"]:
            return self.board_json
        if argv[1] == "config-txt":
            return self.fragment
        return None


class FirstbootTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp()
        self.boot = os.path.join(self.tmp, "boot")
        os.makedirs(self.boot)
        with open(os.path.join(self.boot, "config.txt"), "w") as f:
            f.write("dtparam=audio=on\n[pi5]\ndtoverlay=nospi10\n")
        self.patches = {
            "BOOT_DIRS": [self.boot],
            "STATE_DIR": os.path.join(self.tmp, "state"),
            "ETC_DIR": os.path.join(self.tmp, "etc"),
            "DATA_DIR": os.path.join(self.tmp, "data"),
            "TEMPLATE": TEMPLATE,
            "ROOT": self.tmp,
            "PIXELPLUS_CLI": sys.executable,  # any absolute path: "installed"
        }
        self.saved = {k: getattr(firstboot, k) for k in self.patches}
        for k, v in self.patches.items():
            setattr(firstboot, k, v)
        self.saved_nm = nmconn.NM_DIR
        nmconn.NM_DIR = os.path.join(self.tmp, "nm")
        os.makedirs(os.path.join(self.tmp, "etc"), exist_ok=True)
        with open(os.path.join(self.tmp, "etc", "hosts"), "w") as f:
            f.write("127.0.0.1\tlocalhost\n127.0.1.1\traspberrypi\n")
        self.saved_have = firstboot.Sys.have
        self.saved_user = firstboot.first_login_user
        firstboot.Sys.have = staticmethod(lambda cmd: cmd in ("raspi-config",))
        import pwd
        home = os.path.join(self.tmp, "home", "pi")
        firstboot.first_login_user = lambda: pwd.struct_passwd(("pi", "x", os.getuid(), os.getgid(), "", home, "/bin/bash"))

    def tearDown(self):
        firstboot.Sys.have = self.saved_have
        firstboot.first_login_user = self.saved_user
        for k, v in self.saved.items():
            setattr(firstboot, k, v)
        nmconn.NM_DIR = self.saved_nm
        shutil.rmtree(self.tmp)

    def write_txt(self, extra):
        with open(TEMPLATE) as f:
            text = f.read()
        for k, v in extra.items():
            self.assertIn(f"\n{k}=\n", text, k)
            text = text.replace(f"\n{k}=\n", f"\n{k}={v}\n", 1)
        with open(os.path.join(self.boot, "pixelplus.txt"), "w") as f:
            f.write(text)

    def read(self, *parts):
        with open(os.path.join(self.tmp, *parts)) as f:
            return f.read()

    def test_missing_file_restored_from_template(self):
        s = RecordingSys()
        self.assertEqual(firstboot.apply(s, allow_reboot=False), 0)
        self.assertTrue(os.path.isfile(os.path.join(self.boot, "pixelplus.txt")))
        state = json.load(open(os.path.join(self.tmp, "state", "state.json")))
        self.assertIn("first_boot_done", state)
        # hotspot defaults handed to netwatch
        nw = json.load(open(os.path.join(self.tmp, "etc", "netwatch.json")))
        self.assertEqual(nw, {"hotspot": True, "hotspotPassword": "pixelplus", "hotspotTimeout": 75})

    def test_full_apply_then_idempotent(self):
        self.write_txt({
            "wifi_ssid": "Home", "wifi_password": "hunter22!", "wifi_country": "US", "hostname": "pp-garage",
            "role": "follower", "ui_password": "letmein", "ssh": "on", "ssh_password": "sshpassword1",
        })
        s = RecordingSys()
        firstboot.apply(s, allow_reboot=False)
        flat = [" ".join(c) for c in s.commands]
        self.assertIn("raspi-config nonint do_wifi_country US", flat)
        self.assertIn("hostnamectl set-hostname pp-garage", flat)
        self.assertIn("raspi-config nonint do_ssh 0", flat)
        self.assertIn("nmcli connection reload", flat)
        self.assertTrue(any(c.startswith("chpasswd") and "sshpassword1" in c for c in flat))
        self.assertIn("127.0.1.1\tpp-garage", self.read("etc", "hosts"))

        kf = self.read("nm", "pixelplus-wifi.nmconnection")
        self.assertIn("ssid=Home", kf)
        self.assertIn("psk=hunter22!", kf)

        prov = json.load(open(os.path.join(self.tmp, "data", "provision.json")))
        self.assertEqual(prov["role"], "follower")
        self.assertEqual(prov["uiPassword"], "letmein")
        self.assertEqual(os.stat(os.path.join(self.tmp, "data", "provision.json")).st_mode & 0o777, 0o600)

        # secrets scrubbed, SSID kept
        txt = open(os.path.join(self.boot, "pixelplus.txt")).read()
        self.assertNotIn("hunter22!", txt)
        self.assertNotIn("letmein", txt)
        self.assertNotIn("sshpassword1", txt)
        self.assertIn("wifi_ssid=Home", txt)
        self.assertEqual(txt.count("[applied"), 3)

        # board detected via CLI -> config.txt block, reboot requested (but not allowed here)
        cfg = open(os.path.join(self.boot, "config.txt")).read()
        self.assertEqual(cfg.count(firstboot.BLOCK_BEGIN), 0)  # no board json -> nothing

        # second boot: nothing re-applied
        s2 = RecordingSys()
        firstboot.apply(s2, allow_reboot=False)
        flat2 = [" ".join(c) for c in s2.commands]
        self.assertFalse(any("hostnamectl" in c or "chpasswd" in c or "do_wifi_country" in c for c in flat2))

        # user edits only the hostname: wifi keyfile keeps the scrubbed password
        txt = txt.replace("hostname=pp-garage", "hostname=pp-porch")
        open(os.path.join(self.boot, "pixelplus.txt"), "w").write(txt)
        s3 = RecordingSys()
        firstboot.apply(s3, allow_reboot=False)
        flat3 = [" ".join(c) for c in s3.commands]
        self.assertIn("hostnamectl set-hostname pp-porch", flat3)
        self.assertNotIn("nmcli connection reload", flat3)
        self.assertIn("psk=hunter22!", self.read("nm", "pixelplus-wifi.nmconnection"))

        # new SSID without password => open network profile
        txt = txt.replace("wifi_ssid=Home", "wifi_ssid=Cafe")
        open(os.path.join(self.boot, "pixelplus.txt"), "w").write(txt)
        firstboot.apply(RecordingSys(), allow_reboot=False)
        kf = self.read("nm", "pixelplus-wifi.nmconnection")
        self.assertIn("ssid=Cafe", kf)
        self.assertNotIn("psk=", kf)

    def test_errors_file(self):
        self.write_txt({"hostname": "bad_name!", "wifi_country": "ZZZ"})
        firstboot.apply(RecordingSys(), allow_reboot=False)
        err = open(os.path.join(self.boot, "pixelplus-errors.txt")).read()
        self.assertIn("hostname", err)
        self.assertIn("wifi_country", err)
        # fix it -> file removed
        txt = open(os.path.join(self.boot, "pixelplus.txt")).read()
        txt = txt.replace("hostname=bad_name!", "hostname=good").replace("wifi_country=ZZZ", "wifi_country=US")
        open(os.path.join(self.boot, "pixelplus.txt"), "w").write(txt)
        firstboot.apply(RecordingSys(), allow_reboot=False)
        self.assertFalse(os.path.exists(os.path.join(self.boot, "pixelplus-errors.txt")))

    def conf(self):
        p = os.path.join(self.boot, "pixelplus.conf")
        return open(p).read() if os.path.exists(p) else None

    def test_board_conf_and_reboot_once(self):
        det = '{"simulated": false, "pi": {"model": "Raspberry Pi 5"}, "board": {"board": "difftxlarge", "rev": "A", "source": "eeprom"}}'
        frag = "[all]\ndtoverlay=pixelplus-dpi-pi5,vactive=807\ndtoverlay=i2c-rtc,ds3231\n"
        s = RecordingSys(board_json=det, fragment=frag)
        firstboot.apply(s, allow_reboot=True)
        flat = [" ".join(c) for c in s.commands]
        self.assertIn(f"{sys.executable} config-txt --board difftxlarge", flat)
        self.assertIn("systemctl --no-block reboot", flat)
        cfg = open(os.path.join(self.boot, "config.txt")).read()
        self.assertTrue(cfg.startswith("dtparam=audio=on\n[pi5]\ndtoverlay=nospi10\n"))
        self.assertTrue(cfg.endswith("[all]\ninclude pixelplus.conf\n"))
        self.assertIn("dtoverlay=pixelplus-dpi-pi5,vactive=807\n", self.conf())
        # next boot: nothing changes, no reboot
        s2 = RecordingSys(board_json=det, fragment=frag)
        firstboot.apply(s2, allow_reboot=True)
        flat2 = [" ".join(c) for c in s2.commands]
        self.assertNotIn("systemctl --no-block reboot", flat2)
        self.assertFalse(any("config-txt" in c for c in flat2))
        self.assertEqual(open(os.path.join(self.boot, "config.txt")).read(), cfg)
        # the daemon (via the helper) regenerated it for longer strings: firstboot keeps it
        open(os.path.join(self.boot, "pixelplus.conf"), "w").write("# daemon\ndtoverlay=pixelplus-dpi-pi5,vactive=1607\n")
        firstboot.apply(RecordingSys(board_json=det, fragment=frag), allow_reboot=True)
        self.assertIn("vactive=1607", self.conf())

    def test_board_config_command_forces(self):
        s = RecordingSys(fragment="[all]\ndtoverlay=pixelplus-dpi,vactive=1607\n")
        s_orig = firstboot.Sys
        firstboot.Sys = lambda dry_run=False: s  # main() builds its own Sys
        try:
            os.environ["PIXELPLUS_FIRSTBOOT_LOG"] = os.path.join(self.tmp, "fb.log")
            firstboot.main(["--dry-run", "board-config", "--board", "difftx", "--pixels", "1600"])
        finally:
            firstboot.Sys = s_orig
        flat = [" ".join(c) for c in s.commands]
        self.assertIn(f"{sys.executable} config-txt --board difftx --pixels 1600", flat)
        self.assertIn("vactive=1607", self.conf())

    def run_main(self, sysops, argv):
        s_orig = firstboot.Sys

        def fake(dry_run=False):
            return sysops

        fake.have = s_orig.have  # detect_board() calls Sys.have
        firstboot.Sys = fake
        try:
            os.environ["PIXELPLUS_FIRSTBOOT_LOG"] = os.path.join(self.tmp, "fb.log")
            return firstboot.main(argv)
        finally:
            firstboot.Sys = s_orig

    def test_board_config_failure_exits_nonzero(self):
        # the root helper maps the exit code to {"state": "failed"} for the web UI
        s = RecordingSys(fragment=None)
        self.assertEqual(self.run_main(s, ["--dry-run", "board-config", "--board", "difftx", "--pixels", "1600"]), 2)
        s = RecordingSys(board_json=None)
        self.assertEqual(self.run_main(s, ["--dry-run", "board-config"]), 2)
        s = RecordingSys(fragment="[all]\ndtoverlay=pixelplus-dpi,vactive=807\n")
        self.assertEqual(self.run_main(s, ["--dry-run", "board-config", "--board", "difftx"]), 0)

    def test_private_json_ignores_planted_symlinks(self):
        data = os.path.join(self.tmp, "data")
        os.makedirs(data, exist_ok=True)
        victim = os.path.join(self.tmp, "victim")
        with open(victim, "w") as f:
            f.write('{"secret": 1}')
        os.chmod(victim, 0o640)
        ppath = os.path.join(data, "provision.json")
        # the service user owns /var/lib/pixelplus and could plant these links
        os.symlink(victim, ppath)
        os.symlink(victim, ppath + ".tmp")
        self.assertEqual(firstboot.read_json_nofollow(ppath), {})
        firstboot.write_private_json(ppath, {"role": "leader"})
        with open(victim) as f:
            self.assertEqual(f.read(), '{"secret": 1}')
        self.assertEqual(os.stat(victim).st_mode & 0o777, 0o640)
        self.assertFalse(os.path.islink(ppath))
        self.assertEqual(firstboot.read_json_nofollow(ppath), {"role": "leader"})
        self.assertEqual(os.stat(ppath).st_mode & 0o777, 0o600)

    def test_reboot_loop_guard(self):
        boards = ["difftx", "diffsmart"]
        for i in range(firstboot.MAX_BOARD_REBOOTS + 2):
            s = RecordingSys(board_json='{"board": {"board": "%s"}}' % boards[i % 2], fragment=f"# variant {i}\n")
            firstboot.apply(s, allow_reboot=True)
            rebooted = "systemctl --no-block reboot" in [" ".join(c) for c in s.commands]
            self.assertEqual(rebooted, i < firstboot.MAX_BOARD_REBOOTS, i)

    def test_board_override_from_txt(self):
        self.write_txt({})
        txt = open(os.path.join(self.boot, "pixelplus.txt")).read().replace("board=auto", "board=diffsmart")
        open(os.path.join(self.boot, "pixelplus.txt"), "w").write(txt)
        s = RecordingSys(board_json='{"board": {"board": "difftx"}}')
        firstboot.apply(s, allow_reboot=False)
        flat = [" ".join(c) for c in s.commands]
        self.assertIn(f"{sys.executable} config-txt --board diffsmart", flat)
        self.assertFalse(any("detect" in c for c in flat))

    def test_blank_eeprom_leaves_boot_config_alone(self):
        s = RecordingSys(board_json='{"board": {"board": null, "suggested": "difftx"}}')
        firstboot.apply(s, allow_reboot=True)
        self.assertIsNone(self.conf())
        self.assertNotIn("include", open(os.path.join(self.boot, "config.txt")).read())

    def test_include_and_legacy_block(self):
        legacy = "a=1\n\n" + firstboot.BLOCK_BEGIN + "\n[all]\nx=1\n" + firstboot.BLOCK_END + "\n"
        out = firstboot.ensure_include(legacy)
        self.assertEqual(out, firstboot.ensure_include(out))
        self.assertNotIn("x=1", out)
        self.assertTrue(out.endswith("[all]\ninclude pixelplus.conf\n"))

    def test_check_command(self):
        p = os.path.join(self.tmp, "t.txt")
        open(p, "w").write("hostname=ok\n")
        self.assertEqual(firstboot.main(["check", p]), 0)
        open(p, "w").write("role=boss\n")
        self.assertEqual(firstboot.main(["check", p]), 2)


if __name__ == "__main__":
    unittest.main()
