# Building PixelPlus

Developer guide for the daemon, web UI, Debian package, SD-card image, Docker image and
PixelPlus Imager. Architecture and contracts: [ARCHITECTURE.md](ARCHITECTURE.md).
End-user installation: [INSTALL.md](INSTALL.md).

```
crates/        Rust workspace: pixelplusd (daemon), pixelplus (CLI), core/output/hw libs
web/           SvelteKit SPA -> web/build (served by pixelplusd from /usr/share/pixelplus/web)
tts/, games/   Python sidecars
packaging/     Debian package: build-deb.sh, systemd units, polkit, avahi, udev, ...
image/         pi-gen stage, pixelplus.txt, first-boot applier, setup hotspot, Imager JSON
imager/        PixelPlus Imager (Tauri 2 + Svelte) with a GUI-free core crate
docker/        Dockerfile + docker-compose.yml for a PC/NAS leader
.github/       CI and release workflows
```

## Prerequisites

| For | Needs |
|---|---|
| Rust | rustup, stable toolchain (MSRV 1.80); `pkg-config libasound2-dev` (ALSA headers for the daemon's audio output; `--no-default-features` builds a lights-only daemon without them) |
| Web / Imager UI | Node 22, pnpm 10 (`corepack enable`) |
| Python parts | Python 3.11+ (`pip install pytest jsonschema` for tests) |
| arm64 .deb on x86 | `sudo packaging/ci/install-arm64-cross-deps.sh` (aarch64 gcc + `libasound2-dev:arm64`) + `rustup target add aarch64-unknown-linux-gnu`, or `cross` (Cross.toml installs the arm64 ALSA headers) |
| .deb | `dpkg-deb`, `device-tree-compiler` (arm64: DPI overlays) |
| SD image | Docker with `--privileged`, ~25 GB disk; on x86: `qemu-user-static binfmt-support` |
| Imager app | Tauri 2 prerequisites: Linux `libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev libssl-dev`; macOS Xcode CLT; Windows WebView2 + MSVC |

## Daemon + web UI (development)

```sh
cargo build                                    # workspace
cd web && pnpm install && pnpm dev             # UI on :5173, proxies /api to PIXELPLUS_API
PIXELPLUS_DATA_DIR=./data-dev PIXELPLUS_HTTP_PORT=8080 PIXELPLUS_OUTPUT=sim cargo run -p pixelplus-daemon
```

Environment variables read by `pixelplusd`: `PIXELPLUS_DATA_DIR` (/var/lib/pixelplus),
`PIXELPLUS_WEB_DIR` (/usr/share/pixelplus/web), `PIXELPLUS_HTTP_PORT` (80),
`PIXELPLUS_HTTP_BIND`, `PIXELPLUS_CLUSTER_PORT` (32420; overlay +1), `PIXELPLUS_SENSOR_PORT` (32422),
`PIXELPLUS_HTTPS_PORT` (443, 0 = off), `PIXELPLUS_PUBLIC_PORT` (8081 on 127.0.0.1, 0 = off), `PIXELPLUS_OUTPUT`
(dpi|sim|none|auto), `PIXELPLUS_BOARD` (board override: difftx|difftxlarge|diffsmart|bare-pi|virtual|auto),
`PIXELPLUS_AUDIO` (`none` disables audio output), `PIXELPLUS_SHM_DIR` (/dev/shm),
`PIXELPLUS_TTS_URL`, `PIXELPLUS_GAMES_SOCKET`, `PIXELPLUS_MDNS` (0 = no mDNS), `PIXELPLUS_DEV`;
for tests: `PIXELPLUS_RUN_DIR` (/run/pixelplus), `PIXELPLUS_NETWATCH_STATUS`, `PIXELPLUS_BOOT_DIR`.
On the Pi they come from `pixelplusd.service` and optional overrides in `/etc/default/pixelplus`.
Cluster tuning: `PIXELPLUS_CLUSTER_PEERS` (static peers `host[:port],…` for networks without
broadcast), `PIXELPLUS_CLUSTER_OVERLAY_PORT` (cluster port + 1), `PIXELPLUS_CLUSTER_BIND`,
`PIXELPLUS_CLUSTER_BROADCAST` (0 = unicast to peers only).

`PIXELPLUS_DEV=1` (or `PIXELPLUS_OUTPUT=sim`) enables `GET /api/v1/debug/output`: the last frame
written to this controller's outputs, `{frameNo, atMs, wallMs, sequence: {id, frame}, master,
player, outputs: [{index, pixels, rgb, wire}]}` with base64 `rgb` (rendered, colour order not
applied) and `wire` (what the output backend received), optionally `?outputs=1,2`. With
`PIXELPLUS_DEV=1` on a machine without I²C, the board's sensors are simulated.

### A three-node cluster on one machine

`scripts/dev-cluster.sh` runs a leader (difftxlarge) and two followers (difftx), each with its
own data directory, HTTP port and UDP ports, finding each other through
`PIXELPLUS_CLUSTER_PEERS` on 127.0.0.1 (broadcast and mDNS off), simulated output, no audio:

```sh
cargo build -p pixelplus-daemon && (cd web && pnpm build)
scripts/dev-cluster.sh start [--fresh]   # leader http://127.0.0.1:18080, followers :18081 / :18082
scripts/dev-cluster.sh status            # PIDs, URLs, /public/health
scripts/dev-cluster.sh logs [leader|f1|f2]
scripts/dev-cluster.sh restart f1        # or: kill leader (SIGKILL, like a power cut)
scripts/dev-cluster.sh stop
```

State and logs live in `PP_CLUSTER_DIR` (default `./.dev-cluster`); `PP_BIN`, `PP_WEB_DIR`,
`PP_HTTP_BASE`, `PP_CLUSTER_BASE` and `PP_AUDIO` change the binary, UI, ports and leader audio.
A fresh cluster starts unconfigured: open the leader and run the setup wizard, choose
"follower" on the other two, then adopt them under **Controllers**.

### End-to-end tests against the real daemons

```sh
node scripts/e2e/run.mjs            # fresh cluster in a temp dir, whole scenario, then stop
node scripts/e2e/run.mjs --keep     # leave the cluster running with the e2e show
cd web && pnpm test:real            # Playwright: every page + real button clicks, desktop & phone
PIXELPLUS_E2E_URL=http://127.0.0.1:18080 pnpm vitest run src/lib/api/contract.test.ts
```

`run.mjs` (Node ≥ 22.15, no dependencies) drives the HTTP API like a user would: setup wizard,
discovery and adoption, xLights import (`crates/pixelplus-core/testdata/xlights_2025_*.xml`),
a generated zstd `.fseq` with a known pattern (every prop a solid colour that changes each
second; its first pixel encodes the frame number) plus a WAV, byte-for-byte checks of the
followers' `.ppseq` slices, a playlist started by the schedule, frame-accurate sync of all three
nodes through `/debug/output` (±1 frame, every pixel compared), test patterns, fault finder,
blackout, brightness, live looks, overlays, song requests, snapshots, health, power, sensors,
password/login, a follower restarted mid-show, a leader crash, live prop changes and
remove/re-adopt. `pnpm test:real` (`web/playwright.real.config.ts`) walks every page against the
running leader (no console errors, no failed requests) and saves screenshots to `$SCREENS_DIR`.
The contract test compares every GET endpoint's JSON shape with the demo backend the UI is built on.

## Debian package

```sh
packaging/build-deb.sh --arch amd64                # native
packaging/build-deb.sh --arch arm64                # cross (see below)
packaging/build-deb.sh --arch arm64 --bin-dir target/aarch64-unknown-linux-gnu/release --web-dir web/build
```

Output: `dist/pixelplus_<version>_<arch>.deb`. The version is the workspace version, plus
`~git<date>.<sha>` for untagged commits.

**Cross-compiling for arm64** – the script picks, in order: `cross` (Docker-based;
old glibc, so binaries run on Bookworm and Trixie), or the Debian/Ubuntu cross gcc. For
the latter it sets `CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER`,
`CC_aarch64_unknown_linux_gnu`/`AR_…` (needed by `zstd-sys`) and `PKG_CONFIG_ALLOW_CROSS` /
`PKG_CONFIG_LIBDIR` for the arm64 ALSA headers (`alsa-sys`, via cpal); install both with
`sudo packaging/ci/install-arm64-cross-deps.sh` (Ubuntu: adds the ports.ubuntu.com arm64
sources). Plain `cargo build|check --target aarch64-unknown-linux-gnu -p pixelplus-daemon`
works too once those are installed: `.cargo/config.toml` sets the same linker, `CC`/`AR` and
target-scoped `PKG_CONFIG_*` variables (values already in the environment win). Build on the *oldest*
distribution you target (Ubuntu 22.04 / Debian Bookworm) because glibc is only forward
compatible.

### Package layout

| Path | What |
|---|---|
| `/usr/bin/pixelplusd`, `/usr/bin/pixelplus` | daemon, CLI |
| `/usr/share/pixelplus/web/` | web UI |
| `/usr/lib/pixelplus/firstboot/` | `pixelplus.txt` applier (`firstboot.py`, `pptxt.py`, `nmconn.py`); `/usr/sbin/pixelplus-firstboot` |
| `/usr/lib/pixelplus/netwatch/` | Wi-Fi watchdog + setup hotspot + captive portal |
| `/usr/lib/pixelplus/games/` | games sidecar (Python, system interpreter) |
| `/usr/lib/pixelplus/overlays/*.dtbo` | DPI overlays (arm64); postinst copies them to `/boot/firmware/overlays/` |
| `/usr/lib/pixelplus/pixelplus-helper` | root helper (see below) |
| `/usr/lib/pixelplus/tts-capable` | `ExecCondition` for the TTS sidecar (Pi 4/5 ≥ 2 GB, amd64) |
| `/usr/share/pixelplus/pixelplus.txt.template` | the commented `pixelplus.txt` |
| `/usr/lib/systemd/system/` | `pixelplusd`, `pixelplus-tts`, `pixelplus-games`, `pixelplus-firstboot`, `pixelplus-reapply.{path,service}`, `pixelplus-netwatch`, `pixelplus-helper@`, `pixelplus-update-verify` (boot-time end of an interrupted update), `pixelplus-cloudflared{,-quick}` (remote access) |
| `/usr/share/pixelplus/keys/*.pub` | minisign keys signed updates must be signed with (`packaging/keys/`) |
| `/var/cache/pixelplus/{staged,rollback}`, `/var/lib/pixelplus-helper` | root-only: staged and kept (rollback) packages, the pending-update marker |
| `/usr/share/polkit-1/rules.d/50-pixelplus.rules` | what the `pixelplus` user may do |
| `/etc/avahi/services/pixelplus.service` | `_http._tcp` on port 80 (`_pixelplus._tcp` is published by pixelplusd, see below) |
| `/usr/lib/sysctl.d`, `/usr/lib/udev/rules.d`, `/usr/lib/tmpfiles.d`, `/etc/logrotate.d` | UDP buffers, device groups, `/run/pixelplus`, log rotation |
| `/usr/share/pixelplus/appliance/` | NetworkManager configs, linked into `/etc` only on appliances |

### Security model (decided)

`pixelplusd` runs as the **unprivileged system user `pixelplus`** with supplementary
groups `video render i2c gpio spi audio netdev dialout pixelplus-overlay` and only two
capabilities: `CAP_NET_BIND_SERVICE` (port 80) and `CAP_SYS_NICE` (the output thread switches
itself to `SCHED_FIFO`; `LimitRTPRIO=95`, `LimitMEMLOCK=infinity`). It is sandboxed
(`NoNewPrivileges`, `ProtectSystem=full`, `ProtectHome`, `PrivateTmp`, `UMask=0027`). Privileged
operations go through system services, authorised by polkit for that user only, and only for
what it uses (`packaging/polkit/50-pixelplus.rules`: NetworkManager `network-control`,
`settings.modify.system/own`, `wifi.scan`, `enable-disable-wifi`; logind reboot/power-off;
hostnamed; timedated `set-timezone` — not `set-time`, NTP keeps the clock):

| Operation | How |
|---|---|
| Wi-Fi scan/connect, Ethernet settings | NetworkManager D-Bus (`org.freedesktop.NetworkManager.*`) |
| Reboot / power off | logind (`org.freedesktop.login1.reboot`, `power-off`) |
| Hostname, time zone, NTP | hostnamed (`hostnamectl --static --transient set-hostname`) / timedated (`timedatectl set-timezone`); avahi follows via `avahi-set-host-name` (netdev group) |
| Start/stop/restart `pixelplus-{tts,games,netwatch}`; restart `pixelplusd` | systemd `manage-units` |
| Board boot config, updates, SSH on/off, re-apply `pixelplus.txt`, Wi-Fi country, `/etc/hosts` | `systemctl start --no-block pixelplus-helper@<verb>.service` (root oneshot, whitelisted verbs) |
| Board EEPROM read/write | `/dev/i2c-1` (group `i2c`, `I2C_RDWR` works even while at24 is bound); the at24 sysfs file only if it is accessible. `new_device` (root) is never used |
| Update check | signed release index (`pixelplus-<channel>.json`, below); without a signing key in the build `apt-cache policy pixelplus`, the package lists refreshed by the helper's `refresh-index` verb (at most every 6 h; the image turns apt's own daily timers off) |
| Install / roll back updates | helper `update-stage`, `update-commit`, `update-rollback`, `update-verify` (re-verify signatures as root, health gate) |
| Tailscale, Cloudflare Tunnel | helper `tailscale-*`, `cloudflared-*` (secrets through 0600 files, never argv) |

**Sidecars run as their own users, without polkit rights.**

| Service | User / groups | Sees of `/var/lib/pixelplus` | Talks to pixelplusd |
|---|---|---|---|
| `pixelplus-games` (internet-facing web page on :8088) | `pixelplus-games`, primary group `pixelplus-overlay`, `audio` | only `games/` (`TemporaryFileSystem` + `BindPaths`; 2770 `pixelplus:pixelplus-overlay`, setgid) | HTTP on loopback with the local token (below); control socket `/run/pixelplus-games/games.sock` (its `RuntimeDirectory`, 0770 → pixelplusd via the group); overlay buffers in `/dev/shm` (0660, group `pixelplus-overlay`) |
| `pixelplus-tts` (loopback :7081) | `pixelplus-tts`, `pixelplus` (to read music beds) | only `media/` (read-only) and `tts/` (its own) | pixelplusd calls it |

The **local token** replaces the old `X-PixelPlus-Local: 1` loopback trust: pixelplusd writes a
random token to `/run/pixelplus/local-token` at every start (0640, group `pixelplus-overlay`); the
games sidecar sends `X-PixelPlus-Local: <token>` (re-reading the file after a 401). pixelplusd
accepts it only from loopback, never together with proxy headers (so a reverse proxy or tunnel on
the Pi can't be used to reach the API), and only for the sidecar's routes (show, player
pause/resume/stop, overlays, events) — no settings, network, SSH or updates. The show it reads
has its secrets redacted.

**Cluster keys** are per follower and authenticate only cluster calls (ARCHITECTURE §7.5): no
bearer key on the wire, signed requests with replay protection, X25519 at adoption, clear rules
for who may adopt a controller (a leader only after its owner chose *Join another show*).

**Browser side** (ARCHITECTURE §8): Host allow-list against DNS rebinding (tunnel / own domains
under Settings → Security → *Other names for this controller*), `X-PixelPlus-Request: 1` on every
state-changing call against CSRF, WebSocket origin check, CSP with the UI's inline-script hashes,
`nosniff`, no framing, write-only SMTP/MQTT passwords, sign-in throttling (6+ character
passwords; Argon2 on at most two blocking threads), song-request limits per real client address
(forwarding headers only from this machine or `settings.security.trustedProxies`), a new
controller can be set up only from the local network, snapshot/WebSocket size limits, `nmcli
--ask` with the Wi-Fi password on stdin (never on a command line), ffmpeg restricted to local
files (`-protocol_whitelist file,pipe`).

**Remote access** (ARCHITECTURE §12.12): tunnels and funnels point at the **public-only
listener** (`127.0.0.1:8081`: song requests, games, `/api/v1/public/*`; everything else 404).
The admin UI is reachable remotely only on explicit opt-in (Tailscale `serve` to the owner's
tailnet, or a Cloudflare admin hostname), only with a password, and its host name is allowed
only while exposed. Any admin API call arriving through a proxy on the Pi (loopback peer with
forwarding headers) is refused while no password is set.

**Signed updates** (ARCHITECTURE §12.13): minisign (Ed25519) signatures by a key compiled into
the daemon and installed root-owned; verified by the daemon, by followers on packages their
leader serves, and again by the root helper before `dpkg -i`; only newer versions are
installed (rollbacks come from the helper's own root-only copies). A new release must start
healthy within 180 s or the previous package is reinstalled.

**xLights FPP Connect** (WS6, ARCHITECTURE §12.14) is root-mounted, outside `/api/v1`'s guard;
`security::fpp_compat_authorize` replaces the CSRF header for it: off unless enabled, LAN
peers without proxy headers only (never through a tunnel or the public listener), Host
allow-list, reads open, writes need the dedicated upload password as HTTP Basic auth (throttled;
remembered 10 minutes so 16 MiB chunks don't each cost an Argon2 check) or, without a
password, a non-CORS-simple request (`PATCH`, JSON body) that no web page can forge.

Known limits: the first adoption of a new or released controller is trust-on-first-use (someone
on the LAN could adopt it first; the owner sees who adopted it on the controller's page and in
its log, and can release it). Plain HTTP on the LAN: a sign-in session cookie and MQTT/SMTP
traffic without TLS can be sniffed by someone who can already read the LAN's traffic.
`SameSite=Lax` cookies don't separate ports (the games page on :8088 is "same-site"); the CSRF
header covers that.

Helper verbs (`packaging/bin/pixelplus-helper`), arguments `:`-separated in the instance name:

* `config-txt:<board>[:<pixels>]` – regenerate `/boot/firmware/pixelplus.conf` via
  `pixelplus config-txt --board <board> [--pixels N]`. **No reboot**; the daemon asks the user
  and then reboots via logind.
* `update` – `apt-get update` + upgrade `pixelplus` (postinst restarts the daemon).
* `refresh-index` – `apt-get update` only (fresh package lists for the update check).
* `ssh-on`, `ssh-off`, `reapply`.
* `wifi-country:<CC>` – `raspi-config nonint do_wifi_country` (or `iw reg set`).
* `hosts` – point `/etc/hosts`' `127.0.1.1` line at the current hostname (after the daemon
  renamed the host through hostnamed) and keep cloud-init from resetting it.
* `update-stage:<ver>`, `update-commit:<ver>`, `update-rollback`, `update-verify`,
  `update-channel:<stable|beta>` – signed updates (ARCHITECTURE §12.13). Versions with `~`
  (`1.3.0~beta1`) arrive systemd-escaped (`\x7e`); the polkit rule allows the backslash and the
  helper validates every argument.
* `tailscale-install`, `tailscale-up`, `tailscale-serve:<on|off>`, `tailscale-funnel:<on|off>`,
  `tailscale-down`, `cloudflared-install`, `cloudflared-quick:<on|off>`, `cloudflared-token`,
  `cloudflared-stop` – remote access (ARCHITECTURE §12.12). Auth keys / tunnel tokens are read
  from 0600 files in `/var/lib/pixelplus/remote/` (never through a symlink, size- and
  format-checked, deleted after use).

Each writes `/run/pixelplus/helper-<verb>.json`
(`{"verb","state":"running|ok|failed","message","updatedAt"}`) for the UI to poll.
`/run/pixelplus` belongs to `pixelplus` (tmpfiles.d), so root writers never write through a
path there: the helper writes in root-only `/run/pixelplus-helper` and renames into place,
netwatch and firstboot create files with `O_EXCL` under random names and rename them.

pixelplusd (`crates/pixelplus-daemon/src/services/platform.rs`) polls the file (only results
newer than the start count; a unit that fails without writing one is detected with
`systemctl is-failed`), forwards progress as `helper` WebSocket messages and toasts, and lists
the latest runs at `GET /api/v1/system/helpers`. Without the helper (development machine,
Docker) the API answers 403 with an explanation; running as root without the package it does
the same work directly.

### mDNS (decided)

avahi-daemon owns the host's mDNS names (`<hostname>.local`) on PixelPlus images. The daemon
publishes its cluster record `_pixelplus._tcp` (TXT `id`, `role`, `board`, `ver`) **through
avahi** (`avahi-publish -s`, D-Bus, package `avahi-utils`), and the static avahi service file
carries only `_http._tcp`, so there is exactly one responder and one `_pixelplus._tcp` instance
per controller. Two responders sharing UDP 5353 is not the problem (avahi and mdns-sd both
use `SO_REUSEADDR`/`SO_REUSEPORT` and receive every multicast packet); the problem is two
responders *claiming the same host name* with different address sets: each treats the other's
answer as a conflict and renames the host (`pixelplus-2.local`). Where avahi isn't running
(Docker, PCs) the built-in `mdns-sd` responder publishes the record; in Docker (the host may
run its own responder) its SRV target is `pixelplus-<id>.local`, never the machine's own name.
`PIXELPLUS_MDNS=0` turns mDNS off (UDP beacons still find controllers).

### Provisioning (`provision.json`)

`firstboot.py` hands the settings only the daemon may apply to
`/var/lib/pixelplus/provision.json` (0600, owner `pixelplus`; keys all optional: `role`,
`uiPassword`, `board`, `source`, `createdAt`; `name`, `showName`, `timezone` are also
accepted). pixelplusd checks for it at startup and every 5 s, applies it exactly like
`POST /api/v1/system/setup` (password hashed with Argon2, role/board set, show defaults
seeded for a leader), and deletes it. A file that can't be applied is deleted too (it holds
a plain-text password) and the reason is shown as a toast and logged.

### CLI contract used by the image

`firstboot.py` runs `pixelplus --json detect` (reads `board.board`: id or `null`, and
`board.rev`), `pixelplus config-txt --board <id> [--pixels N]` (prints the fragment; the
comment `up to N pixels per output` is what pixelplusd reads back from `pixelplus.conf`), and
`pixelplusd.service` runs `pixelplus pins release` after every stop (board from `--board`,
`PIXELPLUS_BOARD`, `/run/pixelplus/board` written by pixelplusd, or the EEPROM; no board =
nothing to do, exit 0). All of them work with `--simulate <board>` on a PC.

### apt repository (optional)

```sh
packaging/apt-repo.sh --repo ./apt --suite stable --key <GPG-KEYID> dist/*.deb   # a release
packaging/apt-repo.sh --repo ./apt --suite beta   --key <GPG-KEYID> dist/*.deb   # also beta
```

creates `apt/dists/<suite>/main/binary-{arm64,amd64}/Packages`, signed `Release`/`InRelease`
and `pixelplus-archive-keyring.gpg`. Host it on GitHub Pages or any HTTPS server; see the
script header for the client `sources.list` line. Settings → Updates → Channel switches the
suite (helper `update-channel`). Controllers on PixelPlus images update through signed
releases instead and don't need it.

### Signed updates (release key handling)

Over-the-air updates only install packages signed with a key listed in
`packaging/keys/pixelplus-release.pub` (compiled into pixelplusd, installed to
`/usr/share/pixelplus/keys/`). Until a key is listed there, builds don't offer signed
updates (apt still works). One-time setup by the maintainer, on a trusted machine:

```sh
sudo apt install minisign
minisign -G -p pixelplus-release.pub -s minisign.key     # choose a strong password
grep '^RW' pixelplus-release.pub >> packaging/keys/pixelplus-release.pub   # commit this
```

Store the **contents** of `minisign.key` (it is itself encrypted with the password) as the
repository secret `MINISIGN_SECRET_KEY` and the password as `MINISIGN_PASSWORD`; keep an
offline backup of both, and never commit `minisign.key`. The release workflow's `sign` job then
signs every `.deb`, writes `pixelplus-stable.json` / `pixelplus-beta.json`
(`packaging/release-index.py`), signs them, checks every signature against the committed
public key, and attaches everything to the (draft) release; publishing the release runs
`ota-publish.yml`, which copies the indexes to the `gh-pages` branch under `ota/` (enable
GitHub Pages for that branch; controllers read `https://<owner>.github.io/PixelPlus/ota`,
overridable with `PIXELPLUS_UPDATE_URL`). Tags like `v1.3.0-beta1` become `1.3.0~beta1` and
go to the beta channel only.

**Rotating the key:** add the new public key as a second line, ship a release signed with the
old key (controllers now trust both), switch the secrets to the new key, and remove the old
line a release later. **A leaked key:** remove it from the file and ship a release signed with
a new key through apt / a new image; controllers that still trust the old key can be offered
packages signed with it until they update. Signing by hand:
`minisign -S -s minisign.key -m pixelplus_1.2.3_arm64.deb` (creates `.minisig`).

## SD-card image (pi-gen)

```sh
packaging/build-deb.sh --arch arm64
image/build-in-docker.sh --deb dist/pixelplus_<ver>_arm64.deb                      # Trixie
image/build-in-docker.sh --deb dist/pixelplus_<ver>_arm64.deb --release bookworm
```

`image/build.sh` clones [pi-gen](https://github.com/RPi-Distro/pi-gen) at a **pinned tag**
(`2026-09-15-raspios-trixie-arm64` / `…-bookworm-arm64`), copies `image/stage-pixelplus`
into it, writes the pi-gen `config`, and builds `stage0 stage1 stage2 stage-pixelplus`
(= Raspberry Pi OS Lite + PixelPlus; stage2's own export is skipped). `--docker` (what
`build-in-docker.sh` does) runs pi-gen's `build-docker.sh`, which needs a **privileged**
container (loop devices, binfmt_misc); on x86 hosts qemu-user-static must be installed.
The release workflow builds on GitHub's native `ubuntu-24.04-arm` runners. Other options:
`--no-tts`, `--no-tts-models`, `--version`, `--out`, `--continue`, `--pigen-ref`.

Output in `image/deploy/`: `pixelplus-<ver>-<release>-arm64.img.xz`, `.sha256`, `.info`,
and `os-list-<release>.json` (Raspberry Pi Imager fragment). The release workflow merges
the fragments into `pixelplus-imager.json`:

```sh
python3 image/rpi-imager/make-os-list.py --merge image/deploy/os-list-*.json --out pixelplus-imager.json
```

### What the stage does

* **00-packages**: NetworkManager, dnsmasq-base, nftables, iw, rfkill, avahi, alsa-utils,
  ffmpeg, i2c-tools, raspi-utils (`pinctrl`), python3-venv/numpy/qrcode, libretro-nestopia,
  polkitd, logrotate.
* **01-pixelplus**: creates `/etc/pixelplus/appliance` (marker that enables the first-boot
  and hotspot services), installs the `.deb`.
* **02-system**: `config.txt` base block (i2c on, UART off, no splash, camera/display
  auto-detect off, **`include pixelplus.conf`** at the end; `vc4-kms-v3d` stays on),
  `cpufreq.default_governor=performance` on the kernel command line (build time only),
  `pixelplus.txt` template and an empty `pixelplus.conf` on the boot partition,
  `i2c-dev` module, volatile journald (32 MB), zram-only swap (`rpi-swap` on Trixie,
  `zram-tools` on Bookworm), Wi-Fi radio enabled, apt timers / wait-online /
  `raspi-config.service` disabled, console banner with the URL, user `pi` **locked**.
* **03-tts** (optional): `/opt/pixelplus-tts/venv` from `tts/` (binary wheels only) and
  the int8 Kokoro model in `/var/lib/pixelplus/tts/models`.

Image defaults: hostname `pixelplus`, locale `en_US.UTF-8`, time zone UTC, SSH off, user
`pi` with a random locked password (unlocked by `ssh_password=`/`ssh_key=` or replaced by a
Raspberry Pi Imager user), cloud-init kept on Trixie.

### Board boot configuration

The Pi's firmware reads `config.txt` → `include pixelplus.conf`. `pixelplus.conf` holds
the output of `pixelplus config-txt --board <id> [--pixels N]` (DPI overlay with the
right geometry, `gpio=…=op,dl`, RTC overlay, Pi 5 USB current…). It is written:

1. on first boot by `pixelplus-firstboot` after `pixelplus --json detect` identified the
   board (or `board=` in `pixelplus.txt`), followed by **one automatic reboot** (loop
   guard: at most 3 consecutive reboots). It is regenerated at boot only if missing, if the
   board changed, or if the card moved to a different Pi model;
2. whenever the daemon needs a different geometry (longest string) or the user picks a
   board in the wizard: `pixelplus-helper@config-txt:<board>:<pixels>`, then a user-confirmed
   reboot.

Older images that wrote a managed block into `config.txt` are migrated automatically.

### First boot / `pixelplus.txt` (image/firstboot)

`pixelplus-firstboot.service` (every boot, before `pixelplusd`, after NetworkManager and
cloud-init's local/network stages) and `pixelplus-reapply.path` (when the file is saved
while running) run `firstboot.py apply`:

1. Parse + validate (`pptxt.py`); problems go to `pixelplus-errors.txt` on the boot partition.
2. If the file's SHA-256 changed since the last run: apply only changed values –
   country (`raspi-config nonint do_wifi_country`), hostname (`hostnamectl`, `/etc/hosts`,
   cloud-init `preserve_hostname`), time zone, Wi-Fi profiles as NetworkManager keyfiles
   (`pixelplus-wifi`, `pixelplus-wifi2`, `pixelplus-ethernet`; secrets never on a command
   line), SSH (`raspi-config nonint do_ssh`), login password/key for uid 1000.
3. Role, UI password and board go to **`/var/lib/pixelplus/provision.json`** (0600,
   owner `pixelplus`) for the daemon; hotspot settings to `/etc/pixelplus/netwatch.json`.
4. Applied secrets are blanked in `pixelplus.txt` (comment `# [applied <date>] …`), and
   the new digest is stored in `/var/lib/pixelplus-system/state.json`.
5. Wi-Fi radio unblocked; board boot configuration as above.

Logs: `/var/log/pixelplus-firstboot.log` and `journalctl -u pixelplus-firstboot`.
Validate a file on your PC: `python3 image/firstboot/firstboot.py check pixelplus.txt`.

**Raspberry Pi Imager compatibility.** Current Raspberry Pi OS uses two customisation
formats: Bookworm images use `firstrun.sh` (Imager adds
`systemd.run=/boot/firmware/firstrun.sh … systemd.unit=kernel-command-line.target` to
`cmdline.txt`; the script runs in its own boot and reboots), Trixie images use cloud-init
(`user-data`, `network-config`, `meta-data` on the boot partition, `init_format:
cloudinit-rpi`). PixelPlus keeps both mechanisms intact: it never edits `cmdline.txt` at
runtime, keeps cloud-init and `userconf-pi`, orders its first-boot unit after them, and
treats empty `pixelplus.txt` values as "leave alone", so Imager's hostname, Wi-Fi (a
netplan-generated NetworkManager profile, which the hotspot logic counts as a known
network), user and SSH survive. The os_list entries advertise `cloudinit-rpi` (Trixie)
and `systemd` (Bookworm) so Imager uses the right format.

### Setup hotspot (image/netwatch)

`pixelplus-netwatch.service` (root, standalone Python, stdlib only):

* **Boot:** online (any Ethernet/Wi-Fi device connected) → nothing to do. Otherwise, after
  `hotspot_timeout` (75 s; 25 s when no Wi-Fi profile exists at all) and with no Ethernet
  → hotspot. **Runtime:** offline for 5 min → hotspot, but at least 10 min when a network was
  connected within the last 30 min (a deauthentication attack has to last that long).
* **Hotspot:** NetworkManager AP profile `pixelplus-hotspot`, SSID `PixelPlus-XXXX` (last 4
  hex digits of the Wi-Fi MAC), WPA2 `pixelplus` by default (or open) until the controller has
  been online once; after that `hotspot_password=` if the owner set one, else a random
  per-device password (`/var/lib/pixelplus-system/netwatch-state.json`, root 0600) written to
  `PIXELPLUS-HOTSPOT.txt` on the boot partition and shown to the signed-in owner under
  Settings → Network (`netwatch.json` is 0640 root:pixelplus) — never open. 2.4 GHz channel 6,
  `ipv4.method=shared` at 10.42.0.1/24. NM's dnsmasq gets
  `/etc/NetworkManager/dnsmasq-shared.d/pixelplus-portal.conf`: every DNS name → 10.42.0.1
  and DHCP option 114 (RFC 8910 captive-portal URL). An nftables table redirects TCP 80
  from the hotspot interface to the portal on 10.42.0.1:8099 and rejects 443 (so phones
  fall back to HTTP probes quickly); pixelplusd keeps port 80 everywhere else.
* **Portal** (`portal.py` + `portal/index.html`, 10 s socket timeout, bounded threads): Apple/Android/Windows/Firefox probe URLs
  and foreign hosts get a 302 to `http://10.42.0.1/`; API `GET /api/status`,
  `GET /api/scan[?rescan=1]` (scan cached before the AP starts – most Pi radios can't scan
  in AP mode), `POST /api/connect {ssid,password,hidden,country}`, `POST /api/stay`.
* **Connect:** reply first, then drop the AP, write profile `pixelplus-portal-<ssid>`
  (WPA3-only networks get `key-mgmt=sae`), `nmcli connection up` (45 s). Failure →
  profile removed, hotspot back with the error shown on the page.
* **Retry:** every 5 min while no phone is associated, stop the AP for ~45 s to let
  NetworkManager rejoin known networks (router came back after a power cut).
* **Ethernet** plugged in → hotspot off.
* Status for the daemon/UI: `/run/pixelplus/netwatch.json`
  (`{state, hotspotSsid, hotspotSecured, portalUrl, lastError, lastJoined, updatedAt}`).

Try the portal page on your PC with fake data:

```sh
python3 image/netwatch/netwatch.py --portal-only 127.0.0.1:8099   # open http://127.0.0.1:8099
```

### Signed-update rollback copy

The stage also copies the installed package (and its `.minisig`, if `image/build.sh` was
given a signed `.deb`) to `/var/cache/pixelplus/rollback/`, so the first signed update can go
back to the image's version without apt or `dpkg-repack`.

### Testing

```sh
python3 -m pytest image/tests          # parser, scrubbing, keyfiles, firstboot (sandboxed), portal, state machine, os_list
RPI_IMAGER_SCHEMA=/path/to/rpi-imager/doc/json-schema/os-list-schema.json python3 -m pytest image/tests/test_os_list.py
shellcheck image/*.sh image/stage-pixelplus/*/*.sh packaging/*.sh packaging/bin/*
node packaging/tests/polkit-rules.test.js
python3 -m pytest packaging/tests    # root helper: verbs, argument checks, symlink safety,
                                     # signed updates (fake dpkg/minisign), tunnels; release index
systemd-analyze verify packaging/systemd/*
```

Nothing in `image/` can boot a real Pi in CI. Before a release, flash the image and check:
first boot with only `pixelplus.txt` Wi-Fi; with Raspberry Pi Imager 2 customisation
(Trixie and Bookworm images); no network → hotspot → portal on iPhone and Android →
join; wrong password → hotspot returns with the error; Ethernet only; board detection +
single reboot on each board; `pixelplus.txt` edit while running.

## Docker image

```sh
docker build -f docker/Dockerfile -t pixelplus .                          # full: + TTS venv + games
docker build -f docker/Dockerfile --build-arg VARIANT=slim -t pixelplus:slim .
docker compose -f docker/docker-compose.yml up -d
```

Multi-stage: `node:22` builds `web/`, `rust:1-bookworm` builds `pixelplusd` + `pixelplus`,
the runtime is `debian:bookworm-slim` with ffmpeg/alsa, user `pixelplus` (uid 1000,
`cap_net_bind_service` file capability for port 80), `tini` as PID 1,
`PIXELPLUS_OUTPUT=none`, `PIXELPLUS_BOARD=virtual`, `PIXELPLUS_AUDIO=none` (set `auto` when
passing `/dev/snd`), volume `/var/lib/pixelplus`, and a healthcheck on the unauthenticated
`GET /api/v1/public/health` (`{"ok", "version", "role"}`); the build stage installs
`libasound2-dev`. The compose file uses host networking
(UDP broadcast + mDNS) and runs TTS/games as optional profiles from the same image
(`games` shares the daemon's IPC namespace for the `/dev/shm` overlay buffers).
`docker/Dockerfile.dockerignore` keeps the build context small. Multi-arch
(amd64 + arm64) images are pushed to `ghcr.io/<owner>/pixelplus` by the release workflow.

## PixelPlus Imager

```sh
cd imager
pnpm install
pnpm dev                    # UI only, in a browser, with a mock backend (nothing is written)
pnpm tauri dev              # the real app
pnpm tauri build            # installers for this OS (src-tauri/target/release/bundle/)
cargo test -p pixelplus-imager-core     # core logic, no GUI deps
```

* `imager/core` (`pixelplus-imager-core`, no Tauri): settings validation + rendering into
  the image's own `pixelplus.txt` (golden file shared with the Python parser:
  `image/tests/fixtures/imager-rendered.txt`; regenerate with `UPDATE_GOLDEN=1`),
  MBR + FAT32 access via the `fatfs` crate (no mounting), streaming `.img.xz`
  decompression (`lzma-rs`, pure Rust), SHA-256 read-back verification, safe drive
  listing (Linux `lsblk`, macOS `diskutil`, Windows `Get-Disk`; never system/boot disks,
  never >1 TB unless `PIXELPLUS_IMAGER_ALLOW_LARGE=1`), per-OS raw device access, GitHub
  release + Raspberry Pi Imager JSON parsing. It also builds `pixelplus-imager-cli`
  (`list`, `customize IMAGE.img SETTINGS.json`, `write JOB.json`).
* **Order of operations:** stream-write the image → flush caches and verify by reading back
  → write `pixelplus.txt` into the FAT boot partition *on the card* through the same raw
  handle (sector-aligned adapter) → sync/eject. (Injecting after verification keeps the
  verification comparable to the published image checksum.)
* **Privileges:** the app re-runs itself as a helper (`pixelplus-imager --helper write
  JOB.json --progress FILE`) with admin rights: Linux `pkexec` (the AppImage file itself
  via `$APPIMAGE`, since root cannot read the user's FUSE mount; the .deb/.rpm ship a
  polkit action for a clear prompt), macOS `osascript … with administrator privileges`
  (writes `/dev/rdiskN`), Windows `requireAdministrator` manifest (the app starts elevated;
  volumes are locked and dismounted before `\\.\PhysicalDriveN` is written). The job file
  (contains the Wi-Fi password) is 0600 in a private temp dir and deleted by the helper as
  soon as it is read; progress is a JSON-lines file; cancel = create `<progress>.cancel`.
* Releases: `tauri-apps/tauri-action` builds Windows (.msi/NSIS), macOS (universal .dmg)
  and Linux (.AppImage/.deb/.rpm). Unsigned builds work but trigger SmartScreen /
  Gatekeeper warnings; add Apple/Windows signing secrets to the release workflow for
  public releases.

## CI

`.github/workflows/ci.yml`: Rust fmt/clippy/test + aarch64 cross build; web check/test/build;
Python tests for `image/`, `games/`, `tts/`; shellcheck, `systemd-analyze verify`, polkit
rule test, overlay compile; Imager core tests on Linux/macOS/Windows and the Tauri app
check; Docker build.

`.github/workflows/release.yml` (tag `v*`): `.deb` for amd64/arm64, SD images (Trixie,
Bookworm) on native arm64 runners, `pixelplus-imager.json`, multi-arch Docker image to
GHCR, Imager installers, the minisign signatures and signed update indexes (job `sign`, see
"Signed updates"), all attached to a draft GitHub release. `.github/workflows/ota-publish.yml`
copies the update indexes to GitHub Pages when the release is published.
