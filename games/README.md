# PixelPlus Games

Let people walking by your Christmas display play **Super Mario Bros. on your pixel matrix**,
using their phone as the controller. A sidecar service for `pixelplusd`, ported from
[tlchandler/fpp-mariobros](https://github.com/tlchandler/fpp-mariobros).

* During the show, the matrix flashes your URL (text and/or a QR code) every few minutes.
* A visitor opens it and gets an NES-style gamepad that says **PRESS START TO BEGIN**.
* On START, the show pauses, a **random level** plays on the matrix for **60 seconds** with the
  game's **music and sound effects on your show audio**, then the show resumes exactly where it was.
* A **cooldown** then keeps the matrix on the show for a while: no invites, no new games.
* Mario wears a **Santa hat**.
* Optional **Arcade mode**: a full-time NES emulator. The matrix lists every game you uploaded
  (with a scroll bar), visitors take turns picking and playing any of them.

Everything is configured in the PixelPlus web UI: **Settings → Games**.

## What you need

* PixelPlus on the leader that drives the matrix.
* A **matrix prop** (Props → a prop of kind *matrix* with its grid). xLights imports fill in the
  grid automatically; it can also be edited in the prop drawer.
* Your own backup of **Super Mario Bros.** as a `.nes` file (not included, and never sent to
  visitors — the game runs on the Pi; phones only send button presses). Arcade mode lists every
  `.nes` file you upload.
* A way for phones on the street to reach the controller page (see below).

## Install

PixelPlus images ship with the games service installed. To add it by hand (Raspberry Pi OS /
Debian), from a checkout of the repository:

```sh
sudo games/install.sh
```

It installs `libretro-nestopia` (the NES emulator), `python3-numpy`, `python3-qrcode` and
`alsa-utils` with apt, copies the program to `/usr/lib/pixelplus/games`, creates
`/var/lib/pixelplus/games/roms/` and enables `pixelplus-games.service`
(`--help` lists the options). Logs: `journalctl -u pixelplus-games -f`.

The service runs as the unprivileged `pixelplus` user (the same user as `pixelplusd`, member
of `audio` for game sound), with the unit the `pixelplus` package ships
(`packaging/systemd/pixelplus-games.service`, identical to `games/pixelplus-games.service`).
It writes frames into the prop's overlay buffer `/dev/shm/pixelplus-overlay-<prop>` (created
by pixelplusd, mode 0660) and listens on the control socket `/run/pixelplus/games.sock`
(`/run/pixelplus` is created by tmpfiles.d, owned by `pixelplus`).

Then open **Settings → Games** in PixelPlus:

1. Upload your Super Mario Bros. ROM (stored as `/var/lib/pixelplus/games/roms/smb.nes`) and,
   for Arcade mode, any other `.nes` games (stored next to it).
2. Pick your **matrix prop** and press **Test pattern**: you should see a blue border, a red
   top-left corner, a green top-right corner and `MARIO 80X40` centred. If the corners are
   swapped, fix the prop's matrix orientation (start corner / direction) in the Props page.
3. Turn **Games** on, then open `http://<pi-address>:8088` on your phone (same Wi-Fi) and play a
   round.
4. Set the **Public URL** and **Show the invite every** (minutes; 0 = only when a playlist runs
   the `games.invite` command).

### Letting phones on the street reach it

Visitors are on mobile data, so the page must be reachable from the internet. Expose **only the
controller port** (default 8088), never PixelPlus's own web interface on port 80. The controller
page has no login by design: anyone who can reach it can queue for a turn, and that is all it can
do.

* **Cloudflare Tunnel** (recommended, free, no router changes, works with WebSockets and gives you
  HTTPS): install `cloudflared` on the Pi or another machine on your network and route a
  hostname such as `mario.yourdomain.com` to `http://<pi-address>:8088`. This is a third-party
  service you set up yourself; PixelPlus does not install it.
* **Port forward** on your router from an external port to `<pi-address>:8088`, plus a dynamic DNS
  name. Simpler, but plain HTTP and it puts your home IP in the URL.
* **Guest Wi-Fi** near the display (with the URL as `http://<pi-address>:8088`) if you'd rather
  not touch the internet at all.

What the controller port does to stay up with the whole internet able to reach it: only `/`,
`/ws` and `/healthz` exist; WebSocket messages are capped at 4 KiB and parsed defensively; a phone
may send about 60 messages a second (bursts to 120) and is disconnected if it keeps flooding; a
connection that stays silent for 2 minutes is closed (the page pings every 20 s); and one address
may hold at most 16 connections (300 in all). Behind Cloudflare Tunnel or another reverse proxy on
the Pi or the LAN, the visitor's address is taken from `CF-Connecting-IP` / `X-Forwarded-For`;
those headers are ignored on connections from the internet.

Short URLs read best on an 80×40 matrix (about 20 characters fit; longer ones scroll).

## Settings (Settings → Games)

Stored in the show as `settings.games` (`GameSettings` in `pixelplus-core`); changes apply live.

| Setting (`show.json` key) | Default | Notes |
|---|---|---|
| Games (`enabled`) | off | Master switch. Opens the controller port; off closes it and the arcade. |
| Controller port (`port`) | 8088 | |
| Matrix (`matrixPropId`) | — | Any prop with a matrix grid. If unset and the show has exactly one matrix prop, that one is used. |
| Game length (`gameSeconds`) | 60 s | 10–600. |
| Cooldown after a game (`cooldownMinutes`) | 5 min | No invites and no games during it; phones count down. |
| Levels (`levels`) | all | Or e.g. `1-1,1-2,4-1`; one is picked at random. |
| Games allowed (`playWindow`) | during the show | `duringShow`: while a show plays, is paused, or inside a schedule window. Or `anytime`. |
| Pause the show during a game (`pauseShow`) | on | Pauses the player and resumes it afterwards. |
| Santa hat on Mario (`santaHat`) | on | |
| Arcade mode (`arcadeMode`) | off | See below. |
| Turn length (`arcadeMinutes`) | 0 (unlimited) | Minutes; 0 = until the player leaves or goes idle. |
| End a turn after idle (`arcadeIdleSeconds`) | 600 s | 0 = never. |
| Public URL (`publicUrl`) | — | What the invite shows. |
| Show the invite every (`inviteEveryMinutes`) | 5 min | 0 = only via the playlist command. |
| Invite style (`inviteStyle`) | text | `text`, `qr`, or `alternate`. |
| Flashes each time (`inviteFlashes`) | 3 | |
| URL text colour (`inviteColor`) | red | |
| Picture fit (`scaleMode`) | fit | `fit` keeps Mario's proportions (black bars left/right); `stretch` fills. |
| Matrix frame rate (`outputFps`) | 40 fps | 20 or 40. The game itself always runs at 60. |
| Brightness / game volume (`brightness`, `volume`) | 100% / 80% | Game volume scales the game's own sound. |
| Crop (`crop`) | 8, 32, 256, 224 | Part of the NES screen shown in Mario mode (drops the score bar). |

The game's audio goes to the show's audio device (`settings.audio.device`). Use a device that
can be shared (`default`, a `dmix` or PipeWire device) if pixelplusd keeps the card open while
the show is paused.

A queued visitor has 20 s to press START when their turn comes (`turnTimeoutSeconds`, if the show
ever carries it). For unusual setups, environment variables in the service override the rest:
`PIXELPLUS_API` (default `http://127.0.0.1`), `PIXELPLUS_DATA_DIR`, `PIXELPLUS_GAMES_SOCKET`,
`PIXELPLUS_NES_CORE` (a specific libretro core), `PIXELPLUS_NES_CORE_OPTIONS`
(`key=value;key=value`), `PIXELPLUS_GAMES_DEBUG=1`.

### Playlist, schedule and trigger commands

* **`games.invite`** (`args: {flashes?, style?}`): flash the URL / QR now. Ignored during a game,
  the arcade or the cooldown.
* **`games.stop`**: end the current game or arcade turn.

## How a turn works

1. The phone connects over a WebSocket and shows **PRESS START TO BEGIN**.
2. START: if nobody is playing the visitor plays now, otherwise they join a line and their phone
   shows their place. When it's their turn their phone buzzes and they have 20 s to press START.
3. The show is paused, the overlay is switched on for the matrix prop, `WORLD 4-2` shows on the
   matrix while the emulator boots straight into that level (the level is chosen by writing the
   game's World/Level/Area bytes at the title menu, exactly like the game's own continue feature).
4. The game runs at the NES's 60 fps; the matrix gets 20 or 40 of those frames per second. START
   and SELECT are disabled so nobody can pause the matrix, lives are topped up, and the last ten
   seconds count down in the margin.
5. `TIME UP` and the score show, the overlay is switched off, the show resumes where it stopped
   (unless someone stopped it meanwhile), and the cooldown starts. If the player closes the page,
   their game ends after 10 s.

### The phone controller

* The pad fills the phone's screen, with the D-pad against the left edge and B/A against the right,
  where thumbs rest. Sideways gives the biggest buttons; upright works too.
* **Full screen** (top-right corner): on Android and desktop browsers it hides the browser bars and,
  on Android, locks the phone to landscape. iPhone Safari can't make a web page full screen, so on an
  iPhone the button explains **Share → Add to Home Screen** instead: opened from that icon, the
  controller fills the whole screen.
* One controller per phone: if the same browser opens the page a second time (another tab, or the
  home-screen icon), the older copy says so and waits for a tap instead of fighting over the turn.
* A keyboard works too: arrows/WASD, X/K = A, Z/J = B, Enter = START, right Shift = SELECT.

## Arcade mode

Turn on **Arcade mode** (with Games on) and the Pi becomes a full-time NES arcade:

* A playing show is stopped, and kept stopped: if the scheduler starts it, it is stopped again
  within a few seconds. Turn Arcade mode off to give the matrix back, then start your show as usual
  (it is not restarted automatically).
* The matrix shows **PICK A GAME** and the list of uploaded games, with a highlight bar, a scroll
  bar and scrolling for long names. Up/Down move (hold to repeat), Left/Right page, A or START plays.
* **Hold SELECT + START + B + A for 2 seconds** to leave a game and go back to the list.
* Turns (length and idle timeout configurable) and the line work as in Mario mode. Invites are off.

## Performance

The emulator has to finish every frame in under 16.7 ms. The service does what it can: the matrix
only gets 20 or 40 frames a second, frames that won't be shown aren't copied, and scaling is a
single precomputed gather. Measure on the Pi itself, ideally while the show is running:

```sh
cd /usr/lib/pixelplus/games
python3 -m pixelplus_games.benchmark /var/lib/pixelplus/games/roms/smb.nes 80x40
```

On a 2.1 GHz Xeon it reports ~0.4 ms to emulate and ~0.3 ms to scale a frame (≈3% of a core).
A Pi 4 is comfortable; on slower boards set the matrix frame rate to 20. The show is paused during
a game, which frees up CPU.

## How it talks to PixelPlus

The service runs next to pixelplusd and talks to it over loopback. Every request carries
`X-PixelPlus-Local: 1`, which pixelplusd accepts from 127.0.0.1 / ::1 without a session.

| Call | Use |
|---|---|
| `GET /api/v1/show` | `settings.games`, `settings.audio.device` and the matrix prop's `matrix: {width, height, pixelMap}`. Refetched on every `{type:"show"}` message from `/api/v1/ws` (every 5 s if the WebSocket is down, every 60 s regardless). |
| `GET /api/v1/player` | `state` (and `scheduleEntry`) when no `status` message arrived for 5 s. |
| `POST /api/v1/player/pause`, `/player/resume`, `/player/stop` | Pause/resume around a Mario game; stop (and keep stopped) for the arcade. |
| `POST /api/v1/overlay/:propId/open` → `{shm, width, height}` | Shared-memory frame buffer: 12-byte header (width, height, flags as native-endian u32) + RGB row-major from the top-left. Each frame sets flags bit 0. |
| `PUT /api/v1/overlay/:propId/frame` | Fallback when the shared memory can't be used: raw RGB, width×height×3 bytes. |
| `POST /api/v1/overlay/:propId` `{enabled}` | On while a game, invite or test pattern is shown; off afterwards (and at startup, in case a previous run died). |

pixelplusd reaches the service through its control socket, `/run/pixelplus/games.sock`: one
JSON object per line each way, one request per connection.

| Request | Reply |
|---|---|
| `{"cmd":"status"}` | `{ok, enabled, running, arcade, queueLength, cooldownS, model:{width,height}\|null, player?:{phase, level?, score?, remaining?, game?}, lastError?, propId, clients, port, showState, busy, unavailable}` |
| `{"cmd":"invite","flashes":3,"style":"qr"}` | `{ok}` or `{ok:false, error}`. `flashes`/`style` optional (settings otherwise); `style` is `text`, `qr` or `alternate`; `url` and `force` (ignore cooldown / games off) also accepted. |
| `{"cmd":"stop"}` | `{ok, stopped}` |
| `{"cmd":"test"}` | `{ok}` or `{ok:false, error}`: test pattern for 6 s. |
| `{"cmd":"reload"}` | `{ok}`: refetch the show now. |

From a shell: `python3 -m pixelplus_games.ctl status|invite [flashes] [style]|stop|test|reload`.

## Development

No Pi needed. The tests use `tests/fake_pixelplus.py`, a stand-in for pixelplusd's API (it also
creates the overlay shared memory and serves `/api/v1/ws`), and a tiny homebrew ROM built on the
fly (no commercial ROM). Emulator tests are skipped when no libretro NES core is installed.

```sh
python3 -m unittest discover -s games/tests       # from the repository root
```

To run the whole service against the fake:

```sh
python3 games/tests/fake_pixelplus.py 18080 80 40 &
cd games && PIXELPLUS_API=http://127.0.0.1:18080 PIXELPLUS_DATA_DIR=/tmp/pp \
    PIXELPLUS_GAMES_SOCKET=/tmp/pp/games.sock python3 -m pixelplus_games
# open http://localhost:8088 ; put a ROM at /tmp/pp/games/roms/smb.nes
```

## Files

```
pixelplus_games/     the service (Python 3, standard library + numpy)
  server.py          HTTP/WebSocket server, player line, cooldown, invites, arcade turns, control socket
  api.py             pixelplusd HTTP client
  events.py          pixelplusd WebSocket listener (live settings and show state)
  config.py          paths, and settings.games from the show
  game.py            game sessions: pause/resume the show, the real-time emulation loop
  libretro.py        minimal ctypes frontend for a libretro NES core
  smb.py             Super Mario Bros. RAM map and level select
  hat.py             the Santa hat
  arcade.py          arcade game list
  display.py         scaling and writing to the overlay shared memory
  invite.py          URL / QR invite and test pattern
  font.py            3x5 pixel font
  audio.py           game sound through aplay
  benchmark.py       performance check
  ctl.py             control socket client
  www/controller.html  the phone gamepad
install.sh, pixelplus-games.service, requirements.txt
tests/               unit and integration tests, fake pixelplusd
```

Nintendo, Super Mario Bros. and NES are trademarks of Nintendo. This project is not affiliated
with Nintendo and contains no Nintendo code or graphics.
