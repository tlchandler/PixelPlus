#!/usr/bin/env python3
"""PixelPlus network watchdog + setup hotspot (captive portal).

Runs as root from ``pixelplus-netwatch.service``. Standalone on purpose: it must
work before ``pixelplusd`` is configured (or even if it crashes).

State machine::

    WAITING --(online)--> ONLINE --(offline > after_disconnect s)--> HOTSPOT
       |                                                            ^  |
       +--(no network after hotspot_timeout s, no Ethernet)---------+  |
                                                                       |
    HOTSPOT --(user picks Wi-Fi in portal)--> CONNECTING --ok--> ONLINE |
            <-----------------------------------fail-------------------+
    HOTSPOT --(Ethernet plugged in, or a known network reappears during the
               periodic retry while no phone is connected)--> ONLINE

Hotspot: SSID ``PixelPlus-XXXX`` (XXXX = last 4 hex digits of the Wi-Fi MAC),
address 10.42.0.1/24. Password (WPA2):

* first setup (this controller has never been online): ``pixelplus`` by default,
  or ``hotspot_password=`` from pixelplus.txt (``none`` = open);
* once it has been online (the hotspot is then a fallback a neighbour could
  provoke by jamming Wi-Fi): ``hotspot_password=`` if the owner chose one,
  otherwise a random per-device password, created once and written to
  ``PIXELPLUS-HOTSPOT.txt`` on the SD card's boot partition (and shown in
  PixelPlus under Settings -> Network). Never open.

Runtime fallback only after the network has been gone for
``hotspotAfterDisconnect`` s, and at least 10 minutes when a network was
connected in the last 30 minutes (a deauthentication attack has to last that
long; a real outage just waits a little longer).

NetworkManager "shared" mode (its dnsmasq NetworkManager "shared" mode (its dnsmasq
does DHCP; ``/etc/NetworkManager/dnsmasq-shared.d/pixelplus-portal.conf`` answers every
DNS name with 10.42.0.1 and advertises the portal URL via DHCP option 114).
TCP port 80 arriving on the hotspot is redirected with nftables to the portal on
:8099 (pixelplusd keeps port 80 everywhere else).

Status for other programs (e.g. pixelplusd's Settings -> Network page) is written
to ``/run/pixelplus/netwatch.json``.
"""

from __future__ import annotations

import argparse
import json
import logging
import os
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
from typing import Dict, List, Optional

HERE = os.path.dirname(os.path.abspath(__file__))
for p in (HERE, os.path.join(HERE, "..", "firstboot")):
    if p not in sys.path:
        sys.path.insert(0, p)

import nmconn  # noqa: E402
import portal  # noqa: E402

LOG = logging.getLogger("pixelplus-netwatch")

CONFIG_PATH = os.environ.get("PIXELPLUS_NETWATCH_CONFIG", "/etc/pixelplus/netwatch.json")
STATUS_PATH = os.environ.get("PIXELPLUS_NETWATCH_STATUS", "/run/pixelplus/netwatch.json")
PORTAL_PORT = int(os.environ.get("PIXELPLUS_PORTAL_PORT", "8099"))
NFT_TABLE = "pixelplus_portal"
# Persistent netwatch state (root only): everOnline, devicePassword.
STATE_PATH = os.environ.get("PIXELPLUS_NETWATCH_STATE", "/var/lib/pixelplus-system/netwatch-state.json")
# Where PIXELPLUS-HOTSPOT.txt goes ("" = /boot/firmware or /boot, whichever exists).
BOOT_DIR = os.environ.get("PIXELPLUS_BOOT_DIR", "")
DEFAULT_PASSWORD = "pixelplus"
# Runtime fallback: at least this long offline when a network was connected recently.
MIN_OFFLINE_AFTER_RECENT = 600
RECENT_ONLINE = 1800
PASSWORD_ALPHABET = "abcdefghijkmnpqrstuvwxyz23456789"

DEFAULTS = {
    "hotspot": True,
    "hotspotPassword": "pixelplus",
    "hotspotTimeout": 75,  # boot: seconds to wait for a known network
    "hotspotNoProfileTimeout": 25,  # boot: when no Wi-Fi is configured at all (Ethernet DHCP grace)
    "hotspotAfterDisconnect": 300,  # runtime: offline this long -> hotspot
    "hotspotRetryInterval": 300,  # in hotspot: try known networks again (only if no phone joined)
    "channel": 6,
}


def load_config() -> Dict:
    cfg = dict(DEFAULTS)
    try:
        with open(CONFIG_PATH, encoding="utf-8") as f:
            cfg.update({k: v for k, v in json.load(f).items() if v is not None or k == "hotspotPassword"})
    except (OSError, ValueError):
        pass
    return cfg


def mac_suffix(iface: str) -> str:
    try:
        with open(f"/sys/class/net/{iface}/address", encoding="ascii") as f:
            return f.read().strip().replace(":", "")[-4:].upper() or "0000"
    except OSError:
        return "0000"


def _gid(name: str) -> Optional[int]:
    try:
        import grp

        return grp.getgrnam(name).gr_gid
    except (ImportError, KeyError):
        return None


def write_json_atomic(path: str, obj: Dict, mode: int = 0o644, group: Optional[str] = None) -> None:
    """Write JSON to ``path`` without following links an unprivileged user could plant:
    /run/pixelplus belongs to the pixelplus user, and we run as root. The file is created
    exclusively under a random name (O_EXCL never follows symlinks), chmod-ed (and chown-ed
    to ``group``) by descriptor, then renamed over the target (rename replaces a symlink,
    never follows it)."""
    d = os.path.dirname(path) or "."
    try:
        os.makedirs(d, exist_ok=True)
        fd, tmp = tempfile.mkstemp(prefix=".netwatch-", suffix=".tmp", dir=d)
    except OSError:
        return
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as f:
            os.fchmod(f.fileno(), mode)
            gid = _gid(group) if group else None
            if gid is not None:
                try:
                    os.fchown(f.fileno(), -1, gid)
                except OSError:
                    pass
            json.dump(obj, f)
        os.replace(tmp, path)
    except OSError:
        try:
            os.unlink(tmp)
        except OSError:
            pass


def load_state() -> Dict:
    try:
        with open(STATE_PATH, encoding="utf-8") as f:
            st = json.load(f)
            return st if isinstance(st, dict) else {}
    except (OSError, ValueError):
        return {}


def new_device_password(n: int = 10) -> str:
    import secrets

    return "".join(secrets.choice(PASSWORD_ALPHABET) for _ in range(n))


def boot_dir() -> Optional[str]:
    if BOOT_DIR:
        return BOOT_DIR
    for d in ("/boot/firmware", "/boot"):
        if os.path.isfile(os.path.join(d, "config.txt")):
            return d
    return None


def write_boot_note(ssid: str, password: str) -> None:
    """PIXELPLUS-HOTSPOT.txt on the boot partition (readable on any PC)."""
    d = boot_dir()
    if not d:
        return
    text = (
        "PixelPlus setup hotspot\r\n"
        "=======================\r\n\r\n"
        "If this controller loses its network for a while, it opens its own Wi-Fi so you\r\n"
        "can connect it again from your phone:\r\n\r\n"
        f"  Network:  {ssid}\r\n"
        f"  Password: {password}\r\n\r\n"
        "(Before it has ever been online the password is 'pixelplus'.)\r\n"
        "Choose your own with hotspot_password= in pixelplus.txt.\r\n"
    )
    try:
        tmp = os.path.join(d, ".PIXELPLUS-HOTSPOT.tmp")
        with open(tmp, "w", encoding="utf-8") as f:
            f.write(text)
        os.replace(tmp, os.path.join(d, "PIXELPLUS-HOTSPOT.txt"))
    except OSError as e:
        LOG.warning("could not write the hotspot note to %s: %s", d, e)


class Netwatch:
    def __init__(self, nm: nmconn.NM, cfg: Dict, clock=time.monotonic, sleep=time.sleep):
        self.nm = nm
        self.cfg = cfg
        self.clock = clock
        self.sleep = sleep
        self.lock = threading.RLock()
        self.state = "WAITING"
        self.state_since = clock()
        self.offline_since: Optional[float] = clock()
        self.boot = True
        self.iface = nm.wifi_device() or "wlan0"
        self.hotspot_ssid = f"PixelPlus-{mac_suffix(self.iface)}"
        self.scan_cache: List[Dict] = []
        self.scan_time = 0.0
        self.last_error: Optional[str] = None
        self.last_joined: Optional[Dict] = None
        self.pending: Optional[Dict] = None
        self.stay = False
        self.last_retry = clock()
        self.server: Optional[portal.PortalServer] = None
        self.hostname = socket.gethostname()
        self.last_online: Optional[float] = None
        self.persist = load_state()
        if not self.persist.get("devicePassword"):
            self.persist["devicePassword"] = new_device_password()
            self.save_state()
        write_boot_note(self.hotspot_ssid, self.persist["devicePassword"])

    def save_state(self) -> None:
        write_json_atomic(STATE_PATH, self.persist, mode=0o600)

    def hotspot_password(self) -> Optional[str]:
        """WPA2 password of the hotspot right now (None = open), see the module docs."""
        configured = self.cfg.get("hotspotPassword") or None
        if not self.persist.get("everOnline"):
            return configured
        if configured and configured != DEFAULT_PASSWORD:
            return configured
        return self.persist["devicePassword"]

    # ----- helpers ---------------------------------------------------------
    def set_state(self, st: str) -> None:
        if st != self.state:
            LOG.info("state %s -> %s", self.state, st)
            self.state = st
            self.state_since = self.clock()
        self.write_status()

    def write_status(self) -> None:
        st = {
            "state": self.state.lower(),
            "hotspotSsid": self.hotspot_ssid if self.state in ("HOTSPOT", "CONNECTING") else None,
            "hotspotSecured": bool(self.hotspot_password()),
            # Settings -> Network shows it to the signed-in owner (file is 0640 root:pixelplus).
            "hotspotPassword": self.hotspot_password(),
            "portalUrl": f"http://{nmconn.HOTSPOT_ADDR}/" if self.state == "HOTSPOT" else None,
            "lastError": self.last_error,
            "lastJoined": self.last_joined,
            "updatedAt": int(time.time()),
        }
        write_json_atomic(STATUS_PATH, st, mode=0o640, group="pixelplus")

    def elapsed(self) -> float:
        return self.clock() - self.state_since

    # ----- hotspot ---------------------------------------------------------
    def start_hotspot(self) -> bool:
        LOG.info("starting setup hotspot %s", self.hotspot_ssid)
        self.nm.radio_on()
        # Scan while still in client mode; most Pi radios cannot scan while acting as an AP.
        self.refresh_scan(rescan=True)
        psk = self.hotspot_password()
        nmconn.write_keyfile(
            nmconn.HOTSPOT_CON,
            nmconn.render_hotspot_keyfile(self.hotspot_ssid, psk, self.iface, int(self.cfg.get("channel", 6))),
        )
        self.nm.reload()
        if not self.nm.up(nmconn.HOTSPOT_CON, wait=30):
            LOG.error("could not start the hotspot")
            return False
        self.install_redirect()
        self.start_portal()
        self.last_retry = self.clock()
        self.set_state("HOTSPOT")
        return True

    def stop_hotspot(self) -> None:
        LOG.info("stopping setup hotspot")
        self.stop_portal()
        self.remove_redirect()
        self.nm.down(nmconn.HOTSPOT_CON)

    def install_redirect(self) -> None:
        rules = (
            f"table ip {NFT_TABLE} {{\n"
            "  chain prerouting {\n"
            "    type nat hook prerouting priority dstnat; policy accept;\n"
            f'    iifname "{self.iface}" tcp dport 80 redirect to :{PORTAL_PORT}\n'
            "  }\n"
            "  chain input {\n"
            "    type filter hook input priority filter; policy accept;\n"
            f'    iifname "{self.iface}" tcp dport 443 reject with tcp reset\n'
            "  }\n"
            "}\n"
        )
        self.remove_redirect()
        try:
            res = subprocess.run(["nft", "-f", "-"], input=rules, text=True, capture_output=True, timeout=10, check=False)
            if res.returncode != 0:
                LOG.warning("nft redirect failed: %s", res.stderr.strip())
        except (FileNotFoundError, OSError) as e:
            LOG.warning("nftables not available (%s); phones may not show the setup page automatically", e)

    def remove_redirect(self) -> None:
        self.nm.ok("nft", "delete", "table", "ip", NFT_TABLE)

    def start_portal(self) -> None:
        if self.server:
            return
        for _ in range(10):
            try:
                self.server = portal.PortalServer((nmconn.HOTSPOT_ADDR, PORTAL_PORT), self)
                break
            except OSError as e:
                LOG.debug("portal bind retry: %s", e)
                self.sleep(1)
        if not self.server:
            LOG.error("could not start the setup page on %s:%s", nmconn.HOTSPOT_ADDR, PORTAL_PORT)
            return
        threading.Thread(target=self.server.serve_forever, name="portal", daemon=True).start()
        LOG.info("setup page listening on %s:%s", nmconn.HOTSPOT_ADDR, PORTAL_PORT)

    def stop_portal(self) -> None:
        if self.server:
            srv, self.server = self.server, None
            threading.Thread(target=srv.shutdown, daemon=True).start()
            srv.server_close()

    def refresh_scan(self, rescan: bool) -> List[Dict]:
        nets = self.nm.scan(rescan=rescan)
        nets = [n for n in nets if n["ssid"] != self.hotspot_ssid]
        if nets or rescan:
            with self.lock:
                self.scan_cache = nets
                self.scan_time = time.time()
        return self.scan_cache

    def stations(self) -> int:
        out = self.nm.out("iw", "dev", self.iface, "station", "dump")
        return sum(1 for ln in out.splitlines() if ln.startswith("Station "))

    # ----- portal callbacks (called from HTTP threads) ---------------------
    def portal_status(self) -> Dict:
        with self.lock:
            return {
                "state": self.state.lower(),
                "hostname": self.hostname,
                "hotspotSsid": self.hotspot_ssid,
                "lastError": self.last_error,
                "scanAgeS": int(time.time() - self.scan_time) if self.scan_time else None,
                "stay": self.stay,
            }

    def portal_scan(self, rescan: bool) -> List[Dict]:
        if rescan:
            # Some radios can scan in AP mode; if not, the cached list is returned.
            fresh = [n for n in self.nm.scan(rescan=True) if n["ssid"] != self.hotspot_ssid]
            if fresh:
                with self.lock:
                    self.scan_cache, self.scan_time = fresh, time.time()
        with self.lock:
            return list(self.scan_cache)

    def portal_connect(self, ssid: str, password: str, hidden: bool, country: Optional[str]) -> Dict:
        with self.lock:
            if self.pending or self.state == "CONNECTING":
                return {"ok": False, "error": "Already connecting - please wait."}
            nets = {n["ssid"]: n for n in self.scan_cache}
            net = nets.get(ssid)
            if net and net.get("enterprise"):
                return {"ok": False, "error": "Enterprise (802.1X) networks are not supported by the setup page."}
            self.pending = {
                "ssid": ssid,
                "password": password,
                "hidden": hidden,
                "country": country,
                "keyMgmt": "sae" if net and net.get("wpa3Only") else "wpa-psk",
                "at": self.clock(),
            }
            self.last_error = None
        return {"ok": True, "hostname": self.hostname, "url": f"http://{self.hostname}.local/"}

    def portal_stay(self) -> Dict:
        """'Use PixelPlus without Wi-Fi': keep the hotspot, give port 80 back to pixelplusd."""
        with self.lock:
            self.stay = True
        self.remove_redirect()
        return {"ok": True, "url": f"http://{nmconn.HOTSPOT_ADDR}/"}

    # ----- connecting ------------------------------------------------------
    def do_connect(self, req: Dict) -> None:
        self.set_state("CONNECTING")
        # Give the phone time to receive the "connecting..." page before the AP drops.
        self.sleep(3)
        ssid = req["ssid"]
        con = "pixelplus-portal-" + "".join(c if c.isalnum() else "-" for c in ssid)[:40]
        self.stop_hotspot()
        if req.get("country") and len(req["country"]) == 2:
            cc = req["country"].upper()
            if not self.nm.ok("raspi-config", "nonint", "do_wifi_country", cc, timeout=30):
                self.nm.ok("iw", "reg", "set", cc)
        nmconn.write_keyfile(
            con,
            nmconn.render_wifi_keyfile(
                con,
                ssid,
                req["password"] or None,
                hidden=bool(req.get("hidden")),
                key_mgmt=req.get("keyMgmt", "wpa-psk"),
                priority=15,
                con_uuid=nmconn.read_keyfile_uuid(con),
            ),
        )
        self.nm.reload()
        LOG.info("joining %r chosen on the setup page", ssid)
        if self.nm.up(con, wait=45):
            dev = self.nm.online()
            ips = self.nm.ip4_of(dev["device"]) if dev else []
            self.last_joined = {"ssid": ssid, "ips": ips, "at": int(time.time())}
            self.last_error = None
            LOG.info("joined %r (%s)", ssid, ", ".join(ips) or "no IPv4 yet")
            self.offline_since = None
            self.set_state("ONLINE")
            return
        LOG.warning("could not join %r", ssid)
        nmconn.remove_keyfile(con)
        self.nm.reload()
        self.last_error = (
            f"Couldn't join “{ssid}”. Check the password and that the network is in range, then try again."
        )
        self.start_hotspot()

    # ----- main loop -------------------------------------------------------
    def tick(self) -> None:
        with self.lock:
            pending, self.pending = self.pending, None
        if pending:
            self.do_connect(pending)
            return

        if self.state in ("WAITING", "ONLINE"):
            online = self.nm.online()
            now = self.clock()
            if online:
                if self.state != "ONLINE" or self.offline_since is not None:
                    LOG.info("online via %s (%s)", online["device"], online["connection"])
                self.offline_since = None
                self.last_online = now
                if not self.persist.get("everOnline"):
                    self.persist["everOnline"] = True
                    self.save_state()
                self.boot = False
                self.set_state("ONLINE")
                return
            if self.offline_since is None:
                self.offline_since = now
                LOG.info("network lost")
            if not self.cfg.get("hotspot", True):
                return
            if self.boot:
                has_profiles = bool(self.nm.known_wifi())
                limit = self.cfg["hotspotTimeout"] if has_profiles else min(
                    self.cfg["hotspotNoProfileTimeout"], self.cfg["hotspotTimeout"]
                )
            else:
                limit = self.cfg["hotspotAfterDisconnect"]
                if self.last_online is not None and now - self.last_online < RECENT_ONLINE:
                    # Deauth resistance: a jammer has to keep at it for 10 minutes.
                    limit = max(limit, MIN_OFFLINE_AFTER_RECENT)
            if now - self.offline_since >= limit:
                if self.nm.ethernet_connected():
                    return
                self.boot = False
                if not self.start_hotspot():
                    # Try again later rather than spinning.
                    self.offline_since = now
            return

        if self.state == "HOTSPOT":
            if self.nm.ethernet_connected():
                LOG.info("Ethernet connected - leaving hotspot mode")
                self.stop_hotspot()
                self.set_state("ONLINE")
                return
            if not self.nm.hotspot_active() and self.elapsed() > 20:
                LOG.warning("hotspot went down unexpectedly; restarting it")
                self.stop_hotspot()
                self.start_hotspot()
                return
            if (
                self.nm.known_wifi()
                and self.clock() - self.last_retry >= self.cfg["hotspotRetryInterval"]
                and self.stations() == 0
            ):
                self.retry_known()

    def retry_known(self) -> None:
        LOG.info("no phone connected - checking whether a known network is back")
        self.last_retry = self.clock()
        self.stop_hotspot()
        deadline = self.clock() + 45
        self.refresh_scan(rescan=True)
        while self.clock() < deadline:
            if self.nm.online():
                LOG.info("known network is back")
                self.offline_since = None
                self.set_state("ONLINE")
                return
            self.sleep(3)
        self.start_hotspot()

    def shutdown(self) -> None:
        if self.state in ("HOTSPOT", "CONNECTING"):
            self.stop_hotspot()
        self.remove_redirect()

    def run(self) -> None:
        self.write_status()
        if not self.nm.wifi_device():
            LOG.info("no Wi-Fi hardware; hotspot fallback disabled (monitoring only)")
            self.cfg["hotspot"] = False
        # Clean up leftovers from an unclean stop.
        self.remove_redirect()
        if self.nm.hotspot_active():
            self.nm.down(nmconn.HOTSPOT_CON)
        while True:
            try:
                self.tick()
            except Exception:  # never die: this is the user's way back in
                LOG.exception("netwatch tick failed")
            self.sleep(3)


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(prog="pixelplus-netwatch")
    ap.add_argument("-v", "--verbose", action="store_true")
    ap.add_argument("--portal-only", metavar="ADDR:PORT", help="development: serve the setup page with fake data")
    args = ap.parse_args(argv)
    logging.basicConfig(level=logging.DEBUG if args.verbose else logging.INFO, format="%(levelname)s %(message)s")

    if args.portal_only:
        host, _, port = args.portal_only.rpartition(":")
        return portal.serve_demo(host or "127.0.0.1", int(port))

    nw = Netwatch(nmconn.NM(), load_config())

    def _term(signum, frame):
        LOG.info("stopping")
        nw.shutdown()
        sys.exit(0)

    signal.signal(signal.SIGTERM, _term)
    signal.signal(signal.SIGINT, _term)
    nw.run()
    return 0


if __name__ == "__main__":
    sys.exit(main())
