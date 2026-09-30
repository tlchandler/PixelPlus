"""Tests for packaging/bin/pixelplus-helper (the root helper pixelplusd starts via
``systemctl start pixelplus-helper@<verb>.service``). Runs the script unprivileged with
its paths redirected into a temp dir: ``python -m pytest packaging/tests``."""

import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
HELPER = os.path.join(HERE, "..", "bin", "pixelplus-helper")


@unittest.skipUnless(shutil.which("bash"), "bash required")
class HelperTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp()
        self.status = os.path.join(self.tmp, "run")
        os.makedirs(self.status)
        self.firstboot = os.path.join(self.tmp, "firstboot.py")
        self.calls = os.path.join(self.tmp, "calls.txt")
        with open(self.firstboot, "w") as f:
            f.write(
                "import sys\n"
                f"open({self.calls!r}, 'a').write(' '.join(sys.argv[1:]) + '\\n')\n"
                "sys.exit(3 if '--board' in sys.argv and 'broken' in sys.argv else 0)\n"
            )
        self.hosts = os.path.join(self.tmp, "hosts")
        self.env = dict(
            os.environ,
            PIXELPLUS_HELPER_STATUS_DIR=self.status,
            PIXELPLUS_HELPER_PRIVATE_DIR=os.path.join(self.tmp, "private"),
            PIXELPLUS_FIRSTBOOT=self.firstboot,
            PIXELPLUS_HOSTS_FILE=self.hosts,
        )

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def run_helper(self, instance):
        return subprocess.run(["bash", HELPER, instance], env=self.env, capture_output=True, text=True)

    def status_of(self, verb):
        with open(os.path.join(self.status, f"helper-{verb}.json")) as f:
            return json.load(f)

    def test_config_txt_reports_ok_and_passes_arguments(self):
        r = self.run_helper("config-txt:difftxlarge:1600")
        self.assertEqual(r.returncode, 0, r.stderr)
        st = self.status_of("config-txt")
        self.assertEqual(st["verb"], "config-txt")
        self.assertEqual(st["state"], "ok")
        self.assertIsInstance(st["updatedAt"], int)
        with open(self.calls) as f:
            self.assertIn("board-config --board difftxlarge --pixels 1600", f.read())

    def test_refresh_index_runs_apt_get_update_only(self):
        # PixelPlus images disable apt's daily timers: the update check needs this.
        apt = os.path.join(self.tmp, "apt-get")
        with open(apt, "w") as f:
            f.write(f"#!/bin/sh\necho \"$@\" >> {self.calls}\nexit ${{APT_FAIL:-0}}\n")
        os.chmod(apt, 0o755)
        self.env["PIXELPLUS_APT_GET"] = apt
        r = self.run_helper("refresh-index")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(self.status_of("refresh-index")["state"], "ok")
        with open(self.calls) as f:
            self.assertEqual(f.read().split(), ["update", "-q"])
        self.env["APT_FAIL"] = "100"
        r = self.run_helper("refresh-index")
        self.assertNotEqual(r.returncode, 0)
        self.assertEqual(self.status_of("refresh-index")["state"], "failed")
        self.assertNotEqual(self.run_helper("refresh-index:x").returncode, 0)

    def test_config_txt_failure_is_reported(self):
        r = self.run_helper("config-txt:broken")
        self.assertNotEqual(r.returncode, 0)
        self.assertEqual(self.status_of("config-txt")["state"], "failed")

    def test_rejects_bad_arguments(self):
        for inst, verb in [("config-txt:a;b", "config-txt"), ("config-txt:difftx:12x", "config-txt"),
                           ("wifi-country:USA", "wifi-country"), ("update:now", "update"),
                           ("a:b:c:d", "invalid"), ("frobnicate", "frobnicate")]:
            r = self.run_helper(inst)
            self.assertNotEqual(r.returncode, 0, inst)
            self.assertEqual(self.status_of(verb)["state"], "failed", inst)

    def test_status_messages_are_valid_json(self):
        r = self.run_helper('config-txt:x"y')
        self.assertNotEqual(r.returncode, 0)
        json.loads(open(os.path.join(self.status, "helper-config-txt.json")).read())

    def test_planted_symlinks_are_not_followed(self):
        victim = os.path.join(self.tmp, "victim")
        with open(victim, "w") as f:
            f.write("precious\n")
        os.chmod(victim, 0o600)
        # the unprivileged user owns /run/pixelplus: it could plant these
        os.symlink(victim, os.path.join(self.status, "helper-hosts.json"))
        os.symlink(victim, os.path.join(self.status, "helper-hosts.json.tmp"))
        r = self.run_helper("hosts")
        self.assertEqual(r.returncode, 0, r.stderr)
        with open(victim) as f:
            self.assertEqual(f.read(), "precious\n")
        self.assertEqual(os.stat(victim).st_mode & 0o777, 0o600)
        self.assertFalse(os.path.islink(os.path.join(self.status, "helper-hosts.json")))
        self.assertEqual(self.status_of("hosts")["state"], "ok")

    def test_hosts_rewrites_the_127_0_1_1_line(self):
        with open(self.hosts, "w") as f:
            f.write("127.0.0.1\tlocalhost\n127.0.1.1\told-name\n::1\tlocalhost\n")
        r = self.run_helper("hosts")
        self.assertEqual(r.returncode, 0, r.stderr)
        with open(self.hosts) as f:
            lines = f.read().splitlines()
        self.assertIn("127.0.0.1\tlocalhost", lines)
        self.assertNotIn("127.0.1.1\told-name", lines)
        self.assertEqual(len([ln for ln in lines if ln.startswith("127.0.1.1")]), 1)


FAKE_DPKG = r"""#!/usr/bin/env python3
import os, sys
root = os.environ["FAKE_ROOT"]
inst = os.path.join(root, "installed")
def key(v):
    import re
    out = []
    for part in re.split(r"([0-9]+)", v.replace("~", "\x00")):
        out.append((0, int(part)) if part.isdigit() else (1, part))
    return out
a = sys.argv[1:]
open(os.path.join(root, "dpkg.log"), "a").write(" ".join(a) + "\n")
if a[0] == "--print-architecture":
    print("arm64")
elif a[0] == "--compare-versions":
    x, op, y = a[1], a[2], a[3]
    kx, ky = key(x), key(y)
    ok = {"gt": kx > ky, "lt": kx < ky, "ge": kx >= ky, "le": kx <= ky, "eq": kx == ky}[op]
    sys.exit(0 if ok else 1)
elif a[0] == "--audit":
    pass
elif a[0] == "--configure":
    pass
elif a[0] == "-i":
    f = a[-1]
    fields = dict(l.split(": ", 1) for l in open(f).read().splitlines() if ": " in l)
    if os.environ.get("DPKG_FAIL_VERSION") == fields["Version"]:
        sys.exit(1)
    open(inst, "w").write(fields["Version"])
    # the "daemon" of that version starts (unless told it's broken)
    if os.environ.get("BROKEN_VERSION") != fields["Version"]:
        run = os.environ["PIXELPLUS_HELPER_STATUS_DIR"]
        open(os.path.join(run, "healthy.json"), "w").write('{"version":"%s","engine":true}' % fields["Version"])
"""

FAKE_DPKG_DEB = r"""#!/usr/bin/env python3
import sys
f, field = sys.argv[2], sys.argv[3]
fields = dict(l.split(": ", 1) for l in open(f).read().splitlines() if ": " in l)
print(fields.get(field, ""))
"""

FAKE_DPKG_QUERY = r"""#!/bin/sh
cat "$FAKE_ROOT/installed" 2>/dev/null
"""

# "Signature" = GOOD <sha256 of the file>; the key file must hold an RW line.
FAKE_MINISIGN = r"""#!/usr/bin/env python3
import hashlib, sys
a = sys.argv[1:]
key, msg, sig = a[a.index("-p") + 1], a[a.index("-m") + 1], a[a.index("-x") + 1]
if not any(l.startswith("RW") for l in open(key).read().splitlines()):
    sys.exit(1)
want = "GOOD " + hashlib.sha256(open(msg, "rb").read()).hexdigest()
sys.exit(0 if open(sig).read().strip() == want else 1)
"""

FAKE_APT = r"""#!/bin/sh
echo "apt-get $*" >> "$FAKE_ROOT/calls.txt"
if [ "$1" = download ]; then
    v="${3#pixelplus=}"
    printf 'Package: pixelplus\nVersion: %s\nArchitecture: arm64\n' "$v" > "pixelplus_${v}_arm64.deb"
fi
exit 0
"""

FAKE_LOGGER = r"""#!/bin/sh
echo "$(basename "$0") $*" >> "$FAKE_ROOT/calls.txt"
exit ${FAKE_FAIL:-0}
"""

FAKE_TAILSCALE = r"""#!/bin/sh
echo "tailscale $*" >> "$FAKE_ROOT/calls.txt"
case "$1" in
status) echo '{"BackendState": "NeedsLogin", "AuthURL": "https://login.tailscale.com/a/abc123"}' ;;
esac
exit 0
"""


@unittest.skipUnless(shutil.which("bash"), "bash required")
class FleetVerbTests(unittest.TestCase):
    """Signed updates (update-*) and remote access (tailscale-*, cloudflared-*)."""

    def setUp(self):
        self.tmp = tempfile.mkdtemp()
        j = lambda *p: os.path.join(self.tmp, *p)
        self.status = j("run")
        self.data = j("data")
        self.cache = j("cache")
        self.state = j("helper-state")
        self.keys = j("keys")
        self.bin = j("bin")
        self.etc = j("etc")
        for d in (self.status, self.data, self.cache, self.keys, self.bin, j("data", "updates", "incoming"),
                  j("data", "remote")):
            os.makedirs(d, exist_ok=True)
        with open(j("keys", "release.pub"), "w") as f:
            f.write("untrusted comment: test\nRWQtestkey\n")
        fakes = {"dpkg": FAKE_DPKG, "dpkg-deb": FAKE_DPKG_DEB, "dpkg-query": FAKE_DPKG_QUERY,
                 "minisign": FAKE_MINISIGN, "apt-get": FAKE_APT, "systemctl": FAKE_LOGGER,
                 "systemd-run": FAKE_LOGGER, "tailscale": FAKE_TAILSCALE, "curl": FAKE_LOGGER}
        for name, body in fakes.items():
            with open(j("bin", name), "w") as f:
                f.write(body)
            os.chmod(j("bin", name), 0o755)
        with open(j("installed"), "w") as f:
            f.write("1.0.0")
        with open(j("health-ok"), "w") as f:
            f.write("#!/bin/sh\nexit 0\n")
        os.chmod(j("health-ok"), 0o755)
        with open(j("default"), "w") as f:
            f.write("PIXELPLUS_PUBLIC_PORT=8099\n")
        self.env = dict(
            os.environ,
            PATH=self.bin + ":" + os.environ["PATH"],
            FAKE_ROOT=self.tmp,
            PIXELPLUS_HELPER_STATUS_DIR=self.status,
            PIXELPLUS_HELPER_PRIVATE_DIR=j("private"),
            PIXELPLUS_HELPER_DATA_DIR=self.data,
            PIXELPLUS_UPDATE_CACHE=self.cache,
            PIXELPLUS_HELPER_STATE=self.state,
            PIXELPLUS_KEYS_DIR=self.keys,
            PIXELPLUS_DPKG=j("bin", "dpkg"),
            PIXELPLUS_DPKG_DEB=j("bin", "dpkg-deb"),
            PIXELPLUS_DPKG_QUERY=j("bin", "dpkg-query"),
            PIXELPLUS_MINISIGN=j("bin", "minisign"),
            PIXELPLUS_APT_GET=j("bin", "apt-get"),
            PIXELPLUS_SYSTEMCTL=j("bin", "systemctl"),
            PIXELPLUS_SYSTEMD_RUN=j("bin", "systemd-run"),
            PIXELPLUS_TAILSCALE=j("bin", "tailscale"),
            PIXELPLUS_CURL=j("bin", "curl"),
            PIXELPLUS_ETC_DIR=self.etc,
            PIXELPLUS_DEFAULTS=j("default"),
            PIXELPLUS_HEALTH_CMD=j("health-ok"),
            PIXELPLUS_HEALTH_TIMEOUT="2",
            PIXELPLUS_HEALTH_POLL="0.1",
            PIXELPLUS_ARCH="arm64",
        )

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def run_helper(self, instance):
        return subprocess.run(["bash", HELPER, instance], env=self.env, capture_output=True, text=True)

    def status_of(self, verb):
        with open(os.path.join(self.status, f"helper-{verb}.json")) as f:
            return json.load(f)

    def installed(self):
        return open(os.path.join(self.tmp, "installed")).read()

    def calls(self):
        p = os.path.join(self.tmp, "calls.txt")
        return open(p).read() if os.path.exists(p) else ""

    def offer(self, version, good=True, arch="arm64", signed_content=None):
        """What pixelplusd puts into updates/incoming."""
        import hashlib
        name = f"pixelplus_{version}_{arch}.deb"
        path = os.path.join(self.data, "updates", "incoming", name)
        body = f"Package: pixelplus\nVersion: {version}\nArchitecture: {arch}\n"
        with open(path, "w") as f:
            f.write(body)
        digest = hashlib.sha256((signed_content or body).encode()).hexdigest()
        with open(path + ".minisig", "w") as f:
            f.write(("GOOD " if good else "BAD ") + digest)
        return path

    def test_stage_verifies_and_keeps_the_installed_version(self):
        self.offer("1.1.0")
        r = self.run_helper("update-stage:1.1.0")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(self.status_of("update-stage")["state"], "ok")
        self.assertTrue(os.path.exists(os.path.join(self.cache, "staged", "pixelplus_1.1.0_arm64.deb")))
        # The installed version was fetched for going back.
        self.assertTrue(os.path.exists(os.path.join(self.cache, "rollback", "pixelplus_1.0.0_arm64.deb")))
        self.assertIn("apt-get download -q pixelplus=1.0.0", self.calls())

    def test_stage_refuses_unsigned_tampered_old_and_planted_packages(self):
        self.offer("1.1.0", good=False)
        self.assertNotEqual(self.run_helper("update-stage:1.1.0").returncode, 0)
        self.assertIn("isn't signed", self.status_of("update-stage")["message"])
        # Signed, but changed afterwards.
        self.offer("1.1.0", signed_content="something else")
        self.assertNotEqual(self.run_helper("update-stage:1.1.0").returncode, 0)
        # Correctly signed but older than what runs (downgrade attack).
        self.offer("0.9.0")
        self.assertNotEqual(self.run_helper("update-stage:0.9.0").returncode, 0)
        self.assertIn("isn't newer", self.status_of("update-stage")["message"])
        # A package claiming another version inside.
        p = self.offer("1.2.0")
        with open(p, "w") as f:
            f.write("Package: pixelplus\nVersion: 6.6.6\nArchitecture: arm64\n")
        import hashlib
        with open(p + ".minisig", "w") as f:
            f.write("GOOD " + hashlib.sha256(open(p, "rb").read()).hexdigest())
        self.assertNotEqual(self.run_helper("update-stage:1.2.0").returncode, 0)
        # A symlink planted by the service user is never followed.
        secret = os.path.join(self.tmp, "secret")
        with open(secret, "w") as f:
            f.write("root only\n")
        link = os.path.join(self.data, "updates", "incoming", "pixelplus_1.3.0_arm64.deb")
        os.symlink(secret, link)
        self.assertNotEqual(self.run_helper("update-stage:1.3.0").returncode, 0)
        self.assertEqual(os.listdir(os.path.join(self.cache, "staged")), [])
        for bad in ["update-stage:1.0;rm", "update-stage:../1", "update-stage:", "update-commit:x y"]:
            self.assertNotEqual(self.run_helper(bad).returncode, 0, bad)

    def test_commit_installs_and_passes_the_health_gate(self):
        self.offer("1.1.0")
        self.assertEqual(self.run_helper("update-stage:1.1.0").returncode, 0)
        r = self.run_helper("update-commit:1.1.0")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(self.installed(), "1.1.0")
        st = self.status_of("update-commit")
        self.assertEqual(st["state"], "ok", st)
        res = json.load(open(os.path.join(self.status, "update-result.json")))
        self.assertEqual((res["from"], res["to"], res["ok"]), ("1.0.0", "1.1.0", True))
        self.assertFalse(os.path.exists(os.path.join(self.state, "pending-verify")))
        # The new package is kept for the next rollback; the late check was armed.
        self.assertTrue(os.path.exists(os.path.join(self.cache, "rollback", "pixelplus_1.1.0_arm64.deb")))
        self.assertIn("--on-active=6min", self.calls())
        # Going back by hand reinstalls 1.0.0.
        r = self.run_helper("update-rollback")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(self.installed(), "1.0.0")
        self.assertIn("--force-downgrade", open(os.path.join(self.tmp, "dpkg.log")).read())

    def test_unhealthy_update_is_rolled_back(self):
        self.offer("1.1.0")
        self.assertEqual(self.run_helper("update-stage:1.1.0").returncode, 0)
        self.env["BROKEN_VERSION"] = "1.1.0"
        r = self.run_helper("update-commit:1.1.0")
        self.assertNotEqual(r.returncode, 0)
        self.assertEqual(self.installed(), "1.0.0")
        st = self.status_of("update-commit")
        self.assertEqual(st["state"], "failed")
        self.assertIn("rolled back to 1.0.0", st["message"])
        res = json.load(open(os.path.join(self.status, "update-result.json")))
        self.assertEqual((res["to"], res["ok"]), ("1.1.0", False))
        self.assertFalse(os.path.exists(os.path.join(self.state, "pending-verify")))

    def test_failed_install_and_unprepared_commit(self):
        self.assertNotEqual(self.run_helper("update-commit:1.1.0").returncode, 0)
        self.assertIn("isn't prepared", self.status_of("update-commit")["message"])
        self.offer("1.1.0")
        self.assertEqual(self.run_helper("update-stage:1.1.0").returncode, 0)
        self.env["DPKG_FAIL_VERSION"] = "1.1.0"
        self.assertNotEqual(self.run_helper("update-commit:1.1.0").returncode, 0)
        self.assertEqual(self.installed(), "1.0.0")

    def test_verify_after_a_power_cut(self):
        self.assertEqual(self.run_helper("update-verify").returncode, 0)  # nothing pending
        os.makedirs(self.state, exist_ok=True)
        with open(os.path.join(self.state, "pending-verify"), "w") as f:
            f.write("from=1.0.0\nto=1.1.0\narch=arm64\n")
        # 1.1.0 was installed before the power went; its daemon comes up healthy.
        with open(os.path.join(self.tmp, "installed"), "w") as f:
            f.write("1.1.0")
        import time
        time.sleep(0.05)
        open(os.path.join(self.state, "commit-start"), "w").close()
        time.sleep(0.05)
        with open(os.path.join(self.status, "healthy.json"), "w") as f:
            f.write('{"version":"1.1.0"}')
        r = self.run_helper("update-verify")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertFalse(os.path.exists(os.path.join(self.state, "pending-verify")))
        # Same, but it never becomes healthy: the kept 1.0.0 comes back.
        with open(os.path.join(self.state, "pending-verify"), "w") as f:
            f.write("from=1.0.0\nto=1.1.0\narch=arm64\n")
        os.remove(os.path.join(self.status, "healthy.json"))
        os.makedirs(os.path.join(self.cache, "rollback"), exist_ok=True)
        with open(os.path.join(self.cache, "rollback", "pixelplus_1.0.0_arm64.deb"), "w") as f:
            f.write("Package: pixelplus\nVersion: 1.0.0\nArchitecture: arm64\n")
        self.env["BROKEN_VERSION"] = "1.1.0"
        with open(os.path.join(self.tmp, "health-ok"), "w") as f:
            f.write("#!/bin/sh\nexit 1\n")
        r = self.run_helper("update-verify")
        self.assertNotEqual(r.returncode, 0)
        self.assertEqual(self.installed(), "1.0.0")

    def test_channel_switch(self):
        lst = os.path.join(self.tmp, "pixelplus.list")
        with open(lst, "w") as f:
            f.write("deb [signed-by=/usr/share/keyrings/pixelplus.gpg] https://repo.example stable main\n")
        self.env["PIXELPLUS_APT_LIST"] = lst
        self.assertEqual(self.run_helper("update-channel:beta").returncode, 0)
        self.assertIn(" beta main", open(lst).read())
        self.assertNotEqual(self.run_helper("update-channel:nightly").returncode, 0)

    def test_tailscale_serve_admin_and_funnel_only_the_public_port(self):
        self.assertEqual(self.run_helper("tailscale-serve:on").returncode, 0)
        self.assertEqual(self.run_helper("tailscale-funnel:on").returncode, 0)
        calls = self.calls()
        self.assertIn("tailscale serve --bg --https=443 http://127.0.0.1:80", calls)
        self.assertIn("tailscale funnel --bg --https=8443 http://127.0.0.1:8099", calls)
        self.assertNotIn("funnel --bg --https=8443 http://127.0.0.1:80\n", calls)
        self.assertEqual(self.run_helper("tailscale-funnel:off").returncode, 0)
        self.assertIn("tailscale funnel --https=8443 off", self.calls())
        self.assertNotEqual(self.run_helper("tailscale-funnel:maybe").returncode, 0)

    def test_tailscale_login_link_and_auth_key(self):
        r = self.run_helper("tailscale-up")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("https://login.tailscale.com/a/abc123", self.status_of("tailscale-up")["message"])
        key = os.path.join(self.data, "remote", "tailscale.authkey")
        with open(key, "w") as f:
            f.write("tskey-auth-kTEST123-abcdef")
        r = self.run_helper("tailscale-up")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("--auth-key=file:", self.calls())
        self.assertNotIn("tskey-auth-kTEST123", self.calls(), "never on a command line")
        self.assertFalse(os.path.exists(key), "the key file is consumed")
        with open(key, "w") as f:
            f.write("not a key; rm -rf /")
        self.assertNotEqual(self.run_helper("tailscale-up").returncode, 0)

    def test_cloudflared_token_goes_to_a_root_env_file(self):
        tok = os.path.join(self.data, "remote", "cloudflared.token")
        token = "eyJhIjoiMTIzNDU2Nzg5MCIsInQiOiJhYmNkZWYiLCJzIjoiWldGbiJ9"
        with open(tok, "w") as f:
            f.write(token + "\n")
        r = self.run_helper("cloudflared-token")
        self.assertEqual(r.returncode, 0, r.stderr)
        env = os.path.join(self.etc, "cloudflared.env")
        self.assertEqual(open(env).read(), f"TUNNEL_TOKEN={token}\n")
        self.assertEqual(os.stat(env).st_mode & 0o777, 0o600)
        self.assertFalse(os.path.exists(tok))
        self.assertNotIn(token, self.calls())
        self.assertIn("systemctl restart pixelplus-cloudflared.service", self.calls())
        # A planted symlink (e.g. to /etc/shadow) is refused.
        os.symlink(os.path.join(self.tmp, "installed"), tok)
        self.assertNotEqual(self.run_helper("cloudflared-token").returncode, 0)
        r = self.run_helper("cloudflared-stop")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertFalse(os.path.exists(env))
        self.assertIn("disable --now pixelplus-cloudflared.service", self.calls())

    def test_secret_files_are_read_without_following_links_or_blocking(self):
        """Security audit 2: the service user's file is opened once with O_NOFOLLOW and checked
        on that descriptor (no test-then-read race), a FIFO can't hang the root helper, and the
        root copy is 0600 from the start."""
        tok = os.path.join(self.data, "remote", "cloudflared.token")
        os.mkfifo(tok)
        r = subprocess.run(["bash", HELPER, "cloudflared-token"], env=self.env, capture_output=True,
                           text=True, timeout=20)
        self.assertNotEqual(r.returncode, 0)
        self.assertFalse(os.path.exists(os.path.join(self.etc, "cloudflared.env")))
        self.assertEqual([f for f in os.listdir(self.etc) if f.startswith(".cloudflared")], [])
        # An auth key that is a symlink to a root-only file is refused too.
        key = os.path.join(self.data, "remote", "tailscale.authkey")
        secret = os.path.join(self.tmp, "root-secret")
        with open(secret, "w") as f:
            f.write("tskey-auth-kSECRETSECRETSECRET\n")
        os.symlink(secret, key)
        r = self.run_helper("tailscale-up")
        self.assertNotEqual(r.returncode, 0)
        self.assertNotIn("--auth-key", self.calls())
        # A good token: the root env file is 0600.
        self.assertFalse(os.path.lexists(tok), "the unusable file is removed")
        with open(tok, "w") as f:
            f.write("eyJ" + "A" * 60 + "\n")
        r = self.run_helper("cloudflared-token")
        self.assertEqual(r.returncode, 0, r.stderr)
        env = os.path.join(self.etc, "cloudflared.env")
        self.assertEqual(os.stat(env).st_mode & 0o777, 0o600)

    def test_quick_tunnel_uses_its_own_unit(self):
        self.assertEqual(self.run_helper("cloudflared-quick:on").returncode, 0)
        self.assertIn("systemctl restart pixelplus-cloudflared-quick.service", self.calls())
        self.assertEqual(self.run_helper("cloudflared-quick:off").returncode, 0)


if __name__ == "__main__":
    sys.exit(unittest.main())
