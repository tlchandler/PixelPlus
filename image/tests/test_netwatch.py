import http.client
import json
import os
import sys
import tempfile
import threading
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "firstboot"))
sys.path.insert(0, os.path.join(HERE, "..", "netwatch"))

import nmconn  # noqa: E402
import portal  # noqa: E402

os.environ.setdefault("PIXELPLUS_NETWATCH_STATUS", os.path.join(tempfile.gettempdir(), "pp-netwatch-test.json"))
import netwatch  # noqa: E402


class PortalTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.ctl = portal.DemoController()
        cls.srv = portal.PortalServer(("127.0.0.1", 0), cls.ctl, public_host="10.42.0.1")
        cls.port = cls.srv.server_address[1]
        threading.Thread(target=cls.srv.serve_forever, daemon=True).start()

    @classmethod
    def tearDownClass(cls):
        cls.srv.shutdown()
        cls.srv.server_close()

    def req(self, method, path, host="10.42.0.1", body=None):
        c = http.client.HTTPConnection("127.0.0.1", self.port, timeout=5)
        headers = {"Host": host}
        data = None
        if body is not None:
            data = json.dumps(body).encode()
            headers["Content-Type"] = "application/json"
        c.request(method, path, body=data, headers=headers)
        r = c.getresponse()
        return r.status, dict(r.getheaders()), r.read()

    def test_page(self):
        st, h, body = self.req("GET", "/")
        self.assertEqual(st, 200)
        self.assertIn(b"Connect PixelPlus to Wi-Fi", body)
        self.assertIn("no-store", h["Cache-Control"])

    def test_captive_probes_redirect(self):
        for host, path in [
            ("captive.apple.com", "/hotspot-detect.html"),
            ("connectivitycheck.gstatic.com", "/generate_204"),
            ("www.msftconnecttest.com", "/connecttest.txt"),
            ("detectportal.firefox.com", "/canonical.html"),
            ("10.42.0.1", "/generate_204"),
        ]:
            st, h, _ = self.req("GET", path, host=host)
            self.assertEqual(st, 302, (host, path))
            self.assertEqual(h["Location"], "http://10.42.0.1/")

    def test_status_and_scan(self):
        st, _, body = self.req("GET", "/api/status")
        self.assertEqual(st, 200)
        j = json.loads(body)
        self.assertEqual(j["hotspotSsid"], "PixelPlus-3F2A")
        self.assertTrue(len(j["countries"]) > 5)
        st, _, body = self.req("GET", "/api/scan?rescan=1")
        self.assertEqual(json.loads(body)[0]["ssid"], "Chandler Home")

    def test_connect_validation(self):
        st, _, body = self.req("POST", "/api/connect", body={"ssid": "", "password": ""})
        self.assertEqual(st, 400)
        st, _, body = self.req("POST", "/api/connect", body={"ssid": "Home", "password": "short"})
        self.assertEqual(st, 400)
        st, _, body = self.req("POST", "/api/connect", body={"ssid": "Home", "password": "longenough", "country": "us"})
        self.assertEqual(st, 200)
        self.assertEqual(json.loads(body)["url"], "http://pixelplus.local/")
        self.assertEqual(self.ctl.connected[-1]["country"], "us")
        st, _, _ = self.req("POST", "/api/connect", body=["not", "a", "dict"])
        self.assertEqual(st, 400)

    def test_slow_clients_time_out(self):
        self.assertEqual(portal.Handler.timeout, 10)
        self.assertGreater(portal.PortalServer.max_clients, 0)

    def test_foreign_host_post_redirects(self):
        st, _, _ = self.req("POST", "/api/connect", host="evil.example", body={"ssid": "x"})
        self.assertEqual(st, 302)


class FakeNM:
    """Scriptable NetworkManager double for the state machine."""

    def __init__(self):
        self.online_dev = None
        self.eth = False
        self.hotspot = False
        self.profiles = []
        self.up_ok = True
        self.calls = []
        self.stations_n = 0
        self.ps = True

    def wifi_device(self):
        return "wlan0"

    def online(self, ignore=()):
        return self.online_dev

    def ethernet_connected(self):
        return self.eth

    def hotspot_active(self):
        return self.hotspot

    def known_wifi(self):
        return list(self.profiles)

    def radio_on(self):
        pass

    def scan(self, rescan=True):
        return [{"ssid": "Home", "signal": 70, "secure": True, "wpa3Only": False, "enterprise": False}]

    def reload(self):
        self.calls.append("reload")
        return True

    def up(self, con, wait=45):
        self.calls.append(f"up {con}")
        if con == nmconn.HOTSPOT_CON:
            self.hotspot = True
            return True
        if self.up_ok:
            self.online_dev = {"device": "wlan0", "connection": con}
        return self.up_ok

    def down(self, con):
        self.calls.append(f"down {con}")
        if con == nmconn.HOTSPOT_CON:
            self.hotspot = False
        return True

    def ok(self, *argv, timeout=30):
        return True

    def out(self, *argv, timeout=30):
        if argv[:2] == ("iw", "dev"):
            return "Station aa:bb\n" * self.stations_n
        return ""

    def ip4_of(self, dev):
        return ["192.168.1.77"]

    def power_save(self, iface):
        return self.ps

    def power_save_off(self, iface):
        self.calls.append(f"power_save off {iface}")
        self.ps = False
        return self.ps


class Clock:
    def __init__(self):
        self.t = 1000.0

    def __call__(self):
        return self.t

    def sleep(self, s):
        self.t += s


class StateMachineTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp()
        self.saved_dir = nmconn.NM_DIR
        nmconn.NM_DIR = self.tmp
        self.nm = FakeNM()
        self.clock = Clock()
        self.saved_paths = (netwatch.STATE_PATH, netwatch.BOOT_DIR, netwatch.STATUS_PATH, netwatch.NODE_JSON)
        netwatch.STATE_PATH = os.path.join(self.tmp, "state.json")
        netwatch.NODE_JSON = os.path.join(self.tmp, "node.json")
        netwatch.BOOT_DIR = self.tmp
        netwatch.STATUS_PATH = os.path.join(self.tmp, "netwatch.json")
        self.nw = netwatch.Netwatch(self.nm, dict(netwatch.DEFAULTS), clock=self.clock, sleep=self.clock.sleep)
        # no real sockets / nftables in unit tests
        self.nw.start_portal = lambda: None
        self.nw.stop_portal = lambda: None
        self.nw.install_redirect = lambda: None
        self.nw.remove_redirect = lambda: None

    def tearDown(self):
        nmconn.NM_DIR = self.saved_dir
        netwatch.STATE_PATH, netwatch.BOOT_DIR, netwatch.STATUS_PATH, netwatch.NODE_JSON = self.saved_paths

    def hotspot_psk(self):
        kf = open(os.path.join(self.tmp, "pixelplus-hotspot.nmconnection")).read()
        return [ln.split("=", 1)[1] for ln in kf.splitlines() if ln.startswith("psk=")]

    def advance(self, seconds, step=3):
        end = self.clock.t + seconds
        while self.clock.t < end:
            self.nw.tick()
            self.clock.sleep(step)

    def test_wifi_power_save_is_turned_off_when_online(self):
        self.nm.online_dev = {"device": "wlan0", "connection": "pixelplus-wifi"}
        self.nw.tick()
        self.assertIn("power_save off wlan0", self.nm.calls)
        self.assertFalse(self.nm.ps)
        # Re-checked only every few minutes; it came back on meanwhile.
        self.nm.ps = True
        self.nm.calls.clear()
        self.advance(60)
        self.assertNotIn("power_save off wlan0", self.nm.calls)
        self.advance(netwatch.POWER_SAVE_EVERY)
        self.assertIn("power_save off wlan0", self.nm.calls)
        # Ethernet: not touched.
        self.nm.calls.clear()
        self.nm.online_dev = {"device": "eth0", "connection": "Wired"}
        self.advance(netwatch.POWER_SAVE_EVERY + 10)
        self.assertNotIn("power_save off wlan0", self.nm.calls)

    def test_online_at_boot(self):
        self.nm.online_dev = {"device": "wlan0", "connection": "pixelplus-wifi"}
        self.nw.tick()
        self.assertEqual(self.nw.state, "ONLINE")

    def test_hotspot_after_timeout_with_profiles(self):
        self.nm.profiles = ["pixelplus-wifi"]
        self.advance(60)
        self.assertEqual(self.nw.state, "WAITING")
        self.advance(30)
        self.assertEqual(self.nw.state, "HOTSPOT")
        self.assertTrue(os.path.exists(os.path.join(self.tmp, "pixelplus-hotspot.nmconnection")))

    def test_hotspot_fast_without_profiles(self):
        self.advance(30)
        self.assertEqual(self.nw.state, "HOTSPOT")

    def test_no_hotspot_with_ethernet(self):
        self.nm.eth = True  # carrier but (say) DHCP slow: never hotspot while Ethernet is connected
        self.advance(200)
        self.assertEqual(self.nw.state, "WAITING")

    def test_hotspot_disabled(self):
        self.nw.cfg["hotspot"] = False
        self.advance(200)
        self.assertEqual(self.nw.state, "WAITING")

    def test_portal_connect_success(self):
        self.advance(30)
        self.assertEqual(self.nw.state, "HOTSPOT")
        res = self.nw.portal_connect("Home", "hunter22!", False, "US")
        self.assertTrue(res["ok"])
        self.nw.tick()
        self.assertEqual(self.nw.state, "ONLINE")
        self.assertFalse(self.nm.hotspot)
        self.assertEqual(self.nw.last_joined["ips"], ["192.168.1.77"])
        kf = open(os.path.join(self.tmp, "pixelplus-portal-Home.nmconnection")).read()
        self.assertIn("psk=hunter22!", kf)

    def test_portal_connect_failure_returns_to_hotspot(self):
        self.advance(30)
        self.nm.up_ok = False
        self.nw.portal_connect("Home", "wrongpass", False, None)
        self.nw.tick()
        self.assertEqual(self.nw.state, "HOTSPOT")
        self.assertIn("Couldn't join", self.nw.last_error)
        self.assertFalse(os.path.exists(os.path.join(self.tmp, "pixelplus-portal-Home.nmconnection")))

    def test_ethernet_ends_hotspot(self):
        self.advance(30)
        self.nm.eth = True
        self.nm.online_dev = {"device": "eth0", "connection": "Wired connection 1"}
        self.nw.tick()
        self.assertEqual(self.nw.state, "ONLINE")
        self.assertFalse(self.nm.hotspot)

    def test_retry_known_network(self):
        self.nm.profiles = ["pixelplus-wifi"]
        self.advance(90)
        self.assertEqual(self.nw.state, "HOTSPOT")
        # a phone is connected: never interrupt it
        self.nm.stations_n = 1
        self.advance(400)
        self.assertEqual(self.nw.state, "HOTSPOT")
        # nobody connected, router came back
        self.nm.stations_n = 0

        orig_down = self.nm.down

        def down(con):
            r = orig_down(con)
            self.nm.online_dev = {"device": "wlan0", "connection": "pixelplus-wifi"}
            return r

        self.nm.down = down
        self.advance(400)
        self.assertEqual(self.nw.state, "ONLINE")

    def test_first_setup_hotspot_uses_the_documented_password(self):
        self.advance(30)
        self.assertEqual(self.nw.state, "HOTSPOT")
        self.assertEqual(self.hotspot_psk(), ["pixelplus"])
        note = open(os.path.join(self.tmp, "PIXELPLUS-HOTSPOT.txt")).read()
        self.assertIn(self.nw.hotspot_ssid, note)
        self.assertIn(self.nw.persist["devicePassword"], note)

    def test_runtime_disconnect(self):
        self.nm.online_dev = {"device": "wlan0", "connection": "pixelplus-wifi"}
        self.nw.tick()
        self.nm.profiles = ["pixelplus-wifi"]
        self.nm.online_dev = None
        self.nm.up_ok = False  # the router stays away
        self.advance(620)
        # It has been online: a passing outage never opens the hotspot (30 min).
        self.assertEqual(self.nw.state, "ONLINE")
        self.advance(1170)
        self.assertEqual(self.nw.state, "ONLINE")
        self.advance(30)
        self.assertEqual(self.nw.state, "HOTSPOT")
        # A configured controller never opens the published default password.
        pw = self.nw.persist["devicePassword"]
        self.assertEqual(len(pw), 10)
        self.assertEqual(self.hotspot_psk(), [pw])
        st = json.load(open(netwatch.STATUS_PATH))
        self.assertEqual(st["hotspotPassword"], pw)
        self.assertEqual(os.stat(netwatch.STATUS_PATH).st_mode & 0o777, 0o640)
        # Remembered across restarts.
        again = netwatch.Netwatch(self.nm, dict(netwatch.DEFAULTS), clock=self.clock, sleep=self.clock.sleep)
        self.assertEqual(again.hotspot_password(), pw)

    def ups(self):
        return [c for c in self.nm.calls if c == "up pixelplus-wifi"]

    def test_established_controller_waits_30_minutes_and_keeps_rejoining(self):
        self.nw.persist["everOnline"] = True  # online before this boot
        self.nm.profiles = ["pixelplus-wifi"]
        self.nm.up_ok = False
        self.advance(600)
        # Boot: a known network is retried every 20 s for the first 10 minutes ...
        self.assertEqual(self.nw.state, "WAITING")
        self.assertTrue(28 <= len(self.ups()) <= 31, len(self.ups()))
        self.nm.calls.clear()
        self.advance(600)
        # ... then every 60 s.
        self.assertTrue(9 <= len(self.ups()) <= 11, len(self.ups()))
        self.advance(590)
        self.assertEqual(self.nw.state, "WAITING")
        self.advance(15)
        self.assertEqual(self.nw.state, "HOTSPOT")

    def test_established_controller_rejoins_when_the_router_is_back(self):
        self.nw.persist["everOnline"] = True
        self.nm.profiles = ["pixelplus-wifi"]
        self.nm.up_ok = False
        self.advance(300)
        self.nm.up_ok = True  # router finished rebooting
        self.advance(30)
        self.assertEqual(self.nw.state, "ONLINE")
        self.assertEqual(self.nm.online_dev["connection"], "pixelplus-wifi")

    def test_offline_timer_restarts_after_a_brief_reconnect(self):
        self.nw.persist["everOnline"] = True
        self.nm.up_ok = False
        self.advance(1500)
        self.nm.online_dev = {"device": "wlan0", "connection": "pixelplus-wifi"}
        self.nw.tick()
        self.nm.online_dev = None
        self.advance(1500)
        self.assertEqual(self.nw.state, "ONLINE")  # 25 + 25 min, never 30 in a row
        self.advance(330)
        self.assertEqual(self.nw.state, "HOTSPOT")

    def test_adopted_follower_counts_as_established(self):
        with open(netwatch.NODE_JSON, "w") as f:
            json.dump({"id": "n1", "role": "follower", "leaderId": "L", "leaderUrl": "http://10.0.0.2"}, f)
        self.assertTrue(self.nw.established())
        self.advance(900)
        self.assertEqual(self.nw.state, "WAITING")
        with open(netwatch.NODE_JSON, "w") as f:
            json.dump({"id": "n1", "role": "unconfigured"}, f)
        self.assertFalse(self.nw.established())
        self.assertFalse(netwatch.adopted_follower(os.path.join(self.tmp, "missing.json")))

    def test_never_online_controller_keeps_the_quick_setup_hotspot(self):
        self.assertFalse(self.nw.established())
        self.nm.profiles = ["pixelplus-wifi"]
        self.advance(90)
        self.assertEqual(self.nw.state, "HOTSPOT")

    def test_owner_chosen_or_open_password_after_being_online(self):
        self.nw.persist["everOnline"] = True
        self.nw.cfg["hotspotPassword"] = "my own pass"
        self.assertEqual(self.nw.hotspot_password(), "my own pass")
        self.nw.cfg["hotspotPassword"] = None  # "none" in pixelplus.txt: open only for first setup
        self.assertEqual(self.nw.hotspot_password(), self.nw.persist["devicePassword"])


class StatusFileTests(unittest.TestCase):
    def test_status_write_does_not_follow_planted_symlinks(self):
        d = tempfile.mkdtemp()
        victim = os.path.join(d, "victim")
        with open(victim, "w") as f:
            f.write("precious\n")
        os.chmod(victim, 0o600)
        target = os.path.join(d, "netwatch.json")
        os.symlink(victim, target)
        os.symlink(victim, target + ".tmp")
        netwatch.write_json_atomic(target, {"state": "hotspot"})
        with open(victim) as f:
            self.assertEqual(f.read(), "precious\n")
        self.assertEqual(os.stat(victim).st_mode & 0o777, 0o600)
        self.assertFalse(os.path.islink(target))
        with open(target) as f:
            self.assertEqual(json.load(f), {"state": "hotspot"})
        self.assertEqual(os.stat(target).st_mode & 0o777, 0o644)
        leftovers = [n for n in os.listdir(d) if n.startswith(".netwatch-")]
        self.assertEqual(leftovers, [])


if __name__ == "__main__":
    unittest.main()
