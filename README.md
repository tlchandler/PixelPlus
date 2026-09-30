# PixelPlus

**A modern show player for Raspberry Pi pixel controllers.** PixelPlus plays the
Christmas (or any) light shows you design in [xLights](https://github.com/xLightsSequencer/xLights),
on Raspberry Pi based differential pixel controllers — with an interface built around your
**props**, not channels and universes.

It is a from-scratch replacement for Falcon Player (FPP) for the PixelPlus / Chandler board
family (difftx pHAT, 60-port difftxlarge, diffsmart smart receiver), and also runs on a bare
Pi or in Docker as a show director.

---

## Why PixelPlus

| | |
|---|---|
| **Props, not universes** | Import your xLights layout and every prop shows up by name, wired to its controller, jack, receiver and port — in plain language ("Main Controller › J1 › Front Yard receiver › Port 2 › pixels 51–100"). You never type a start channel. |
| **Configure once, on the leader** | One Pi is the show leader. New controllers appear on their own; click **Adopt** and they receive their props, settings and their slice of every sequence automatically — and stay in sync to within a frame. |
| **Easy to change** | Every setting is edited in place and saved automatically, with Undo. Every change is versioned; **Backups** restore the whole show in one click. |
| **Setup that just works** | Put your Wi-Fi in `pixelplus.txt` on the SD card, use Raspberry Pi Imager's settings, or use the **PixelPlus Imager** app. No Wi-Fi? The Pi opens a `PixelPlus-XXXX` hotspot with a friendly Wi-Fi picker. |
| **Made for the yard at night** | Dark "studio" interface that works beautifully on a phone: big touch targets, live preview of your whole display, test and fault-find any prop from where you stand. |
| **No artificial limits** | Up to ~1600 pixels per output at 20 fps (800 at 40 fps) on every output, 60 outputs on the difftxlarge. |

## Features

- **Live dashboard** — now playing, next show countdown, pre-show health check, controller
  health, temperatures, 12 V voltage/current (difftxlarge), song requests.
- **Props** — grid and list views with live mini-previews, groups, bulk edit, drag-to-reorder
  daisy chains, reverse runs, dark spacer pixels, color order, brightness and color correction
  per port, power estimates against your receiver fuses.
- **Test & fault finder** — solid/chase/count/walk patterns on any prop or port; a guided
  "is it lit?" binary search finds the first bad pixel in a handful of taps.
- **Layout** — a live 2D picture of your whole display while the show plays.
- **Sequences & audio** — drag-and-drop upload, automatic song matching, volume leveling,
  thumbnails; plays xLights `.fseq` v1/v2 (zstd, zlib, sparse).
- **Playlists** — songs, DJ clips, looks, pauses and commands; intro/outro, shuffle, repeat,
  crossfades.
- **Schedule** — show windows by day and season, times relative to sunset, special nights
  that take over, idle looks between shows, a neighbor-friendly volume curfew.
- **DJ Studio** — radio-style announcements with Kokoro voices (Nick & Holly included, or
  blend your own), multi-voice scripts, hype levels, live info ("{days until Christmas}",
  "next up: {song}") re-rendered at showtime. Renders on a Pi 4/5 or right in your browser.
- **Effects** — 13 built-in effects and 15 ready-made looks for idle time and quick tests,
  identical on every controller.
- **Games** — visitors play Super Mario Bros. (or an NES arcade) on your pixel matrix using
  their phone as the controller, with QR invites flashed on the matrix.
- **Song requests** — a public page (with QR code and printable yard sign) where visitors pick
  the next song.
- **Alerts & integrations** — email and push (ntfy) alerts, MQTT with Home Assistant
  discovery, physical button triggers, OLED status screen.
- **Only what you use** — Settings → Features turns optional parts on and off (presets:
  Essentials, Everything, Custom), so menus and pages show just what this display uses.
  Nothing is deleted; turned-off features stop their background work and their playlist items
  are skipped.
- **Secure by default** — optional password, per-controller keys with signed cluster
  traffic, hardened against hostile uploads and cross-site attacks.

**New in this release**

- **Light shows from music** — beat, tempo and energy analysis builds a show for your props
  in one click, in six styles; preview any sequence on your phone without touching the lights.
- **Sync lights to sound** — your phone's camera and microphone measure the audio delay; a
  built-in certificate authority gives phones the HTTPS they need, with a one-page trust guide.
- **Countdown to showtime** — a matrix countdown with the first song starting exactly on the
  scheduled minute, on every controller.
- **Map my yard** — film the house with your phone and PixelPlus finds where each prop is and
  which runs are swapped or reversed; plus a pixel-count check and a guided receiver wizard.
- **Smart playlists** — tags, rules and nightly rotation pick tonight's songs without repeats.
- **Seasons** — keep Halloween and Christmas shows side by side and switch by date.
- **Power limiter** — keeps every fuse and supply within its rating on every controller, plus
  late-night dimming.
- **Nightly report** — how last night went, every morning, by email or push.
- **Upload from xLights** — FPP Connect compatible, or a drop folder.
- **Sensors and surprises** — ESP32 motion, button and beam sensors set off effects layered
  over the song that's playing.
- **Remote access** — Tailscale or Cloudflare Tunnel without port forwarding; only the request
  page and games go public, with per-visitor limits and an alert on every remote sign-in.
- **Fleet care** — signed updates for every controller at once with automatic rollback, and
  one-step replacement of a dead controller or leader from an encrypted transfer file.

## Getting started

1. **Make an SD card** — see [docs/INSTALL.md](docs/INSTALL.md) (PixelPlus Imager,
   Raspberry Pi Imager, or any writer + `pixelplus.txt`).
2. **Open** `http://pixelplus.local` (or the name you chose) and follow the setup wizard.
3. **Import** your xLights layout, **upload** sequences, **build** a playlist and **schedule** it.

Running the leader on a PC or NAS: see [docker/README.md](docker/README.md).

## Hardware

| Board | Outputs | Notes |
|---|---|---|
| **difftx** (PixelPlus pHAT) | 4 on one RJ45 | Pi Zero 2 W outline. Rev D needs a 4/5-swapped patch lead on port 3 (PixelPlus warns you). |
| **difftxlarge** | 60 on 15 RJ45 | 3 latch banks, INA226 power monitor, 2× LM75, DS3231 RTC, optional OLED. |
| **diffsmart** | 4 direct | Runs PixelPlus in standalone (PI) mode; set SW1 to **PI**. |
| **diffrx** | receiver | Passive 4-port differential receiver (no Pi). |

Supported Pis: Zero 2 W, 3, 4, 5 (64-bit Raspberry Pi OS Bookworm or Trixie). Pixel output
uses the Pi's DPI peripheral with an original WS281x encoder — see
[crates/pixelplus-output/DESIGN.md](crates/pixelplus-output/DESIGN.md), including a
**bring-up checklist** for first hardware tests.

## Repository layout

```
crates/pixelplus-core     show model, fseq, xLights import, mapping, effects, schedule
crates/pixelplus-output   DPI WS281x encoder (direct + latched), output backends
crates/pixelplus-hw       board detection, EEPROM, sensors, RTC, OLED, GPIO
crates/pixelplus-daemon   pixelplusd: API, playback engine, scheduler, cluster, services
crates/pixelplus-cli      pixelplus: detect, eeprom, test-output, config-txt, doctor
web/                      SvelteKit web interface (demo mode: add ?mock=1)
tts/                      Kokoro DJ voice service
games/                    phone-controlled matrix games
image/ packaging/ docker/ Raspberry Pi image, Debian package, Docker
imager/                   PixelPlus Imager desktop app
docs/                     ARCHITECTURE, INSTALL, BUILDING
scripts/                  dev-cluster.sh (local leader + 2 followers), e2e tests
```

## Development

See [docs/BUILDING.md](docs/BUILDING.md). Quick start:

```sh
cargo test --workspace                  # Rust
cd web && pnpm install && pnpm dev      # UI (demo mode when no daemon is running)
scripts/dev-cluster.sh start --fresh    # a real leader + 2 followers on your PC
node scripts/e2e/run.mjs                # the full end-to-end scenario
```

The design authority is [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) (multi-controller timing:
§7.4); board-level timing notes for future hardware revisions are in
[docs/HARDWARE-NOTES.md](docs/HARDWARE-NOTES.md).

## License

PixelPlus is free software under the [GNU General Public License v3.0 or later](LICENSE).
It contains no code from FPP's CC-BY-ND licensed components.
Nintendo and Super Mario Bros. are trademarks of Nintendo; no Nintendo code or ROMs are included.
