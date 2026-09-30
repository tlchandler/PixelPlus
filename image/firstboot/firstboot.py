#!/usr/bin/env python3
"""PixelPlus first-boot / pixelplus.txt applier.

Runs as root from ``pixelplus-firstboot.service`` on every boot (before
``pixelplusd``) and from ``pixelplus-firstboot.path`` whenever
``/boot/firmware/pixelplus.txt`` changes. It is idempotent: the file's SHA-256
(after secrets were scrubbed) is remembered, and nothing is re-applied unless the
file changed.

Sub-commands::

    pixelplus-firstboot [apply]            # default: apply pixelplus.txt + board config
    pixelplus-firstboot apply --force      # re-apply even if unchanged
    pixelplus-firstboot board-config [--board B] [--pixels N] [--reboot]
    pixelplus-firstboot check FILE         # validate a pixelplus.txt, print problems

Coexistence with Raspberry Pi Imager "OS customisation":
* We never edit cmdline.txt (Imager's ``systemd.run=firstrun.sh`` and
  ``cfg80211.ieee80211_regdom=`` entries are left alone; ``raspi-config`` may update the
  latter when ``wifi_country`` is set, which is what Imager does too).
* Empty values in pixelplus.txt mean "leave alone", so hostname / Wi-Fi / SSH / user
  set by Imager (firstrun.sh on Bookworm, cloud-init on Trixie) are kept.
* The unit is ordered after cloud-init's local+network stages, so a value that is set in
  BOTH places ends up as the pixelplus.txt value; when we set a hostname we also tell
  cloud-init to preserve it.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import logging
import os
import pwd
import re
import shutil
import subprocess
import sys
from typing import Dict, List, Optional, Sequence

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import nmconn  # noqa: E402
import pptxt  # noqa: E402

LOG = logging.getLogger("pixelplus-firstboot")

BOOT_DIRS = [d for d in (os.environ.get("PIXELPLUS_BOOT_DIR"), "/boot/firmware", "/boot") if d]
STATE_DIR = os.environ.get("PIXELPLUS_STATE_DIR", "/var/lib/pixelplus-system")
ETC_DIR = os.environ.get("PIXELPLUS_ETC_DIR", "/etc/pixelplus")
DATA_DIR = os.environ.get("PIXELPLUS_DATA_DIR", "/var/lib/pixelplus")
LOG_FILE = os.environ.get("PIXELPLUS_FIRSTBOOT_LOG", "/var/log/pixelplus-firstboot.log")
TEMPLATE = os.environ.get("PIXELPLUS_TXT_TEMPLATE", "/usr/share/pixelplus/pixelplus.txt.template")
ROOT = os.environ.get("PIXELPLUS_ROOT", "/")  # prefix for /etc files (tests)
PIXELPLUS_CLI = os.environ.get("PIXELPLUS_CLI", "pixelplus")
SERVICE_USER = "pixelplus"

BOARD_CONF = "pixelplus.conf"
INCLUDE_LINE = "include pixelplus.conf"
BLOCK_BEGIN = "# >>> PixelPlus board settings (managed automatically - set board= in pixelplus.txt instead) >>>"
BLOCK_END = "# <<< PixelPlus board settings <<<"
MAX_BOARD_REBOOTS = 3


def rootp(path: str) -> str:
    return os.path.join(ROOT, path.lstrip("/"))


# ---------------------------------------------------------------------------
# small utilities
# ---------------------------------------------------------------------------

class Sys:
    """Side-effecting operations, overridable in tests (dry run records commands)."""

    def __init__(self, dry_run: bool = False):
        self.dry_run = dry_run
        self.commands: List[List[str]] = []

    def run(self, argv: Sequence[str], timeout: Optional[int] = 60, input_text: Optional[str] = None) -> bool:
        self.commands.append(list(argv))
        if self.dry_run:
            LOG.info("[dry-run] %s", " ".join(argv))
            return True
        try:
            r = subprocess.run(list(argv), input=input_text, capture_output=True, text=True, timeout=timeout, check=False)
        except (FileNotFoundError, subprocess.TimeoutExpired) as e:
            LOG.warning("command failed: %s: %s", argv[0], e)
            return False
        if r.returncode != 0:
            LOG.warning("command %s exited %s: %s", " ".join(argv[:3]), r.returncode, (r.stderr or r.stdout).strip()[:300])
        return r.returncode == 0

    def output(self, argv: Sequence[str], timeout: int = 30) -> Optional[str]:
        self.commands.append(list(argv))
        if self.dry_run and not os.environ.get("PIXELPLUS_DRY_RUN_EXEC"):
            return None
        try:
            r = subprocess.run(list(argv), capture_output=True, text=True, timeout=timeout, check=False)
        except (FileNotFoundError, subprocess.TimeoutExpired):
            return None
        return r.stdout if r.returncode == 0 else None

    @staticmethod
    def have(cmd: str) -> bool:
        return shutil.which(cmd) is not None


def nm_runner(sysops: Sys):
    def run(argv, timeout=30):
        ok = sysops.run(argv, timeout)
        return subprocess.CompletedProcess(list(argv), 0 if ok else 1, "", "")

    return run


def find_boot_dir() -> Optional[str]:
    for d in BOOT_DIRS:
        if os.path.isfile(os.path.join(d, "config.txt")) or os.path.isfile(os.path.join(d, "pixelplus.txt")):
            return d
    return None


def load_state() -> Dict:
    try:
        with open(os.path.join(STATE_DIR, "state.json"), encoding="utf-8") as f:
            return json.load(f)
    except (OSError, ValueError):
        return {}


def save_state(state: Dict) -> None:
    os.makedirs(STATE_DIR, mode=0o700, exist_ok=True)
    path = os.path.join(STATE_DIR, "state.json")
    tmp = path + ".tmp"
    with open(tmp, "w", encoding="utf-8") as f:
        json.dump(state, f, indent=2, sort_keys=True)
        f.flush()
        os.fsync(f.fileno())
    os.replace(tmp, path)


def write_private_json(path: str, obj: Dict, owner: Optional[str] = None, mode: int = 0o600) -> None:
    os.makedirs(os.path.dirname(path), exist_ok=True)
    tmp = path + ".tmp"
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, mode)
    with os.fdopen(fd, "w", encoding="utf-8") as f:
        json.dump(obj, f, indent=2, sort_keys=True)
        f.flush()
        os.fsync(f.fileno())
    if owner:
        try:
            pw = pwd.getpwnam(owner)
            os.chown(tmp, pw.pw_uid, pw.pw_gid)
        except (KeyError, PermissionError):
            pass
    os.replace(tmp, path)


def first_login_user() -> Optional[pwd.struct_passwd]:
    """The interactive admin user: uid 1000 ('pi' in our image, or whatever Imager renamed it to)."""
    try:
        return pwd.getpwuid(1000)
    except KeyError:
        for p in pwd.getpwall():
            if 1000 <= p.pw_uid < 60000 and p.pw_shell not in ("/usr/sbin/nologin", "/bin/false"):
                return p
    return None


# ---------------------------------------------------------------------------
# individual appliers
# ---------------------------------------------------------------------------

def apply_country(sysops: Sys, cc: str) -> None:
    LOG.info("Wi-Fi country -> %s", cc)
    if Sys.have("raspi-config"):
        sysops.run(["raspi-config", "nonint", "do_wifi_country", cc])
    else:
        sysops.run(["iw", "reg", "set", cc])
        sysops.run(["rfkill", "unblock", "wifi"])
    sysops.run(["nmcli", "radio", "wifi", "on"])


def ensure_radio(sysops: Sys, s: pptxt.Settings) -> None:
    """Raspberry Pi OS keeps Wi-Fi soft-blocked until a country is set. PixelPlus needs the
    radio for the setup hotspot and for Wi-Fi given without a country; without a country the
    kernel uses the conservative world regulatory domain (2.4 GHz channels 1-11)."""
    if not (s.hotspot or s.wifi_ssid or s.wifi2_ssid):
        return
    sysops.run(["rfkill", "unblock", "wifi"])
    sysops.run(["nmcli", "radio", "wifi", "on"])


def apply_hostname(sysops: Sys, name: str) -> None:
    LOG.info("hostname -> %s", name)
    if not sysops.run(["hostnamectl", "set-hostname", name]):
        if not sysops.dry_run:
            with open(rootp("/etc/hostname"), "w", encoding="utf-8") as f:
                f.write(name + "\n")
    if not sysops.dry_run:
        update_hosts_file(rootp("/etc/hosts"), name)
        # cloud-init (Trixie images) would otherwise reset the hostname from its
        # user-data on every boot.
        if os.path.isdir(rootp("/etc/cloud/cloud.cfg.d")):
            with open(rootp("/etc/cloud/cloud.cfg.d/99-pixelplus-hostname.cfg"), "w", encoding="utf-8") as f:
                f.write("# Written by PixelPlus: hostname is managed by pixelplus.txt / the PixelPlus UI\n")
                f.write("preserve_hostname: true\n")
    sysops.run(["systemctl", "try-reload-or-restart", "avahi-daemon.service"])


def update_hosts_file(path: str, name: str) -> None:
    try:
        with open(path, encoding="utf-8") as f:
            lines = f.read().splitlines()
    except OSError:
        lines = ["127.0.0.1\tlocalhost", "::1\t\tlocalhost ip6-localhost ip6-loopback"]
    out, done = [], False
    for ln in lines:
        if re.match(r"^\s*127\.0\.1\.1\s", ln):
            if not done:
                out.append(f"127.0.1.1\t{name}")
                done = True
            continue
        out.append(ln)
    if not done:
        out.append(f"127.0.1.1\t{name}")
    with open(path, "w", encoding="utf-8") as f:
        f.write("\n".join(out) + "\n")


def apply_timezone(sysops: Sys, tz: str) -> None:
    LOG.info("timezone -> %s", tz)
    if not sysops.run(["timedatectl", "set-timezone", tz]) and not sysops.dry_run:
        link = rootp("/etc/localtime")
        try:
            os.remove(link)
        except FileNotFoundError:
            pass
        os.symlink(os.path.join("/usr/share/zoneinfo", tz), link)
        with open(rootp("/etc/timezone"), "w", encoding="utf-8") as f:
            f.write(tz + "\n")


def read_keyfile_value(con_id: str, key: str) -> Optional[str]:
    try:
        with open(nmconn.keyfile_path(con_id), encoding="utf-8") as f:
            for ln in f:
                if ln.startswith(key + "="):
                    return nmconn.gkey_unescape(ln.rstrip("\n").split("=", 1)[1])
    except OSError:
        pass
    return None


def apply_wifi_profile(
    sysops: Sys,
    con_id: str,
    ssid: Optional[str],
    psk: Optional[str],
    prev_ssid: Optional[str],
    hidden: bool,
    priority: int,
    ip: Optional[Dict],
) -> bool:
    """Create/refresh one Wi-Fi profile. Returns True if something was written."""
    if not ssid:
        return False
    if psk is None:
        if prev_ssid == ssid:
            # Password was applied earlier and scrubbed from pixelplus.txt: keep it.
            psk = read_keyfile_value(con_id, "psk")
        # else: a new SSID without password = open network
    LOG.info("Wi-Fi profile %s -> ssid=%r (%s)", con_id, ssid, "secured" if psk else "open")
    content = nmconn.render_wifi_keyfile(
        con_id,
        ssid,
        psk,
        hidden=hidden,
        priority=priority,
        ipv4_address=(ip or {}).get("address"),
        ipv4_gateway=(ip or {}).get("gateway"),
        ipv4_dns=(ip or {}).get("dns", ()),
        con_uuid=nmconn.read_keyfile_uuid(con_id),
    )
    if not sysops.dry_run:
        nmconn.write_keyfile(con_id, content)
    return True


def apply_ethernet_static(sysops: Sys, ip: Optional[Dict]) -> bool:
    if ip is None:
        return False
    if ip.get("address") in (None, "dhcp"):
        LOG.info("Ethernet -> automatic (DHCP)")
        if not sysops.dry_run:
            return nmconn.remove_keyfile("pixelplus-ethernet")
        return True
    LOG.info("Ethernet -> fixed address %s", ip["address"])
    content = nmconn.render_ethernet_keyfile(
        "pixelplus-ethernet", ip["address"], ip.get("gateway"), ip.get("dns", ()),
        con_uuid=nmconn.read_keyfile_uuid("pixelplus-ethernet"),
    )
    if not sysops.dry_run:
        nmconn.write_keyfile("pixelplus-ethernet", content)
    return True


def apply_ssh(sysops: Sys, enable: bool) -> None:
    LOG.info("SSH -> %s", "on" if enable else "off")
    if Sys.have("raspi-config"):
        sysops.run(["raspi-config", "nonint", "do_ssh", "0" if enable else "1"])
    elif enable:
        sysops.run(["systemctl", "enable", "--now", "ssh.service"])
    else:
        sysops.run(["systemctl", "disable", "--now", "ssh.service"])


def apply_login_password(sysops: Sys, password: str) -> None:
    user = first_login_user()
    if not user:
        LOG.warning("no login user found; ssh_password ignored")
        return
    LOG.info("setting password for login user %s", user.pw_name)
    sysops.run(["chpasswd"], input_text=f"{user.pw_name}:{password}\n")
    sysops.run(["usermod", "--unlock", user.pw_name])


def apply_ssh_key(sysops: Sys, key: str) -> None:
    user = first_login_user()
    if not user:
        LOG.warning("no login user found; ssh_key ignored")
        return
    LOG.info("adding SSH key for %s", user.pw_name)
    if sysops.dry_run:
        return
    sshdir = os.path.join(user.pw_dir, ".ssh")
    os.makedirs(sshdir, mode=0o700, exist_ok=True)
    ak = os.path.join(sshdir, "authorized_keys")
    existing = ""
    try:
        with open(ak, encoding="utf-8") as f:
            existing = f.read()
    except OSError:
        pass
    if key not in existing.splitlines():
        with open(ak, "a", encoding="utf-8") as f:
            if existing and not existing.endswith("\n"):
                f.write("\n")
            f.write(key + "\n")
    for p in (sshdir, ak):
        os.chown(p, user.pw_uid, user.pw_gid)
    os.chmod(ak, 0o600)


# ---------------------------------------------------------------------------
# board / config.txt
# ---------------------------------------------------------------------------

def detect_board(sysops: Sys) -> Optional[Dict]:
    """Ask the PixelPlus CLI which board is attached (``pixelplus --json detect`` reads
    the PPX1 EEPROM on i2c-1). Returns {"board": id, "rev": ...} or None."""
    if not Sys.have(PIXELPLUS_CLI) and not os.path.isabs(PIXELPLUS_CLI):
        return None
    out = sysops.output([PIXELPLUS_CLI, "--json", "detect"], timeout=30)
    if not out:
        return None
    try:
        info = json.loads(out)
    except ValueError:
        return None
    det = info.get("board") if isinstance(info, dict) else None
    if isinstance(det, dict):  # {"board": {"board": "difftx", "rev": "E", ...}, "pi": ...}
        if det.get("board"):
            return {"board": det["board"], "rev": det.get("rev")}
        return None
    if isinstance(det, str) and det:  # flat form {"board": "difftx"}
        return {"board": det, "rev": info.get("rev")}
    return None


def pi_model() -> str:
    try:
        with open(rootp("/proc/device-tree/model"), "rb") as f:
            return f.read().rstrip(b"\0").decode("utf-8", "replace")
    except OSError:
        return ""


def board_fragment(sysops: Sys, board: str, pixels: Optional[int]) -> Optional[str]:
    argv = [PIXELPLUS_CLI, "config-txt", "--board", board]
    if pixels:
        argv += ["--pixels", str(pixels)]
    return sysops.output(argv, timeout=30)


def strip_legacy_block(config_text: str) -> str:
    """Remove the managed block older images wrote directly into config.txt."""
    out, skipping = [], False
    for ln in config_text.splitlines():
        if ln.strip() == BLOCK_BEGIN:
            skipping = True
            continue
        if ln.strip() == BLOCK_END:
            skipping = False
            continue
        if not skipping:
            out.append(ln)
    return "\n".join(out) + "\n"


def ensure_include(config_text: str) -> str:
    """config.txt must end with ``[all]`` + ``include pixelplus.conf`` so the board file
    applies to every Pi model regardless of earlier conditional sections."""
    text = strip_legacy_block(config_text)
    lines = text.rstrip("\n").splitlines()
    if any(ln.strip() == INCLUDE_LINE for ln in lines):
        return text if text.endswith("\n") else text + "\n"
    lines += ["", "# PixelPlus board settings (pixel output overlay, RTC, ...) live in pixelplus.conf,",
              "# generated by `pixelplus config-txt`. Do not remove this line.", "[all]", INCLUDE_LINE]
    return "\n".join(lines) + "\n"


def render_board_conf(fragment: str, board: str, pixels: Optional[int]) -> str:
    head = [
        "# pixelplus.conf - PixelPlus board settings, included from config.txt.",
        f"# Board: {board}. Written by PixelPlus (pixelplus-firstboot / the setup wizard);",
        "# regenerate with:  sudo pixelplus-firstboot board-config --board <id> [--pixels N]",
        "# Changes take effect after a reboot.",
        "",
    ]
    return "\n".join(head) + fragment.strip("\n") + "\n"


def board_config(
    sysops: Sys,
    boot_dir: str,
    board: Optional[str],
    state: Dict,
    allow_reboot: bool,
    pixels: Optional[int] = None,
    force: bool = False,
) -> bool:
    """Make sure /boot/firmware/pixelplus.conf holds the settings for the attached board
    and config.txt includes it. Regenerated only when missing, when the board or the Pi
    model changed, or when forced (setup wizard / helper) - so a file the daemon
    regenerated for longer strings (--pixels) is kept. Returns True if a reboot is
    required (and allowed)."""
    source = "pixelplus.txt"
    rev = None
    if not board or board == "auto":
        info = detect_board(sysops)
        if not info:
            LOG.info("board: not detected (blank EEPROM or CLI unavailable) - the setup wizard will ask")
            return False
        board, rev, source = info["board"], info.get("rev"), "eeprom"
    model = pi_model()
    conf = os.path.join(boot_dir, BOARD_CONF)
    cfg = os.path.join(boot_dir, "config.txt")
    prev = state.get("board") or {}
    have_conf = os.path.isfile(conf) and os.path.getsize(conf) > 0
    changed = force or not have_conf or prev.get("id") != board or prev.get("model") != model
    LOG.info("board: %s%s (from %s) on %s", board, f" rev {rev}" if rev else "", source, model or "unknown Pi")

    try:
        with open(cfg, encoding="utf-8") as f:
            cfg_text = f.read()
    except OSError:
        LOG.warning("board: %s not readable", cfg)
        return False
    new_cfg = ensure_include(cfg_text)
    wrote = False
    if new_cfg != cfg_text:
        LOG.info("board: adding '%s' to %s", INCLUDE_LINE, cfg)
        if not sysops.dry_run:
            shutil.copy2(cfg, cfg + ".pixelplus-bak")
            pptxt.write_file_atomic(cfg, new_cfg)
        wrote = True

    if changed:
        frag = board_fragment(sysops, board, pixels)
        if frag is None:
            LOG.warning("board: 'pixelplus config-txt' failed; %s left unchanged", conf)
        else:
            new_conf = render_board_conf(frag, board, pixels)
            old_conf = ""
            if have_conf:
                with open(conf, encoding="utf-8", errors="replace") as f:
                    old_conf = f.read()
            if new_conf != old_conf:
                LOG.info("board: writing %s", conf)
                if not sysops.dry_run:
                    pptxt.write_file_atomic(conf, new_conf)
                wrote = True
            state["board"] = {"id": board, "rev": rev, "source": source, "model": model, "pixels": pixels}

    if not wrote:
        state["board_reboots"] = 0
        return False
    n = int(state.get("board_reboots", 0))
    if n >= MAX_BOARD_REBOOTS:
        LOG.error("board: boot configuration keeps changing; not rebooting again (check %s)", conf)
        return False
    state["board_reboots"] = n + 1
    return allow_reboot


# ---------------------------------------------------------------------------
# main apply
# ---------------------------------------------------------------------------

def ip_for(s: pptxt.Settings, which: str) -> Optional[Dict]:
    if not s.ip_address or s.effective_ip_interface() != which:
        return None
    if s.ip_address == "dhcp":
        return {"address": "dhcp"}
    return {"address": s.ip_address, "gateway": s.ip_gateway, "dns": s.ip_dns}


def write_errors_file(boot_dir: str, errors: List[str], warnings: List[str]) -> None:
    path = os.path.join(boot_dir, "pixelplus-errors.txt")
    if not errors and not warnings:
        try:
            os.remove(path)
        except OSError:
            pass
        return
    stamp = dt.datetime.now().strftime("%Y-%m-%d %H:%M")
    body = [
        "PixelPlus could not use some settings in pixelplus.txt",
        f"(checked {stamp}). Fix them in pixelplus.txt and restart the Pi.",
        "Everything else was applied. This file disappears once all is well.",
        "",
    ]
    body += [f"  PROBLEM  {e}" for e in errors]
    body += [f"  NOTE     {w}" for w in warnings]
    try:
        pptxt.write_file_atomic(path, "\r\n".join(body) + "\r\n")
    except OSError as e:
        LOG.warning("cannot write %s: %s", path, e)


def apply(sysops: Sys, force: bool = False, allow_reboot: bool = True) -> int:
    boot_dir = find_boot_dir()
    if not boot_dir:
        LOG.error("boot partition not found (looked in %s)", ", ".join(BOOT_DIRS))
        return 1
    path = os.path.join(boot_dir, "pixelplus.txt")
    state = load_state()
    first_boot = not state.get("first_boot_done")

    if not os.path.isfile(path) and os.path.isfile(TEMPLATE):
        LOG.info("pixelplus.txt missing - restoring the commented template")
        if not sysops.dry_run:
            shutil.copyfile(TEMPLATE, path)

    need_reboot = False
    if os.path.isfile(path):
        text = pptxt.read_file(path)
        digest = pptxt.sha256_text(text)
        if digest == state.get("sha256") and not force:
            LOG.info("pixelplus.txt unchanged since last boot")
            s, _ = pptxt.validate(pptxt.parse(text), pptxt.load_countries())
        else:
            s = apply_settings(sysops, boot_dir, path, text, state)
    else:
        s = pptxt.Settings()

    ensure_radio(sysops, s)

    board = s.board if s.board != "auto" else None
    need_reboot = board_config(sysops, boot_dir, board, state, allow_reboot)

    if first_boot:
        state["first_boot_done"] = dt.datetime.now().isoformat(timespec="seconds")
    save_state(state)
    if need_reboot:
        LOG.info("rebooting once to load the board's overlays")
        sysops.run(["systemctl", "--no-block", "reboot"])
    return 0


def apply_settings(sysops: Sys, boot_dir: str, path: str, text: str, state: Dict) -> pptxt.Settings:
    pr = pptxt.parse(text)
    s, errors = pptxt.validate(pr, pptxt.load_countries())
    for w in pr.warnings:
        LOG.warning("pixelplus.txt: %s", w)
    for e in errors:
        LOG.error("pixelplus.txt: %s", e)
    prev: Dict = state.get("applied", {})
    applied: Dict = dict(prev)
    applied_secrets: List[str] = []
    nm = nmconn.NM(nm_runner(sysops))

    def changed(key: str, value) -> bool:
        return value is not None and prev.get(key) != value

    if s.wifi_country and changed("wifi_country", s.wifi_country):
        apply_country(sysops, s.wifi_country)
        applied["wifi_country"] = s.wifi_country
    if s.hostname and changed("hostname", s.hostname):
        apply_hostname(sysops, s.hostname)
        applied["hostname"] = s.hostname
    if s.timezone and changed("timezone", s.timezone):
        apply_timezone(sysops, s.timezone)
        applied["timezone"] = s.timezone

    wifi_ip = ip_for(s, "wifi")
    wifi_ip_key = json.dumps(wifi_ip, sort_keys=True) if wifi_ip else None
    nm_changed = False
    for idx, (con, ssid, psk, prio) in enumerate(
        (
            ("pixelplus-wifi", s.wifi_ssid, s.wifi_password, 20),
            ("pixelplus-wifi2", s.wifi2_ssid, s.wifi2_password, 10),
        )
    ):
        sk = "wifi_ssid" if idx == 0 else "wifi2_ssid"
        opts = {"hidden": s.wifi_hidden if idx == 0 else False, "ip": wifi_ip_key if idx == 0 else None}
        if ssid and (psk or changed(sk, ssid) or prev.get(sk + "_opts") != opts):
            ip = {"address": None} if wifi_ip and wifi_ip["address"] == "dhcp" else wifi_ip
            if apply_wifi_profile(sysops, con, ssid, psk, prev.get(sk), bool(opts["hidden"]), prio, ip if idx == 0 else None):
                nm_changed = True
                applied[sk] = ssid
                applied[sk + "_opts"] = opts
                if psk:
                    applied_secrets.append("wifi_password" if idx == 0 else "wifi2_password")

    eth_ip = ip_for(s, "ethernet")
    if eth_ip and changed("ethernet_ip", json.dumps(eth_ip, sort_keys=True)):
        if apply_ethernet_static(sysops, eth_ip):
            nm_changed = True
        applied["ethernet_ip"] = json.dumps(eth_ip, sort_keys=True)

    if nm_changed:
        nm.reload()
        if not nm.hotspot_active() and s.wifi_ssid:
            online = nm.online()
            if not online or online.get("connection") != "pixelplus-wifi":
                # Non-fatal: at boot NetworkManager autoconnects anyway.
                nm.up("pixelplus-wifi", wait=5)

    if s.ssh is not None and changed("ssh", s.ssh):
        apply_ssh(sysops, s.ssh)
        applied["ssh"] = s.ssh
    if s.ssh_password:
        apply_login_password(sysops, s.ssh_password)
        applied_secrets.append("ssh_password")
    if s.ssh_key and changed("ssh_key", s.ssh_key):
        apply_ssh_key(sysops, s.ssh_key)
        applied["ssh_key"] = s.ssh_key

    # Things the daemon applies itself (it owns node.json and the password hash).
    provision: Dict = {}
    if s.role and changed("role", s.role):
        provision["role"] = s.role
        applied["role"] = s.role
    if s.ui_password:
        provision["uiPassword"] = s.ui_password
        applied_secrets.append("ui_password")
    if s.board != "auto" and changed("board", s.board):
        provision["board"] = s.board
        applied["board"] = s.board
    if provision:
        provision["source"] = "pixelplus.txt"
        provision["createdAt"] = dt.datetime.now().astimezone().isoformat(timespec="seconds")
        ppath = os.path.join(DATA_DIR, "provision.json")
        existing = {}
        try:
            with open(ppath, encoding="utf-8") as f:
                existing = json.load(f)
        except (OSError, ValueError):
            pass
        existing.update(provision)
        LOG.info("handing %s to pixelplusd via %s", sorted(k for k in provision if k != "uiPassword"), ppath)
        if not sysops.dry_run:
            write_private_json(ppath, existing, owner=SERVICE_USER)
        sysops.run(["systemctl", "try-restart", "pixelplusd.service"])

    netwatch = {
        "hotspot": s.hotspot,
        "hotspotPassword": s.hotspot_password,
        "hotspotTimeout": s.hotspot_timeout,
    }
    if prev.get("netwatch") != netwatch:
        LOG.info("hotspot settings: enabled=%s timeout=%ss %s", s.hotspot, s.hotspot_timeout,
                 "open" if not s.hotspot_password else "password-protected")
        if not sysops.dry_run:
            write_private_json(os.path.join(ETC_DIR, "netwatch.json"), netwatch)
        applied["netwatch"] = netwatch
        if prev.get("netwatch") is not None:
            sysops.run(["systemctl", "try-restart", "pixelplus-netwatch.service"])

    # Scrub applied secrets from the FAT file, then remember the digest of what is on disk.
    final_text = text
    if applied_secrets:
        final_text = pptxt.scrub_secrets(pr, applied_secrets)
        if not sysops.dry_run:
            pptxt.write_file_atomic(path, final_text)
        LOG.info("removed applied secrets from pixelplus.txt: %s", ", ".join(applied_secrets))
    if not sysops.dry_run:
        write_errors_file(boot_dir, errors, pr.warnings)
    state["applied"] = applied
    state["sha256"] = pptxt.sha256_text(final_text)
    state["applied_at"] = dt.datetime.now().isoformat(timespec="seconds")
    return s


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

def setup_logging(verbose: bool) -> None:
    fmt = logging.Formatter("%(asctime)s %(levelname)s %(message)s")
    root = logging.getLogger()
    root.setLevel(logging.DEBUG if verbose else logging.INFO)
    h = logging.StreamHandler(sys.stdout)
    h.setFormatter(logging.Formatter("%(levelname)s %(message)s"))
    root.addHandler(h)
    try:
        fh = logging.FileHandler(LOG_FILE)
        fh.setFormatter(fmt)
        root.addHandler(fh)
        os.chmod(LOG_FILE, 0o640)
    except OSError:
        pass


def main(argv: Optional[Sequence[str]] = None) -> int:
    ap = argparse.ArgumentParser(prog="pixelplus-firstboot", description=__doc__.split("\n\n")[0])
    ap.add_argument("--dry-run", action="store_true", help="log what would be done, change nothing")
    ap.add_argument("-v", "--verbose", action="store_true")
    sub = ap.add_subparsers(dest="cmd")
    a = sub.add_parser("apply", help="apply pixelplus.txt (default)")
    a.add_argument("--force", action="store_true", help="re-apply even if the file is unchanged")
    a.add_argument("--no-reboot", action="store_true")
    b = sub.add_parser("board-config", help="(re)write /boot/firmware/pixelplus.conf for a board")
    b.add_argument("--board", help="board id (default: detect from the EEPROM)")
    b.add_argument("--pixels", type=int, help="longest string to support (LEDs per output)")
    b.add_argument("--reboot", action="store_true", help="reboot if the boot configuration changed")
    c = sub.add_parser("check", help="validate a pixelplus.txt file")
    c.add_argument("file")
    args = ap.parse_args(argv)
    setup_logging(args.verbose)
    sysops = Sys(dry_run=args.dry_run)

    if args.cmd == "check":
        pr = pptxt.parse(pptxt.read_file(args.file))
        _, errors = pptxt.validate(pr, pptxt.load_countries())
        for w in pr.warnings:
            print("NOTE   ", w)
        for e in errors:
            print("PROBLEM", e)
        print("OK" if not errors else f"{len(errors)} problem(s)")
        return 0 if not errors else 2

    if os.geteuid() != 0 and not args.dry_run:
        LOG.error("must run as root")
        return 1

    if args.cmd == "board-config":
        boot_dir = find_boot_dir()
        if not boot_dir:
            LOG.error("boot partition not found")
            return 1
        state = load_state()
        state["board_reboots"] = 0  # explicit user action resets the loop guard
        reboot = board_config(sysops, boot_dir, args.board, state, args.reboot, pixels=args.pixels, force=True)
        save_state(state)
        if reboot:
            sysops.run(["systemctl", "--no-block", "reboot"])
        return 0

    force = getattr(args, "force", False)
    no_reboot = getattr(args, "no_reboot", False)
    return apply(sysops, force=force, allow_reboot=not no_reboot)


if __name__ == "__main__":
    sys.exit(main())
