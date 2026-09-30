# syntax=docker/dockerfile:1.7
#
# PixelPlus leader for a PC / NAS (no pixel outputs of its own: board "virtual").
# Build from the repository root:
#
#   docker build -f docker/Dockerfile -t pixelplus .                  # full (with TTS + games)
#   docker build -f docker/Dockerfile --build-arg VARIANT=slim -t pixelplus:slim .
#
# Run: see docker/docker-compose.yml (host networking is required for UDP broadcast
# discovery on 32420 and mDNS).

ARG DEBIAN=bookworm
ARG VARIANT=full

# ---------------------------------------------------------------------------
# web UI (SvelteKit static build -> web/build)
# ---------------------------------------------------------------------------
FROM --platform=$BUILDPLATFORM node:22-${DEBIAN}-slim AS web
WORKDIR /src/web
RUN corepack enable
COPY web/package.json web/pnpm-lock.yaml ./
RUN --mount=type=cache,id=pnpm-store,target=/root/.local/share/pnpm/store \
    pnpm install --frozen-lockfile
COPY web/ ./
RUN pnpm build && test -f build/index.html

# ---------------------------------------------------------------------------
# Rust daemon + CLI
# ---------------------------------------------------------------------------
FROM rust:1-${DEBIAN} AS rust
ARG DEBIAN_FRONTEND=noninteractive
# libasound2-dev: ALSA headers for the daemon's audio output (cpal, feature "audio").
RUN apt-get update \
 && apt-get install -y --no-install-recommends pkg-config libasound2-dev \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
RUN --mount=type=cache,id=cargo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=pixelplus-target,target=/src/target \
    cargo build --release --locked -p pixelplus-daemon -p pixelplus-cli \
 && install -D -m 0755 target/release/pixelplusd /out/pixelplusd \
 && install -D -m 0755 target/release/pixelplus /out/pixelplus

# ---------------------------------------------------------------------------
# runtime base
# ---------------------------------------------------------------------------
FROM debian:${DEBIAN}-slim AS base
ARG DEBIAN_FRONTEND=noninteractive
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
        ca-certificates curl tini ffmpeg alsa-utils libasound2 libcap2-bin tzdata \
 && rm -rf /var/lib/apt/lists/*
RUN groupadd --system --gid 1000 pixelplus \
 && useradd --system --uid 1000 --gid pixelplus --groups audio \
        --home-dir /var/lib/pixelplus --shell /usr/sbin/nologin pixelplus \
 && install -d -o pixelplus -g pixelplus -m 0750 /var/lib/pixelplus \
 && install -d -o pixelplus -g pixelplus -m 0775 /run/pixelplus
COPY --from=rust /out/pixelplusd /out/pixelplus /usr/bin/
COPY --from=web /src/web/build/ /usr/share/pixelplus/web/
# Bind port 80 as the unprivileged user.
RUN setcap cap_net_bind_service=+ep /usr/bin/pixelplusd
ENV PIXELPLUS_DATA_DIR=/var/lib/pixelplus \
    PIXELPLUS_WEB_DIR=/usr/share/pixelplus/web \
    PIXELPLUS_HTTP_PORT=80 \
    PIXELPLUS_HTTPS_PORT=8443 \
    PIXELPLUS_OUTPUT=none \
    PIXELPLUS_BOARD=virtual \
    PIXELPLUS_AUDIO=none \
    PIXELPLUS_TTS_URL=http://127.0.0.1:7081 \
    PIXELPLUS_GAMES_SOCKET=/run/pixelplus/games.sock \
    TZ=Etc/UTC
VOLUME ["/var/lib/pixelplus"]
EXPOSE 80/tcp 8443/tcp 32420/udp 32421/udp 32422/udp 7081/tcp 8088/tcp
# Unauthenticated and cheap: {"ok": true, "version": "...", "role": "..."}
HEALTHCHECK --interval=30s --timeout=5s --start-period=30s --retries=3 \
    CMD curl -fsS -o /dev/null "http://127.0.0.1:${PIXELPLUS_HTTP_PORT}/api/v1/public/health" || exit 1
USER pixelplus
WORKDIR /var/lib/pixelplus
ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["/usr/bin/pixelplusd"]

# ---------------------------------------------------------------------------
# slim: daemon + web only (DJ clips are rendered in the browser)
# ---------------------------------------------------------------------------
FROM base AS runtime-slim

# ---------------------------------------------------------------------------
# full: + Kokoro TTS sidecar venv and games sidecar
#   docker compose runs them as separate services from this same image
# ---------------------------------------------------------------------------
FROM base AS runtime-full
USER root
ARG DEBIAN_FRONTEND=noninteractive
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
        python3 python3-venv python3-numpy python3-qrcode libretro-nestopia \
 && rm -rf /var/lib/apt/lists/*
COPY tts/ /opt/pixelplus-tts/src/
RUN --mount=type=cache,id=pip,target=/root/.cache/pip \
    python3 -m venv /opt/pixelplus-tts/venv \
 && /opt/pixelplus-tts/venv/bin/pip install --prefer-binary --only-binary=:all: -r /opt/pixelplus-tts/src/requirements.txt \
 && { /opt/pixelplus-tts/venv/bin/pip install --only-binary=:all: "praat-parselmouth>=0.4.3" \
      || echo "no praat-parselmouth wheel for $(uname -m): built-in energy prosody"; } \
 && /opt/pixelplus-tts/venv/bin/pip install --no-deps /opt/pixelplus-tts/src \
 && rm -rf /opt/pixelplus-tts/src
COPY games/pixelplus_games/ /usr/lib/pixelplus/games/pixelplus_games/
ENV PIXELPLUS_TTS_HOST=127.0.0.1 PIXELPLUS_TTS_PORT=7081
USER pixelplus

FROM runtime-${VARIANT}
LABEL org.opencontainers.image.title="PixelPlus" \
      org.opencontainers.image.description="PixelPlus show leader (web UI, scheduler, cluster sync) for a PC or NAS" \
      org.opencontainers.image.source="https://github.com/tlchandler/PixelPlus" \
      org.opencontainers.image.licenses="GPL-3.0-or-later"
