# PixelPlus web UI

The PixelPlus admin app and the public song-request page. A SvelteKit (Svelte 5 runes) single-page
app, built with `@sveltejs/adapter-static` into `web/build/` and served by `pixelplusd` on port 80
(any unknown path falls back to `index.html`).

Everything is prop-oriented: the UI never shows universes or channels. See
[`docs/ARCHITECTURE.md`](../docs/ARCHITECTURE.md) §8 (API), §8.1 (WebSocket) and §9 (UI).

## Quick start

```sh
cd web
pnpm install
pnpm dev            # http://localhost:5173 — proxies /api to pixelplusd on :8080
pnpm dev:mock       # same, but always uses the in-browser demo backend
```

If `pixelplusd` isn't reachable during `pnpm dev`, the app falls back to the demo backend
automatically (a purple banner says so). Set `PIXELPLUS_API=http://pi.local` to proxy to a real
controller instead.

### Demo (mock) mode

Add `?mock=1` to any URL (remembered for the browser tab; `?mock=0` leaves it), or build with
`VITE_MOCK=1`. `?mock=1&setup=1` starts in the first-run wizard. The mock backend
(`src/lib/mock/`) implements the whole HTTP API and WebSocket in the browser with a realistic show:
a 60-port "Main Controller" leader, a rev D "Garage" follower, receivers, ~30 props including an
80×40 "Singing Matrix", 8 sequences, playlists, a sunset schedule, DJ voices Nick/Holly/Santa,
sensors, song requests, and live preview frames rendered from simple effects. It is lazily loaded,
so production bundles don't carry it unless demo mode is used.

## Scripts

| Script                               | What it does                                                                           |
| ------------------------------------ | -------------------------------------------------------------------------------------- |
| `pnpm build`                         | Production build into `build/`                                                         |
| `pnpm check`                         | `svelte-kit sync` + `svelte-check` (fails on warnings)                                 |
| `pnpm lint` / `pnpm format`          | Prettier + ESLint                                                                      |
| `pnpm test`                          | Vitest unit tests, then the Playwright smoke test                                      |
| `pnpm test:unit`                     | Vitest only                                                                            |
| `pnpm test:e2e`                      | Playwright only (builds first if `build/` is missing — run `pnpm build` after changes) |
| `pnpm serve [port]`                  | Serve `build/` like pixelplusd does (SPA fallback), default :4173                      |
| `pnpm screens [base] [dir] [filter]` | Screenshots of every page in demo mode (desktop + phone); needs `pnpm serve`           |

Playwright uses the Chromium in `PLAYWRIGHT_BROWSERS_PATH` (pinned to `@playwright/test@1.56.1`
to match the preinstalled browser build); don't run `playwright install` on CI images that ship it.

## Layout

```
src/
  app.css                 design system: tokens (dark "studio" + light), buttons, inputs, cards…
  lib/api/                types.ts (mirror of pixelplus-core model.rs), client.ts (typed HTTP client),
                          socket.ts (WebSocket + preview frame parser), mode.ts (real vs. mock backend)
  lib/stores/             app.svelte.ts (show, system, live status, nodes, sensors, logs, preview
                          subscriptions, auto-reconnect), toasts/confirm, theme
  lib/mock/               demo show + in-browser pixelplusd (HTTP routes, WS stream, playback sim)
  lib/effects/render.ts   approximate client-side effect renderer + fallback param schema
  lib/preview.ts          one shared preview subscription + rAF loop for all live canvases
  lib/tts.ts              DJ rendering: device (POST /tts/render) or browser (lib/tts-browser)
  lib/tts-browser/        in-browser Kokoro TTS (owned by the TTS workstream, lazily imported)
  lib/components/         ui/ (Modal, Drawer, Switch, Segmented…), shell/ (sidebar, tab bar,
                          transport bar), viz/ (LayoutCanvas, PropPreview, BoardDiagram, EffectPreview,
                          QrCode, Waveform), props/, schedule/, dj/, effects/
  routes/(admin)/         Dashboard, Props, Layout, Controllers, Sequences, Playlists, Schedule,
                          DJ Studio, Effects, Games, Settings (shared shell + transport bar)
  routes/setup/           first-run wizard (leader/follower, board, name & location, password)
  routes/request/         public song-request page (no auth, mobile first)
```

## Conventions

- JSON is camelCase and mirrors `crates/pixelplus-core/src/model.rs`. When the model changes,
  update `src/lib/api/types.ts` and the mock.
- The show is kept as one immutable object (`app.show`, `$state.raw`). Pages edit a local draft and
  save; the server bumps `show.version`, the WebSocket `show` message triggers a refetch.
  `app.updateShow()` applies optimistic edits.
- Destructive actions confirm first and offer **Undo** in a toast (undo re-creates entities with the
  same id, so `POST` on CRUD endpoints must accept a client-supplied `id`).
- Every page works at 390 px wide; touch targets are ≥ 44 px on coarse pointers; motion respects
  `prefers-reduced-motion`. Keyboard: <kbd>Space</kbd> play/pause, <kbd>/</kbd> search,
  <kbd>B</kbd> blackout, <kbd>?</kbd> shortcuts, <kbd>G</kbd> then a letter to jump between pages.

## API endpoints the UI uses beyond ARCHITECTURE.md

These are assumed by the UI and implemented by the mock; the daemon should provide them exactly:

| Endpoint                                                                            | Shape                                                                                                           |
| ----------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------- |
| `GET /effects/schema`                                                               | `{ [effectKind]: ParamSpec[] }` (`ParamSpec` = `{key,label,kind,min?,max?,step?,default,options?,unit?,help?}`) |
| `PUT /show/name`                                                                    | `{name}` → Show                                                                                                 |
| `POST /props/reorder`                                                               | `{ids: string[]}` — new display order of `show.props`                                                           |
| `POST /nodes/:id/identify`                                                          | blink the controller's status LED/OLED                                                                          |
| `CRUD /dj-voices`                                                                   | like the other CRUD collections, on `show.djVoices`                                                             |
| `PUT /pronunciations`                                                               | full `Pronunciation[]` list                                                                                     |
| `GET /media/:id/peaks?n=160`                                                        | `number[]` 0..1 waveform peaks                                                                                  |
| `POST /player/blackout`                                                             | `{enabled: boolean}`; `PlayerStatus.blackout` reflects it                                                       |
| `POST /player/effect`                                                               | `{effect: EffectPreset \| null}` — show a look live (state `effect`), `null` stops it                           |
| `GET /system/audio/devices`                                                         | `[{id, name}]` ALSA outputs                                                                                     |
| `POST /alerts/test`                                                                 | `{channel: "email" \| "ntfy"}` → `{ok, message}`                                                                |
| `POST /mqtt/test`                                                                   | → `{ok, message}`                                                                                               |
| `GET /games/roms`, `POST /games/roms` (multipart `rom`), `DELETE /games/roms/:name` | ROM library                                                                                                     |
| `POST /games/test-pattern`                                                          | `{propId}` — border/corners/size pattern on the matrix                                                          |
| `POST /triggers/:id`                                                                | fire an HTTP trigger (shown in Settings → Triggers)                                                             |
| `PUT /auth/password`                                                                | `{current?, password: string \| null}` (`null` removes the password)                                            |

Small additions to documented shapes: `SystemInfo.detectedBoard` (board read from EEPROM, `null` if
blank) and `SystemInfo.leaderName` (follower: who adopted it); `SetupRequest.writeEeprom`;
`Sensor.nodeId`; `PublicRequests` = `{title, message, enabled, showName, maxQueue, songs, queue,
nowPlaying}`; `POST /public/requests` → `{ok, position}`; `FaultStep` = `{session, litFrom, litTo,
step, totalSteps, question, done?, result?}`; `Snapshot` = `{id, label, createdAt, sizeBytes,
showVersion?, auto?}`; `UpdateInfo` = `{current, latest, available, notes?, channel?}`;
`GamesStatus.roms`; `DiscoveredNode.ip`; `POST /nodes/adopt` accepts an optional `name`.
