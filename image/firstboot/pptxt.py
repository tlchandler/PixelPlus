"""Parse, validate and scrub ``pixelplus.txt`` (the boot-partition settings file).

Pure standard library, no I/O side effects except in the explicit ``read_*`` /
``write_*`` helpers, so it can be unit-tested anywhere (see image/tests/).

File format
-----------
* UTF-8 (a BOM is tolerated), LF or CRLF line endings (Notepad!).
* ``key=value`` per line. Whitespace around the key and around ``=`` is ignored.
* The value is everything after the first ``=``, trimmed. Wrap it in double quotes
  to keep leading/trailing spaces (``"  my ssid "``). Inside quotes, ``\\"`` and
  ``\\\\`` are escapes.
* Lines whose first non-blank character is ``#`` are comments. There are NO inline
  comments: ``wifi_ssid=Joe's #1 wifi`` is the SSID ``Joe's #1 wifi``.
* Keys are case-insensitive; a few friendly aliases are accepted (``ssid``, ``psk``...).
* An empty value means "leave the current setting alone".
"""

from __future__ import annotations

import datetime as _dt
import hashlib
import ipaddress
import os
import re
from dataclasses import dataclass, field
from typing import Dict, List, Optional, Tuple

# key -> kind. Order is the canonical order used for documentation / rendering.
KEYS: Dict[str, str] = {
    "wifi_ssid": "ssid",
    "wifi_password": "wifi_password",
    "wifi_country": "country",
    "wifi_hidden": "bool",
    "wifi2_ssid": "ssid",
    "wifi2_password": "wifi_password",
    "hostname": "hostname",
    "role": "role",
    "timezone": "timezone",
    "board": "board",
    "ip_address": "cidr",
    "ip_gateway": "ip",
    "ip_dns": "ip_list",
    "ip_interface": "iface",
    "ui_password": "ui_password",
    "ssh": "onoff",
    "ssh_password": "login_password",
    "ssh_key": "ssh_key",
    "hotspot": "onoff",
    "hotspot_password": "hotspot_password",
    "hotspot_timeout": "seconds",
}

ALIASES: Dict[str, str] = {
    "ssid": "wifi_ssid",
    "wifi_name": "wifi_ssid",
    "psk": "wifi_password",
    "wifi_psk": "wifi_password",
    "wifi_pass": "wifi_password",
    "country": "wifi_country",
    "wifi2_psk": "wifi2_password",
    "password": "ui_password",
    "tz": "timezone",
    "static_ip": "ip_address",
    "gateway": "ip_gateway",
    "dns": "ip_dns",
}

#: Keys whose values are removed from the file after they were applied.
SECRET_KEYS = ("wifi_password", "wifi2_password", "ui_password", "ssh_password")

#: Marker prefix of the comment line inserted above a scrubbed secret.
APPLIED_MARKER = "# [applied "

ROLES = ("leader", "follower")
BOARDS = ("auto", "difftx", "difftxlarge", "diffsmart", "bare-pi", "virtual")
TRUE_WORDS = ("1", "yes", "y", "true", "on", "enable", "enabled")
FALSE_WORDS = ("0", "no", "n", "false", "off", "disable", "disabled")

_HOSTNAME_RE = re.compile(r"^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$")
_HEX64_RE = re.compile(r"^[0-9a-fA-F]{64}$")
_TZ_RE = re.compile(r"^[A-Za-z0-9_+\-]+(?:/[A-Za-z0-9_+\-]+)*$")
_SSH_KEY_RE = re.compile(
    r"^(ssh-(ed25519|rsa|dss)|ecdsa-sha2-nistp(256|384|521)|sk-(ssh-ed25519|ecdsa-sha2-nistp256)@openssh\.com)"
    r" [A-Za-z0-9+/=]+( .*)?$"
)


@dataclass
class Line:
    """One physical line of the file (kept so we can rewrite it faithfully)."""

    raw: str  # without line terminator
    key: Optional[str] = None  # canonical key if this is a key=value line
    value: Optional[str] = None


@dataclass
class ParseResult:
    values: Dict[str, str] = field(default_factory=dict)  # canonical key -> value ("" = unset)
    lines: List[Line] = field(default_factory=list)
    warnings: List[str] = field(default_factory=list)
    newline: str = "\n"

    def get(self, key: str) -> str:
        return self.values.get(key, "")


def _unquote(v: str) -> str:
    if len(v) >= 2 and v[0] == '"' and v[-1] == '"':
        inner = v[1:-1]
        out = []
        i = 0
        while i < len(inner):
            c = inner[i]
            if c == "\\" and i + 1 < len(inner) and inner[i + 1] in '"\\':
                out.append(inner[i + 1])
                i += 2
                continue
            out.append(c)
            i += 1
        return "".join(out)
    return v


def quote_if_needed(v: str) -> str:
    """Quote a value so ``parse`` returns it unchanged."""
    if v == "":
        return ""
    if v != v.strip() or (v[0] == '"' and v[-1] == '"' and len(v) >= 2):
        return '"' + v.replace("\\", "\\\\").replace('"', '\\"') + '"'
    return v


def canonical_key(k: str) -> Optional[str]:
    k = k.strip().lower().replace("-", "_")
    if k in KEYS:
        return k
    return ALIASES.get(k)


def parse(text: str) -> ParseResult:
    res = ParseResult()
    if text.startswith("﻿"):
        text = text[1:]
    if "\r\n" in text:
        res.newline = "\r\n"
    for lineno, raw in enumerate(text.splitlines(), start=1):
        stripped = raw.strip()
        line = Line(raw=raw)
        res.lines.append(line)
        if not stripped or stripped.startswith("#") or stripped.startswith(";"):
            continue
        if "=" not in stripped:
            res.warnings.append(f"line {lineno}: ignored (no '='): {stripped[:40]!r}")
            continue
        k, v = stripped.split("=", 1)
        ck = canonical_key(k)
        if ck is None:
            res.warnings.append(f"line {lineno}: unknown setting '{k.strip()}' ignored")
            continue
        v = _unquote(v.strip())
        line.key, line.value = ck, v
        if ck in res.values and res.values[ck] != "" and v != "":
            res.warnings.append(f"line {lineno}: '{ck}' appears more than once; the last one wins")
        # A later empty duplicate must not wipe an earlier value.
        if v != "" or ck not in res.values:
            res.values[ck] = v
    return res


def parse_bool(v: str) -> Optional[bool]:
    lv = v.strip().lower()
    if lv in TRUE_WORDS:
        return True
    if lv in FALSE_WORDS:
        return False
    return None


@dataclass
class Settings:
    """Validated, typed settings. ``None`` means "not set / leave alone"."""

    wifi_ssid: Optional[str] = None
    wifi_password: Optional[str] = None
    wifi_country: Optional[str] = None
    wifi_hidden: bool = False
    wifi2_ssid: Optional[str] = None
    wifi2_password: Optional[str] = None
    hostname: Optional[str] = None
    role: Optional[str] = None
    timezone: Optional[str] = None
    board: str = "auto"
    ip_address: Optional[str] = None  # "a.b.c.d/nn", or "dhcp" to go back to automatic
    ip_gateway: Optional[str] = None
    ip_dns: List[str] = field(default_factory=list)
    ip_interface: Optional[str] = None  # wifi | ethernet
    ui_password: Optional[str] = None
    ssh: Optional[bool] = None
    ssh_password: Optional[str] = None
    ssh_key: Optional[str] = None
    hotspot: bool = True
    hotspot_password: Optional[str] = "pixelplus"  # None = open hotspot
    hotspot_timeout: int = 75

    def effective_ip_interface(self) -> str:
        if self.ip_interface:
            return self.ip_interface
        return "wifi" if self.wifi_ssid else "ethernet"


def _valid_wifi_password(v: str) -> Optional[str]:
    if _HEX64_RE.match(v):
        return None
    if not 8 <= len(v.encode("utf-8")) <= 63:
        return "must be 8 to 63 characters (or 64 hex digits)"
    if any(ord(c) < 32 for c in v):
        return "contains control characters"
    return None


def validate(
    pr: ParseResult,
    countries: Optional[set] = None,
    zoneinfo_dir: Optional[str] = "/usr/share/zoneinfo",
) -> Tuple[Settings, List[str]]:
    """Turn raw values into ``Settings``. Returns (settings, errors).

    Invalid values are reported and left as ``None`` (i.e. not applied); valid
    ones are still applied, so one typo does not block everything else.
    """
    s = Settings()
    errors: List[str] = []
    g = pr.get

    def err(key: str, msg: str) -> None:
        errors.append(f"{key}: {msg}")

    for key in ("wifi_ssid", "wifi2_ssid"):
        v = g(key)
        if v:
            if len(v.encode("utf-8")) > 32:
                err(key, "a Wi-Fi name can be at most 32 bytes long")
            elif any(ord(c) < 32 for c in v):
                err(key, "contains control characters")
            else:
                setattr(s, key, v)

    for key, ssid_key in (("wifi_password", "wifi_ssid"), ("wifi2_password", "wifi2_ssid")):
        v = g(key)
        if v:
            problem = _valid_wifi_password(v)
            if problem:
                err(key, problem)
            elif not g(ssid_key):
                err(key, f"is set but {ssid_key} is empty")
            else:
                setattr(s, key, v)

    v = g("wifi_country").upper()
    if v:
        if not re.fullmatch(r"[A-Z]{2}", v) or (countries is not None and v not in countries):
            err("wifi_country", f"'{g('wifi_country')}' is not a two-letter country code (e.g. US, GB, DE)")
        else:
            s.wifi_country = v

    v = g("wifi_hidden")
    if v:
        b = parse_bool(v)
        if b is None:
            err("wifi_hidden", "use yes or no")
        else:
            s.wifi_hidden = b

    v = g("hostname")
    if v:
        hv = v.strip().lower()
        if hv.endswith(".local"):
            hv = hv[: -len(".local")]
        if not _HOSTNAME_RE.match(hv):
            err("hostname", f"'{v}' - use 1-63 letters, digits and dashes (not at the start or end)")
        else:
            s.hostname = hv

    v = g("role").lower()
    if v:
        if v not in ROLES:
            err("role", "use leader or follower (or leave empty)")
        else:
            s.role = v

    v = g("timezone")
    if v and v.lower() != "auto":
        if not _TZ_RE.match(v) or ".." in v:
            err("timezone", f"'{v}' is not a time zone name like America/Chicago")
        elif zoneinfo_dir and os.path.isdir(zoneinfo_dir) and not os.path.isfile(os.path.join(zoneinfo_dir, v)):
            err("timezone", f"unknown time zone '{v}' (examples: America/New_York, Europe/London)")
        else:
            s.timezone = v

    v = g("board").lower()
    if v:
        if v not in BOARDS:
            err("board", f"unknown board '{v}'; use one of: {', '.join(BOARDS)}")
        else:
            s.board = v

    v = g("ip_address")
    if v and v.lower() in ("dhcp", "auto", "automatic"):
        s.ip_address = "dhcp"
    elif v:
        try:
            iface = ipaddress.IPv4Interface(v if "/" in v else v + "/24")
            if iface.ip == iface.network.network_address or iface.ip == iface.network.broadcast_address:
                raise ValueError("network/broadcast address")
            s.ip_address = str(iface.with_prefixlen)
        except ValueError:
            err("ip_address", f"'{v}' is not an address like 192.168.1.50/24")
    v = g("ip_gateway")
    if v:
        try:
            s.ip_gateway = str(ipaddress.IPv4Address(v))
        except ValueError:
            err("ip_gateway", f"'{v}' is not an IPv4 address")
    v = g("ip_dns")
    if v:
        dns = []
        for part in re.split(r"[\s,;]+", v.strip()):
            if not part:
                continue
            try:
                dns.append(str(ipaddress.ip_address(part)))
            except ValueError:
                err("ip_dns", f"'{part}' is not an IP address")
        s.ip_dns = dns
    if (s.ip_gateway or s.ip_dns) and (not g("ip_address") or s.ip_address == "dhcp"):
        err("ip_gateway", "ip_gateway/ip_dns are only used together with ip_address")
    v = g("ip_interface").lower()
    if v:
        if v in ("wifi", "wlan", "wlan0", "wireless"):
            s.ip_interface = "wifi"
        elif v in ("ethernet", "eth", "eth0", "wired", "lan"):
            s.ip_interface = "ethernet"
        else:
            err("ip_interface", "use wifi or ethernet")

    v = g("ui_password")
    if v:
        if len(v) < 4:
            err("ui_password", "use at least 4 characters")
        else:
            s.ui_password = v

    v = g("ssh")
    if v:
        b = parse_bool(v)
        if b is None:
            err("ssh", "use on or off")
        else:
            s.ssh = b
    v = g("ssh_password")
    if v:
        if len(v) < 8:
            err("ssh_password", "use at least 8 characters")
        elif ":" in v or "\n" in v:
            err("ssh_password", "must not contain ':'")
        else:
            s.ssh_password = v
    v = g("ssh_key")
    if v:
        if not _SSH_KEY_RE.match(v.strip()):
            err("ssh_key", "does not look like an SSH public key (ssh-ed25519 AAAA... name)")
        else:
            s.ssh_key = v.strip()

    v = g("hotspot")
    if v:
        b = parse_bool(v)
        if b is None:
            err("hotspot", "use on or off")
        else:
            s.hotspot = b
    v = g("hotspot_password")
    if v:
        if v.lower() in ("none", "open", "off"):
            s.hotspot_password = None
        else:
            problem = _valid_wifi_password(v)
            if problem or _HEX64_RE.match(v):
                err("hotspot_password", problem or "use a normal 8-63 character password")
            else:
                s.hotspot_password = v
    v = g("hotspot_timeout")
    if v:
        try:
            n = int(v)
            if not 20 <= n <= 3600:
                raise ValueError
            s.hotspot_timeout = n
        except ValueError:
            err("hotspot_timeout", "use a number of seconds between 20 and 3600")

    return s, errors


def scrub_secrets(pr: ParseResult, applied_keys, now: Optional[_dt.datetime] = None) -> str:
    """Return the file text with the values of ``applied_keys`` (secret keys that were
    applied) blanked and an explanatory comment above each. Everything else, including
    comments and user formatting, is preserved byte-for-byte."""
    now = now or _dt.datetime.now()
    stamp = now.strftime("%Y-%m-%d %H:%M")
    applied = set(applied_keys) & set(SECRET_KEYS)
    out: List[str] = []
    for idx, line in enumerate(pr.lines):
        if line.raw.lstrip().startswith(APPLIED_MARKER):
            # Drop old markers; they are re-added only above freshly scrubbed lines.
            continue
        if line.key in applied and line.value:
            out.append(
                f"{APPLIED_MARKER}{stamp}] Saved on the device and removed from this file. "
                "Type a new one to change it."
            )
            prefix = line.raw.split("=", 1)[0]
            out.append(prefix.rstrip() + "=")
            continue
        if line.key in SECRET_KEYS and not line.value:
            # keep an existing marker for a still-empty secret line
            if idx > 0 and pr.lines[idx - 1].raw.lstrip().startswith(APPLIED_MARKER):
                out.append(pr.lines[idx - 1].raw)
        out.append(line.raw)
    nl = pr.newline
    return nl.join(out) + nl



def sha256_text(text: str) -> str:
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def read_file(path: str) -> str:
    with open(path, "rb") as f:
        data = f.read()
    try:
        return data.decode("utf-8")
    except UnicodeDecodeError:
        # Windows editors sometimes save as ANSI (cp1252).
        return data.decode("cp1252", errors="replace")


def write_file_atomic(path: str, text: str) -> None:
    """Write via temp file + rename (safe on FAT too) and fsync."""
    d = os.path.dirname(os.path.abspath(path))
    tmp = os.path.join(d, ".pixelplus.txt.tmp")
    with open(tmp, "w", encoding="utf-8", newline="") as f:
        f.write(text)
        f.flush()
        os.fsync(f.fileno())
    os.replace(tmp, path)
    try:
        fd = os.open(d, os.O_RDONLY)
        try:
            os.fsync(fd)
        finally:
            os.close(fd)
    except OSError:
        pass


def load_countries(path: str = "/usr/share/zoneinfo/iso3166.tab") -> Optional[set]:
    try:
        with open(path, encoding="utf-8") as f:
            return {ln.split("\t", 1)[0] for ln in f if ln and not ln.startswith("#")}
    except OSError:
        return None
