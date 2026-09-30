# PixelPlus Architecture

PixelPlus is a modern, prop-oriented show player for Raspberry Pi pixel controllers.
It replaces Falcon Player (FPP) for the Chandler board family (difftx, difftxlarge,
diffsmart) and plays sequences created in xLights.

This document is the **design authority**. Every component (engine, UI, image,
tools) must follow the contracts here. Change this file first when a contract changes.

---

## 1. Principles

1. **Props, not channels.** Users never see universes, channels or start-channel
   math. They see *props* ("Left Arch", "Mega Tree") attached to *ports*.
   Channels exist internally only, as the byte offsets xLights wrote into `.fseq`.
2. **Configure once, on the leader.** Every setting lives on the leader. Followers
   are appliances: they announce themselves, get adopted, then automatically
   receive their config, their slice of every sequence, and all commands.
3. **Easy to change.** Every entity is editable in place in the UI; changes apply
   live without restarts. Every change is versioned (show snapshots).
4. **Safe by default.** Pre-show health checks, power estimation vs. fuse ratings,
   temperature alerts, and undo via snapshots.
5. **Works offline.** No cloud dependency. Everything (including TTS on Pi 4/5 or
   in the browser) runs locally.

---

## 2. Components

```
repo/
├── crates/
│   ├── pixelplus-core      # data model, fseq, xLights import, mapping, effects, power estimation
│   ├── pixelplus-output    # WS281x DPI framebuffer encoder (+latch banks), output backends
│   ├── pixelplus-hw        # board detection, EEPROM format, sensors (LM75/INA226/DS3231), OLED
│   ├── pixelplus-daemon    # `pixelplusd`: HTTP/WS API, playback, scheduler, audio, cluster sync,
│   │                       #   alerts, MQTT, snapshots, health checks, DJ/TTS orchestration
│   └── pixelplus-cli       # `pixelplus`: admin CLI (eeprom write/read, status, test patterns, doctor)
├── web/                    # SvelteKit SPA (static build served by pixelplusd)
├── tts/                    # Kokoro TTS sidecar (Python, kokoro-onnx) for Pi 4/5 & Docker
├── image/                  # pi-gen stage + first-boot, Wi-Fi config, hotspot captive portal
├── imager/                 # PixelPlus Imager desktop flasher (Tauri)
├── docker/                 # Docker image for running a leader on a PC/NAS
└── docs/                   # user & developer documentation
```

Runtime on a Pi:

```
systemd: pixelplusd.service  (Rust, realtime-priority output thread)
         pixelplus-tts.service (optional, Pi 4/5 only, loopback HTTP :7081)
         NetworkManager (Wi-Fi / hotspot), avahi (mDNS: <hostname>.local)
data:    /var/lib/pixelplus/  (see §6)
config:  /boot/firmware/pixelplus.txt (first-boot settings, human editable)
```

`pixelplusd` listens on **TCP 80** (HTTP + WebSocket; UI + API) and **UDP 32320**
(cluster sync). Environment `PIXELPLUS_HTTP_PORT`, `PIXELPLUS_DATA_DIR`,
`PIXELPLUS_OUTPUT=dpi|sim|none` override defaults (used by Docker and dev).

---

## 3. Hardware model

### 3.1 Boards (`BoardKind`)

| id | Name in UI | Outputs | Output labels | Notes |
|---|---|---|---|---|
| `difftx` | PixelPlus pHAT (difftx) | 4 | `Port 1`–`Port 4` on one RJ45 | rev D (EEPROM rev `D`/cape version 1.0) needs a 4/5-swapped lead on port 3 — show warning |
| `difftxlarge` | 60-Port Transmitter (difftxlarge) | 60 | `J1-1`…`J15-4`, 15 RJ45 jacks, 3 latch banks | INA226 12 V V/A, 2×LM75, DS3231 RTC, optional SSD1306 OLED |
| `diffsmart` | Smart Receiver — standalone (diffsmart) | 4 | `Out 1`–`Out 4` screw terminals | 2×LM75. Only runs PixelPlus in standalone (PI) mode; UI reminds user to set SW1 to **PI** |
| `bare-pi` | Raspberry Pi (no board) | 0 | – | Show director/audio only |
| `virtual` | Virtual (Docker / PC) | 0 | – | Leader on PC/NAS; `sim` output |

Board GPIO maps (BCM). The DPI framebuffer bit *n* drives GPIO *n+4*.

* **difftx / diffsmart**: Port1=GPIO5 (bit1), Port2=GPIO6 (bit2), Port3=GPIO7 (bit3), Port4=GPIO4 (bit0).
* **difftxlarge**: data lines D0..D19 = GPIO4..23 (bits 0..19). Latch enables LE0=GPIO27 (bit 23),
  LE1=GPIO26 (bit 22), LE2=GPIO25 (bit 21). Output k (0-based): bank = k/20, bit = k%20,
  jack = k/4+1, port = k%4+1. Label `J{jack}-{port}`.

### 3.2 Receivers

Receivers are *passive* things attached to a transmitter jack; they are modelled so the
UI can show "Garage receiver → Port 3 → Candy Canes".

| id | Name | Ports |
|---|---|---|
| `diffrx` | Chandler 4D/8P Differential Receiver | 4 |
| `diffsmart-rx` | Chandler Smart Receiver (RX mode) | 4 |
| `generic-4` | Generic 4-port differential receiver (Falcon/Kulp style) | 4 |
| `direct` | Direct (no receiver, e.g. diffsmart own outputs) | 1 per output |

A receiver on transmitter jack *J* maps receiver port *p* (1–4) to transmitter output
`(J-1)*4 + p`. For difftx there is one jack.

### 3.3 PixelPlus EEPROM format (`PPX1`)

Our boards carry an AT24C256 at i2c-1 0x50. PixelPlus uses its own simple format
(not FPP-compatible):

```
offset 0   magic      "PPX1"             4 bytes
offset 4   length     u16 LE             length of JSON payload
offset 6   crc32      u32 LE             CRC-32 (IEEE) of JSON payload
offset 10  json       UTF-8 JSON         {"board":"difftx","rev":"E","serial":"PPX-...","made":"2026-10-01","notes":"..."}
```

Unknown/blank EEPROM (0xFF) → user is asked to pick the board in the setup wizard; the
wizard offers to write the EEPROM. `pixelplus eeprom write --board difftx --rev E`
does the same from the CLI.

### 3.4 Pixel output (DPI)

All boards drive WS281x (800 kHz) via the Pi's DPI peripheral in 24-bit mode clocked
at 38.4 MHz (see `crates/pixelplus-output/DESIGN.md`). PixelPlus has **no artificial
pixel limits**. Practical limit per output ≈ 1600 px at 20 fps / 800 px at 40 fps.
Implementation is original (FPP's DPIPixels is CC-BY-ND and must not be copied); see
`crates/pixelplus-output/DESIGN.md`. The maximum string length is fixed at boot by the DPI overlay's
vertical size: the daemon sizes it from the longest configured string (`DpiGeometry::for_pixels`),
regenerates the config.txt fragment and asks the user to reboot (health check + banner in the UI).

---

## 4. Show model (JSON; Rust types in `pixelplus-core::model`)

All ids are short random strings (`nanoid`-style, 10 chars, `[a-z0-9]`). All entities
have `id`, `name`, and optional `notes`. JSON uses camelCase.

```ts
Show {
  version: number              // monotonically increasing revision, bumped on every change
  name: string
  nodes: Node[]
  receivers: Receiver[]
  props: Prop[]
  propGroups: PropGroup[]
  sequences: Sequence[]
  media: Media[]               // audio files (songs, DJ clips, rendered TTS)
  djClips: DjClip[]
  djVoices: DjVoice[]          // custom voices (blends); built-ins "nick" & "holly" seeded on first run
  pronunciations: {word: string, say: string}[]   // whole-word fixes; say = sound-alike or /IPA/
  effects: EffectPreset[]      // built-in effect presets ("looks")
  playlists: Playlist[]
  schedule: Schedule
  settings: ShowSettings
}

Node {                          // a Pi running PixelPlus
  id, name, hostname
  role: "leader" | "follower"
  board: BoardKind              // detected or chosen
  boardRev?: string
  piModel?: string              // e.g. "Raspberry Pi 4 Model B Rev 1.5" (reported)
  outputs: OutputConfig[]       // length = board output count
  adopted: boolean
  lastSeen?: string             // ISO time (runtime, not persisted meaningfully)
}

OutputConfig {
  index: number                 // 1-based
  label: string                 // "J3-2"
  pixelType: "ws2811"           // 800 kHz WS281x family (WS2811/2812/2815)
  colorOrder: "RGB"|"RBG"|"GRB"|"GBR"|"BRG"|"BGR"
  brightness: number            // 0..100 (%)
  gamma: number                 // 1.0 = none, 2.2 typical
  enabled: boolean
}

Receiver {
  id, name, kind: "diffrx"|"diffsmart-rx"|"generic-4"|"direct"
  nodeId: string, jack: number  // transmitter jack it is plugged into (1-based)
  location?: string             // "Garage", "Front porch"
  fuseAmps?: number             // per-port fuse rating for power warnings (diffrx: 6)
}

Prop {
  id, name
  kind: "arch"|"candycane"|"tree"|"matrix"|"line"|"circle"|"star"|"spinner"|"window"|"icicles"|"custom"|"other"
  pixelCount: number
  xlightsModel?: string         // source model name in xLights layout
  channelStart: number          // 0-based byte offset into the fseq frame (internal; never shown as "channel")
  channelsPerPixel: 3           // 3 = RGB (only RGB supported for now)
  segments: PropSegment[]       // where the pixels physically are (in order)
  groupIds: string[]
  layout?: PropLayout           // 2D preview geometry
  color?: string                // UI accent color for the prop
  maxMilliampsPerPixel?: number // default 60 (full white 12V 3-LED pixel ≈ 20mA*3)
}

PropSegment {                   // a run of consecutive prop pixels on one output
  nodeId: string
  output: number                // 1-based node output index
  startPixel: number            // 0-based pixel position on that output (after nulls)
  pixelCount: number
  propOffset: number            // 0-based index into the prop's pixels where this segment starts
  reverse: boolean
  nullPixels: number            // null pixels before this segment on the output (informational + mapping)
}

PropLayout { x: number, y: number, w: number, h: number, rotation: number, points?: [number,number][] }
// Coordinates: y-down canvas ("world units"; imported layouts keep xLights world units). x,y = top-left of the
// box, rotation = degrees clockwise around the box centre, points normalized with [0,0] = top-left of the box.
// points = normalized (0..1) per-pixel positions within the w×h box; if absent, derived from `kind`.

PropGroup { id, name, propIds: string[], color?: string }

Sequence {
  id, name
  file: string                  // relative path under data dir: sequences/<id>.fseq
  durationMs: number, frameMs: number, channelCount: number
  mediaId?: string              // linked audio
  xlightsName?: string          // original filename
  thumbnail?: string            // generated preview strip (PNG under data dir)
}

Media { id, name, kind: "song"|"dj"|"sfx", file, durationMs, loudnessLufs?: number, gainDb?: number }

DjClip {
  id, name
  lines: { voice: string, text: string, pauseMs: number, energy?: number }[]   // multi-voice dialog; voice = DjVoice id or kokoro voice id; energy 0 calm .. 0.4 normal .. 1 hype .. 1.5 extra hype
  dynamic: boolean              // contains {placeholders}; re-rendered at showtime (Pi 4/5 / Docker only)
  mediaId?: string              // last rendered audio
  speed: number                 // 0.5..2.0
  musicBedMediaId?: string      // optional background bed, ducked under speech
}
// Placeholders: {time} {date} {day} {daysUntilChristmas} {nextSong} {prevSong}
//               {showName} {temperature} {sunset} {requestName}

DjVoice {                      // same shape as tlchandler/fpp-voices voices.json entries
  id, name, description
  blend: Record<string, number> // kokoro voice id -> weight, e.g. {am_echo:.3, am_fenrir:.3, am_puck:.4}
  speed: number, lang: string, eq?: string, defaultEnergy: number
  energy: {pitch, range, speed, stretch, boost, lift, ceiling, maxLift}
}

EffectPreset {
  id, name
  effect: "solid"|"chase"|"twinkle"|"rainbow"|"colorwash"|"candycane"|"fire"|"snow"|"sparkle"|"wave"|"meteor"|"strobe"|"breathe"
  params: Record<string, number|string|boolean|string[]>  // e.g. {colors:["#ff0000","#ffffff"], speed:1.0, density:0.3}
  target: { all?: boolean, propIds?: string[], groupIds?: string[] }
}

Playlist {
  id, name
  items: PlaylistItem[]
  intro?: PlaylistItem[]        // played once at start
  outro?: PlaylistItem[]        // played once at end
  shuffle: boolean
  repeat: boolean
  crossfadeMs: number           // 0 = gapless cut
}

PlaylistItem =
  | { id, type: "sequence", sequenceId }
  | { id, type: "dj", djClipId }
  | { id, type: "effect", effectId, durationMs }
  | { id, type: "media", mediaId }                 // audio only
  | { id, type: "pause", durationMs }
  | { id, type: "command", command: string, args?: any }   // e.g. games.invite, overlay.text

Schedule {
  enabled: boolean
  location: { lat: number, lon: number, timezone: string, label?: string }
  entries: ScheduleEntry[]
  idleEffectId?: string         // look shown inside show windows when nothing plays / between entries
  offEffectId?: string          // null = dark outside show windows
  volumeCurfew?: { time: TimeSpec, volume: number }   // lower volume after e.g. 9pm
}

ScheduleEntry {
  id, name, enabled: boolean
  playlistId: string
  days: ("mon"|"tue"|"wed"|"thu"|"fri"|"sat"|"sun")[]
  dateRange?: { start: "MM-DD", end: "MM-DD" }   // wraps year end (e.g. 11-25 .. 01-06)
  start: TimeSpec, end: TimeSpec
  priority: number              // higher wins on overlap (special nights > regular)
  endBehavior: "finishSong"|"stopNow"|"fadeOut"
}
TimeSpec = { kind: "clock", time: "HH:MM" } | { kind: "sunset"|"sunrise", offsetMin: number }

ShowSettings {
  audio: { device: string, volume: number, normalize: boolean, targetLufs: number }
  alerts: { email?: {smtpHost, smtpPort, username, password, from, to, tls}, ntfy?: {server, topic}, rules: {tempC: number, voltageMin: number, followerOffline: boolean, showFailure: boolean} }
  mqtt: { enabled: boolean, host, port, username?, password?, baseTopic: string, homeAssistantDiscovery: boolean }
  requests: { enabled: boolean, maxQueue: number, playlistId?: string, title: string, message: string }
  tts: { mode: "auto"|"device"|"browser" }
  oled: { enabled: boolean }
  security: { passwordHash?: string }
  triggers: { id, name, kind: "gpio"|"http", gpio?: number, action: {type: "playPlaylist"|"playSequence"|"stop"|"effect", ref?: string} }[]
}
```

### 4.1 Channel mapping (internal)

For each prop, pixel `i` (0-based in xLights model order) reads 3 bytes at
`channelStart + 3*i` from the fseq frame. The segment containing `i` (by
`propOffset`) gives `(node, output, startPixel + (reverse ? count-1-(i-propOffset) : i-propOffset))`.
Output colour order, brightness and gamma are applied at output time. xLights has already
applied model-level colour order? **No**: xLights fseq data is always RGB per model; colour
order is a *controller port* property, applied by PixelPlus.

---

## 5. xLights import

User uploads `xlights_rgbeffects.xml` (and optionally `xlights_networks.xml`). The importer
(`pixelplus-core::xlights`):

1. Parses controllers from networks.xml (including `Ethernet` DDP/E1.31 controllers with
   universes) to compute each controller's absolute start channel.
2. Parses `<model>` elements: `name`, `DisplayAs`, `StartChannel` (supports absolute `1234`,
   `!Controller:123`, `@OtherModel:1`, `>OtherModel:1`, `#universe:ch` and `#ip:universe:ch`),
   `parm1/parm2/parm3` (strings × nodes), `ControllerConnection` (`Port`, `Protocol`), `Controller` name,
   `CustomModel` data, `WorldPosX/Y`, `ScaleX/Y`, and model dimensions for the preview.
3. Computes pixel count and absolute `channelStart` for each model.
4. Proposes a mapping: xLights controller name → PixelPlus node (matched by name/hostname or
   chosen by the user in the import wizard), model `ControllerConnection.Port` → output, chained
   models on the same port get consecutive `startPixel` in order of their start channel.
5. Returns an `ImportPreview` (props with proposed segments + warnings); the UI shows it and
   the user confirms. Re-import merges by `xlightsModel` name and keeps user edits.

**Recommended xLights setup** (documented in the UI): add each PixelPlus node in xLights as a
controller of vendor *PixelPlus/Generic*, protocol **DDP**, "Auto size", ports per board. The user
never types a universe.

Sequences: user uploads `.fseq` (v1, v2 uncompressed, zstd, zlib; sparse ranges supported). The
matching audio (mp3/ogg/m4a/wav/flac) is uploaded with it or picked from media; the fseq header's
media filename is used to auto-link.

---

## 6. Data directory (`/var/lib/pixelplus`)

```
show.json                    # the Show (source of truth, leader)
node.json                    # this node's identity: {id, role, leaderUrl?, clusterKey?}
sequences/<id>.fseq          # leader: full fseq
sequences/<id>.ppseq         # follower: node-specific slice (see §7.3)
media/<id>.<ext>             # audio
media/<id>.meta.json         # loudness etc.
thumbnails/<id>.png
snapshots/<timestamp>-<label>.tar.zst   # show.json + referenced small files (not sequences unless requested)
logs/
tts/models/                  # kokoro model (Pi 4/5, Docker)
```

---

## 7. Cluster (leader / followers)

### 7.1 Discovery & adoption
* Every node advertises mDNS `_pixelplus._tcp` (TXT: `id`, `role`, `board`, `ver`; through
  avahi-daemon when it runs, else the built-in responder - see BUILDING.md "mDNS") and sends a UDP
  broadcast **beacon** on port 32320 every 2 s:
  `{"t":"beacon","id","name","role","board","boardRev","pi","ver","http":80,"adoptedBy":<leaderId|null>}`.
* The leader UI lists unadopted nodes under **Controllers → New controllers found**. Clicking
  **Adopt** calls `POST http://<follower>/api/v1/cluster/adopt {leaderId, leaderUrl, clusterKey}`.
  The follower persists it in `node.json`. From then on all leader→follower HTTP calls carry
  header `X-PixelPlus-Key: <clusterKey>`.
* A fresh node boots as `role: "unconfigured"`: its own UI shows a welcome screen:
  "Make this the show leader" or "Waiting to be adopted by a leader…" (with its name/IP).

### 7.2 Configuration push
* Leader computes a **NodeManifest** per follower whenever `show.version` changes:
  `{showVersion, node: Node, props: [props with segments on this node, channel ranges remapped],
   sequences: [{id, name, hash, durationMs, frameMs}], effects, outputs}`.
* Heartbeats (below) carry `showVersion`; a follower whose manifest version differs calls
  `GET <leader>/api/v1/cluster/manifest/<nodeId>` and then fetches missing sequence slices with
  `GET <leader>/api/v1/cluster/slice/<nodeId>/<sequenceId>` (ETag = hash). Downloads resume.
  Followers delete slices no longer referenced.

### 7.3 `.ppseq` slice format
A follower only needs the bytes for its own outputs. The leader precomputes the node's pixel
buffer layout: `output-major, pixel-order` RGB bytes (colour order NOT yet applied). Format:

```
"PPSQ" | u16 version=1 | u32 frameCount | u32 frameMs(×1000 µs) | u32 frameBytes |
u16 outputCount | outputCount × u32 pixelsPerOutput | 32-byte sha256 of source fseq |
frames: zstd-compressed blocks, each block = up to 64 frames; block index table at end:
[u64 offset, u32 compressedLen, u32 firstFrame] × nBlocks | u32 nBlocks | "PPSQ"
```
The leader uses the same renderer for its own outputs (it plays from the full fseq directly).

### 7.4 Sync
* Effects sent to followers (sync packets, commands, manifests) must be copies passed through
  `effects::stamp_world_bounds(&mut copy, &show.props)` so display-wide effects line up across nodes.
* Leader broadcasts (UDP 32320, and unicast to adopted followers) **sync packets** every 250 ms
  while playing and on every state change:
  `{"t":"sync","leader":id,"showVersion","state":"playing|paused|stopped|effect","item":{type,id},"startedAtMs":<leader monotonic ms>,"posMs":number,"sentAtMs":number,"effect"?:EffectPreset,"brightness":number}`
* Followers keep a clock offset estimate using NTP-style ping (`{"t":"ping"}` / `pong`) and set
  their playback position to `posMs + (now - sentAt)`; drift > 1 frame → slew, > 250 ms → jump.
* Audio plays only on the leader. Leader position is derived from the audio clock when audio is playing.
* Commands (test patterns, fault finder, effects, blackout) are sent as HTTP POSTs to
  `/api/v1/cluster/command` on the follower.

---

## 8. HTTP API (`/api/v1`, JSON, camelCase)

Auth: if a password is set, `POST /api/v1/auth/login {password}` → session cookie `pp_session`.
Unauthenticated requests get 401 except `/auth/*`, `/public/*` (song requests page), and
`/cluster/*` (which require `X-PixelPlus-Key`).

Errors: `{ "error": { "code": "not_found", "message": "Human readable" } }` with HTTP status.

| Method & path | Description |
|---|---|
| `GET /system` | `SystemInfo` {version, nodeId, role, hostname, board, boardRev, piModel, uptimeS, cpuPct, memPct, diskFreeMb, tempC, ips[], time, timezone, wifi:{ssid, signal}, needsSetup, passwordSet} |
| `POST /system/setup` | first-run wizard: {role:"leader"|"follower", showName?, board?, boardRev?, location?, timezone?, password?} |
| `POST /system/reboot`, `/system/shutdown`, `/system/restart-service` | |
| `GET /system/logs?lines=500` | text |
| `GET/PUT /system/network` | {hostname, wifi:{ssid, psk?, country}, ethernet:{dhcp, address?, gateway?, dns?}, managed (read-only), netwatch (read-only: `/run/pixelplus/netwatch.json` {state:"waiting"|"online"|"hotspot"|"connecting", hotspotSsid, hotspotSecured, portalUrl, lastError, lastJoined:{ssid, ips, at}, updatedAt} or null)}; `GET /system/network/scan` → [{ssid, signal, secure}] |
| `GET /system/helpers` | latest root-helper jobs [{verb, state:"running"|"ok"|"failed", message, updatedAt}] (also pushed as WS `helper`) |
| `GET/PUT /system/ssh` | {enabled:boolean|null, canChange, job?} / {enabled} → helper `ssh-on`/`ssh-off` |
| `POST /system/reapply` | re-apply `/boot/firmware/pixelplus.txt` (helper `reapply`) |
| `GET /system/output-geometry` | {ok, longestString, maxPixels, configuredPixels, pendingReboot, canApply, targetPixels, piMaxPixels, message} |
| `POST /system/output-geometry/apply` | {reboot?:true}: helper `config-txt:<board>:<pixels>` for the longest string, then reboot via logind → {ok, job, geometry}; 409 when nothing to do |
| `GET /public/health` | {ok, version, role}: unauthenticated liveness (Docker healthcheck) |
| `GET /system/sensors` | [{id, label, kind:"temperature"|"voltage"|"current"|"power", value, unit, warn?, crit?}]; ids: cpuTemp, driverTemp, powerTemp, enclosureTemp, inputVoltage, inputCurrent, inputPower. For voltage sensors warn/crit are minimums, otherwise maximums |
| `GET /system/sensors/history?minutes=60` | {series: {id: [[t,v]...]}} |
| `POST /system/eeprom` | {board, rev} write EEPROM |
| `GET /system/update`, `POST /system/update` | check (`apt-cache policy`; {current, latest, available, canApply, message?, job?}) / install (helper `update`) |
| `POST /auth/login`, `POST /auth/logout`, `PUT /auth/password` | |
| `GET /show` | full `Show` |
| `PUT /show/settings` | ShowSettings (partial merge) |
| `GET/POST /nodes`, `GET/PUT/DELETE /nodes/:id` | nodes; `GET /nodes/discovered` → beacons of unadopted nodes; `POST /nodes/adopt {id}` |
| `PUT /nodes/:id/outputs/:index` | OutputConfig |
| CRUD `/receivers`, `/props`, `/prop-groups`, `/effects`, `/playlists`, `/dj-clips` | `GET` list, `POST` create, `GET/PUT/DELETE /:id` |
| `POST /props/bulk` | {ops:[{op:"update"|"delete", id, patch?}]} |
| `POST /import/xlights` | multipart `rgbeffects` (+`networks`) → `ImportPreview` {props, controllers:[{name, suggestedNodeId, ip, protocol, ports, propCount}], groups, warnings[]}. In the preview, `segments[].nodeId` holds the **xLights controller name** (placeholder) until applied |
| `POST /import/xlights/apply` | {preview, controllerMap:{xlightsControllerName: nodeId}} → Show |
| `GET /sequences`, `POST /sequences` (multipart fseq + optional audio) , `GET/PUT/DELETE /sequences/:id` | |
| `GET /sequences/:id/thumbnail` | PNG |
| `GET /media`, `POST /media` (multipart), `GET/PUT/DELETE /media/:id`, `GET /media/:id/file` | |
| `GET /schedule`, `PUT /schedule` | |
| `GET /schedule/preview?days=14` | expanded occurrences [{date, start, end, entryId, playlistId, name}] |
| `GET /player` | PlayerStatus |
| `POST /player/play` | {playlistId?} | {sequenceId?} | {djClipId?} |
| `POST /player/stop` {fade?:bool}, `/player/pause`, `/player/resume`, `/player/next`, `/player/previous`, `/player/seek {posMs}` | |
| `PUT /player/volume {volume}`, `PUT /player/brightness {brightness}` | |
| `POST /test/start` | {mode:"solid"|"chase"|"rgbCycle"|"countPixels"|"walk"|"effect", color?, target:{nodeId?, output?, propIds?, groupIds?, all?}, effect?: EffectPreset} |
| `POST /test/stop` | |
| `POST /faultfinder/start` {propId} → FaultSession; `POST /faultfinder/:session/answer {lit:boolean}` → next step or result {pixelIndex, message} ; `POST /faultfinder/stop` | binary search for first bad pixel: lights pixels [0..mid], asks "do all lit pixels light correctly?" |
| `GET /power/estimate?sequenceId=` | {perOutput:[{nodeId, output, peakAmps, avgAmps}], perReceiverPort:[...], perProp:[...], warnings[]} |
| `GET /health` | HealthReport {ok, checks:[{id, label, status:"ok"|"warn"|"fail", detail, action?:"applyOutputGeometry"|"reboot"}]} |
| `POST /health/run` | run pre-show check now |
| `GET /snapshots`, `POST /snapshots {label}`, `POST /snapshots/:id/restore`, `GET /snapshots/:id/download`, `POST /snapshots/import` (multipart), `DELETE /snapshots/:id` | |
| `GET /effects/catalog` | `effect_catalog()` → [{kind, label, description, params: ParamSpec[]}]; `GET /effects/schema` → {kind: ParamSpec[]} ; `GET /effects/builtin` → builtin presets |
| `GET /tts/status` | {mode:"device"|"browser", available:boolean, voices:[{id, name, language, gender}] } |
| `POST /tts/render` | {lines, speed} → audio/mpeg (device mode) |
| `POST /dj-clips/:id/render` | render on device → updates mediaId |
| `POST /dj-clips/:id/upload` | multipart audio rendered in browser |
| `GET /public/requests` | public song list + queue (no auth) ; `POST /public/requests {sequenceId, name?}` |
| `GET /requests`, `DELETE /requests/:id` | admin view of queue |
| `GET /cluster/manifest/:nodeId`, `GET /cluster/slice/:nodeId/:seqId`, `POST /cluster/adopt`, `POST /cluster/command`, `POST /cluster/release` | cluster internal |

### 8.1 WebSocket `/api/v1/ws`
Server → client messages `{type, data}`:
* `status` (every 250 ms while playing, 2 s idle): `PlayerStatus`
  `{state:"idle"|"playing"|"paused"|"testing"|"effect", playlist?:{id,name,index,count}, item?:{type,id,name}, posMs, durationMs, volume, brightness, nextItem?:{type,id,name}, scheduleEntry?:{id,name,endsAt}, nextShow?:{name, startsAt}, fps}`
* `show` `{version}` — show changed; client refetches `/show`.
* `nodes` `[{id, name, online, lastSeen, board, syncOffsetMs, syncState:"synced"|"syncing"|"offline", files:{pending, total}}]`
* `sensors` `[Sensor]` every 5 s
* `log` `{level, message, time}` (warnings and errors only)
* `toast` `{kind:"info"|"success"|"warning"|"error", message}`
* `preview` binary frames (only after client sends `{"type":"subscribePreview","fps":20}`):
  `u8 0x50 | u32 frameNo | then per prop in show.props order: pixelCount×3 RGB bytes` (post-brightness).

Client → server: `{type:"subscribePreview", fps}`, `{type:"unsubscribePreview"}`.

---

## 9. UI information architecture

Left sidebar (collapses to bottom tab bar on phones):

1. **Dashboard** — now playing (big transport), next show countdown, controllers health tiles,
   sensors, alerts, quick actions (Play show now, Stop, Blackout, Test all props).
2. **Props** — the heart of the app. Grid/list of props with live mini-preview, search, groups,
   drag-to-reorder; prop drawer: name, type, pixels, wiring (controller → jack → receiver → port →
   start pixel, reverse, nulls), colour order/brightness, power estimate, *Test* and *Find fault*.
3. **Layout** — live 2D preview of the whole display (canvas), used for both live monitoring and editing
   positions.
4. **Controllers** — nodes (leader + followers) with board picture/diagram of jacks, which receivers
   and props hang off each jack, output settings, sync state, sensors; adopt new controllers; receivers.
5. **Sequences & Audio** — upload fseq + audio (drag-and-drop, multi-file), auto-link, loudness, preview.
6. **Playlists** — drag-and-drop builder mixing sequences, DJ clips, effects, pauses; intro/outro; crossfade.
7. **Schedule** — calendar/week view with show windows, sunset-relative times, special nights, idle look,
   volume curfew, 14-day preview.
8. **DJ Studio** — voice gallery with audition, multi-voice script editor, placeholders, music bed, render,
   add to playlist.
9. **Effects** — built-in looks, live-apply to props/groups, save as preset.
10. **Settings** — network/Wi-Fi, audio, alerts, MQTT/Home Assistant, song requests, triggers,
   security, snapshots (time machine), updates, about/hardware (EEPROM), logs.

Public page `/request` (no auth, mobile-first): song request/vote page with QR code shown in Settings.

Design language: dark "studio" theme default + light theme; Inter font; 8-px grid; accent = warm amber
(#F5A524) with semantic green/red/blue; large touch targets (≥44 px); every destructive action has
confirm + undo toast; empty states with guidance; skeleton loaders; keyboard shortcuts (space =
play/pause, `/` = search).

---

## 10. Overlays (live content on props)

An **overlay** temporarily replaces a prop's pixels with live content: games, scrolling text,
QR codes, the fault finder, test patterns. Overlays are composited after sequence/effect
rendering, before output-config (colour order, brightness, gamma).

* Matrix props carry `matrix: {width, height, pixelMap}` (computed by the xLights importer from
  the model's strings/nodes/start corner/direction, or edited in the UI). `pixelMap[y*width+x]`
  = prop pixel index or -1.
* **Shared-memory fast path** (local processes such as the games sidecar):
  `POST /api/v1/overlay/:propId/open` → `{shm: "/dev/shm/pixelplus-overlay-<propId>", width, height}`.
  Layout = 12-byte header of three native-endian u32 (width, height, flags) + width×height×3 RGB,
  row-major from top-left. Writer sets flags bit 0 after writing a frame; pixelplusd copies the
  frame on its next output pass and clears the bit. (Same shape as FPP's overlay buffers so the
  mario port changes minimally.)
* HTTP: `PUT /api/v1/overlay/:propId/frame` (body = raw RGB width×height×3),
  `POST /api/v1/overlay/:propId {enabled: bool}`, `POST /api/v1/overlay/:propId/text {text, color, scroll, durationMs}`,
  `POST /api/v1/overlay/:propId/qr {url, durationMs}`.
* If the matrix prop's pixels live on a follower, the leader forwards overlay frames to that
  follower over UDP 32321 (`u8 'O' | propId len-prefixed | frameNo u32 | RGB`).

## 11. Games (port of tlchandler/fpp-mariobros)

`games/` is a Python sidecar (`pixelplus-games.service`) that lets passers-by play Super Mario
Bros. (random level, Santa hat, 60 s turns, cooldown, queue, invites) or an NES **Arcade mode**
on a matrix prop, using their phone as a gamepad (port 8088 by default). It uses:
* `GET /api/v1/show` → `settings.games` + matrix prop geometry.
* `POST /api/v1/player/pause` / `resume` to pause the show around a game.
* Overlay API (§10) to draw frames; audio via ALSA on the leader's audio device.
* `POST /api/v1/games/invite`, `POST /api/v1/games/stop` exposed by pixelplusd (proxied to the
  sidecar's local control socket `/run/pixelplus/games.sock`) so playlists/schedule/triggers can
  show the invite. PlaylistItem gains `{type:"command", command:"games.invite"|"games.stop", args}`.
* Status: `GET /api/v1/games/status` → {enabled, running, arcade, queueLength, cooldownS, player?, lastError?}.
ROMs are uploaded in the UI (Settings → Games) and stored in `/var/lib/pixelplus/games/roms/`.
