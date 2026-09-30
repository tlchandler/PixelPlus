import configparser
import os
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "firstboot"))

import nmconn  # noqa: E402


def ini(text):
    cp = configparser.ConfigParser(interpolation=None, strict=True)
    cp.optionxform = str
    cp.read_string(text)
    return cp


class KeyfileTests(unittest.TestCase):
    def test_wifi_psk(self):
        k = ini(nmconn.render_wifi_keyfile("pixelplus-wifi", "Home", "hunter22", priority=20, con_uuid="u-1"))
        self.assertEqual(k["connection"]["id"], "pixelplus-wifi")
        self.assertEqual(k["connection"]["uuid"], "u-1")
        self.assertEqual(k["connection"]["autoconnect-priority"], "20")
        self.assertEqual(k["wifi"]["ssid"], "Home")
        self.assertEqual(k["wifi"]["powersave"], "2")
        self.assertEqual(k["wifi-security"]["key-mgmt"], "wpa-psk")
        self.assertEqual(k["wifi-security"]["psk"], "hunter22")
        self.assertEqual(k["ipv4"]["method"], "auto")

    def test_open_hidden_static(self):
        k = ini(
            nmconn.render_wifi_keyfile(
                "x", "Cafe", None, hidden=True, ipv4_address="192.168.1.50/24", ipv4_gateway="192.168.1.1",
                ipv4_dns=["1.1.1.1", "8.8.8.8"],
            )
        )
        self.assertNotIn("wifi-security", k)
        self.assertEqual(k["wifi"]["hidden"], "true")
        self.assertEqual(k["ipv4"]["method"], "manual")
        self.assertEqual(k["ipv4"]["address1"], "192.168.1.50/24,192.168.1.1")
        self.assertEqual(k["ipv4"]["dns"], "1.1.1.1;8.8.8.8;")

    def test_awkward_ssid_uses_bytes(self):
        v = nmconn.ssid_value("a;b")
        self.assertEqual(v, "97;59;98;")
        self.assertEqual(nmconn.ssid_value(" lead"), "32;108;101;97;100;")
        self.assertEqual(nmconn.ssid_value("Café ✨"), "Café ✨")

    def test_escape_roundtrip(self):
        for s in ["plain", " lead", "back\\slash", "trail ", "tab\tin"]:
            self.assertEqual(nmconn.gkey_unescape(nmconn._gkey_escape(s)), s)

    def test_hotspot(self):
        k = ini(nmconn.render_hotspot_keyfile("PixelPlus-3F2A", "pixelplus"))
        self.assertEqual(k["wifi"]["mode"], "ap")
        self.assertEqual(k["wifi"]["band"], "bg")
        self.assertEqual(k["connection"]["autoconnect"], "false")
        self.assertEqual(k["ipv4"]["method"], "shared")
        self.assertEqual(k["ipv4"]["address1"], "10.42.0.1/24")
        self.assertEqual(k["wifi-security"]["psk"], "pixelplus")
        k2 = ini(nmconn.render_hotspot_keyfile("PixelPlus-3F2A", None))
        self.assertNotIn("wifi-security", k2)

    def test_ethernet(self):
        k = ini(nmconn.render_ethernet_keyfile("pixelplus-ethernet", "10.0.0.5/8", "10.0.0.1"))
        self.assertEqual(k["connection"]["interface-name"], "eth0")
        self.assertEqual(k["ipv4"]["address1"], "10.0.0.5/8,10.0.0.1")

    def test_write_is_private(self):
        with tempfile.TemporaryDirectory() as d:
            p = nmconn.write_keyfile("pixelplus wifi/../x", "[connection]\nuuid=abc\n", nm_dir=d)
            self.assertEqual(os.path.dirname(p), d)
            self.assertEqual(os.stat(p).st_mode & 0o777, 0o600)
            self.assertEqual(nmconn.read_keyfile_uuid("pixelplus wifi/../x", nm_dir=d), "abc")
            self.assertTrue(nmconn.remove_keyfile("pixelplus wifi/../x", nm_dir=d))


class NmcliParsingTests(unittest.TestCase):
    def fake(self, outputs):
        def run(argv, timeout=None):
            key = " ".join(argv)
            for k, v in outputs.items():
                if key.startswith(k):
                    return subprocess.CompletedProcess(argv, 0, v, "")
            return subprocess.CompletedProcess(argv, 1, "", "")

        return nmconn.NM(run)

    def test_split_terse(self):
        self.assertEqual(nmconn.split_terse(r"My\:Net:80:WPA2"), ["My:Net", "80", "WPA2"])

    def test_scan_dedup_and_sort(self):
        nm = self.fake({
            "nmcli -t -f SSID,SIGNAL,SECURITY,CHAN device wifi list": (
                "Home:40:WPA2:1\nHome:70:WPA2:36\n:90:WPA2:6\nCafe:55::11\nW3:30:WPA3:1\nCorp:20:WPA2 802.1X:1\n"
            )
        })
        nets = nm.scan()
        self.assertEqual([n["ssid"] for n in nets], ["Home", "Cafe", "W3", "Corp"])
        self.assertEqual(nets[0]["signal"], 70)
        self.assertFalse(nets[1]["secure"])
        self.assertTrue(nets[2]["wpa3Only"])
        self.assertTrue(nets[3]["enterprise"])

    def test_online_and_known(self):
        nm = self.fake({
            "nmcli -t -f DEVICE,TYPE,STATE,CONNECTION device status": (
                "eth0:ethernet:unavailable:\nwlan0:wifi:connected:pixelplus-hotspot\nlo:loopback:connected (externally):lo\n"
            ),
            "nmcli -t -f NAME,UUID,TYPE,AUTOCONNECT connection show": (
                "pixelplus-hotspot:u1:802-11-wireless:no\npixelplus-wifi:u2:802-11-wireless:yes\n"
                "netplan-wlan0-Home:u3:802-11-wireless:yes\nWired connection 1:u4:802-3-ethernet:yes\n"
            ),
        })
        self.assertIsNone(nm.online())
        self.assertTrue(nm.hotspot_active())
        self.assertFalse(nm.ethernet_connected())
        self.assertEqual(nm.known_wifi(), ["pixelplus-wifi", "netplan-wlan0-Home"])
        self.assertEqual(nm.wifi_device(), "wlan0")


if __name__ == "__main__":
    unittest.main()


class PowerSaveTests(unittest.TestCase):
    def test_parse(self):
        self.assertIs(nmconn.parse_power_save("Power save: on\n"), True)
        self.assertIs(nmconn.parse_power_save("\tPower save: off"), False)
        self.assertIsNone(nmconn.parse_power_save("command failed: No such device (-19)"))
        self.assertIsNone(nmconn.parse_power_save(""))

    def test_turned_off_when_on(self):
        state = {"ps": "on"}
        calls = []

        def run(argv, timeout=30):
            calls.append(list(argv))
            if argv[-1] == "power_save":
                return subprocess.CompletedProcess(argv, 0, f"Power save: {state['ps']}\n", "")
            if argv[-2:] == ("power_save", "off") or list(argv[-2:]) == ["power_save", "off"]:
                state["ps"] = "off"
            return subprocess.CompletedProcess(argv, 0, "", "")

        nm = nmconn.NM(run)
        self.assertIs(nm.power_save_off("wlan0"), False)
        self.assertIn(["iw", "dev", "wlan0", "set", "power_save", "off"], calls)
        calls.clear()
        self.assertIs(nm.power_save_off("wlan0"), False)
        self.assertNotIn(["iw", "dev", "wlan0", "set", "power_save", "off"], calls, "already off")
