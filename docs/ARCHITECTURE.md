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

Until its workstream lands, a `countdown` item plays as a dark pause of its length, and a
`surprise` trigger action answers "not available yet".

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
| `remote` (WS5) | `GET /remote/status`, `POST /remote/tailscale/{install,up,serve,funnel,down}`, `POST /remote/cloudflare/{install,quick,token,stop}`, `POST /remote/test` |
| `power` (WS3) | CRUD `/power-supplies`, `GET /power/live` (`/power/estimate` gains `simulateLimiter`) |
| `sensornodes` (WS6) | `GET /sensor-nodes/discovered`, `POST /sensor-nodes/adopt`, CRUD `/sensor-nodes`, `POST /sensor-nodes/:id/release`, `GET /sensor-nodes/:id/live`, `POST /surprises/test`, `GET /cluster/sensor-config/:id` |
| `fppcompat` (WS6) | **root-mounted** (not `/api/v1`): `GET /config.php`, `/api/system/info`, `/api/sequence/:name/meta`, `/api/media/:name/meta`, `PATCH /api/file/:dir`, … (§12.14) |
| cluster/system (WS5) | `POST /nodes/:id/replace`, `POST /system/transfer/export`, `GET/POST /system/update` (extended), `PUT /system/update/settings`, `POST /system/update/rollback`, `GET /cluster/update/:file` |

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
_To be written by WS4._ `MapPlan`/`MapTarget` live in `pixelplus-core::mapcode`.

### 12.6 Pixel-count check (F7, WS4)
_To be written by WS4._

### 12.7 Season profiles (F8, WS6)
_To be written by WS6._ Followers get the active profile's prop mask as manifest
`settings.disabledPropIds`.

### 12.8 New receiver wizard (F9, WS4)
_To be written by WS4._

### 12.9 Controller replacement (F10, WS5)
_To be written by WS5._

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

**Nightly report**: _to be written by WS6._

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
**WS5 populates `NodeManifest.power` with `node_budget(show, nodeId)`**; followers read it from their
`cluster/manifest.json` (`player::limiter::budget_for`), the leader computes its own.

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
(`player::limiter::follower_report`; WS5: please also copy it into `NodeStatus.limiter` so the leader's
`/power/live` and the dashboard see it). `GET /power/live` → `{nodes:[{nodeId, mode, limiting, minScale,
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
_To be written by WS5._ Public-only listener: `127.0.0.1:${PIXELPLUS_PUBLIC_PORT:-8081}`.

### 12.13 Signed OTA updates and cluster coordination (F15, WS5)
_To be written by WS5._ `Show.formatVersion` (1) is bumped only by a migration older
releases cannot read.

### 12.14 xLights FPP Connect (F16, WS6)
_To be written by WS6._ Routes are root-mounted (`api/fppcompat.rs`), outside `/api/v1` auth
and CSRF; the upload password is `settings.xlights.passwordHash` (write-only; stripped from
`PUT /show/settings`, returned as `""` when set).

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

