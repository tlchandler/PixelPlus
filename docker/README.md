# PixelPlus in Docker (show leader on a PC or NAS)

The Pis with PixelPlus boards become followers; the leader (show, music, schedule, web UI)
runs here. Linux host or NAS required - **host networking** is needed for follower
discovery (UDP broadcast 32420 + mDNS), which Docker Desktop on macOS/Windows cannot do.

```sh
docker compose -f docker/docker-compose.yml up -d                  # daemon + web UI on port 80
docker compose -f docker/docker-compose.yml --profile tts up -d    # + DJ voices (Kokoro TTS)
docker compose -f docker/docker-compose.yml --profile games up -d  # + games sidecar
```

| Setting | Default | |
|---|---|---|
| `PIXELPLUS_HTTP_PORT` | `80` | use e.g. `8080` if port 80 is taken (Synology DSM) |
| `PIXELPLUS_HTTPS_PORT` | `8443` | HTTPS for phone camera/mic pages (local CA); `0` = off |
| `TZ` | `Etc/UTC` | your time zone (schedules, sunset) |
| `PIXELPLUS_OUTPUT` | `none` | a PC has no pixel outputs |
| `PIXELPLUS_BOARD` | `virtual` | board override (any of `difftx`, `difftxlarge`, `diffsmart`, `bare-pi`, `virtual`, `auto`) |
| `PIXELPLUS_AUDIO` | `none` | `auto` once `/dev/snd` is passed through (see below) |

* Data: volume `pixelplus-data` → `/var/lib/pixelplus` (show, sequences, media, snapshots).
* Runs as uid 1000 (`pixelplus`); `chown -R 1000:1000` a bind-mounted folder.
* Sound: uncomment the `/dev/snd` lines in the compose file and set `PIXELPLUS_AUDIO=auto`.
* Health: `docker inspect --format '{{.State.Health.Status}}' pixelplus`. The check calls
  `GET /api/v1/public/health` (no sign-in needed; answers `{"ok": true, "version": ..., "role": ...}`),
  which monitoring tools can use too.
* Settings that belong to the host (Wi-Fi, hostname, time zone, reboot, updates, SSH) are
  greyed out in the web UI: change them on the host / by pulling a new image
  (`docker compose pull && docker compose up -d`; signed over-the-air updates are for the
  Pi images and packages).
* **Remote access** (song requests from the street, games, managing the show while away):
  the daemon serves a **public-only port** on `127.0.0.1:8081` (`PIXELPLUS_PUBLIC_PORT`;
  host networking makes it the host's loopback) with just the request page, its API and
  the games controller. Point a tunnel there, never at port 80:
  * Cloudflare Tunnel: create a tunnel in the Zero Trust dashboard, add a public hostname
    → `http://localhost:8081`, then `TUNNEL_TOKEN=… docker compose --profile tunnel up -d`.
  * Tailscale: install it on the host, then `tailscale funnel --bg --https=8443 http://127.0.0.1:8081`
    (public page) and, for managing the show from your own devices, `tailscale serve --bg
    --https=443 http://127.0.0.1:80` (tailnet only; set a PixelPlus password first).
  * Add the tunnel's host name under Settings → Security → Other names only if it serves the
    admin pages. PixelPlus refuses admin calls through a local proxy while no password is set.
* Build locally: `docker build -f docker/Dockerfile -t pixelplus .`
  (`--build-arg VARIANT=slim` leaves out TTS and games).

More: [docs/INSTALL.md](../docs/INSTALL.md#running-the-leader-in-docker),
[docs/BUILDING.md](../docs/BUILDING.md#docker-image).
