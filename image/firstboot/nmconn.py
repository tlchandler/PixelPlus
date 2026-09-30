"""NetworkManager helpers shared by firstboot and netwatch.

Wi-Fi profiles are written as NetworkManager *keyfiles* (0600, root) instead of
``nmcli ... password <psk>`` so secrets never appear on a command line (visible in
``ps``) and so the result is deterministic. ``nmcli connection reload`` picks them up.

Profiles owned by PixelPlus are named ``pixelplus-wifi``, ``pixelplus-wifi2``,
``pixelplus-portal-<n>`` (joined via the captive portal), ``pixelplus-ethernet``
(only when a static IP is requested for Ethernet) and ``pixelplus-hotspot``.
"""

from __future__ import annotations

import ipaddress
import os
import subprocess
import uuid
from typing import Callable, Dict, List, Optional, Sequence

NM_DIR = os.environ.get("PIXELPLUS_NM_DIR", "/etc/NetworkManager/system-connections")
HOTSPOT_CON = "pixelplus-hotspot"
HOTSPOT_ADDR = "10.42.0.1"
HOTSPOT_PREFIX = 24

Runner = Callable[[Sequence[str], Optional[int]], "subprocess.CompletedProcess[str]"]


def default_runner(argv: Sequence[str], timeout: Optional[int] = 30) -> "subprocess.CompletedProcess[str]":
    return subprocess.run(list(argv), capture_output=True, text=True, timeout=timeout, check=False)


# ---------------------------------------------------------------------------
# keyfile rendering (pure)
# ---------------------------------------------------------------------------

def _gkey_escape(v: str) -> str:
    """Escape a string value for GLib's key-file format."""
    out = v.replace("\\", "\\\\").replace("\n", "\\n").replace("\t", "\\t").replace("\r", "\\r")
    if out.startswith(" "):
        out = "\\s" + out[1:]
    if out.endswith(" ") and len(out) > 1:
        out = out[:-1] + "\\s"
    return out


def gkey_unescape(v: str) -> str:
    out, i = [], 0
    table = {"s": " ", "n": "\n", "t": "\t", "r": "\r", "\\": "\\"}
    while i < len(v):
        c = v[i]
        if c == "\\" and i + 1 < len(v) and v[i + 1] in table:
            out.append(table[v[i + 1]])
            i += 2
            continue
        out.append(c)
        i += 1
    return "".join(out)


def ssid_value(ssid: str) -> str:
    """SSID for a keyfile. Plain text when unambiguous, else NM's byte-list form."""
    raw = ssid.encode("utf-8")
    simple = (
        ssid
        and ssid == ssid.strip()
        and not any(c in ssid for c in ';\\"')
        and all(32 <= ord(c) for c in ssid)
        and not ssid.replace(";", "").isdigit()
    )
    if simple:
        return _gkey_escape(ssid)
    return ";".join(str(b) for b in raw) + ";"


def render_wifi_keyfile(
    con_id: str,
    ssid: str,
    psk: Optional[str],
    *,
    hidden: bool = False,
    key_mgmt: str = "wpa-psk",
    priority: int = 0,
    ipv4_address: Optional[str] = None,
    ipv4_gateway: Optional[str] = None,
    ipv4_dns: Sequence[str] = (),
    con_uuid: Optional[str] = None,
    interface: Optional[str] = None,
) -> str:
    lines = [
        "[connection]",
        f"id={_gkey_escape(con_id)}",
        f"uuid={con_uuid or uuid.uuid4()}",
        "type=wifi",
    ]
    if interface:
        lines.append(f"interface-name={interface}")
    lines += [
        "autoconnect=true",
        f"autoconnect-priority={int(priority)}",
        "",
        "[wifi]",
        "mode=infrastructure",
        f"ssid={ssid_value(ssid)}",
    ]
    if hidden:
        lines.append("hidden=true")
    # Wi-Fi power save off: lower latency for sync packets (2 = disable).
    lines += ["powersave=2", ""]
    if psk:
        lines += [
            "[wifi-security]",
            f"key-mgmt={key_mgmt}",
            f"psk={_gkey_escape(psk)}",
            "",
        ]
    lines += _render_ipv4(ipv4_address, ipv4_gateway, ipv4_dns)
    lines += ["[ipv6]", "addr-gen-mode=default", "method=auto", ""]
    return "\n".join(lines)


def render_ethernet_keyfile(
    con_id: str,
    ipv4_address: Optional[str],
    ipv4_gateway: Optional[str] = None,
    ipv4_dns: Sequence[str] = (),
    con_uuid: Optional[str] = None,
    interface: str = "eth0",
) -> str:
    lines = [
        "[connection]",
        f"id={_gkey_escape(con_id)}",
        f"uuid={con_uuid or uuid.uuid4()}",
        "type=ethernet",
        f"interface-name={interface}",
        "autoconnect=true",
        "autoconnect-priority=10",
        "",
        "[ethernet]",
        "",
    ]
    lines += _render_ipv4(ipv4_address, ipv4_gateway, ipv4_dns)
    lines += ["[ipv6]", "addr-gen-mode=default", "method=auto", ""]
    return "\n".join(lines)


def _render_ipv4(address: Optional[str], gateway: Optional[str], dns: Sequence[str]) -> List[str]:
    if not address:
        return ["[ipv4]", "method=auto", ""]
    iface = ipaddress.IPv4Interface(address)
    addr = iface.with_prefixlen
    if gateway:
        addr += f",{ipaddress.IPv4Address(gateway)}"
    out = ["[ipv4]", "method=manual", f"address1={addr}"]
    if dns:
        out.append("dns=" + ";".join(str(ipaddress.ip_address(d)) for d in dns) + ";")
    out.append("")
    return out


def render_hotspot_keyfile(ssid: str, psk: Optional[str], interface: str = "wlan0", channel: int = 6) -> str:
    """Access-point profile. 2.4 GHz channel 6 is legal in every regulatory domain."""
    lines = [
        "[connection]",
        f"id={HOTSPOT_CON}",
        f"uuid={uuid.uuid4()}",
        "type=wifi",
        f"interface-name={interface}",
        "autoconnect=false",
        "",
        "[wifi]",
        "mode=ap",
        "band=bg",
        f"channel={int(channel)}",
        f"ssid={ssid_value(ssid)}",
        "powersave=2",
        "",
    ]
    if psk:
        # WPA2-only (no PMF requirement) for maximum phone compatibility.
        lines += [
            "[wifi-security]",
            "key-mgmt=wpa-psk",
            "proto=rsn",
            "pairwise=ccmp",
            "group=ccmp",
            "pmf=1",
            f"psk={_gkey_escape(psk)}",
            "",
        ]
    lines += [
        "[ipv4]",
        "method=shared",
        f"address1={HOTSPOT_ADDR}/{HOTSPOT_PREFIX}",
        "",
        "[ipv6]",
        "method=disabled",
        "",
    ]
    return "\n".join(lines)


def keyfile_path(con_id: str, nm_dir: Optional[str] = None) -> str:
    safe = "".join(c if c.isalnum() or c in "-_." else "_" for c in con_id)
    return os.path.join(nm_dir or NM_DIR, f"{safe}.nmconnection")


def write_keyfile(con_id: str, content: str, nm_dir: Optional[str] = None) -> str:
    path = keyfile_path(con_id, nm_dir)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    tmp = path + ".tmp"
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w", encoding="utf-8") as f:
        f.write(content)
        f.flush()
        os.fsync(f.fileno())
    os.chmod(tmp, 0o600)
    os.replace(tmp, path)
    return path


def read_keyfile_uuid(con_id: str, nm_dir: Optional[str] = None) -> Optional[str]:
    """Keep a profile's UUID stable across rewrites."""
    try:
        with open(keyfile_path(con_id, nm_dir), encoding="utf-8") as f:
            for ln in f:
                if ln.startswith("uuid="):
                    return ln.strip().split("=", 1)[1]
    except OSError:
        pass
    return None


def remove_keyfile(con_id: str, nm_dir: Optional[str] = None) -> bool:
    try:
        os.remove(keyfile_path(con_id, nm_dir))
        return True
    except FileNotFoundError:
        return False


# ---------------------------------------------------------------------------
# nmcli wrappers
# ---------------------------------------------------------------------------

def split_terse(line: str) -> List[str]:
    """Split one line of ``nmcli -t`` output (':' separated, '\\:' escaped)."""
    fields, cur, i = [], [], 0
    while i < len(line):
        c = line[i]
        if c == "\\" and i + 1 < len(line):
            cur.append(line[i + 1])
            i += 2
            continue
        if c == ":":
            fields.append("".join(cur))
            cur = []
        else:
            cur.append(c)
        i += 1
    fields.append("".join(cur))
    return fields


def parse_power_save(text: str) -> Optional[bool]:
    """`iw dev wlan0 get power_save` -> True (on), False (off), None (unknown)."""
    for line in (text or "").splitlines():
        line = line.strip()
        if line.startswith("Power save:"):
            v = line.split(":", 1)[1].strip().lower()
            return {"on": True, "off": False}.get(v)
    return None


class NM:
    def __init__(self, run: Runner = default_runner):
        self.run = run

    def ok(self, *argv: str, timeout: int = 30) -> bool:
        try:
            return self.run(argv, timeout).returncode == 0
        except (subprocess.TimeoutExpired, FileNotFoundError):
            return False

    def out(self, *argv: str, timeout: int = 30) -> str:
        try:
            r = self.run(argv, timeout)
        except (subprocess.TimeoutExpired, FileNotFoundError):
            return ""
        return r.stdout if r.returncode == 0 else ""

    def reload(self) -> bool:
        return self.ok("nmcli", "connection", "reload")

    def up(self, con_id: str, wait: int = 45) -> bool:
        return self.ok("nmcli", "--wait", str(wait), "connection", "up", "id", con_id, timeout=wait + 10)

    def down(self, con_id: str) -> bool:
        return self.ok("nmcli", "connection", "down", "id", con_id)

    def radio_on(self) -> None:
        self.ok("rfkill", "unblock", "wifi")
        self.ok("nmcli", "radio", "wifi", "on")

    def power_save(self, iface: str) -> Optional[bool]:
        """Wi-Fi power saving state of `iface` (None when unknown)."""
        return parse_power_save(self.out("iw", "dev", iface, "get", "power_save", timeout=5))

    def power_save_off(self, iface: str) -> Optional[bool]:
        """Turn Wi-Fi power saving off if it is on. It delays packets for a dozing
        station by 50-1000 ms, which breaks show sync; NetworkManager's
        `wifi.powersave = 2` normally handles this, `iw` is the fallback.
        Returns the state afterwards (None when unknown / no Wi-Fi)."""
        state = self.power_save(iface)
        if state:
            self.ok("iw", "dev", iface, "set", "power_save", "off", timeout=5)
            state = self.power_save(iface)
        return state

    def devices(self) -> List[Dict[str, str]]:
        out = self.out("nmcli", "-t", "-f", "DEVICE,TYPE,STATE,CONNECTION", "device", "status")
        devs = []
        for ln in out.splitlines():
            f = split_terse(ln)
            if len(f) >= 4:
                devs.append({"device": f[0], "type": f[1], "state": f[2], "connection": f[3]})
        return devs

    def wifi_device(self) -> Optional[str]:
        for d in self.devices():
            if d["type"] == "wifi":
                return d["device"]
        return None

    def connections(self) -> List[Dict[str, str]]:
        out = self.out("nmcli", "-t", "-f", "NAME,UUID,TYPE,AUTOCONNECT", "connection", "show")
        cons = []
        for ln in out.splitlines():
            f = split_terse(ln)
            if len(f) >= 4:
                cons.append({"name": f[0], "uuid": f[1], "type": f[2], "autoconnect": f[3]})
        return cons

    def known_wifi(self) -> List[str]:
        """Names of Wi-Fi client profiles (anything except our hotspot)."""
        return [
            c["name"]
            for c in self.connections()
            if c["type"] in ("802-11-wireless", "wifi") and c["name"] != HOTSPOT_CON
        ]

    def ssid_of(self, con_id: str) -> Optional[str]:
        out = self.out("nmcli", "-t", "-g", "802-11-wireless.ssid", "connection", "show", "id", con_id)
        return out.strip() or None

    def scan(self, rescan: bool = True) -> List[Dict[str, object]]:
        out = self.out(
            "nmcli", "-t", "-f", "SSID,SIGNAL,SECURITY,CHAN", "device", "wifi", "list",
            "--rescan", "yes" if rescan else "no",
            timeout=40,
        )
        best: Dict[str, Dict[str, object]] = {}
        for ln in out.splitlines():
            f = split_terse(ln)
            if len(f) < 3 or not f[0]:
                continue
            ssid, sec = f[0], f[2].strip()
            try:
                sig = int(f[1] or 0)
            except ValueError:
                sig = 0
            entry = {
                "ssid": ssid,
                "signal": sig,
                "secure": sec not in ("", "--"),
                "security": sec,
                "wpa3Only": ("WPA3" in sec or "SAE" in sec) and "WPA2" not in sec and "WPA1" not in sec,
                "enterprise": "802.1X" in sec,
            }
            if ssid not in best or sig > int(best[ssid]["signal"]):  # type: ignore[arg-type]
                best[ssid] = entry
        return sorted(best.values(), key=lambda e: -int(e["signal"]))  # type: ignore[arg-type]

    def online(self, ignore: Sequence[str] = (HOTSPOT_CON,)) -> Optional[Dict[str, str]]:
        """First connected ethernet/wifi device (excluding the hotspot)."""
        for d in self.devices():
            if d["type"] in ("ethernet", "wifi") and d["state"] == "connected" and d["connection"] not in ignore:
                return d
        return None

    def ethernet_connected(self) -> bool:
        return any(d["type"] == "ethernet" and d["state"] == "connected" for d in self.devices())

    def hotspot_active(self) -> bool:
        return any(d["connection"] == HOTSPOT_CON and d["state"] == "connected" for d in self.devices())

    def ip4_of(self, device: str) -> List[str]:
        out = self.out("nmcli", "-t", "-g", "IP4.ADDRESS", "device", "show", device)
        return [a.split("/")[0] for a in out.replace("|", "\n").splitlines() if a.strip()]
