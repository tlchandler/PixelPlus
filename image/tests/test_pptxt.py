import datetime as dt
import os
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "firstboot"))

import pptxt  # noqa: E402

TEMPLATE = os.path.join(HERE, "..", "boot", "pixelplus.txt")
FIXTURE = os.path.join(HERE, "fixtures", "imager-rendered.txt")


class ParseTests(unittest.TestCase):
    def test_template_parses_clean(self):
        pr = pptxt.parse(pptxt.read_file(TEMPLATE))
        self.assertEqual(pr.warnings, [])
        s, errors = pptxt.validate(pr, zoneinfo_dir=None)
        self.assertEqual(errors, [])
        # defaults of the template
        self.assertIsNone(s.wifi_ssid)
        self.assertTrue(s.hotspot)
        self.assertEqual(s.hotspot_password, "pixelplus")
        self.assertEqual(s.hotspot_timeout, 75)
        self.assertEqual(s.board, "auto")
        self.assertIsNone(s.ssh)
        # every documented key is known
        keys = {ln.key for ln in pr.lines if ln.key}
        self.assertEqual(keys, set(pptxt.KEYS))

    def test_crlf_bom_quotes_and_hash(self):
        text = '﻿wifi_ssid=Joe\'s #1 wifi\r\nwifi_password = "  spaced pass  "\r\n# comment=ignored\r\n'
        pr = pptxt.parse(text)
        self.assertEqual(pr.newline, "\r\n")
        self.assertEqual(pr.get("wifi_ssid"), "Joe's #1 wifi")
        self.assertEqual(pr.get("wifi_password"), "  spaced pass  ")
        self.assertNotIn("comment", pr.values)

    def test_aliases_and_case(self):
        pr = pptxt.parse("SSID=Home\nPSK=secretpass\nCountry=us\nTZ=UTC\n")
        s, errors = pptxt.validate(pr, countries={"US", "GB"}, zoneinfo_dir=None)
        self.assertEqual(errors, [])
        self.assertEqual((s.wifi_ssid, s.wifi_password, s.wifi_country, s.timezone), ("Home", "secretpass", "US", "UTC"))

    def test_unknown_key_warns(self):
        pr = pptxt.parse("wifi_sid=oops\nnot a setting\n")
        self.assertEqual(len(pr.warnings), 2)

    def test_empty_duplicate_does_not_wipe(self):
        pr = pptxt.parse("hostname=garage\nhostname=\n")
        self.assertEqual(pr.get("hostname"), "garage")

    def test_validation_errors(self):
        pr = pptxt.parse(
            "wifi_ssid=Home\nwifi_password=short\nwifi_country=USA\nhostname=-bad-\nrole=boss\n"
            "ip_address=192.168.1.0/24\nhotspot_password=1234\nhotspot_timeout=5\nssh=maybe\nboard=fpp\n"
        )
        s, errors = pptxt.validate(pr, countries={"US"}, zoneinfo_dir=None)
        keys = sorted(e.split(":")[0] for e in errors)
        self.assertEqual(
            keys,
            sorted(["wifi_password", "wifi_country", "hostname", "role", "ip_address", "hotspot_password",
                    "hotspot_timeout", "ssh", "board"]),
        )
        # valid values still applied
        self.assertEqual(s.wifi_ssid, "Home")

    def test_hostname_normalised(self):
        s, errors = pptxt.validate(pptxt.parse("hostname=PixelPlus-Garage.local\n"), zoneinfo_dir=None)
        self.assertEqual(errors, [])
        self.assertEqual(s.hostname, "pixelplus-garage")

    def test_static_ip(self):
        s, errors = pptxt.validate(
            pptxt.parse("ip_address=192.168.1.50\nip_gateway=192.168.1.1\nip_dns=1.1.1.1, 8.8.8.8\n"), zoneinfo_dir=None
        )
        self.assertEqual(errors, [])
        self.assertEqual(s.ip_address, "192.168.1.50/24")
        self.assertEqual(s.ip_dns, ["1.1.1.1", "8.8.8.8"])
        self.assertEqual(s.effective_ip_interface(), "ethernet")
        s2, _ = pptxt.validate(pptxt.parse("wifi_ssid=x\nip_address=dhcp\n"), zoneinfo_dir=None)
        self.assertEqual(s2.ip_address, "dhcp")
        self.assertEqual(s2.effective_ip_interface(), "wifi")

    def test_hotspot_open(self):
        s, errors = pptxt.validate(pptxt.parse("hotspot_password=none\nhotspot=off\n"), zoneinfo_dir=None)
        self.assertEqual(errors, [])
        self.assertIsNone(s.hotspot_password)
        self.assertFalse(s.hotspot)

    def test_hex_psk_and_ssid_limits(self):
        s, errors = pptxt.validate(pptxt.parse("wifi_ssid=A\nwifi_password=" + "ab" * 32 + "\n"), zoneinfo_dir=None)
        self.assertEqual(errors, [])
        _, errors = pptxt.validate(pptxt.parse("wifi_ssid=" + "x" * 33 + "\n"), zoneinfo_dir=None)
        self.assertEqual(len(errors), 1)

    def test_password_without_ssid(self):
        _, errors = pptxt.validate(pptxt.parse("wifi_password=longenough\n"), zoneinfo_dir=None)
        self.assertTrue(any("wifi_ssid is empty" in e for e in errors))

    def test_ssh_key(self):
        key = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIB0x me@laptop"
        s, errors = pptxt.validate(pptxt.parse("ssh=on\nssh_key=" + key + "\n"), zoneinfo_dir=None)
        self.assertEqual(errors, [])
        self.assertEqual(s.ssh_key, key)
        self.assertTrue(s.ssh)

    def test_timezone_checked_against_zoneinfo(self):
        zi = "/usr/share/zoneinfo"
        if not os.path.isfile(os.path.join(zi, "America/Chicago")):
            self.skipTest("no zoneinfo")
        _, errors = pptxt.validate(pptxt.parse("timezone=America/Chicago\n"), zoneinfo_dir=zi)
        self.assertEqual(errors, [])
        _, errors = pptxt.validate(pptxt.parse("timezone=Mars/Olympus\n"), zoneinfo_dir=zi)
        self.assertEqual(len(errors), 1)


class UiPasswordTests(unittest.TestCase):
    def test_ui_password_needs_six_characters_like_the_daemon(self):
        s, errors = pptxt.validate(pptxt.parse("ui_password=abcde\n"), zoneinfo_dir=None)
        self.assertTrue(any("ui_password" in e for e in errors), errors)
        self.assertIsNone(s.ui_password)
        s, errors = pptxt.validate(pptxt.parse("ui_password=abcdef\n"), zoneinfo_dir=None)
        self.assertEqual((s.ui_password, errors), ("abcdef", []))


class ScrubTests(unittest.TestCase):
    NOW = dt.datetime(2026, 10, 1, 18, 22)

    def test_scrub_keeps_everything_else(self):
        text = "# hi\nwifi_ssid=Home\nwifi_password=hunter22!\nui_password=abcdef\nhostname=pp\n"
        pr = pptxt.parse(text)
        out = pptxt.scrub_secrets(pr, ["wifi_password"], now=self.NOW)
        self.assertEqual(
            out,
            "# hi\nwifi_ssid=Home\n# [applied 2026-10-01 18:22] Saved on the device and removed from this file. "
            "Type a new one to change it.\nwifi_password=\nui_password=abcdef\nhostname=pp\n",
        )
        # re-parsing: no warnings, password gone, ssid kept
        pr2 = pptxt.parse(out)
        self.assertEqual(pr2.warnings, [])
        self.assertEqual(pr2.get("wifi_password"), "")
        self.assertEqual(pr2.get("wifi_ssid"), "Home")

    def test_scrub_is_idempotent_and_markers_do_not_stack(self):
        text = "wifi_ssid=Home\r\nwifi_password=hunter22!\r\n"
        once = pptxt.scrub_secrets(pptxt.parse(text), ["wifi_password"], now=self.NOW)
        twice = pptxt.scrub_secrets(pptxt.parse(once), [], now=self.NOW)
        self.assertEqual(once, twice)
        self.assertEqual(once.count("[applied"), 1)
        self.assertIn("\r\n", once)
        # a new password later: old marker replaced by a new one
        again = once.replace("wifi_password=\r\n", "wifi_password=newpassword\r\n")
        third = pptxt.scrub_secrets(pptxt.parse(again), ["wifi_password"], now=self.NOW)
        self.assertEqual(third.count("[applied"), 1)

    def test_scrub_ignores_non_secrets(self):
        pr = pptxt.parse("wifi_ssid=Home\n")
        self.assertEqual(pptxt.scrub_secrets(pr, ["wifi_ssid"], now=self.NOW), "wifi_ssid=Home\n")

    def test_quote_roundtrip(self):
        for v in ["plain", " lead", "trail ", '"quoted"', 'a"b', "back\\slash "]:
            pr = pptxt.parse("wifi_ssid=" + pptxt.quote_if_needed(v) + "\n")
            self.assertEqual(pr.get("wifi_ssid"), v, v)


class ImagerGoldenTests(unittest.TestCase):
    """The PixelPlus Imager (imager/core, Rust) renders pixelplus.txt; its golden output
    lives in fixtures/ and is asserted byte-for-byte by the Rust tests too."""

    def test_imager_output_parses(self):
        pr = pptxt.parse(pptxt.read_file(FIXTURE))
        self.assertEqual(pr.warnings, [])
        s, errors = pptxt.validate(pr, countries={"US"}, zoneinfo_dir=None)
        self.assertEqual(errors, [])
        self.assertEqual(s.wifi_ssid, "Chandler Home")
        self.assertEqual(s.wifi_password, "  pa ss#word  ")
        self.assertEqual(s.wifi_country, "US")
        self.assertEqual(s.hostname, "pixelplus-garage")
        self.assertEqual(s.role, "follower")
        self.assertEqual(s.timezone, "America/Chicago")
        self.assertEqual(s.ui_password, "letmein1")
        self.assertTrue(s.ssh)
        self.assertEqual(s.ssh_password, "sshpassword")
        self.assertTrue(s.hotspot)


if __name__ == "__main__":
    unittest.main()
