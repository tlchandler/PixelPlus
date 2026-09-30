# PixelPlus in Docker (show leader on a PC or NAS)

The Pis with PixelPlus boards become followers; the leader (show, music, schedule, web UI)
runs here. Linux host or NAS required - **host networking** is needed for follower
discovery (UDP broadcast 32320 + mDNS), which Docker Desktop on macOS/Windows cannot do.

```sh
docker compose -f docker/docker-compose.yml up -d                  # daemon + web UI on port 80
docker compose -f docker/docker-compose.yml --profile tts up -d    # + DJ voices (Kokoro TTS)
docker compose -f docker/docker-compose.yml --profile games up -d  # + games sidecar
```

| Setting | Default | |
|---|---|---|
| `PIXELPLUS_HTTP_PORT` | `80` | use e.g. `8080` if port 80 is taken (Synology DSM) |
| `TZ` | `Etc/UTC` | your time zone (schedules, sunset) |
| `PIXELPLUS_OUTPUT` | `none` | a PC has no pixel outputs |
| `PIXELPLUS_BOARD` | `virtual` | |

* Data: volume `pixelplus-data` → `/var/lib/pixelplus` (show, sequences, media, snapshots).
* Runs as uid 1000 (`pixelplus`); `chown -R 1000:1000` a bind-mounted folder.
* Sound: uncomment the `/dev/snd` lines in the compose file.
* Health: `docker inspect --format '{{.State.Health.Status}}' pixelplus`.
* Build locally: `docker build -f docker/Dockerfile -t pixelplus .`
  (`--build-arg VARIANT=slim` leaves out TTS and games).

More: [docs/INSTALL.md](../docs/INSTALL.md#running-the-leader-in-docker),
[docs/BUILDING.md](../docs/BUILDING.md#docker-image).
