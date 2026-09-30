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

`pixelplusd` listens on **TCP 80** (HTTP + WebSocket; UI + API), **UDP 32420** (cluster
sync; overlay frames on **32421**), and — once their features land — **TCP 443** (HTTPS, §12.1;
8443 in Docker), **UDP 32422** (ESP32 sensor nodes, §12.16) and **127.0.0.1:8081** (public-only
listener for tunnels, §12.12). The cluster used UDP 32320 before; that is FPP's multisync
port, which xLights *FPP Connect* pings, so it moved (§7.4.1). Environment
`PIXELPLUS_HTTP_PORT`, `PIXELPLUS_CLUSTER_PORT`, `PIXELPLUS_SENSOR_PORT`,
`PIXELPLUS_HTTPS_PORT`, `PIXELPLUS_PUBLIC_PORT` (0 = off), `PIXELPLUS_DATA_DIR`,
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

**When a frame lights up.** The display scans out on a free-running vblank grid (refresh
R = 24.8 ms at 800 px, 49.3 ms at 1600 px) and a string of L LEDs latches L × 30.6 µs + 0.28 ms
after scan-out starts. The DPI backend reports every page flip's vblank sequence and
CLOCK_MONOTONIC timestamp (`PixelOutput::present_timing`, `VblankModel`); the player paces its
output thread from that grid and chooses each frame for the moment it actually lights up (§7.4.6).
Optional, experimental: `settings.output.latchAlign` bottom-aligns every string's data so all
strings on a controller latch together (DESIGN.md "Latch alignment").

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
  channelStart: number          // 0-based byte offset into the fseq frame (internal; never shown as "channel");
                                //   with channelRuns: the lowest byte offset of any run
  channelsPerPixel: 3           // 3 = RGB (only RGB supported for now)
  channelRuns?: ChannelRun[]    // non-contiguous channels (xLights individual/"Advanced" start channels);
                                //   absent = contiguous from channelStart. Set by the importer, read-only in the UI.
  segments: PropSegment[]       // where the pixels physically are (in order)
  groupIds: string[]
  layout?: PropLayout           // 2D preview geometry
  color?: string                // UI accent color for the prop
  maxMilliampsPerPixel?: number // default 60 (full white 12V 3-LED pixel ≈ 20mA*3)
}

ChannelRun {                    // prop pixels whose channels are contiguous in the fseq frame (one xLights string)
  propOffset: number            // 0-based index of the run's first prop pixel
  channelStart: number          // 0-based byte offset of that pixel in the fseq frame
  pixelCount: number
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
  audio: { device: string, volume: number, normalize: boolean, targetLufs: number,
           outputDelayMs?: number }   // lights delayed by this (−500…2000 ms), §7.4.7
  output?: { latchAlign: boolean }    // experimental: all strings latch together, §3.4
  alerts: { email?: {smtpHost, smtpPort, username, password, from, to, tls}, ntfy?: {server, topic}, rules: {tempC: number, voltageMin: number, followerOffline: boolean, showFailure: boolean} }
  mqtt: { enabled: boolean, host, port, username?, password?, baseTopic: string, homeAssistantDiscovery: boolean }
  requests: { enabled: boolean, maxQueue: number, playlistId?: string, title: string, message: string }
  tts: { mode: "auto"|"device"|"browser" }
  oled: { enabled: boolean }
  security: { passwordHash?: string }
  triggers: { id, name, kind: "gpio"|"http", gpio?: number, action: {type: "playPlaylist"|"playSequence"|"stop"|"effect", ref?: string} }[]
}
```

### 4.0 Feature-wave additions (compatibility)

Every field below is `#[serde(default)]`; optional ones are omitted while empty, so a
`show.json` from an older version loads unchanged and saves with only these new keys:
`formatVersion` and `settings.{https,reports,power,remote,updates,xlights}` (test:
`crates/pixelplus-core/tests/show_compat.rs`, fixture `testdata/show-head-2026-09.json`).
Details per feature in §12.

```ts
Show += { formatVersion: number /*1*/, profiles?: ShowProfile[], activeProfileId?, profileAutoSwitch?,
          powerSupplies?: PowerSupply[], tagDefs?: {name, color?}[], sensorNodes?: SensorNode[] }
Node += { serial?, hardwareHistory?: {at, serial?, board, piModel?, reason}[] }                 // §12.9
OutputConfig += { measuredPixels?: {count, method:"camera"|"manual"|"current", at, dead?: number[]} } // §12.6
Receiver += { mainFuseAmps? }                                                                   // §12.11
Prop += { suspectPixels?: number[] }                                                            // §12.6
PropLayout += { source?: "xlights"|"manual"|"camera" }                                          // §12.5
Sequence += { generated?: {kind:"autoShow"|"voice", mediaId, style, propIds, seed, analysisVersion, propsHash}, tags? }
Media += { analysis?: {version, bpm, bpmConfidence, beatCount, firstBeatMs, energy, sections}, tags?,
           originalName?, originalSize? }
EffectPreset.effect += "countdown"                                                              // §12.4
PlaylistItem += { id, type:"countdown", durationMs, matrixPropId?, text?="{s}", color?,
                  others?:"fill"|"pulse"|"dark", finale?:"flash"|"none", djClipId?, djOffsetMs?, tick? }
Playlist += { smart?: SmartRules }                                                              // §12.15
ScheduleEntry += { startExact?: boolean }                                                       // §12.4
ShowSettings.audio += { lastCalibration?: {measuredAt, method:"phone"|"manual", residualMs, spreadMs, matches, appliedDelayMs, device?} }
ShowSettings += { https: {enabled=true, extraNames?}, reports: {enabled, time="07:00", email, push, onlyWhenProblems, keepDays=90},
                  power: {mode:"off"|"warn"(default)|"limit", safety=0.9, globalAmps?, globalWatts?, dim: DimWindow[], maxBrightness=100},
                  remote: {publicListener, tailscale?, cloudflare?},
                  updates: {channel:"stable"|"beta", auto:"off"|"notify"(default)|"install", window:{from="10:00", to="14:00", days}, avoidShowHours=2},
                  xlights: {fppConnect, passwordHash? /*write-only: "" when set*/, addToPlaylists=true, watchFolder?} }
Trigger += { kind:"sensor", sensor?: {sensorNodeId, input}, cooldownS?, when?:"always"|"showOnly"|"idleOnly"|"offOnly",
             activeWindow?: {from: TimeSpec, to: TimeSpec}, maxPerHour? }
TriggerAction += { type:"surprise", target?: Target, durationMs?, source?:"sequence"|"effect" }
```

Countdown items and surprise actions are rendered by the engine (§12.4, §12.16).

### 4.1 Channel mapping (internal)

For each prop, pixel `i` (0-based in xLights model order) reads 3 bytes at
`channelStart + 3*i` from the fseq frame. If the prop has `channelRuns`, pixel `i` instead
reads from the run containing it, at `run.channelStart + 3*(i - run.propOffset)`; pixels
covered by no run have no data (black). Runs beyond `pixelCount` are clipped. Everything
that reads prop channels (node routing, `.ppseq` slices, power estimates, the live preview,
effects/overlays written into channel space, sequence channel-count checks) goes through
`Prop::channel_ranges` / `Prop::channel_of_pixel`, and the `NodeMap` splits a segment into
one precomputed copy run per channel run, so rendering stays allocation-free. The segment containing `i` (by
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
3. Computes pixel count and absolute `channelStart` for each model. Models with individual
   start channels (`Advanced="1"` plus `String1`, `String2`, … — any of the start-channel forms
   above) become `channelRuns`, following xLights' node numbering per model type: arches,
   candy canes, matrices, trees, spheres, lines, poly lines (split at `PolyNodeN`), multi-point,
   circles, wreaths, stars and spinners put each string at its own start channel; icicles,
   window frames, cubes and layered arches number every node from `String1`; custom models
   number every node from the lowest string start. `@Model:n` / `>Model:n` referring to such a
   model use its lowest / highest channel, like xLights. Missing `StringN` attributes keep the
   contiguous position (with a warning).
   Inactive models (`Active="0"`) are skipped with a warning (xLights never outputs them) but
   still count for `>`/`@` chaining. Controllers and outputs of types xLights does not know
   are dropped (they take no channels), as xLights does, with a warning.
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
node.json                    # this node's identity: {id, role, leaderUrl?, clusterKey? (follower: its own key)}
cluster/keys.json            # leader: one key per follower; follower: key-pending flag (0600)
sequences/<id>.fseq          # leader: full fseq
sequences/<id>.ppseq         # follower: node-specific slice (see §7.3)
media/<id>.<ext>             # audio
media/<id>.meta.json         # loudness etc.
thumbnails/<id>.png
snapshots/<timestamp>-<label>.tar.zst   # show.json + referenced small files (not sequences unless requested)
logs/
tts/models/                  # kokoro model (Pi 4/5, Docker); owned by the pixelplus-tts user on packages
games/roms/                  # NES ROMs (group pixelplus-overlay, shared with the games sidecar)
```

File paths inside `show.json` (`media.file`, `sequence.file`, `sequence.thumbnail`) are never
trusted (`services/paths.rs`): they must be `media/<id>.<audio ext>`, `sequences/<id>.fseq|ppseq`,
`thumbnails/<id>.png` with `<id>` of `[A-Za-z0-9_-]{1,64}`. Every store write (and loading, and a
snapshot restore) rebuilds any other path from the entity id or blanks it; file-serving code
resolves them through the same check. Snapshot restore unpacks only such files, within a size
budget (16 GiB and the free space minus 256 MiB); snapshot uploads are capped at 2 GiB. Audio is
served only with an audio content type from a whitelist, `Content-Disposition: attachment` and
`nosniff`.

---

## 7. Cluster (leader / followers)

### 7.1 Discovery & adoption
* Every node advertises mDNS `_pixelplus._tcp` (TXT: `id`, `role`, `board`, `ver`; through
  avahi-daemon when it runs, else the built-in responder - see BUILDING.md "mDNS") and sends a UDP
  broadcast **beacon** on port 32320 every 2 s:
  `{"t":"beacon","id","name","role","board","boardRev","pi","ver","http":80,"adoptedBy":<leaderId|null>,"proto":2}`
  (`proto` = cluster protocol version, §7.4.1; absent = 1). Broadcast is used for discovery only:
  everything timing-related is unicast (§7.4.1).
* The leader UI lists unadopted nodes under **Controllers → New controllers found**. Clicking
  **Adopt** calls `POST http://<follower>/api/v1/cluster/adopt {leaderId, leaderUrl, dh, force?, name?}`;
  the follower answers `{id, name, hostname, board, …, dh, proof}`. Both sides derive **a key for this
  follower only** from the X25519 exchange (`dh`), so the key never crosses the network; `proof`
  shows the leader that the follower derived the same key. The follower keeps it in `node.json`
  (`clusterKey`), the leader in `cluster/keys.json` (0600). See §7.5 for how it is used and for the
  rules deciding who may adopt a controller.
* A controller that is itself a leader is offered for adoption only while its owner has
  **Controllers → Join another show** open (15 minutes, optionally for one leader address:
  `POST /api/v1/system/join-show {leaderUrl?}`, a normal signed-in call); adopting it replaces its
  show (a copy is kept in `cluster/show-before-adopt-*.json`).
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

Goal: every controller shows the frame the audience should see *now*, i.e. the leader's
timeline at the moment the lights change, and that timeline matches what the audience hears.
Error sources, from largest (before protocol 2) to smallest: the follower's old ±1-frame
deadband (a 0..F sawtooth), vblank quantisation and pipeline lag (0..R late per node), WS281x
latch delay (0–49 ms by string length), and only then the network (sub-ms with the measures
below). Measured accuracy and remaining limits: §7.4.9; board options: `docs/HARDWARE-NOTES.md`.

* Effects sent to followers (sync packets, commands, manifests) must be copies passed through
  `effects::stamp_world_bounds(&mut copy, &show.props)` so display-wide effects line up across nodes.
* Audio plays only on the leader. Commands (test patterns, fault finder, effects, blackout) are
  HTTP POSTs to `/api/v1/cluster/command` on the follower.

#### 7.4.1 Transport and protocol version
* Clock probes (`ping`/`pong`) and sync packets are **unicast** UDP 32420 (`PIXELPLUS_CLUSTER_PORT`;
  before this release 32320, which is FPP's multisync port that xLights FPP Connect pings) to/from
  each adopted follower, MACed with that follower's key (§7.5). All controllers of a show must run
  the same release; the leader and followers of different releases don't see each other. Broadcast carries only the unauthenticated
  discovery beacon: Wi-Fi broadcast waits for the next DTIM beacon, goes out at the lowest rate
  and is never retransmitted.
* The cluster socket is marked **DSCP EF** (`IP_TOS 0xB8`, IPv6 traffic class) and
  `SO_PRIORITY 6`; Linux maps EF to the Wi-Fi voice/video access category (WMM), so timing
  packets skip the best-effort queue. The overlay socket uses AF41.
* **Protocol version** `proto` (`proto.rs PROTOCOL_VERSION`): 1 = 3-timestamp pong, integer-ms
  sync; **2** = 4-timestamp pong (`t2`), timeline anchors, sync-quality reports. A leader and its
  followers must run the same version: a mismatch is shown on the follower's node card, in the
  leader's log and health check ("update both to the same version"). Protocol 2 still parses
  protocol 1 packets (missing fields fall back), but only matching versions are supported.
  Beacons may carry `protoMin`/`protoMax` (§12.13, version tolerance for cluster updates).
* Optional fields added by the feature wave (all skipped when absent, protocol stays 2): sync
  `surprise` {id, kind, ref, targets[], startPos, durationMs, epoch} (§12.16) and
  `test.{map, mapRunId, identify, cal}` (§12.1, §12.5–§12.8); follower report `limiter`
  {activeGroups[], minScale, secondsLimited} (§12.11); manifest `power` (NodePowerBudget) and
  `settings.disabledPropIds` (§12.7, §12.11).

#### 7.4.2 Clock exchange (4 timestamps, kernel receive stamps)
```
ping : follower → leader  {"t":"ping","id","t0"}            t0 = follower clock when sent
pong : leader → follower  {"t":"pong","id","t0","t1","t2","boot"}
                           t1 = leader kernel RX stamp of the ping, t2 = leader clock when the pong is sent
                           t3 = follower kernel RX stamp of the pong
delay  = (t3 − t0) − (t2 − t1)        round trip without the leader's processing time
offset = ((t1 − t0) + (t2 − t3)) / 2  leader clock − follower clock
```
* Clocks are CLOCK_MONOTONIC ms since each daemon's start (f64). Receive times come from
  `SO_TIMESTAMPNS` (taken in the kernel network core, free of tokio wake-up, JSON and HMAC time;
  CLOCK_REALTIME converted to monotonic at receive time; stamps older than 100 ms are refused),
  falling back to userspace time where unavailable. `t0` is taken after the ping is encoded,
  `t2` just before encoding plus the lower envelope of recent encode times.
* Pings go out in **bursts**: 5 pings 20 ms apart every 2 s; a **fast-start burst** of 16 pings
  10 ms apart (every 200 ms) after adoption, a leader restart (new boot id) or a detected clock
  step, until 5 good samples exist (a usable estimate in ~200 ms). Unanswered pings over the last
  90 s give the loss rate.

#### 7.4.3 Clock model (`cluster/clock.rs ClockModel`)
Fits `offset(t) = a + b·(t − t_ref)` (offset **and drift** b) over the last **90 s**:
the best sample of each burst and of each 4 s bin; only samples within
`3 · max(0.3 ms, ½(p30 − min))` of the minimum delay; weights `1/(delay − min + 0.1 ms)²`;
the drift regularised toward the previous estimate (σ 200 ppm, clamp ±1000 ppm); one robust
pass drops residual outliers. A sample further from the fit than half its excess delay (+ fit
noise, drift uncertainty, 0.5 ms) is **quarantined**; low-delay quarantined samples in 3 bursts in a
row mean the clock stepped (e.g. NTP) and the fit restarts from them. An offset jump > 50 ms
(leader restart) or a new leader boot id resets at once. Without pongs the fit extrapolates
(holdover: 60 s cost 0.06 ms in simulation). Reported error bound: `min_delay/2 + rms`.

Simulated (unit tests, 30 min, drift ±45–100 ppm, heavy-tailed jitter, spikes, 5 % loss): clock
error mean 0.045 ms, worst < 0.5 ms; busy 2.4 GHz (asymmetric jitter, 15 % loss) worst < 1 ms.

#### 7.4.4 Sync packets: timeline anchors
Every 250 ms while active (every 2 s idle) and at once on a change (state, item, brightness,
blackout, anchor epoch), unicast to each follower:
```
{"t":"sync","leader":id,"showVersion","state":"idle|playing|paused|testing|effect","item":{type,id,name},
 "posMs":u64,"sentAtMs":u64,                                  // protocol 1 rendering (integer ms)
 "anchor":{"posMs":f64,"atMs":f64,"rate":f64,"epoch":u64},     // protocol 2
 "effect"?:EffectPreset,"test"?:TestRequest,"brightness","blackout"}
```
The anchor says: the lights timeline was at `posMs` when the leader clock read `atMs`, and
advances `rate` ms per leader ms (0 while paused). `atMs` is the **engine's own timestamp** of
the position (µs precision), not the time the packet was built. `epoch` changes on every
discontinuity (new item, seek, pause/resume, sound-delay change, a jump of the leader's audio
clock). Anchors are idempotent: loss and reordering cost nothing, holdover is exact to the
drift estimate. The follower converts an anchor to its own clock with the clock model:
`atLocal = to_local(atMs)`, `rateLocal = rate · (1 + b)`, so
`target(t) = posMs + rateLocal · (t − atLocal)` for any local time t.

#### 7.4.5 Following the timeline (`player/clock.rs Servo`)
Followers evaluate the anchor **every output frame** (not only when a packet arrives) and track
it with a servo: a new epoch or an error > 100 ms jumps; otherwise
`rate = rateLocal + clamp(e / 500 ms, ±2 %)` — continuous, **no deadband**, so model updates
of a fraction of a millisecond are absorbed within about a second. The leader's own lights use
the same kind of servo on its audio clock (a slow PI loop, τ ≈ 2 s, that learns the sound card's
rate and filters ALSA delay jitter); its anchors carry that smooth timeline and its learnt rate.
Loss: after 3 s without packets a follower plays on (holdover) to the end of the item, holds
3 s, fades to dark.

#### 7.4.6 Presentation time (DPI)
Each frame is chosen for the moment it **lights up**, on the leader and on followers:
`t_light = v + latch`, where v is the vblank the flip will take effect at (predicted from the
page-flip timestamps: the grid `last + n·R`, after any flip still pending) and `latch` the WS281x
latch delay of this node's strings (`L × line + 0.28 ms`; L = the longest string when
bottom-aligned, else the mean string length). The frame shown is the one covering the middle of
its slot: `idx = floor((pos(t_light) + R/2) / F)`. The output thread is paced from the vblank
grid: the next update is timed for the next frame boundary of the timeline
(`clock::next_update`), moved to the vblank at which it will show, and composing starts early
enough (smoothed tick time × 1.5 + 1 ms) for the flip to make it. When R < F, frames change only at
frame boundaries (the vblanks in between are skipped); when R > F, every refresh shows the frame at
the middle of its slot. Frame changes then land within **±R/2 of the ideal instant** on every node
(±12 ms at 800 px/40 Hz, ±6 ms at 400 px, ±1.7 ms at 100 px) instead of 0..R late plus a frame of
pipeline lag. Without a scanned-out display (simulation) frames light up when written and the
loop wakes at the timeline's frame boundaries; `PIXELPLUS_SIM_REFRESH_HZ` simulates a vblank grid.

#### 7.4.7 Sound delay and calibration
`settings.audio.outputDelayMs` (i32, −500…2000, default 0) is how much later the audience hears
the sound than it leaves the leader's audio output (FM transmitter, HDMI TV, Bluetooth, and
~2.9 ms per metre of air). The leader's lights timeline is `audio position − outputDelayMs`;
anchors carry that timeline, so every controller's lights are delayed alike. Changing it starts a
new epoch (followers jump at once). **Settings → Audio → Sync lights to sound** calibrates it:
`POST /player/calibration {on}` plays a click every second (`cache/calibration-click.wav`) while
every prop on every controller flashes white for 50 ms at the same timeline instants (item type
`calibration`); the user moves a slider (±1/±10 ms steps, presets) until flash and click
coincide where the audience stands. Item ends and the status `posMs` stay on the audio clock
(what is heard).

#### 7.4.8 Wi-Fi power save, sync quality, health
* Power save makes the access point hold packets for a dozing station (50–1000 ms). The image
  sets NetworkManager `wifi.powersave = 2` (`/etc/NetworkManager/conf.d/40-pixelplus-wifi-powersave.conf`
  and the package's appliance `50-pixelplus.conf`); `pixelplus-firstboot` and `pixelplus-netwatch`
  run `iw dev wlan0 set power_save off` as a fallback; the daemon checks `iw dev wlan0 get
  power_save` every minute.
* Followers report in their beacon (`report.quality`): clock error bound, fit jitter, drift ppm,
  RTT min/p50/p95, loss %, samples, the servo's timeline error, pixel refresh (Hz), kernel
  timestamps in use, and `wifiPowerSave`. `GET /nodes` / the `nodes` WebSocket message carry them as
  `sync`, `wifiPowerSave`, `protocol`. The Controllers page shows a badge per follower ("In sync
  ±0.3 ms": excellent < 2 ms, good < 5 ms, fair < 1 frame, poor otherwise or with power save on,
  > 10 % loss, another protocol) with a detail popover and fixes. The health check's **Timing**
  item fails on a protocol mismatch and warns on power save, > 5 ms error or > 10 % loss.

#### 7.4.9 Accuracy and limits
* Network timeline error: sub-ms on decent Wi-Fi (simulation: mean ≈ 0.04 ms, worst ≈ 0.2 ms at
  up to 100 ppm drift; busy 2.4 GHz worst ≈ 0.6 ms), tens of µs on loopback/Ethernet (e2e on
  one machine: ≤ 0.035 ms). The residual floor is the (unmeasurable) asymmetry of the minimum
  path delays.
* Visible alignment per node: ±R/2 (vblank quantisation; only shorter strings, i.e. a higher
  refresh, reduce it), plus string-length skew unless latch alignment is on.
* Lights vs sound: aligned to what leaves the leader's audio device; everything after it is the
  user's `outputDelayMs`. The Pi headphone jack reports its delay in ~10 ms steps (filtered).
* Hardware options (SYNC header, GPS PPS): `docs/HARDWARE-NOTES.md`.

### 7.5 Cluster security
**Keys.** One key per follower (§7.1), never shared: compromising one follower gives no access
to the leader's admin API, to other followers, or to anything but that follower's own slot. A
cluster key **never authenticates the admin API** (`/api/v1/*` outside `/cluster/*`), only the
calls between that follower and its leader. Manifests and slices carry no secrets (no
passwords, SMTP/MQTT settings or keys). Followers adopted by an older PixelPlus (one show-wide
key) keep working and are re-keyed automatically by a signed re-adoption.

**Signed HTTP** (`cluster/sig.rs`). Leader → follower (`/cluster/command`, `/cluster/release`,
re-adoption) and follower → leader (`/cluster/manifest/<own id>`, `/cluster/slice/<own id>/…`)
carry `X-PixelPlus-Auth: v1 <senderId> <unixTime> <nonce> <hmac>`: HMAC-SHA256 with that
follower's key over method, path + query, sender, time, nonce and the SHA-256 of the body. The
receiver rejects a time more than 30 s off its clock and any nonce it has seen. Controllers
without a real-time clock may disagree about the time: a correctly signed request outside the
window gets `401` with `X-PixelPlus-Time: <unixTime> <hmac>` (MACed over the request nonce), and
the sender retries once with the corrected offset. The leader's manifest and slice replies carry
`X-PixelPlus-Reply` (HMAC over the request nonce and the body hash / slice ETag + checksum), so
a follower only installs what its leader sent. The key itself is never sent.

**UDP** (`proto.rs`). Every authenticated datagram carries the sender's boot id (`bt`) and a
sequence number (`sq`) that grows with every packet, inside the HMAC. Receivers keep, per sender,
the current boot id, the highest sequence number and which of the 64 numbers below it were
seen, and drop anything seen before or older than that window (replays of sync, overlay, pong
or beacon packets; slightly reordered packets are still accepted once). A follower accepts a *new* leader boot id only from a pong that
answers one of its own pings of the last 5 s (a replay can't); an authenticated packet from an
unknown run triggers such a ping. Boot ids that were replaced are never accepted again. Pong
timestamps (`t1`, `t2`) and sync anchors are covered by the MAC like everything else. The
leader unicasts sync packets, pongs, overlay frames (`'P'` frames with boot id and sequence
number) and a copy of its beacon to each follower, MACed with that follower's key; its
broadcast beacon is unauthenticated (discovery only).

**Sensor nodes (§12.16, UDP 32422).** ESP32 sensor nodes use the same key derivation
(X25519 + HKDF-SHA256 as `sig.rs`) and the same JSON MAC canonicalization as cluster datagrams:
the object is serialized, `,"bt":"<boot>","sq":<seq>` is appended before the closing brace, then
`,"mac":"<64 hex>"}` where the MAC is HMAC-SHA256 over every byte before `,"mac"` plus the closing
`}`. Messages: `sbeacon` (unauthenticated discovery), `sevent`/`sstatus` (MACed, replay window as
followers), leader reply `sack`. Shared test vectors: `firmware/esp32-sensor/test/vectors.json`
(WS6). A sensor key authorizes only that node's events and its own config.

**Who may adopt a controller** (`POST /cluster/adopt`, `follower::handle_adopt`):

| This controller | Accepted when |
|---|---|
| unconfigured, or a follower without leader (released) | always: trust on first use, logged ("Adopted by show leader … at …") and shown on its page |
| follower of leader L | the call is signed by L with the current key; or it repeats an adoption by L whose key L never used yet (e.g. after a timeout); or its signed-in owner clicked **Allow a new leader** (15 min, `POST /system/join-show`); or — only when this controller has no password — `force` from the local subnet after L has been silent for 10 minutes |
| leader | only while its owner has **Join another show** open (15 min, optionally only for one leader address) |

`leaderUrl` must be an IP literal on a local network (private, loopback, link-local, CGNAT or
IPv6), so a stranger's adoption can't make the controller call arbitrary hosts.

**Beacon spoofing.** An unauthenticated beacon never replaces a peer entry whose beacon proved a
key in the last 30 s, and a second unverified device announcing the same id from another address
is kept out; either marks the entry as a **possible duplicate** (shown in *New controllers found*,
adoption refused until it clears for 60 s). A spoofed beacon can at most obtain a key for that
(unadopted) slot, which grants that slot's manifest and nothing else; the leader then trusts only
traffic MACed with that key. Right after an adoption the leader ignores the follower's stale,
pre-adoption beacon for up to 10 s (no duplicate "re-adoption").

---

## 8. HTTP API (`/api/v1`, JSON, camelCase)

Auth: if a password is set, `POST /api/v1/auth/login {password}` → session cookie `pp_session`
(minimum 6 characters for new passwords; sign-in is throttled per client address — 5 tries, then
1 min doubling up to 15 min — and globally after 50 failures in 10 min).
Unauthenticated requests get 401 except `/auth/*`, `/public/*` (song requests page), `/system`
(limited info for the sign-in screen), `/system/setup` while unconfigured (local network only) and
`/cluster/*` (signed with a per-follower key, §7.5). A local sidecar (games) sends
`X-PixelPlus-Local: <token>` from `/run/pixelplus/local-token` (random per daemon start, 0640
group `pixelplus-overlay`): accepted only from loopback, never with proxy headers
(`X-Forwarded-For`, `Forwarded`, `CF-Connecting-IP`, …), and only for `GET /show`, `GET /player`,
`GET /system`, `/ws`, `POST /player/{pause,resume,stop}` and `/overlay/*`.

Browser protection (`api/security.rs`), independent of the password:
* **Host allow-list** for `/api/v1/*` except `/public/*`: IP literals, `localhost`,
  `<hostname>`/`<hostname>.local`, and `settings.security.allowedHosts` (e.g. a tunnel domain;
  `*.example.com` allowed; env `PIXELPLUS_ALLOWED_HOSTS`). Others get `421` (friendly HTML page
  for browsers) — this defeats DNS rebinding.
* **CSRF header**: every request other than GET/HEAD/OPTIONS (except `/public/*`) must send
  `X-PixelPlus-Request: 1` (the web UI, sidecars and cluster calls always do), else `403 csrf`.
* **WebSocket** `Origin`, when present, must match `Host`; messages are limited to 64 KiB.
* **Headers** on every response: `Content-Security-Policy` (inline-script hashes of the built UI,
  recomputed when `index.html` changes; `PIXELPLUS_CSP` overrides, `off` disables),
  `X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY`, `Referrer-Policy: same-origin`.
* **Secrets are write-only**: `GET /show` and `PUT /show/settings` return SMTP and MQTT passwords
  as `"********"`; sending that placeholder back keeps the stored value. `POST /mqtt/test` uses the
  stored password only for the stored broker (host + port).
* **Client address** (song-request rate limit, sign-in throttle): forwarding headers count only
  from this machine (`CF-Connecting-IP`, else the right-most `X-Forwarded-For` hop) or from
  `settings.security.trustedProxies`.

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
| `POST /player/play` | {playlistId? \| sequenceId? \| djClipId? \| effectId? \| mediaId?, startIndex?, loopUntilStopped?:bool} — see *Manual playback* below |
| `POST /player/stop` {fade?:bool}, `/player/pause`, `/player/resume`, `/player/next`, `/player/previous`, `/player/seek {posMs}` | |
| `PUT /player/volume {volume}`, `PUT /player/brightness {brightness}` | |
| `POST /player/calibration {on}` | "Sync lights to sound" test pattern (click + white flash every second, all controllers), §7.4.7 |
| `POST /test/start` | {mode:"solid"|"chase"|"rgbCycle"|"countPixels"|"walk"|"effect"|"mapCode"|"identify"|"calibration", color?, target:{nodeId?, output?, propIds?, groupIds?, all?}, effect?: EffectPreset, map?: MapPlan, mapRunId?, identify?: [{nodeId, output, color, blinks}], cal?: {seed, v}} (new modes: §12) |
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
| `GET /cluster/manifest/:nodeId`, `GET /cluster/slice/:nodeId/:seqId`, `POST /cluster/adopt`, `POST /cluster/command`, `POST /cluster/release` | cluster internal (signed, §7.5) |
| `GET/POST/DELETE /system/join-show` | {open, secondsLeft, leaderAddress}; POST {leaderUrl?}: for 15 min another leader may adopt this controller ("Join another show" / "Allow a new leader") |
| `GET /journal?date=YYYY-MM-DD&types=a,b` | one local day of the show journal (§12.10): `[{ts, ev, …fields}]` |

**Feature-wave endpoints** (route modules exist as stubs in `api/<module>.rs`, already merged
into the router; each workstream fills in its own; shapes in §12 and `web/src/lib/api/types.ts`):

| Module (owner) | Endpoints |
|---|---|
| `tls` (WS1) | `GET /tls/status`, `POST /tls/rotate {ca}`, `GET /public/ca.crt`, `GET /public/ca.mobileconfig` |
| `calibration` (WS1) | `POST /calibration/result {residualMs, spreadMs, matches, device?, apply}`; `POST /player/calibration {on, pattern?:"v1"\|"v2", seed?}` (playerapi, WS3) |
| `autoshow` (WS2) | `GET /media/:id/analysis`, `POST /media/:id/analyze`, `GET /autoshow/styles`, `POST /autoshow`, `POST /autoshow/preview`, `POST /sequences/:id/regenerate` |
| `preview` (WS2) | `GET /sequences/:id/preview`, `GET /sequences/:id/preview/data` |
| `library` (WS2) | `POST /sequences/tags`, `GET /playlists/:id/preview?date=&start=`, `GET /library/history?days=` |
| `mapping` (WS4) | `POST /mapping/runs`, `POST /mapping/runs/:id/{stop,results,apply}`, `GET /mapping/runs[/:id]`, `DELETE /mapping/runs/:id` |
| `pixelcount` (WS4) | `POST /pixelcount/start`, `POST /pixelcount/:session/answer`, `POST /pixelcount/:id/{result,apply}` |
| `wizard` (WS4) | `POST /wizard/receiver/identify-jack`, `POST /wizard/receiver/:session/{jack,port/:n/light,finish,cancel}` |
| `profiles` (WS6) | CRUD `/profiles`, `POST /profiles/:id/activate`, `POST /profiles/capture`, `GET /profiles/preview-switch/:id` |
| `reports` (WS6) | `GET /reports?limit=`, `GET /reports/:date`, `POST /reports/run {date?, send?}` |
| `remote` (WS5) | `GET /remote/status`, `POST /remote/tailscale/{install,up,serve,funnel,down}`, `POST /remote/cloudflare/{install,quick,token,hosts,stop}`, `POST /remote/test` (§12.12) |
| `power` (WS3) | CRUD `/power-supplies`, `GET /power/live` (`/power/estimate` gains `simulateLimiter`) |
| `sensornodes` (WS6) | `GET /sensor-nodes/discovered`, `POST /sensor-nodes/adopt`, CRUD `/sensor-nodes`, `POST /sensor-nodes/:id/release`, `GET /sensor-nodes/:id/live`, `POST /surprises/test`, `GET /cluster/sensor-config/:id` |
| `fppcompat` (WS6) | **root-mounted** (not `/api/v1`): `GET /config.php`, `/api/system/info`, `/api/sequence/:name/meta`, `/api/media/:name/meta`, `PATCH /api/file/:dir`, … (§12.14) |
| cluster/system (WS5) | `POST /nodes/:id/replace`, `POST /nodes/:id/release-retired`, `POST /system/transfer/export` → one-time `GET /system/transfer/download/:token`, `POST /system/setup` multipart restore (§12.9); `GET /system/update[?refresh=true]` (extended), `POST /system/update`, `PUT /system/update/settings`, `POST /system/update/rollback`, `GET /cluster/update/:file` (§12.13); WS `updateJob` |

**Manual playback** (`player/scheduler.rs`): playback started by hand *during* a show window
ends with that window: at the window's end it ends with the entry's `endBehavior` (finish the
song and play the outro / stop / fade), and at once if a higher-priority window takes over; if
it finishes by itself inside the window, the window's playlist starts again. Started *outside*
the windows, a playlist plays **once** (its `repeat` is ignored) and a window that starts
meanwhile waits for it. With `loopUntilStopped` ("Loop until I stop" next to *Play show now*)
the playlist repeats and keeps playing past window ends until the user stops it. Pressing Stop
during a window keeps that window quiet until the next one.

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

* Feature wave (§12): `job` {id, kind:"analysis"|"autoshow"|"preview", pct, state, result?}
  (F2/F3), `power` {nodes:[{nodeId, groups:[{id, amps, budget, scale}]}]} every 1 s while
  playing (F12), `mapping` {runId, state, pct} (F6), `sensorInput` {sensorNodeId, input, state,
  at} (F20). `status.item.type` may be `"countdown"` / `"surprise"`; `status.power?` =
  {limiting, minScale}. Web pages listen with `app.onMessage(type, cb)`.

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

11. **Feature wave** (§12): **Reports** (`/reports`), **Map my yard** (`/map`), **Seasons**
   (`/settings/seasons`), **Sync to sound** (`/calibrate`), **Trust this phone** (`/trust`, public);
   Settings → *More*: `/settings/{seasons,power,sensors,reports,xlights,https,remote,updates}`.
   The dashboard's *Play show now* has a **Loop** switch ("Loop until I stop", §8 *Manual playback*).

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
  follower over UDP 32421 (`u8 'O' | propId len-prefixed | frameNo u32 | RGB`).

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

---

## 12. Feature wave (F1–F20)

Design: the feature spec (features 1, 2, 3, 4, 6, 7, 8, 9, 10, 11, 12, 14, 15, 16, 18, 20).
The shared contracts were created up front by WS0 and are frozen: `model.rs` (§4.0),
`player/types.rs` (`TestRequest.{map, mapRunId, identify, cal}`, `PlayRequest.loopUntilStopped`,
`PlayerStatus.power`, `SurpriseAnchor`), `cluster/proto.rs` / `manifest.rs` fields (§7.4.1),
`api/mod.rs` + `services/mod.rs` (stub modules, one file per owner), `Cargo.toml` dependencies
(`rustfft`, `rcgen` + `tokio-rustls` on *ring*, `minisign-verify`, `aes-gcm`),
`web/src/lib/api/types.ts`, the mock (`web/src/lib/mock/feat/<module>.ts`, one per owner), nav
and placeholder pages. Changes to a shared contract go through WS0. Each workstream writes only
inside its own section below.

| § | Feature | Owner | Daemon | Web |
|---|---|---|---|---|
| 12.1 | F1 A/V auto-calibration + HTTPS (local CA) | WS1 | `services/tls.rs`, `api/tls.rs`, `api/calibration.rs`, `core/calpattern.rs` | `/calibrate`, `/trust`, `/settings/https` |
| 12.2 | F2 beat/tempo analysis, auto light show | WS2 | `services/analysis.rs`, `api/autoshow.rs`, `core/audio_analysis.rs`, `core/autoshow.rs` | sequences page |
| 12.3 | F3 browser sequence preview | WS2 | `api/preview.rs`, `core/preview.rs` | layout page |
| 12.4 | F4 countdown / exact show start | WS3 | engine, `EffectKind::Countdown` | playlist editor |
| 12.5 | F6 camera prop mapping | WS4 | `api/mapping.rs`, `services/mapping.rs`, `core/mapcode.rs` | `/map` |
| 12.6 | F7 pixel-count check | WS4 | `api/pixelcount.rs` | props page |
| 12.7 | F8 season profiles | WS6 | `api/profiles.rs`, `services/profiles.rs` | `/settings/seasons` |
| 12.8 | F9 new receiver wizard | WS4 | `api/wizard.rs` | controllers page |
| 12.9 | F10 controller replacement | WS5 | cluster, `api/nodes.rs`, snapshots | controllers page |
| 12.10 | F11 journal + nightly report | WS0 (journal), WS6 | `services/journal.rs`, `services/reports.rs`, `api/reports.rs` | `/reports`, `/settings/reports` |
| 12.11 | F12 power limiter + dimming | WS3 | `player/limiter.rs`, `api/power.rs` | `/settings/power` |
| 12.12 | F14 remote access | WS5 | `services/remote.rs`, `api/remote.rs` | `/settings/remote` |
| 12.13 | F15 signed OTA + cluster updates | WS5 | `services/updates_orch.rs` | `/settings/updates` |
| 12.14 | F16 xLights FPP Connect | WS6 | `api/fppcompat.rs` (root-mounted) | `/settings/xlights` |
| 12.15 | F18 tags, smart playlists | WS2 | `api/library.rs`, `core/smartlist.rs` | sequences / playlists pages |
| 12.16 | F20 surprises + ESP32 sensor nodes | WS3 (engine), WS6 | `services/sensornodes.rs`, `api/sensornodes.rs` | `/settings/sensors` |

### 12.1 A/V auto-calibration and HTTPS (F1, WS1)

**Secure connection (HTTPS).** Browsers only give pages the camera and microphone in a secure
context, so the leader runs its own small certificate authority (`services/tls.rs`):

- **Local CA**: "PixelPlus Local CA – <show> – <id>", EC P-256, 10 years, `CA:true pathlen:0`,
  **critical name constraints**: DNS `local`, `lan`, `home.arpa`, `internal`, `localhost` and the
  single-label host name at creation; IP 10/8, 172.16/12, 192.168/16, 169.254/16, 100.64/10
  (Tailscale), 127/8, fc00::/7, fe80::/10, ::1. A stolen key can't impersonate public sites (tested:
  rustls rejects a CA-signed `www.example.com` leaf with `NameConstraintViolation`). Dates never go
  before 2026-01-01 (a Pi without RTC may boot in 1970).
- **Leaf**: 397 days, SANs = `<host>.local`, `<host>.lan`, `<host>`, `localhost`,
  `settings.https.extraNames`, every interface address (no fe80) — filtered to what the CA may sign
  (`/tls/status.rejectedNames` lists the rest). Checked every 60 s and on every show change;
  re-issued when a wanted name/IP is missing, 30 days before expiry, or when the CA changed. rustls
  reads it through a `ResolvesServerCert`, so re-issue is hot (no restart, no phone action).
- **Files** `<data>/tls/`: `ca.key` (0600), `ca.crt`, `ca.json` (CN + DNS constraints; rcgen rebuilds
  the issuer from them), `leaf.key` (0600), `leaf.crt`, `leaf.json`. Not in normal snapshots; the F10
  transfer bundle carries the CA via `tls::export_ca(data_dir)` / `tls::import_ca(...)` (WS5).
- **Followers** don't serve HTTPS (`tls::active()`: port ≠ 0, `settings.https.enabled`, role ≠ follower).
- **OLED**: while `/trust`, `/public/tls` or `/tls/status` was used in the last 10 minutes, the song
  line shows `CA 1A:2B:3C:4D:5E:6F` (first 6 fingerprint bytes; `tls::oled_line`).

**Listeners** (`src/listeners.rs`, called from `main.rs`; bind lazily, retry every 30 s, never stop
the daemon):

| Listener | Address | Router | Notes |
|---|---|---|---|
| HTTP | `PIXELPLUS_HTTP_BIND:PIXELPLUS_HTTP_PORT` (:80) | `api::router` | never redirected |
| HTTPS | same bind, `PIXELPLUS_HTTPS_PORT` (443; Docker 8443; 0 = off) | `api::router` + `https_layer` | TLS 1.2/1.3 (ring), handshakes in parallel (≤ 64, 10 s timeout), connections dropped while `!tls::active()`; `https_layer` inserts the `ViaHttps` extension and adds `; Secure` to every `Set-Cookie` |
| Public | `127.0.0.1:PIXELPLUS_PUBLIC_PORT` (8081; 0 = off) | `listeners::public_router` = `api::router` behind WS5's `security::public_only` allow-list | 503 while `settings.remote.publicListener` is off; every response `Connection: close`; `/play` → 308 `/play/`, `/play/<rest>` bridged at TCP level to the games controller `127.0.0.1:<settings.games.port>/<rest>` (HTTP and WebSocket), appending the peer to `X-Forwarded-For` |

The games bridge peeks each connection's first request head (≤ 16 KiB); the games sidecar answers
one request per connection, and axum closes every public connection, so every request is routed
afresh even through a tunnel's connection pool. `secureNow` in `/tls/status` is true for `ViaHttps`
or a loopback peer sending `X-Forwarded-Proto: https` (tailscale serve / cloudflared).

**API** (`api/tls.rs`, `api/calibration.rs`):

| Endpoint | Auth | Result |
|---|---|---|
| `GET /tls/status` | admin | `{enabled, port, active, listening, error, role, caFingerprint, caSubject, caCreatedAt, caNotAfter, leafNames[], leafNotAfter, leafIssuedAt, rejectedNames[], urls:{lan[], tailscale?, tunnel?}, secureNow}` |
| `POST /tls/rotate {ca}` | admin | new leaf; `ca:true` = new CA (every phone must trust again; toast) |
| `GET /public/tls` | none | `{available, role, port, caFingerprint, caSubject, leafNames[], urls[], secureNow}`; names/URLs only for LAN peers without proxy headers |
| `GET /public/ca.crt` | none | CA certificate, DER, `application/x-x509-ca-cert`, `PixelPlus-CA.crt` |
| `GET /public/ca.mobileconfig` | none | iOS profile (`com.apple.security.root`, UUIDs derived from the fingerprint) |
| `POST /player/calibration {on, pattern:"v2", seed?}` | admin | (WS3, player API) restarts the pattern from position 0 with a new seed: `{seed, v, eventsMs, flashMs, windowMs, leadInMs, chirp:{ms,f0Hz,f1Hz,rampMs}, startsInMs}` |
| `POST /calibration/result {residualMs, spreadMs, matches, device?, apply}` | admin | `{outputDelayMs, previousDelayMs, previousCalibration, calibration, clamped}`; with `apply`, `outputDelayMs += round(residualMs)` clamped to −500…2000 and `audio.lastCalibration` set, via the show store (followers follow) |
| `POST /calibration/undo {outputDelayMs, lastCalibration?}` | admin | restores what the page saw before applying |

**Pattern v2** (`core/calpattern.rs`, mirrored in `web/src/lib/sensing/schedule.ts`; shared test
vector `events_ms(1)[..8] = 2000, 2570, 3200, 4010, 4760, 5630, 6500, 6950`): cycle `RUN_MS` 60 s
(the engine loops it), 2 s dark lead-in, events from a 32-bit Galois LFSR (taps `0x80200003`,
clocked 8× per event, seed 0 → `0x5EEDCA11`): `gap = 450 + (state mod 8)·60 ms`, none in the last
1 s. Light: everything full white for `max(80 ms, 2 slots)` from each event
(`CalSchedule::flash_on`, O(log n); `flash_on_v2(pos, seed, slot)`). Sound: an 8 ms linear chirp
2→4 kHz with 0.5 ms raised-cosine ramps, level 0.85, starting exactly at each event; mono 16-bit
24 kHz WAV (`calpattern::wav`, cached as `cache/cal-<seed>.wav`).

**Measurement (phone, `web/src/lib/sensing/`)** — no phone↔controller clock sync is needed:
both streams are timed on the phone clock (`performance.now()`) and each is correlated with the
schedule separately.

1. *Microphone* (`mic.ts`, `audio-onset.ts`): raw getUserMedia audio (echo cancellation, noise
   suppression, AGC off) through an AudioWorklet (ScriptProcessor fallback), chunks tagged with the
   context frame. Band-pass 1.5–5 kHz (2+2 biquads) → FFT overlap-save matched filter with the chirp
   passed through the same band-pass (zero net phase) and its Hilbert quadrature (envelope ignores
   polarity/phase smear) → onset = first envelope sample above `median + 8·MAD` of the last 2 s,
   refined to the peak within one template length, parabolic interpolation; 250 ms refractory.
   Frame → phone time: least-squares fit of `getOutputTimestamp()` pairs, minus `outputLatency`,
   minus input latency (`track.stats` latency when present, else `baseLatency`).
2. *Camera* (`camera.ts`, `frames.ts`, `frames.worker.ts`, `video-onset.ts`): back camera
   640×480@30, `requestVideoFrameCallback` time = `captureTime` (else `expectedDisplayTime − 1
   frame`); exposure locked to ≈ 0.95 frame time when the phone allows it. Each frame → 160×120
   luma in a worker (OffscreenCanvas; page fallback), linearised (γ 2.2); signal = mean of the top
   5 % positive differences from the per-pixel minimum of the previous 8 frames. Flash onset =
   `t_j + Δt·(1 − f)` for the first partly lit frame j (`f` = its level relative to the flash's
   plateau), plus a rolling-shutter correction `0.75·Δt·(centroid_y − 0.5)`.
3. *Association* (`associate.ts`): pairwise differences detection − schedule (periodic by
   `windowMs`) in 2 ms bins, ±25 ms box filter → offset; unique matches within ±25 ms, two median
   refinements, then the inlier mean; spread = 1.4826·MAD. Video offset searched ±1.5 s around
   `pos0 + currentDelay` (`pos0` = reply midpoint + `startsInMs`); audio relative to video within
   the plausible residual range (−150…900 ms of delay, or …2300 ms with "radio").
4. *Result* (`calibrate.ts`): `residual = (o_audio − o_video) − phoneBias`; needs
   ≥ min(20, 60 % of expected) matches per stream (≥ 8), spreads ≤ 15 ms and a clear histogram
   peak (≥ 1.5× runner-up), else a specific hint ("Too noisy — move closer to the radio…").
   Badge: spread < 5 ms Excellent, < 12 ms Good, else Measure again. Shown ± = 95 % of the mean's
   error plus 5 ms (2 ms after the clap test) for the phone. Verify run passes at |residual| < 8 ms.
   A reply without a seed (old controller) falls back to the classic 1 s pattern, ±500 ms range.
5. *Phone bias* (`clap.ts`, `device.ts`): 14 s clap test at ~1 m; claps heard (1–10 kHz attack)
   vs hands stopping on camera (motion drop, sub-frame); `bias = median(heard − 2.9 ms − seen)`,
   kept in `localStorage["pp.avBias.<model>"]` if ≥ 5 pairs, spread < 20 ms, |bias| < 150 ms.

Accuracy target (spec): ±10 ms without, ±5 ms with the clap test. Synthetic tests (vitest:
chirps in noise/hum/echo/inversion; frames with flashes mid-exposure, 15–60 fps, drift, jitter,
rolling shutter; association with 30 % drop-outs + false positives; end-to-end sessions) recover
the delay within 2 ms. **Needs real phones**: bypassed-certificate getUserMedia on Android Chrome,
Pixel/Samsung CA install paths, `captureTime`/`track.stats` behaviour per OEM, the rolling-shutter
readout fraction, and accuracy against a scope on the SYNC header (3+ phones, FM and Bluetooth).

**Web**: `/calibrate` (admin; SecureGate → where are you + "radio" → allow camera & microphone →
aim with live "Seeing flashes / Hearing clicks" meters → ~25 s measuring ring (screen kept awake) →
result: Apply (toast with Undo) / Measure again / Adjust by hand, optional "Check it" verify run;
"Fine-tune for this phone" clap test); `/trust` (public: fingerprint, Android / iPhone steps,
download, "Open secure page", Proceed-anyway fallback); `/settings/https` (on/off, status, QR code
to `/trust`, fingerprint, addresses, other names, renew / reset); SyncWizard's
"Measure with my phone" entry. `components/ui/SecureGate.svelte` (used by the mapping pages too)
offers, when a page isn't secure: Tailscale URL → `https://<same host or first LAN IP>[:port]<path>`
(+ "Make this phone trusted" / "Advanced → Proceed") → Cloudflare admin host.

### 12.2 Audio analysis and auto light show (F2, WS2)

**Files:** `core/audio_analysis.rs` (DSP), `core/autoshow.rs` (generator), `services/analysis.rs` (job queue), `services/media.rs` (`decode_mono`), `api/autoshow.rs`. Web: `lib/library/*`, `components/library/{AnalysisView,AutoShowDialog}.svelte`, Sequences page.

**Analysis pipeline.** Every audio import (`api/content.rs` → `import_media_file`) queues a background `analysis` job. The worker decodes with symphonia (ffmpeg fallback), downmixes to mono and streams into `audio_analysis::Analyzer` (input ≥ 32 kHz is halved first). DSP: Hann STFT 1024/256, log-magnitude flux in 8 log bands (30 Hz–11 kHz) → onset strength (1 s mean removed, σ-normalised); tempo by autocorrelation × log-Gaussian prior (µ 120, σ 1 octave) with the 80–160 BPM octave rule plus an "equal off-beats → faster tempo" check; local tempo per 20 s (`tempoCurve`); Ellis DP beat tracker (tightness 100); BPM refined by the least-squares slope of the beats; downbeats = bar phase with most bass onset + chroma change; energy at 10 Hz (RMS, low/mid/high, each 0–255 between the song's 5th–95th percentile); sections from a Gaussian checkerboard novelty (±4 s) over the energy vectors, ≥ 8 s apart, labelled low/mid/high. Memory is per-frame scalars only; audio beyond 20 min is ignored.

**Output.** `media/<id>.analysis.json` (`v`=1; `{v, sr, hopMs, durationMs, bpm, bpmConfidence 0..1, tempoCurve[], beats[ms], downbeats[ms], downbeatConfidence, onsets[{ms,strength 0..1,band 0|1|2}], energy10Hz{rms,low,mid,high: u8[]}, sections[{startMs,endMs,level}]}`, ≈ 50 KB for 4 min) and the summary `Media.analysis`. A maintenance sweep (start-up, show changes, every 5 min) queues missing / outdated analyses; failures are not retried until "Analyze again".

**Jobs.** One worker thread `pp-analysis` at `nice 10`; interactive jobs (auto show, preview, re-analyse) before background ones; on a Pi Zero background work waits while a scheduled show runs. WS `job` `{id, kind:"analysis"|"autoshow"|"preview", pct, state:"queued"|"running"|"done"|"failed", result?:{sequenceId?,message?}, subject?}`; `GET /jobs`, `GET /jobs/:id` for polling.

**Auto light show.** `autoshow::render` is a pure function (props, analysis, style, seed) → FSEQ v2 zstd at 25 ms in the show's full channel space (unused props dark), written through `FseqWriter` with a deterministic header id, so the same inputs give byte-identical files (and followers keep cached slices). Per section an effect family by level (low: wash/twinkle/breathe; mid: chase/wave/candy; high: meteor/fast chase/sparkle) rendered with `EffectRenderer`, each look scaled to its level's mean brightness; palettes rotate on downbeats; section changes cross-fade 400 ms; beats pulse the "beat group" (trees, matrices, arches, the biggest props) with `exp(−Δt/120 ms)`, beats < 330 ms apart skipped; top-5 % bass hits in loud sections flash warm white for 60 ms, ≥ 340 ms apart (≤ 3/s, never saturated red); high-band hits sparkle small props. Styles: classic, candy, rock, calm, party, voice ("speak with lights", an energy follower for DJ clips). The result is a normal `Sequence` (`generated: GeneratedInfo`, tag `auto`); `props_hash` changes (layout edits) re-render it in the background with the same id, name and tags.

**API.** `GET /autoshow/styles`; `POST /autoshow {mediaId, style?, propIds?, seed?, name?}` → `{jobId, seed}`; `POST /autoshow/preview` (same) → `{jobId, seed, sequenceId:"tmp-…"}` (temporary, deleted after 1 h, previewable via §12.3); `POST /sequences/:id/regenerate {style?, seed?, propIds?}`; `GET /media/:id/analysis` (404 `not_ready`); `POST /media/:id/analyze`.

**For the engine / effects (WS3).** Fixed-tempo beat params live in `effects::BeatPulse`. Song-following code uses `Analysis::beat_envelope(t, τ)`, `downbeat_envelope`, `section_at`, `energy_at` and `audio_analysis::envelope(beats, t, τ)`, all pure functions of timeline position; `services::analysis::load_analysis(state, mediaId)` loads a song's analysis.

**Performance** (release build, one core of a 2.1 GHz Xeon; `cargo test --release -p pixelplus-core perf_budget -- --ignored --nocapture`): 4-minute song analysis DSP 0.30 s; auto show for 2,000 px × 4 min (9,600 frames) 1.05 s; its preview 0.44 s (3.9 MB). A Cortex-A53 at 1 GHz (Pi Zero 2 W) is roughly 8–12× slower per core, so expect ≈ 3 s DSP + 5–10 s MP3 decode per song, ≈ 10 s per auto show and ≈ 5 s per preview, all at `nice 10`. **Needs hardware validation** on a Zero 2 W (timings, playback unaffected while a job runs) and a subjective review of the styles on real props.

### 12.3 Browser sequence preview (F3, WS2)

**Files:** `core/preview.rs` (format), `services/analysis.rs` (cache + build jobs), `api/preview.rs`; web `lib/preview/{pppv.ts,source.ts,player.svelte.ts}`, `components/viz/{LayoutCanvas,PreviewTransport,SequencePreview}.svelte`, Layout page (Live · Preview · Arrange; `/layout?preview=<id>`), Sequences page (row action Preview), auto-show dialog.

**Format `PPPV` v1:** `"PPPV" | u32 LE header length | header JSON (space padded) | gzip blocks`. Header `{v, seqId, frameMs (≥ 50), frameCount, frameBytes, props:[{id, n, idx?}], blockFrames: 64, blocks:[{offset, len}], mappingHash}`; offsets are absolute. A frame is, per prop in header order, `n` RGB triplets of the prop pixels listed in `idx` (absent = all, in order) — channel data before colour order/gamma/brightness, identical to the live WebSocket preview for full-resolution props. Props over 300 px keep a uniform subsample (matrices a grid), ≤ 8,192 samples in all. Each 64-frame block is gzip on its own so browsers inflate it with `DecompressionStream('gzip')`.

**Cache.** `cache/preview/<seqId>-<mappingHash>.pppv`; `mappingHash` covers the fseq sha256, frame rate and every prop's id, size, channel runs and sampling, so re-uploads and layout changes make a new file (older ones for the sequence are deleted; deleting a sequence deletes its previews). 500 MB quota, least recently served first. Built once by a `preview` job, served to any number of phones.

**API.** `GET /sequences/:id/preview` → header (`ETag`) or **202** `{jobId, pct, state}`; `GET /sequences/:id/preview/data` → the file with `Range` support; `GET /sequences/:id/preview/block/:n` → one gzip block (what the web client uses). Responses carry `Content-Encoding: identity` so the compression layer leaves them alone. Leader only; never touches outputs.

**Player.** `PreviewPlayer` fetches the header (following the job via WS `job` + polling), keeps an LRU of 8 inflated blocks, prefetches the next block and on seek, and draws `frame = floor(t / frameMs)` through the `FrameSource` interface that `LayoutCanvas` now takes (`source` prop; default `liveSource` = the real lights). The song's `<audio>` is the master clock (speed ½×/1×, scrubbable waveform); light-only sequences and demo mode use a local clock. The transport always says "Preview only — your lights are not affected."

### 12.4 Countdown and exact show start (F4, WS3)
**Item.** `PlaylistItem {type:"countdown", durationMs, matrixPropId?, text="{s}", color?, others:"fill"|"pulse"|"dark",
finale:"flash"|"none", djClipId?, djOffsetMs, tick}` (usually the last intro item; the playlist editor embeds
`components/playlist/CountdownItemEditor.svelte`). It plays as an `EffectKind::Countdown` preset built by
`effects::countdown::countdown_preset` (id `countdown:<itemId>`, target all props; length clamped to 1 s…10 min):
* the matrix (`matrixPropId`, else the largest matrix prop; resolved on the leader so every node draws on
  the same one) shows the text (`{s}` seconds left rounded up, `{mm}`/`{ss}`, `{m}`) with the `core::text`
  5×7 font (3×5 on small matrices) at the largest scale that fits, centred, through `MatrixInfo.pixelMap`;
  the digits get a short accent at each change;
* other props: `fill` = a progress bar along each prop (`pos/duration`), `pulse` = a flash decaying over
  260 ms at every digit change, `dark`;
* `finale: flash` = every prop white for the last 200 ms before zero.

Rendering is a pure function of the item position, so followers draw it from the preset the leader sends as
the sync `effect` (item type `countdown`), on the song's timeline anchor. **Audio:** the DJ clip starts at item
position `durationMs − clipMs + djOffsetMs` (its beginning is skipped when it is longer than the countdown) and
keeps playing past zero when the offset is positive; without a clip, `tick` plays
`cache/countdown-<ms>.wav` (a soft 1 kHz tick at every digit change). No crossfade ever leaves a countdown:
the next item starts at zero (±1 frame). Status: `item.type = "countdown"`; seconds left =
`ceil((durationMs − posMs)/1000)`.

**Exact start.** `ScheduleEntry.startExact`: `scheduler::facts_for` makes the entry active its intro's length
early (`intro_lead_ms`: countdowns, pauses, looks (0 = 30 s), sequences, audio and DJ clips of known length;
commands 0), only when no other window is active. The early window has the same key (`entryId@start`) as the
real one, so nothing restarts at the start time. Facts are evaluated once a second: `ActiveWindow.lateMs`
says how late the intro starts, and the engine starts the first intro item that far in, so the first song
begins at the entry's start within a frame.

### 12.5 Camera prop mapping (F6, WS4)
The phone films the display while every output blinks its own code; the browser decodes which
output (and which pixel of it) lit up where, then proposes wiring fixes and layout positions. Nothing
changes until the user applies it (auto snapshot first, Undo = restore).

**Pattern** (`core/mapcode.rs`, mirrored bit for bit in `web/src/lib/cv/mapcode.ts`; shared test
vectors `crates/pixelplus-core/tests/fixtures/mapcode/vectors.json`, regenerate with
`MAPCODE_BLESS=1 cargo test -p pixelplus-core mapcode`). A pure function of the plan and the test's
timeline position, so every node lights its own outputs without talking during the run. Time is
counted in slots of `bitMs` (default 200; the phone picks 250/320 for cameras under 20/13 fps):

| Part | Slots | Shows |
|---|---|---|
| lead-in | 5 | dark |
| per pass (× `passes`, default 3): preamble | 12 | `1 1 1 0 0 0 1 1 1 0 0 0` — all target pixels at `level` (clock + per-camera-pixel on/off references) |
| phase A (`phases & 1`) | 12 or 16 | codeword bit *i* (MSB first) of target *k* = `codebook(bits)[k]` |
| phase B (`phases & 2`) | 2 × `pixelBits` | Gray bit *j* (MSB first) of the pixel index, then its complement |
| gap | 3 | dark |

Codebooks are constant-weight with pairwise distance ≥ 4: 12 bits = the 132 hexads of the Steiner
system S(5,6,12) (supports of the weight-6 extended ternary Golay words, sorted; optimal A(12,4,6)),
16 bits = 870 greedy weight-8 words (runs with more than 132 outputs). Constant weight lets the
decoder take the top *w* soft values with no global threshold; the differential pairs of phase B
threshold themselves. Pixels ≥ `maxPixels` stay dark; non-target outputs of nodes in the plan are
dark. Engine hooks (WS3): `mapcode::render_output(plan, node, output, pos_ms, rgb) -> bool`,
`frame_for`, `is_done`; `MapPlan.countProbe = k` is a static frame for F7 (pixels `0..k` dim green,
pixel `k` red). Level is capped at 127 (50 %). With 25 outputs and 2048-pixel strings a pass is
9.8 s, the run ≈ 30 s.

**Daemon** (`api/mapping.rs`, `services/mapping.rs`; admin only):

| Endpoint | Result |
|---|---|
| `POST /mapping/runs {scope:{all}\|{nodeId}\|{propIds}, bitMs?, level?, passes?, force?}` | builds the plan (wired outputs of the scope ordered by `(nodeId, output)`, `maxPixels` = end of the output's last segment), stores `mapping/<id>.json`, starts test mode `mapCode` (`TestRequest.map` + `mapRunId`), stops it `totalMs + 1.5 s` later → `{runId, kind, startedAt, plan, schedule:{bitMs, leadInMs, preambleMs, phaseAms, phaseBms, gapMs, passMs, totalMs, codeBits, pixelBits, passes}, codebook:[[bits]], targets:[{k, nodeId, output, label, propIds, configured}]}`; 409 while a scheduled show plays unless `force` |
| `POST /mapping/frame {on, force?}` | dim white (`#262626`) on every prop while the user frames the shot (ends by itself after 5 min) |
| `POST /mapping/runs/:id/stop` | stops the pattern if it is this run's |
| `POST /mapping/runs/:id/results {detected:[{k, pixels:[[idx, x, y, conf]]}], proposals:[{id, kind, propId?, message, data?}], stats?}` | stored with the run (x, y normalised 0..1; `idx −1` = a region where the output was identified but its pixels were too dense to separate); 16 MB limit |
| `PUT /mapping/runs/:id/photo` (raw JPEG ≤ 2 MB), `GET …/photo`, `POST …/photo/background` | the still for the review screen; *background* copies it to `<data>/layout/background.jpg` (`GET /mapping/background`) |
| `POST /mapping/runs/:id/apply {proposalIds}` | dry run, auto snapshot "Before camera mapping", then in one show update: `swap` `{a:{propId, segment}, b:{…}}` exchanges the two segments' node/output/startPixel/nullPixels; `reverse` `{propId, segment}` toggles `reverse`; `pixelCount` `{nodeId, output, count, dead, updatePropCount}` (see §12.6); `layout` `{propId, layout}` sets the layout with `source: "camera"` (points clamped to 0..1, dropped if their count ≠ `pixelCount`); `notSeen`/`info` are information only; overlapping segments are refused → `{show, snapshotId, applied[]}` |
| `GET /mapping/runs` (newest first, `detected` omitted), `GET/DELETE /mapping/runs/:id` | the newest 30 runs are kept |

**Cluster**: the engine distributes the plan (WS3; spec: the full plan once by `/cluster/command`,
only `mapRunId` in sync packets). `pos_ms` must be the shared timeline position of the test, so a
follower's bits line up with the leader's within the sync accuracy (≪ 200 ms bits).

**Browser** (`web/src/lib/cv/`, pure TypeScript, no OpenCV):

- *Capture* (`capture.ts`, on WS1's `sensing/camera.ts` `CameraCapture`): back camera 1280×720,
  exposure locked (WS1) plus focus and white balance where the phone allows; each frame is drawn at
  3× the decoder size and **max-pooled** 3×3 to 320-wide luma (a single distant LED keeps its full
  brightness), stored with its capture timestamp (≤ 40 fps, ≤ 1400 frames); a live "what blinks"
  overlay (per-pixel max − min); wake lock; device-motion "hold still" warning. The photo is taken
  while the display is lit dim for framing.
- *Decode* (`decode.ts`, in `decode.worker.ts`): (1) candidate pixels whose temporal std exceeds
  1.5 × the sensor noise (median frame-difference noise, robust when reflections make the whole
  frame blink); (2) clock: mean of the most active pixels Pearson-correlated with the plan's
  lit-fraction template (±2.5 s around the request time, 20 ms then 2 ms steps); (3) motion: the
  shift of each preamble "all on" block against the first (±12 px, on the 400 strongest points); a
  pass whose preamble blocks disagree or whose shift differs from the next preamble's is dropped
  (the phone moved during it), the others are compensated; (4) per slot, the mean of the frames
  whose timestamps lie inside it by a guard of `0.3 × frame + 10 ms`, after subtracting a 15×15
  local background mean (lit snow, walls); (5) passes are soft-combined; references from the
  preamble, pixels masked unless `on − off ≥ max(6, 4σ_off)`; phase A = top-*w* word, exact
  codeword or one swap of the least-certain pair (margin ≤ 0.3); phase B = sign of each pair,
  rejected if `|Δ| < 0.25·contrast` or `< 2.5σ`; phase A without phase B = a *region* of that
  output; (6) each pass is checked against the combined bits and a disagreeing pass (headlights,
  someone walking by) is dropped; (7) connected components per (output, pixel) label, the strongest
  kept (two equally strong → ambiguous, dropped; large low-contrast blobs → reflections, dropped),
  ghosts (two labels of one string on one spot) and lights far from their string neighbours or alone
  and weak are dropped.
- *Analysis* (`analyze.ts`): output pixel → prop pixel through the segments (reverse-aware);
  image → canvas similarity transform by RANSAC over prop centroids (hypotheses must be upright and
  get each inlier's size right, so swapped or backwards props can't skew it); proposals: **swap**
  (A seen where B is in the layout and vice versa; offered only when their segments have equal
  length or are alone on their outputs), **reverse** (Spearman ρ < −0.6 between decoded index and
  nearest layout point, per segment), **pixelCount** (highest decoded index + 1 < configured − 1
  with ≥ 50 % coverage), **notSeen**, **layout** (decoded points through the transform — or scaled
  into the current layout's bounding box when nothing aligns — missing pixels interpolated along the
  string; ticked by default only for props without an xLights layout) and **info**.
- *Simulator* (`simulate.ts`): Gaussian PSF with max-pool sampling, ambient gradient, 99 Hz
  streetlight, reflections of chosen outputs onto rectangles, occluded runs, AE drift, read + shot
  noise, 8-bit clipping, exposure integration, rolling shutter, timestamp jitter, dropped frames,
  hand shake and jumps. `decode.test.ts` holds the decoder to < 1 % wrong IDs and > 95 % of visible
  lights found at night (30 and 15 fps), distant dim single-pixel lights, reflections + flicker +
  occlusion, snow glow, bloom, shake and a 10 px move, 60 outputs × 20 lights, 140 outputs (16-bit
  codes), and the F7 probe; the all-at-once worst case stays under 1 % wrong at ≥ 70 % found.
- *Page* `/map`: scope (whole display / one controller / some props, time estimate) → camera
  (`lib/cv/CameraScan.svelte`: frame, lock, scan, decode; behind WS1's `SecureGate`) → review (photo
  with each prop's outline, grouped proposals with checkboxes, *Apply selected* with Undo, *Use this
  photo as the layout background*) and earlier runs. In demo mode a simulated yard built from the
  show's layout (one string deliberately backwards) stands in for the camera
  (`tests/e2e/mapping.spec.ts`).

**Needs the real yard**: night field test at 10/20/30 m with three phones (incl. a 15 fps one),
exposure/focus lock behaviour per phone, snow/wall/window reflections, very bright props (bloom:
`stats.saturatedPct`; lower `level`), the engine wiring's cross-node bit alignment, and real
xLights layouts vs. the similarity fit (perspective: a 4-corner homography is a possible follow-up).

### 12.6 Pixel-count check (F7, WS4)
*Props → ruler button* (list view) or *Fault finder → "Only the end stays dark?"*
(`components/props/PixelCount.svelte`) on one of the prop's segments' outputs.

- **Camera**: `POST /pixelcount/start {nodeId, output, method:"camera", bitMs?, force?}` → a
  phase-B-only mapping run (`kind: "pixelCount"`) that lights `max(configured × 1.25, configured +
  64)` pixels (a WS281x string ignores data past its end), clamped to the DPI geometry limit when
  the output is on this node (`limited: true`) and to 4096; the response adds `configured` and
  `maxProbe`. Count = highest decoded index + 1; gaps inside the lit run (< 30 %) are dead/hidden
  suspects. `POST /pixelcount/:runId/result {count, dead[], confidence?}` stores it.
- **By looking**: `method:"manual"` → session with `faultfinder::CountSearch` (binary search over
  counts `0..=maxProbe`: pixels `0..k` dim green, pixel `k` red via `MapPlan.countProbe`; "can you
  see the red pixel?"; ≤ ⌈log₂(maxProbe + 1)⌉ questions — 12 for 2048, tested for every count);
  `POST /pixelcount/:session/answer {seen}`, `…/undo`, `…/stop` → `{session, nodeId, output,
  configured, maxProbe, canUndo, step?:{litUntil, ask, number, maxRemaining}, count?}`. Sessions end
  after 30 min idle; the probe light too.
- **Current**: 400 — needs a receiver-side sensor (F20 ESP32 + INA226); difftxlarge's INA226 only
  sees the transmitter's own input.
- **Apply**: `POST /pixelcount/:id/apply {updatePropCount, count?, dead?}` (session or run id) →
  auto snapshot, `OutputConfig.measuredPixels = {count, method, at, dead}`, dead output pixels →
  `Prop.suspectPixels` (reverse-aware); with `updatePropCount` the last prop on the string takes
  the difference (refused when it continues on another output or the count ends before it starts;
  layout points of the wrong length are dropped) → `{show, snapshotId, message}`.

**Needs the real yard**: probing past the string end on real strings (no harm expected), camera
count accuracy on dense strings (a 300-pixel string at 20 m is only regions, not pixels — use the
manual method there).

### 12.7 Season profiles (F8, WS6)
`services/profiles.rs`, `api/profiles.rs`; UI `/settings/seasons`, dashboard chip
`components/dashboard/SeasonChip.svelte` (embedded by the dashboard's owner with one line).

**Model.** `Show.profiles: ShowProfile[]`, `activeProfileId?`, `profileAutoSwitch`. A profile is a
named copy of the *season-specific* fields; the library (sequences, media, props, playlists, looks) is
shared and only referenced by id.

| Profile field | Live field it replaces | `None` in the profile |
|---|---|---|
| `schedule` (entries, enabled, idle/off looks, quiet hours) | `show.schedule` — **except `location`** (time zone, sunset belong to the place) | — |
| `requestsPlaylistId` | `settings.requests.playlistId` | all songs |
| `requestsMessage`, `gamesEnabled`, `power.{dim, maxBrightness}` | `settings.requests.message`, `settings.games.enabled`, `settings.power.{dim, maxBrightness}` | left unchanged |
| `disabledPropIds` | the render / health mask below | — |
| `defaultDjVoice`, `tags` | read by the UI from the active profile | — |

**Switching copies.** `POST /profiles/:id/activate {saveCurrent=true}` takes an automatic backup,
saves the live fields back into the previously active profile (so edits made on the Schedule, Requests,
Power pages belong to the season that was live), copies the target in, sets `activeProfileId` and
journals `profileSwitch {from?, to}`. Every schedule entry's playlist, the idle/off looks and the
request playlist must exist, otherwise 400 ("Edit the season first") and nothing changes. Editing the
*active* profile (`PUT /profiles/:id`) saves the live fields into it first, then re-applies it.
`GET /profiles/preview-switch/:id` → `{lines}` (one human line per change, shown in the confirm).

**Prop mask** (contract): `services::profiles::disabled_prop_ids(&show)` = the active profile's
`disabledPropIds` that still exist. The engine renders them dark (WS3: content → surprise → season
mask → tests → overlays), health checks skip them, and the leader sends them to followers as manifest
`settings.disabledPropIds` (WS5, `cluster/manifest.rs`). Suspect pixels of masked props are left out
of the nightly report.

**Auto-switch.** With `profileAutoSwitch`, once shortly after boot and daily from 12:00 local, and
never inside a running show window (retried each minute until it ends), the profile whose
`dateRange` (`MM-DD..MM-DD`, may wrap the year end, e.g. `11-01..01-06`) contains today wins: highest
`priority`, then the narrowest range (a Thanksgiving week inside Christmas), then list order. No
match: nothing changes. An automatic switch is announced through the alert channels ("Switched to 🎄
Christmas"); a failed one raises a warning alert and a `warn {code:"profileSwitch"}` journal entry.

**API** (`/api/v1`): `GET/POST /profiles`, `GET/PUT/DELETE /profiles/:id` (merge patch; deleting the
active one keeps the live settings), `POST /profiles/capture {name}` (a copy of the live season; the
first one becomes active), `POST /profiles/:id/activate`, `GET /profiles/preview-switch/:id`,
`GET /profiles/active` → `{id, name, icon, color, autoSwitch, scheduledId, nextSwitch?{profileId,
name, date}}`, `PUT /profiles/auto-switch {enabled}` (needs at least one dated profile).

### 12.8 New receiver wizard (F9, WS4)
*Controllers → Add receiver* (header) or *Guided* on a free jack
(`components/controllers/ReceiverWizard.svelte`): controller → plug in → find the jack → name →
per port: which prop lit, which end the chase starts, colours → summary → create.

| Endpoint | Does |
|---|---|
| `POST /wizard/receiver/identify-jack {nodeId, force?}` | free jacks (no receiver) → test mode `identify`: port 1 of each blinks `(color, blinks)` from 8 signals — white `#707070` or blue `#0000c0`, 1–4 blinks (`mapcode::identify_on`: n × (350 ms on + 350 ms off) + 1.4 s pause), which look the same whatever the strip's colour order; with > 8 free jacks signals repeat and a second round narrows it. An engine without `identify` falls back to `method:"sequential"` (one jack's port 1 steady at a time) → `{sessionId, method, round, candidates:[{jack, color, blinks}], probeJack}` |
| `POST …/:s/pick {color, blinks}` | `{done:true, jack}` or the next round |
| `POST …/:s/probe {jack}`, `POST …/:s/jack {jack}` | sequential method / "I know the jack" |
| `POST …/:s/port/:n/light {pattern: solid\|chase\|red\|green\|blue\|off}` | raw output test on `(jack − 1) × 4 + n` |
| `POST …/:s/color-order {port, red, green}` | what pure red and pure green looked like → the strip's real order, accounting for the output's configured order (`services::mapping::detect_color_order`, all 36 combinations tested) → `{colorOrder, configured, changed}` |
| `POST …/:s/finish {receiver:{name, kind, location?, fuseAmps?, mainFuseAmps?}, ports:[{port, propIds[], reverse, colorOrder?}]}` | dry run, auto snapshot, creates the receiver (fuse defaults to the kind's) and replaces each chosen prop's wiring with one full-length segment, chained in order from pixel 0; sets colour orders; overlaps refused → `{show, receiver, snapshotId}` |
| `POST …/:s/cancel` | lights off, session gone (sessions also expire after 30 min) |

The wizard's blink animation mirrors `identify_on`. **Needs the real yard**: signal
distinguishability outdoors (white vs. blue at a distance, counting blinks), and the colour check
on GRB/BRG strings.

### 12.9 Controller replacement (F10, WS5)
**A dead follower** (`POST /nodes/:id/replace {candidateId, force?}`, *Replace…* on its card,
`components/controllers/ReplaceDialog.svelte`): the leader
1. checks the old node is offline (`409 node_online` unless `force`; forced, it first sends a
   signed release) and the candidate is a fresh controller (unconfigured, or a follower
   without leader; not a possible duplicate); a different board that would lose wired
   outputs is `409 board_mismatch` unless `force` (a blank board — beacon `bare-pi` /
   `virtual` — counts as the old board: its EEPROM gets written, below);
2. **revokes the old key** (removed from `cluster/keys.json`, replay state forgotten) and
   keeps it only in `cluster/retired.json` (0600, never used to authenticate, see 5.);
3. adopts the candidate with `AdoptCall.assumeId = <old id>`, `hostname`, `name` and, for a
   PixelPlus board, `board`/`boardRev`. The follower accepts `assumeId` only on trust on first
   use (unconfigured or released; `409` otherwise): it takes the old id in `node.json`,
   derives its key for that id, renames itself through hostnamed, and writes a `PPX1` record
   for `board` when its EEPROM is readable and blank and the I²C devices fit
   (`eepromWritten` in the reply; a programmed EEPROM is never overwritten). A failed
   adoption restores the old key; an older follower that ignores `assumeId` answers with its
   own id and the old node's wiring is renamed to it instead (`leader::rename_node`);
4. keeps everything that references the old id (outputs, receivers, segments, slices: their
   keys don't change) and records the old hardware in `Node.hardwareHistory`
   (`{at, serial, board, piModel, reason: "replaced"}`, last 20) with the new `serial`
   (board EEPROM serial, else `pi-<CPU serial>`; `net::hardware_serial`, also sent as beacon
   `hw`). The manifest and slices follow within a manifest poll;
5. **retired hardware:** a beacon for an adopted id that is unauthenticated, still claims
   this leader and whose `hw` is the retired serial (or, without `hw`, isn't the
   replacement's address) is kept out of the peer table and listed in `/nodes/discovered`
   with `retired: {replacedAt, name}` (`RetiredControllers.svelte`: *Release it*).
   `POST /nodes/:id/release-retired` signs `POST /cluster/release {"retire": true}` with the
   retired key; the old controller forgets the leader **and takes a new random id**
   (unconfigured), so it can be reused.

**A dead leader** is replaced from its **controller transfer file** (`.ppxfer`,
`services/transfer.rs`): `POST /system/transfer/export {passphrase ≥ 10 chars}` → `{url}` (a
one-time 10-minute link; `GET /system/transfer/download/:token` streams the file straight from
the encryptor, nothing is spooled to disk). Contents (zstd tar inside the encryption):
`transfer.json`, `show.json` (secrets included), `node.json` (id, name, legacy key),
`cluster/keys.json`, the F1 CA (`tls/ca.{key,crt,json}` via `tls::export_ca`) and every
referenced sequence, audio file and thumbnail (not tunnel tokens: they belong to the old
hardware). Container: `"PPXFER\0\x01" | u32 header length | header JSON` then records
`u32 length | AES-256-GCM(≤ 1 MiB)`; key = Argon2id(passphrase, 16-byte salt, m = 64 MiB,
t = 3, p = 1, bounds checked on read); nonce = 7-byte random prefix ‖ record counter (u32) ‖
final flag, magic + header as associated data (STREAM construction: tampering, reordering,
truncation and trailing data are detected); the header's `check` (tag of an empty message
under a reserved nonce) tells a wrong passphrase (`400 wrong_passphrase`) from damage.

Restore is the setup wizard's *Restore a show from a transfer file*: multipart
`POST /system/setup` with `passphrase` then `transfer` (only while unconfigured, local
network only — like any setup). The upload is decrypted and unpacked while it streams into a
staging directory (budget: free space − 256 MiB; data files only through `paths::check`,
no links), and applied only after the final record verified: data files moved into place, CA
imported (`tls::import_ca`, then `poke`), identity = the old leader's id/name/key, follower keys
installed, show restored (Tailscale/Cloudflare state cleared: set up again on this hardware),
the leader node's `hardwareHistory` gets the old hardware (`"replaced from a transfer file"`),
then `ensure_self_node` and the old host name (hostnamed). Followers need nothing: the new
leader's unicast beacon copies are MACed with their keys, a follower confirms the new boot id
with a ping and moves `leaderUrl` to the new address (`follower::on_leader_beacon`).

Tests: `cluster::tests::replacing_a_dead_follower_and_a_dead_leader` (in-process leader,
follower, replacement, new leader: replace, old key 401 / new key 200, wiring and slices kept,
refusal of `assumeId` once set up, retired beacon listed and released, transfer export → wrong
passphrase refused → restore → follower moves over and stays synced),
`services::snapshots::transfer::tests` (round trip across record sizes, wrong passphrase,
bit flips, header change, truncation at and inside records, swapped records, trailing data,
hostile KDF parameters, unsafe tar entries).

### 12.10 Journal and nightly report (F11)
**Journal** (`services/journal.rs`, complete): `journal/<YYYY-MM-DD>.jsonl` under the data dir,
local date in the show's time zone, one object per line `{"ts": RFC 3339 with offset, "ev":
name, …fields camelCase}`. Events (`journal::Event`): `itemStart {item, id, name, playlistId?}`,
`itemEnd {item, id, name, durMs, endedBy}`, `showStart/showEnd {entryId, name}`, `request
{sequenceId, name}`, `game {s}`, `error/warn {code, msg}`, `health {checks}`, `nodeOnline/
nodeOffline {id}`, `restart {reason}` (written at every start), `update {from, to, ok}`,
`trigger {id}`, `profileSwitch {from?, to}`, `limiter {nodeId, port, sec}`, `syncSample
{nodeId, offsetErrorMs, timelineErrorMs?}`, `metric {nodeId?, name, value}`. Record with
`state.services.journal.record(Event::…)` (never blocks: a bounded queue to a writer task; drops
and counts when full). Read with `journal::read_day` / `read_range(from, to)` (show night =
noon to noon); torn lines are skipped. Files older than 120 days are deleted daily; a day's file
stops at 16 MiB. `GET /journal?date=&types=` serves a day (admin).

**Nightly report** (WS6: `services/reports.rs`, `api/reports.rs`, delivery in `services/alerts.rs`;
UI `/reports`, `/settings/reports`).

*Show night* `D` = `[D 12:00, D+1 12:00)` local (23 or 25 hours across a DST change; the earliest
instant is used for an ambiguous noon). The report of night `D` is made at `settings.reports.time`:
`"HH:MM"` before noon = the next morning (default 07:00), from noon = that evening, or `"afterShow"` =
15 minutes after the night's last show window ends (next noon when nothing was scheduled). A report
made early covers the night up to that moment (`window.to`).

*Sampler* (leader only, every minute): `metric {nodeId, name: "tempC" | "volts" | "diskFreePct",
value}` for this controller (hottest board temperature / lowest supply voltage, else the SoC
temperature) and `syncSample {nodeId, offsetErrorMs, timelineErrorMs?}` for each online adopted
follower. `alerts.rs` journals `nodeOnline/nodeOffline`, file-sync problems (`warn {code:"files"}`)
and player failures (`error|warn {code:"show"}`); engine and services journal the rest (§12.10 list).

*Aggregation* (`reports::aggregate`, pure; golden tests on a synthetic journal in
`crates/pixelplus-daemon/testdata/reports/`): shows (paired `showStart/showEnd` per entry, open ones
end at the report time), songs (`itemStart` of sequences), requests + top 5, problems (`error`/`warn`
grouped by code, count, latest wording; plus failing/warning checks of the night's last `health`),
per-controller temperature min/max, lowest voltage, offline minutes, sync p50/p95 (max of
offset/timeline error), limiter seconds per node/port, suspect pixels (`Prop.suspectPixels`, masked
props excluded), disk free, updates, newest backup age, games, triggers, restarts, season, and chart
series (15-minute buckets: temperature max, sync p95).
*Status*: `fail` = any error or a controller offline > 10 min; `warn` = warnings, over-temperature /
low voltage (alert rules), limiter use, suspect pixels, disk < 10 %, newest backup > 30 days, any
offline minute; else `ok`. *Headline*: "3 shows, 42 songs, 118 requests, 1 problem".

*Storage*: `reports/<date>.json` (`NightReport` in `types.ts` + optional `generatedAt, window,
runtimeMin, games, gameMinutes, triggers, restarts, season, series{tempC[], syncMs[]}, delivery[]`),
purged after `keepDays`. A night is made automatically once (a stored report that was delivered is not
sent again after a restart).

*Delivery*: email = `multipart/alternative` (text + inline-styled HTML, all values escaped) through
the `alerts.email` SMTP settings; push = ntfy with a short title ("⚠️ Mostly fine: Tuesday, Dec 1",
RFC 2047-encoded when not ASCII), the headline and worst items, and a `Click` link to
`<publicUrl origin or http://<hostname>.local>/reports?date=D` — no addresses in the text.
`onlyWhenProblems` skips sending `ok` nights. Each channel's result is stored in `delivery`.

*API*: `GET /reports?limit=30` → `[{date, status, headline, itemsPlayed, requests, problems,
runtimeMin, tempMaxC?}]` newest first; `GET /reports/:date` → the report (404 until made);
`GET /reports/:date/email` → the HTML email (`text/html`, CSP `default-src 'none'`, shown in a
sandboxed iframe); `POST /reports/run {date?, send?}` → make now (default: the latest night; up to 120
days back) and optionally send (ignores `onlyWhenProblems`).

### 12.11 Power limiter and late-night dimming (F12, WS3)
The INA226 on the difftxlarge measures only the TX board's own input, so the limiter works from **estimated
current**: `I = Σbytes/765 × mApp` over each output's wire bytes (after colour order, output brightness,
gamma and master brightness: what the pixels really draw for).

**Budgets** (`pixelplus_core::power::node_budget(show, nodeId) -> Option<NodePowerBudget>`, `None` when
`settings.power.mode` is `off` or the node is unknown; `show_budgets(show)` for all nodes). Groups, all ×
`safety` (0.9):

| group id | kind | budget | τ |
|---|---|---|---|
| `port:<receiverId>:<port>` | port | PPTC hold (`fuseAmps`, diffrx 6 A) × 0.8 (temperature unknown; `pptc_derate(°C)`) | 8 s |
| `bus:<receiverId>` | bus | `mainFuseAmps` | 1 s |
| `supply:<supplyId>` | supply | `amps` (split pro rata by possible current when a supply feeds several nodes) | 0 (per frame) |
| `global` | global | `min(globalAmps, globalWatts × 0.85 / V)` (V = first supply's volts, else 12), split pro rata | 1 s |

`mApp[output]` = the output's mean mA/pixel at full white (`Prop.maxMilliampsPerPixel`, default 60).
`NodeManifest.power` carries `node_budget(show, nodeId)` (cluster/manifest.rs); followers run with it
(`ClusterHandle::manifest_power`, see `player::limiter::budget_for`), the leader computes its own. The season
prop mask (§12.7) reaches followers as their show's active profile, so the engine asks
`services::profiles::disabled_prop_ids(show)` on both roles and renders those props dark (after surprises,
before tests and overlays).

**Limiter** (`power::Limiter`, run by `player/limiter.rs` in the output thread, per frame):
supplies: `s = min(1, B/I)`; averaged groups (EMA of the current actually drawn, `k = Δt/τ`): the allowed
current tapers from `4·B` while cold to `B` at the limit, `s = min(1, (4B − 3·EMA)/I)` — the fuse's thermal
headroom is used first, dimming then starts gradually and the average converges to `B` without overshoot.
An output's target is its groups' lowest need; scales drop at once and recover by `Δt/1.5 s` (no pumping on
strobes: every flash gets the same scale). Scaling multiplies the wire bytes (hue kept; current is linear in
duty). Modes: `off`, `warn` (default: computed and reported, never applied), `limit`. Tests: linearity,
attack/release, thermal convergence without overshoot, strobe, warn mode, budgets, splits.

**Reporting.** `PlayerStatus.power {limiting, minScale}` while the mode is not off; followers put
`report.limiter {activeGroups, minScale (lowest since the last report), secondsLimited}` in their beacon
(`player::limiter::follower_report`); `/power/live` reads it from `NodeStatus.limiter` when the cluster
layer exposes it. `GET /power/live` → `{nodes:[{nodeId, mode, limiting, minScale,
groups:[{id, amps, budget, scale}]}]}` (followers: budget groups with `amps: null` and the reported scale);
the WS `power` message carries the same every second while the display is lit. Limiting episodes of ≥ 1 s
are journaled when they end (`limiter {nodeId, port, sec}`, port = the output of a port group, else 0).
`GET /power/budget[?nodeId]` shows the computed budgets. CRUD `/power-supplies` (validated: plausible
volts/amps, existing receivers/outputs, one supply per output).

**Planning.** `GET /power/estimate` (tools) now also returns `perSupply [{supplyId, name, volts, ratedAmps,
peakAmps, avgAmps, peakWatts, status}]` and `limited [{groupId, nodeId, label, seconds, minScale}]`: the same
limiter simulated over the sampled frames (as if in `limit` mode), with a warning "would dim … for N s".

**Late-night dimming.** `settings.power.dim [{from, to, brightness, days}]` (a window runs from `from` on one
of its `days` — empty = every day — to the next `to`) and `settings.power.maxBrightness` cap the master
brightness (`scheduler::brightness_cap`, in `ScheduleFacts.brightnessCap`). The owner's brightness setting is
unchanged; the leader's lights and its sync packets (`brightness`, via the engine-internal
`PlayerStatus.lightBrightness`) use the capped value. Season profiles carry their own `power {dim,
maxBrightness}` (§12.7). UI: Settings → Power (supplies, mode, safety, caps, dim windows, live view,
estimate), the props drawer (mA/pixel, estimated draw) and a dashboard badge while limiting.

### 12.12 Remote access (F14, WS5)
**Public-only listener** (`127.0.0.1:${PIXELPLUS_PUBLIC_PORT:-8081}`, bound by WS1's
`main.rs` with `api::security::public_only`): `/` → `/request`, the `/request` page, the UI's
static files, `/api/v1/public/*` (song requests, health, CA certificate) and `/play/*` (games
controller); everything else, every admin route and the root-mounted FPP Connect API
included, is `404` (`security::public_path_allowed`; tested against the full router in
`security_tests::public_listener_serves_only_the_public_pages`). Funnels and tunnels only ever
point there, so a public hostname can't reach the admin UI whatever its Host header.

**Admin exposure** is explicit: Tailscale `serve` (tailnet members only) or a Cloudflare
*admin hostname*; both need a password (`409 password_required`), and while exposed the name
is allowed by the Host allow-list (`security::remote_admin_hosts`, computed from
`settings.remote`, so it disappears when turned off). Independently, `security::guard` refuses
every admin API call that arrives through a local proxy (`tunnel_request`: loopback peer with
forwarding headers) while no password is set (`403 password_required`). Client addresses of
tunnelled requests come from `CF-Connecting-IP` / `X-Forwarded-For` of the local proxy as
before (rate limits, sign-in throttle).

`services/remote.rs`, `api/remote.rs`:

| Endpoint | Does |
|---|---|
| `GET /remote/status[?fresh=true]` | `{tailscale:{installed, state, dnsName, httpsOk, serve, funnel, loginUrl?, ips}, cloudflare:{installed, running, mode, urls[], publicHost, adminHost, tokenSet}, publicListener, publicPort, passwordSet, canManage, message?}` (3 s cache; `tailscale status --json`, `tailscale serve status --json`, `systemctl is-active`, the quick tunnel's `127.0.0.1:20241/quicktunnel`) |
| `POST /remote/tailscale/install\|up\|serve\|funnel\|down` | helper verbs below; `up {authKey?}` (write-only, 0600 file the helper consumes), `serve`/`funnel` `{on}` |
| `POST /remote/cloudflare/install\|quick\|token\|hosts\|stop` | `quick {on}`; `token {token, publicHost?, adminHost?}` (token write-only; hosts validated); `hosts` saves the names only |
| `POST /remote/test {url}` | `GET <url>/api/v1/public/health` through the tunnel, only for this controller's own remote names (no open proxy) |

Turning a public address on points `requests.publicUrl` / `games.publicUrl` (QR codes) at it
unless the owner set their own. Helper verbs (`pixelplus-helper`, root): `tailscale-install`
(pkgs.tailscale.com signed repo), `tailscale-up` (auth key file → `--auth-key=file:`; else a
transient `pixelplus-tailscale-login` unit runs `tailscale up` and the login URL is reported),
`tailscale-serve:on|off` (`--https=443 http://127.0.0.1:<http port>`; a missing HTTPS/MagicDNS
setting is explained), `tailscale-funnel:on|off` (`--https=8443 http://127.0.0.1:<public
port>`, never port 80), `tailscale-down`, `cloudflared-install` (pkg.cloudflare.com signed
repo), `cloudflared-quick:on|off` (`pixelplus-cloudflared-quick.service`: DynamicUser,
`--url http://127.0.0.1:<public port>`, metrics on `127.0.0.1:20241`), `cloudflared-token`
(the token goes into `/etc/pixelplus/cloudflared.env` 0600 as `TUNNEL_TOKEN=`, read by systemd
for `pixelplus-cloudflared.service`, never on a command line), `cloudflared-stop`. Secret files
the daemon writes are copied without following links, size-limited and format-checked before
use, then deleted. Docker: run cloudflared / Tailscale next to the container (compose profile
`tunnel`, `docker/README.md`).

### 12.13 Signed OTA updates and cluster coordination (F15, WS5)
`Show.formatVersion` (1) is bumped only by a migration older releases cannot read.

**Signed releases.** CI (`release.yml` job `sign`) signs every `.deb` with minisign and writes
a signed index per channel, `pixelplus-<stable|beta>.json` (+ `.minisig`;
`packaging/release-index.py`: `{v:1, channel, version, date, notes, protoMin, protoMax,
formatVersion, files:[{arch, name, size, sha256, url}]}`), published to GitHub Pages `ota/`
when the release is published (`ota-publish.yml`). Controllers read
`${PIXELPLUS_UPDATE_URL:-https://tlchandler.github.io/PixelPlus/ota}/pixelplus-<channel>.json`.
Trusted keys: `packaging/keys/pixelplus-release.pub` (compiled in and installed to
`/usr/share/pixelplus/keys/`; several keys = rotation). The daemon verifies the index and
every package (size, SHA-256 from the signed index, and the package's own signature —
pre-hashed minisign streamed, legacy mode read whole); the **root helper verifies again**
with the installed keys before `dpkg -i`, only installs versions newer than the installed
one (no downgrade attacks; rollbacks come from its own root-only copies), so neither a
compromised daemon user nor a compromised leader can install unsigned code. Pre-release tags
(`v1.3.0-beta1`) become Debian versions `1.3.0~beta1` on the beta channel only. apt
(`apt-repo.sh --suite stable|beta`, helper `update-channel`) stays for hand installs; without
a signing key in the build, `GET/POST /system/update` fall back to apt as before.

**Per node** (helper verbs): `update-stage:<ver>` takes
`/var/lib/pixelplus/updates/incoming/pixelplus_<ver>_<arch>.deb` (+ `.minisig`), verifies it,
checks the package name/version/arch and the free space, makes sure the *installed* version
is in `/var/cache/pixelplus/rollback/` (image-seeded, else `apt-get download` or
`dpkg-repack`; no copy → no update) and keeps it in `staged/` (both 0700 root).
`update-commit:<ver>` writes `/var/lib/pixelplus-helper/pending-verify`, arms a 6-minute late
check, `dpkg -i`s (postinst restarts pixelplusd), keeps the package for the next rollback (3
versions) and runs the **health gate**: within 180 s the new daemon must write
`/run/pixelplus/healthy.json` `{version, engine, output, cluster, at}` (once the player and the
cluster socket are up; `updates_orch::write_health`) and answer
`/api/v1/public/health`; otherwise the previous package is reinstalled
(`--force-downgrade`). Either way `/run/pixelplus/update-result.json` tells the daemon
(`{from, to, ok, kind, message, at}`): a failed update raises an alert, sets the node's update
phase to `failed`, and restores the "Before update to X" snapshot if the show's
`formatVersion` is newer than this release reads. `update-verify` (boot unit
`pixelplus-update-verify.service`, conditioned on `pending-verify`; `dpkg --configure -a`
first) finishes an update interrupted by a power cut. `update-rollback` reinstalls the newest
kept version older than the installed one.

**Cluster** (`services/updates_orch.rs`, leader or standalone controller): `POST
/system/update {version?, scope: "cluster"|"this", force?}` → preflight (every adopted node
online and able to update, nothing playing, no show window now or within
`avoidShowHours`, a package for every node's architecture, 3× package + 256 MB free, protocol
ranges overlap (`negotiate(protoMin..protoMax)`), else `409` with the reasons) → download the
packages for all architectures (followers often have no internet) → snapshot → job:

1. **stage** everywhere in parallel: followers get `/cluster/command {type: "updateStage",
   version, file, size, sha256, sig}`, fetch `GET /cluster/update/:file` from the leader
   (signed with their own key), check size, SHA-256 and signature themselves and run
   `update-stage`; progress in their beacon report (`report.update {arch, canApply,
   diskFreeMb, phase, version, message}`). Any failure stops the job: nothing was installed;
2. **commit the followers** in parallel (`updateCommit`) and wait until each runs the new
   version (online, not `failed`; 8 min);
3. **commit the leader** last (the job is persisted in `updates/job.json` first; its own
   restart resumes it: `resume`);
4. **verify**: every node on the new version → `done`. If any node fails at any point, the
   leader **rolls back every committed node** (`updateRollback`, followers first, the leader
   last) → `rolledBack` (`failed` if a node couldn't be put back).

Progress: `GET /system/update` (`run`, `nodes[]` with version / phase, `problems[]`,
`history[]`, `previous`), WS `updateJob`; history in `updates/history.json`, journal
`update {from, to, ok}`, alerts on failure. `POST /system/update/rollback {scope}` puts every
controller back to the version before the last successful update. `PUT
/system/update/settings` (`UpdateSettings`: channel, `auto` off / notify / install, window
`{from, to, days[]}` wrapping midnight, `avoidShowHours`); the automatic updater checks every
10 minutes (index cached 6 h), notifies once per version, and installs only inside the window,
never during or within `avoidShowHours` of a show window, one attempt per version and day.

**Version tolerance**: beacons carry `protoMin`/`protoMax` (`PROTOCOL_MIN..=PROTOCOL_MAX`,
today 2..2). Nodes whose ranges overlap interoperate (`protocol_mismatch` only warns when they
don't); a future release that changes the wire format keeps speaking the previous protocol
while its peers are older, so the short mixed-version window of a cluster update is safe.

Tests: `updates_orch::tests` (fake fleet: happy path order, bad signature → nothing installed,
follower health failure → everyone back, follower never returning, leader failure → followers
back, resume after the leader's restart, preflight, update window / show times, helper verb
escaping), `updates::tests` (Debian version order, package names, signatures good / bad /
wrong key / rotation, the pre-hashed fixture made by
`packaging/tests/fixtures/make_minisign_fixture.py`, index validation, fetch + download from a
local release server incl. a forged index and a wrong hash), `packaging/tests/test_helper.py`
(stage / commit / health-gated rollback / power-cut verify / downgrade and symlink refusal
with fake dpkg, minisign and health), `test_release_index.py`, `polkit-rules.test.js`.

### 12.14 xLights FPP Connect (F16, WS6)
`api/fppcompat.rs` (+ `api/fppcompat/tests.rs`); UI `/settings/xlights`. Routes are root-mounted,
outside `/api/v1` auth and CSRF; the upload password is `settings.xlights.passwordHash` (write-only;
stripped from `PUT /show/settings`, returned as `""` when set; set with `PUT /xlights/password`).
Matched against xLights master (2026-09) `src-core/controllers/FPP.cpp` and
`src-ui-wx/controllers/FPPConnectDialog.cpp`; tested version: xLights 2025.x/2026.x.

**Detection.** `GET /config.php` (`text/javascript`) has `settings['Title'] = "PixelPlus (Falcon
Player compatible upload)";` — xLights' `parseConfig` treats a target as FPP only if the title contains
"Falcon Player" (a nominative compatibility statement; values are stripped of `" ; ' \ < >`).
`GET /api/system/info` → `{HostName, HostDescription (show name), Platform:"PixelPlus", Variant (Pi
model), Mode:"player", Version:"9.0", majorVersion:9, minorVersion:0, typeId:1, typId:1, uuid:
"PixelPlus-<nodeId>", multisync:false, IPs}` — `uuid` is required by discovery, `typeId` < 0x80 selects
the FPP ≥ 7 upload path (`typId` satisfies a misspelled key check in xLights), version ≥ 7.1 and < 9.3
keeps the plain behaviour, and there is **no `channelRanges`**, so xLights uploads whole files.
"Add FPP" by address also reads `GET /api/fppd/multiSyncSystems` → `{systems:[{hostname, address (the
IPv4 xLights used, ≤ 16 chars), type, model, version, majorVersion, minorVersion, typeId:1, uuid,
fppModeString:"player", channelRanges:""}]}`.

**Skip unchanged.** `GET /api/sequence/<xlightsName>/meta` → `{Name, Version:"2.0", ID (fseq unique id,
decimal string), StepTime, NumFrames, MaxChannel, ChannelCount, CompressionType (0 none, 1 zstd, 2
zlib), Ranges? (sparse only), variableHeaders{mf?}}` from the stored file (imports keep the original
file, so an unchanged re-render compares equal); `GET /api/media/<originalName>/meta` → `{format:{size
(original upload size, as a string), filename, duration}}`. 404 = upload it.

**Upload.** `PATCH /api/file/<dir>` with `Upload-Offset`, `Upload-Length`, `Upload-Name`
(xLights: 16 MiB chunks, `Content-Type: application/offset+octet-stream`, restarts from 0 on any
non-200, 3 tries). Parts go to `uploads/xlights/<sha256(dir,name)>.part` (+ `.json` with name, dir,
length). Offset 0 starts over (disk check: length + 256 MiB free, else 507); another offset must equal
the bytes received for the same name *and* length, else 409; more data than announced → 413; a broken
chunk is cut back to its offset. One request per file at a time (409). Caps: sequences 4 GiB, audio
512 MiB. The last chunk imports: `sequences` → WS2 `content::import_sequence_file` (a sequence with
the same `xlightsName` is **replaced in place**, id/tags/playlists kept; the song is linked by the fseq
`mf` header), `music` → `content::import_media_file` (`replaceSameName`), and the response is 200
only after a successful import (422 with the reason otherwise, which xLights shows).
`virtualdisplay_assets` → 200, dropped; `videos`, `effects` → 415 with a reason. Legacy (FPP < 7)
`POST /api/file/uploads/<name>` + `GET /api/file/move/<name>` also work.

**Playlists.** `GET /api/playlists` (names), `GET /api/playlist/<name>` → FPP JSON (`mainPlaylist`
entries `type:"both"|"sequence"`, `sequenceName`, `mediaName`, `duration`, `playlistInfo`); `POST
/api/playlist/<name>` (with `settings.xlights.addToPlaylists`) adds the named sequences that exist to
the PixelPlus playlist of that name (created when missing; smart playlists refused), never removes;
the answer's `Message` lists names not uploaded yet.

**Ignored controller config.** `GET /api/channel/output/*` → `{channelOutputs:[]}`, `GET
/api/proxies`/`/api/models` → `[]`, `GET /api/cape` and `/api/configfile/*` → 404; `POST/PUT` to
`/api/models`, `/api/proxies*`, `/api/channel/output/*`, `/api/configfile/*`, `/api/settings/*` and
`GET /api/system/fppd/restart` → 200, logged "PixelPlus manages its own wiring". Tell users to leave
"Upload outputs"/"Models" off and choose FSEQ type "V2 zstd".

**Security.** Every route first runs WS5's `security::fpp_compat_authorize` (an extractor
`FppAuth`): feature off → 404; non-LAN peer or proxy headers (tunnels, the public listener) → 404;
Host allow-list → 421; reads pass; writes need HTTP Basic with the upload password (any user name;
verified passwords cached 10 min; sign-in throttle; 401 `WWW-Authenticate: Basic`), or, without an
upload password, must not be CORS-simple. Additionally, **while the show has a sign-in password,
writes are refused (403) until an upload password is set** (LAN-trust mode only for password-less
shows). The UI requires the password before enabling in that case.

**Admin API** (`/api/v1`, merged via `api/profiles.rs`): `GET /xlights/status` → `{enabled,
passwordSet, adminPasswordSet, ready, reason?, addresses[], hostname, uploads[{at, name,
kind:"sequence"|"song"|"ignored", ok, message, bytes, source:"xlights"|"folder", sequenceId?,
mediaId?, replaced}], watch{folder, exists, suggested, lastScan?, error?}}`; `PUT /xlights/password
{password}` (6+ chars, no `:`; `""` removes); `DELETE /xlights/uploads` (clear the log, 50 kept in
`uploads/xlights/log.json`).

**Watch folder** (`settings.xlights.watchFolder`, absolute, not a system directory; suggested
`<data>/xlights-drop`, created when under the data dir): scanned every 10 s (task started from
`services::profiles::start`); `.fseq` and audio files are imported once their size and mtime are
unchanged between two scans and ≥ 5 s old (copied into the data dir first), then moved to `imported/`
or `failed/` (read-only shares: remembered instead). Serving the folder over SMB is packaging's job
(WS5); phase 2: answer FPP multisync pings on UDP 32320 for auto-discovery.

### 12.15 Library tags and smart playlists (F18, WS2)

**Files:** `core/smartlist.rs`, `api/library.rs`; web `lib/library/tags.ts`, `components/library/{TagChips,TagInput,SmartRulesEditor,SmartTonight}.svelte`, Sequences and Playlists pages.

**Tags.** `Sequence.tags`, `Media.tags` (normalised: trimmed, lower case, no commas, ≤ 32 chars, ≤ 24 per item) and optional colours in `Show.tagDefs`. `PATCH /sequences/:id` and `/media/:id` accept `tags`; `POST /sequences/tags {ids[], add[], remove[]}` bulk-edits sequences and media; `GET /library/tags` → `[{name, color?, sequences, media}]`; `PUT /library/tags/:name {name?, color?}` renames (also inside smart rules) / recolours; `DELETE /library/tags/:name`. UI: tag filter bar, per-item tag editor, multi-select bulk tagging.

**Smart playlists.** `Playlist.smart: SmartRules` replaces `items` at play time (intro/outro unchanged). `smartlist::expand(show, rules, history, now, seed)` is pure and deterministic: candidates by include (any/all) / exclude tags and `maxItemMs`, minus songs played in the last `noRepeatNights` show nights (noon to noon; relaxed a night at a time below 3 candidates, with a note); order `leastRecent` (never played first, seeded ≤ 30 min jitter), `shuffle` (seeded), `rotation` (library by name, continuing after the song this playlist played last — the journal is the persistent pointer, so no extra state file), `fixed`; greedy fill simulating start times so that before each time rule only songs with its tags are placed; with a target length the last slot picks the closest fit (rotation never skips); then pinned first/last and one interleave item every N songs. Items get deterministic ids `sm<n>-<seqId>`.

**History** comes from journal `itemStart` events (`sequence`, `request`, `media`) via `journal::read_range`; `GET /library/history?days=14` → `[{sequenceId, plays, lastPlayed?}]`.

**Previews.** `GET /playlists/:id/preview?date=&start=&seed=` and `POST /library/smart-preview {rules, playlistId?, date?, start?, seed?}` (unsaved rules) → `{items, totalMs, notes[], startsAt[], start, seed}`; without `start`, the playlist's scheduled start that day (else 18:00). The seed defaults to `night_seed(night, playlistId)` so the preview equals what plays.

**Engine hook (WS3).** At playlist start and each repeat pass: `if let Some(items) = api::library::smart_items(&state, &playlist.id) { … }` (reads a few journal files; call from `spawn_blocking` in hot async code). `api::library::smart_expansion(state, id, now, seed)` returns notes too.

### 12.16 Surprises and ESP32 sensor nodes (F20, WS3 + WS6)
Sensor UDP port `PIXELPLUS_SENSOR_PORT` (32422); protocol and MAC canonicalization: §7.5.

**Surprise layer (WS3).** A `surprise` trigger action (`{type:"surprise", ref, source?:"sequence"|"effect",
target?, durationMs?}`) draws a sequence or look **on top of** whatever plays (the song goes on) on the
target's props (none = all), for `durationMs` (default: the sequence's length / 5 s; 0.5…120 s) with 150 ms
fades in and out (`player/surprise.rs`). Order: content → surprise → season mask → tests → overlays. One at a
time: a new one replaces the running one. Refused (409) on followers, during blackout, calibration or a test
pattern. Leader API: `PlayerHandle::surprise(SurpriseRequest)`; `POST /player/surprise {ref, source?,
target?, durationMs?}` (try it, no gates) and `POST /player/surprise/stop`.

*Sync.* While it runs, the leader's sync packets carry `surprise {id, kind, ref, targets, startPos,
durationMs, epoch}` where **`startPos` is the surprise's start on the leader clock relative to the packet's
`anchor.atMs`** (≤ 0; the engine always sends an anchor then, `rate 0` when idle). A follower places it on its
own clock exactly like the timeline (`start = anchor.atMs(local) + startPos`) and renders the same frames:
effects from the manifest preset (the leader stamps world bounds with the whole show; built-in looks on a
follower use its own bounds), sequences from its slice (`(id, epoch)` identifies an instance).

*Gates* (`services/triggers.rs`, for every trigger kind): `when` — `showOnly` while a playlist/song plays,
`idleOnly` while a look shows and no song plays, `offOnly` outside show windows with no song; `activeWindow
{from, to}` (show time zone, wraps midnight); `cooldownS`; `maxPerHour` (0 = unlimited). Blocked firings of
`POST /triggers/:id/fire` answer 409 with the reason; successful ones are journaled (`trigger {id}`).

*Entry points for WS6.* `services::triggers::sensor_input(&state, sensorNodeId, input, active) ->
Vec<Fired{triggerId, ok, message}>` — call it for every authenticated input change of an adopted sensor node
(`active` after `activeLow`); it fires the matching `kind:"sensor"` triggers (`sensor {sensorNodeId, input}`)
on the rising edge through their gates. `services::triggers::run_action(&state, &TriggerAction)` carries out
an action without gates (for `POST /surprises/test {action}`); `surprise_request(show, id, action)` validates
one.

**Sensor nodes (WS6).** `services/sensornodes.rs`, `api/sensornodes.rs`, firmware
`firmware/esp32-sensor/` (PlatformIO, Arduino-ESP32; ESP32-C3/S3/classic; README with wiring);
UI `/settings/sensors` and `/settings/triggers` (`components/triggers/TriggerEditor.svelte`, moved out
of the main Settings page; old `#triggers` links redirect).

*Identity.* Node id = `"sn"` + the last 4 bytes of its Wi-Fi MAC in lowercase hex (10 characters,
`24:6F:28:9C:1E:2A` → `sn289c1e2a`). A fresh node opens the captive-portal hotspot
`PixelPlus-Sensor-XXXX` (Wi-Fi + name, stored in NVS), then broadcasts beacons.

*Datagrams* (UDP 32422, one JSON object each, MAC canonicalization §7.5, `sensornodes::seal/verify`):

| `t` | From → to | MAC | Fields |
|---|---|---|---|
| `sbeacon` | node → broadcast, every 2 s (10 s adopted) | no | `id, name, hw, ver, http, adoptedBy, inputs[], proto:1` |
| `sevent` | node → leader | yes | `id, input, state (1 = active after activeLow), ms (uptime), lb` |
| `sstatus` | node → leader, every `statusEverySec` (10) | yes | `id, rssi, uptime, ver, cfg, inputs{id: state}, amps{id: A}, volts{id: V}` |
| `sack` | leader → node | yes | `id, ack (acknowledged sq), sb (node boot echo), ok, lb (leader boot), now (unix s), cfg (config version)` |
| `scmd` | leader → node | yes | `id, cmd:"identify"` |

*Replay.* The leader keeps a `ReplayGuard` per node (boot id, highest `sq`, 64-packet window). Events
carry `lb`, the leader boot id from the node's last `sack`: an event with another `lb` fires nothing
(`sack ok:false` with the current `lb`; the node resends at once with a new `sq`) — so events captured
before a leader restart can't be replayed. A new node boot id is accepted from a heartbeat or from an
event with the current `lb`; replaced boot ids never again. A replayed `sq` is acknowledged again but
acts once. Nodes retry events at 100/200/400 ms until acknowledged (`sb` + `ack` must match), accept
`scmd` only for the current leader boot with growing `sq`, and learn the leader's clock from `now`.

*Adoption* (TOFU): the leader POSTs `http://<node>/adopt {leaderId, leaderUrl ("http://<ipv4>:<port>",
the leader address that routes to the node), sensorPort, dh (X25519 public, hex)}` → `{id, dh, proof,
hw, ver, inputs[{id, pin, kind, activeLow}]}`; both derive `key = hex(HMAC-SHA256(shared, "pixelplus-
sensor-key-v1\n<leaderId>\n<sensorId>\n<leaderPub>\n<sensorPub>"))` and the leader checks `proof =
hex(HMAC(key, "pixelplus-sensor-adopted-v1\n<leaderId>\n<sensorId>"))`. Keys: leader
`<data>/sensor-keys.json` (0600, never in `show.json`/backups), node NVS. An adopted node answers other
leaders 409 until released, reset (BOOT held 10 s) or a short BOOT press opens a 10-minute window
(re-adoption by the current leader may also be signed). Release: `POST http://<node>/release`
signed `X-PixelPlus-Auth` by the leader (monotonic timestamp on the node); the leader forgets the key
and the node even when the node is unreachable (then the UI says to reset it).

*Configuration*: the node fetches `GET /api/v1/cluster/sensor-config/<id>` signed with its key
(`X-PixelPlus-Auth` as §7.5, sender = node id; open path, checked by the handler; a skewed clock gets
`401` + `X-PixelPlus-Time`) → `{version (8 hex of sha256(name, inputs)), name, inputs[SensorInput],
statusEverySec}` with `X-PixelPlus-Reply`; it refetches when a `sack` names another `cfg`.
`SensorInput.kind` `motion|button|beam|contact` use `pin` as GPIO with `debounceMs` (level must hold)
and `holdMs` (reported active at least this long after the last activity: merges PIR re-triggers);
`current` uses `pin` as the INA219/INA226 I²C address (0x40–0x4F) and `shuntMilliohms` (model
addition, WS6); the node reports amps (INA226 shunt LSB 2.5 µV, INA219 10 µV) and bus volts.

*Leader side*: discovery list (30 s), live state (`online` = heartbeat in the last 35 s, RSSI, uptime,
inputs, amps, volts, event and rejected counters), WebSocket `sensorInput {sensorNodeId, input, state,
at}` on each change, and `triggers::sensor_input` on each activation. **Contract for WS3**:
`services::sensornodes::amps(&state, &SensorRef) -> Option<f64>` (latest current of a `kind:"current"`
input, ≤ 30 s old) for `PowerSupply.sensor`.

*API* (`/api/v1`): `GET /sensor-nodes`, `GET /sensor-nodes/discovered`, `POST /sensor-nodes/adopt
{id}`, `GET/PUT /sensor-nodes/:id` (merge patch: name, location, inputs; validated: ids `[a-z0-9_]`, ≤ 8
inputs, GPIO ≤ 48 and unique, INA address and shunt range), `POST /sensor-nodes/:id/release` (= `DELETE`)
→ `{ok, message?}`, `POST /sensor-nodes/:id/identify`, `GET /sensor-nodes/:id/live`, `GET
/sensor-nodes/live` → `{id: live}`, `POST /surprises/test {action}`.

*Tests*: `firmware/esp32-sensor/test/vectors.json` (generated independently in Python, incl. RFC 7748
X25519) is checked by the Rust tests (`sensornodes.rs`) and the firmware's `pio test -e native`
(`test_protocol`, plus `test_debounce`).

