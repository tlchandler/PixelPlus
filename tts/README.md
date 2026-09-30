# PixelPlus TTS (`pixelplus_tts`)

On-device DJ voices for PixelPlus: a small loopback HTTP service (`127.0.0.1:7081`) that
renders multi-voice DJ clips with [Kokoro](https://github.com/hexgrad/kokoro) (82M, Apache-2.0)
through [kokoro-onnx](https://github.com/thewh1teagle/kokoro-onnx). It is a port of
[tlchandler/fpp-voices](https://github.com/tlchandler/fpp-voices), and it keeps that
project's tested sound:

* **Nick** (30% `am_echo` + 30% `am_fenrir` + 40% `am_puck`) and **Holly** (50% `af_heart` + 50% `af_kore`),
  and any custom blend (a weighted average of Kokoro style vectors).
* **Energy and hype that build to the end.** The punchline lands about 2–3 semitones above the lead-in,
  with wider pitch swings, a swell of about 2.5 dB, a soft pitch ceiling and a stretch for Holly. It uses Praat PSOLA
  (praat-parselmouth) with the same algorithm and constants as fpp-voices.
* Radio processing: highpass, per-voice EQ, presence for energetic lines, a de-esser and a compressor.
* Linear loudness normalization to **-16 LUFS** by default, with a true-peak-safe limiter.
* Pronunciation fixes (about 100 built in, plus the show's own list) and exact IPA overrides (`/noʊˈɛl/`).
* An optional music bed, ducked under the speech.

A check that the port is faithful: rendering `examples/show_intro.txt` with fpp-voices' `dj_voice.py`
and with this package, both at -16 LUFS, gives the same loudness (-16.0 / -16.0), the same length to
0.1 ms, and waveform correlation ≥0.99 per 2-second segment.

Runs on **Pi 4 / Pi 5 (64-bit OS) and Docker/x86**. **Pi Zero 2 W and Pi 3 use browser mode**
(`web/src/lib/tts-browser`, kokoro-js in the user's browser), because on-device renders there
would be many times slower than real time and the model barely fits in 512 MB–1 GB RAM.

---

## Install

```bash
sudo ./install.sh                     # /opt/pixelplus-tts/venv, model into /var/lib/pixelplus/tts/models,
                                      # systemd unit pixelplus-tts.service (enabled + started)
sudo ./install.sh --variant fp16      # choose the model variant (see below)
sudo ./install.sh --no-service        # Docker / image builds
./install.sh --prefix ~/pp --data-dir ~/pp-data --no-service --no-apt   # unprivileged dev install
```

The script needs Python 3.10–3.13 (onnxruntime and kokoro-onnx don't support 3.14 yet). When run
as root it installs `ffmpeg` and `python3-venv` with apt if they're missing. If there's no system
ffmpeg, it installs the `bundled-ffmpeg` extra (imageio-ffmpeg) instead. Model files are
downloaded from the kokoro-onnx GitHub release (`model-files-v1.0`). Downloads resume from
`<file>.part` over HTTP Range, and each file is **sha256-verified** before it's moved into place.
Re-running the script is safe. For offline image builds, use `--models-mirror URL` or
`python -m pixelplus_tts download-models --base-url`.

Dev: `pip install -e '.[test,bundled-ffmpeg]' && pytest`. The integration tests run when the model is present in
`$PIXELPLUS_TTS_MODELS_DIR` (or `$PIXELPLUS_DATA_DIR/tts/models`) and are skipped otherwise.

### Model variant (per device RAM)

| Variant | File | Size | Resident after load | Peak while rendering | Speed (x86, measured) |
|---|---|---|---|---|---|
| **fp32** (default) | `kokoro-v1.0.onnx` | 310 MB | 455 MB | ~730 MB | RTF 0.34–0.39 model / 0.42–0.53 full pipeline |
| fp16 | `kokoro-v1.0.fp16.onnx` | 169 MB | 323 MB | ~680 MB | 0.27 model / 0.57–0.66 pipeline (varies) |
| int8 | `kokoro-v1.0.int8.onnx` | 88 MB | 193 MB | ~630 MB | **1.4–1.9 (3–4x slower)** |

`auto` picks **fp32** unless the board has < 1.5 GB RAM (Pi 4 1 GB), which gets **int8**.
The reasoning:

* fp32 is the reference quality, and all of fpp-voices' tuning was done with it.
* int8 (dynamic quantization) saves resident RAM but only about 100 MB of *peak* RAM. It is 3–4x slower
  on x86 because of the ConvInteger/MatMulInteger kernels, and it has no speed advantage on
  Cortex-A72 either (no `sdot`).
* On a Pi 4 with 2 GB RAM, fp32's ~730 MB peak fits alongside pixelplusd, and the model is unloaded
  after the idle time.
* fp16 is fast on CPUs with native fp16 or bf16 support. It is worth trying on a Pi 5 (Cortex-A76 has fp16
  arithmetic): `PIXELPLUS_TTS_MODEL=fp16` in `/etc/default/pixelplus-tts`, then measure with
  `python -m pixelplus_tts bench`.

Override the choice with `PIXELPLUS_TTS_MODEL=fp32|fp16|int8`, or with `install.sh --variant` (which
downloads that file). If `auto` picks a file that isn't present, the service uses whichever
variant is installed.

### Performance

Measured on this dev box (Xeon @2.1 GHz, 3 onnxruntime threads, fp32). A 12 s announcement takes
5–6 s through the whole pipeline (Kokoro, PSOLA energy, ffmpeg radio chain, loudness, MP3), so
RTF ≈ 0.45. The model loads in 1.1 s. A two-line Nick+Holly clip (4.9 s of audio) renders in 3.0 s.

These Pi figures are **estimates**, scaled from the x86 numbers and typical onnxruntime ARM
throughput; confirm them on the device with `python -m pixelplus_tts bench`:

| Device | Expected RTF (fp32) | A 10 s clip renders in | Mode |
|---|---|---|---|
| x86 desktop / NAS (Docker) | 0.2–0.5 | 2–5 s | device |
| Pi 5 (4× A76 2.4 GHz) | ~1.0–1.5 | ~10–15 s | device |
| Pi 4 (4× A72 1.5–1.8 GHz) | ~3–4 | ~30–40 s | device (render ahead of time) |
| Pi 3 / Pi Zero 2 W | ≫5, RAM-limited | minutes | **browser** |

Memory: the service uses about 36 MB while idle with no model loaded, about 730 MB at peak while rendering (fp32),
and about 155 MB after an idle unload (the onnxruntime and Praat libraries stay mapped; the model memory is
returned to the OS with `malloc_trim`). By default the model unloads after `PIXELPLUS_TTS_IDLE_MIN=10` minutes.
The unit runs at `Nice=10`, `CPUSchedulingPolicy=batch` and `IOSchedulingClass=idle`, and uses cores−1 threads,
so renders never starve the light output thread.

---

## HTTP API (`127.0.0.1:7081`, JSON, camelCase)

Errors use the pixelplusd error shape, `{"error": {"code", "message"}}`:

* 400 for `bad_request`, `unknown_voice`, `parse_error`, `forbidden_path`, `not_found` and `too_large`.
* 503 for `model_missing` and `busy` (four requests are already queued).
* 500 for `render_failed`.

Only one render runs at a time; other requests wait in a queue.

### `GET /health`
```json
{"ok": true, "version": "0.1.0", "modelLoaded": false, "modelAvailable": true, "device": "cpu",
 "modelVariant": "fp32", "threads": 3, "idleUnloadMinutes": 10, "renders": 0, "uptimeS": 12}
```
`modelAvailable: false` means the model files aren't downloaded, and renders return 503 `model_missing`.

### `GET /voices`
```json
{"base": [{"id": "af_heart", "name": "Heart", "language": "en-us", "gender": "female", "grade": "A"}, ...54 voices],
 "presets": [{"id": "nick", "name": "Nick", "description": "Male DJ - warm, upbeat, classic radio baritone",
              "blend": {"am_echo": 0.3, "am_fenrir": 0.3, "am_puck": 0.4}, "speed": 1.05, "lang": "en-us",
              "defaultEnergy": 0.4, "eq": "equalizer=f=150:t=q:w=1.0:g=2.5,equalizer=f=3200:t=q:w=1.2:g=2",
              "energy": {"pitch": 1.5, "range": 1.5, "speed": 1.0, "stretch": 1.0, "boost": 3, "lift": 3,
                         "ceiling": 3, "maxLift": 9}},
             {"id": "holly", ...}]}
```
`presets` are `DjVoice` objects that pixelplusd can seed into `show.djVoices`. `grade` is hexgrad's
published grade, given for the English voices only. Non-English voices (es, fr-fr, hi, it, ja, pt-br, cmn) are
best effort: kokoro-onnx phonemizes them with espeak-ng.

### `POST /render` → audio bytes
```jsonc
{
  "lines": [                                   // DjLine[]; voice = id or a DjVoice object
    {"voice": "nick", "text": "Good evening, and welcome to {showName}!", "pauseMs": 0},
    {"voice": "elf",  "text": "Only {daysUntilChristmas} days to go!", "pauseMs": 600, "energy": 1},
    {"voice": {"id": "tmp", "blend": {"af_bella": 0.6, "af_sky": 0.4}}, "text": "Up next, *{nextSong}!*", "energy": 1.5}
  ],
  "voices": [ /* optional DjVoice[] that lines may reference by id or name, e.g. show.djVoices */ ],
  "speed": 1.0,                                // DjClip.speed, 0.5–2.0, multiplies each voice's own speed
  "pronunciations": [{"word": "Griswold", "say": "Griz wold"}, {"word": "Noel", "say": "/noʊˈɛl/"}],
  "builtinPronunciations": true,               // also apply the ~100 built-in fixes (default true)
  "placeholders": {"showName": "the Chandler Family light show", "daysUntilChristmas": 12, "nextSong": "Feliz Navidad"},
  "format": "mp3",                             // mp3 (192k) | wav (s16) | ogg; always 44.1 kHz stereo
  "loudnessLufs": -16,                         // pass ShowSettings.audio.targetLufs
  "fx": true,                                  // radio chain + loudness (false = raw voice)
  "musicBed": {"path": "/var/lib/pixelplus/media/<id>.mp3", "duckDb": 12, "gainDb": -6, "introMs": 1500, "outroMs": 2500},
  "cache": true                                // reuse an identical earlier render (default true)
}
```
The response is `audio/mpeg`, `audio/wav` or `audio/ogg`, with these headers:

* `X-Duration-Ms`: the exact clip length. Store it in `Media.durationMs`.
* `X-Loudness-Lufs`: the measured loudness. Store it in `Media.loudnessLufs`.
* `X-Render-Ms`, `X-Cache: hit|miss` and `X-Model-Variant`.
* `X-Warnings`: a JSON array, for example unresolved placeholders.

Semantics:

* **pauseMs** is the silence *after* a line. `0` means the default 350 ms gap between lines, and no gap after
  the last line. A line with empty `text` is pure silence of `pauseMs`. This reproduces fpp-voices' `[pause N]` timing.
* **energy**: 0 calm, 0.4 normal DJ, 1 hype, 1.5 extra hype. When it's absent, the voice's `defaultEnergy`
  applies. On hype lines the punchline is the text in `*asterisks*`, otherwise the last phrase. Hype lines
  are also leveled up to 1.5 dB above the rest, as in fpp-voices.
* **voice** can be one of these:
  * a preset id, name or alias (`nick`/`male`/`m`/`he`, `holly`/`female`/`f`/`she`);
  * the id or name of an entry in `voices`;
  * a Kokoro id (`af_heart`);
  * an inline DjVoice.

  Blend weights are normalized. A voice without an `energy` block gets a neutral DJ tuning
  (pitch 1.5, range 1.5, lift 2.5, boost 3, ceiling 3, maxLift 8). `eq` may only use audio EQ/dynamics
  ffmpeg filters (such as `equalizer` and `highpass`); anything else is rejected.
* **pronunciations**: whole-word matching. Entries with a capital letter are case-sensitive, lowercase entries
  match any case, and the longest phrase wins. A `/.../` value is exact IPA, spliced into the phonemes.
  User entries override built-ins that have the same word.
* **placeholders**: `{name}` is replaced from the map. An unresolved name is spoken without braces
  ("next Song") and reported in `X-Warnings`. pixelplusd owns the values
  (`DJ_PLACEHOLDERS`: time, date, day, daysUntilChristmas, nextSong, prevSong, showName, temperature,
  sunset, requestName). Write them the way they should be spoken ("December 24th", "5:30 PM").
* **musicBed**: `path` must be under `$PIXELPLUS_DATA_DIR`. The bed is looped or trimmed to
  `introMs + speech + outroMs` and set to `loudnessLufs + gainDb`. Under speech it is ducked by `duckDb`
  (150 ms look-ahead, a hold across word gaps and a 600 ms release), faded in over 0.3 s and faded out over
  the outro. The whole mix is then normalized.
* Output (without a bed) starts with a 250 ms lead-in and ends with a 0.6 s tail, so a player never clips the first word.

### `POST /audition` → audio bytes
`{"voice": "af_heart" | DjVoice, "text"?: string, "energy"?: number, "speed"?: number, "format"?: "mp3"}`.
Without `text` the voice says *"Hi, I'm {name}! Welcome to the show. Now sit back, relax, and enjoy the lights!"*.
Results are always cached on disk. The cache key is a sha256 of the voice's sound-affecting fields, the text and settings,
the model variant and the package version, stored in `$PIXELPLUS_DATA_DIR/tts/cache` (LRU, `PIXELPLUS_TTS_CACHE_MB=200`).
A repeat audition returns in about 0 ms.

### `POST /parse`
`{"script": "nick: Hi!\nholly!!: Merry Christmas!\n[pause 1.0]", "voices"?: DjVoice[]}` →
`{"lines": [{"voice": "nick", "text": "Hi!", "pauseMs": 0}, {"voice": "holly", "text": "Merry Christmas!", "pauseMs": 1000, "energy": 1.5}]}`.
The script format is fpp-voices':

* `voice: text` for a line; `voice!:` means hype (1.0) and `voice!!:` means extra hype (1.5);
* `[pause 1.0]` (seconds; `1.5s` and `500ms` also work) adds silence;
* `# comment` lines are ignored;
* `*punchline*` marks the words hype builds into.

An unknown voice or a line without a colon returns 400 `parse_error` with the line number.
The same parser exists in TypeScript (`web/src/lib/tts-browser/script.ts`).

### `POST /warmup`, `POST /unload`, `GET /pronunciations`
`/warmup` loads the model now; call it about a minute before a dynamic clip is needed. `/unload` frees it.
`/pronunciations` returns the built-in list (`{builtin: [{word, say}]}`) for display in the UI.

---

## CLI (compatible with fpp-voices' `./say`)

```bash
python -m pixelplus_tts say nick "Good evening and welcome to the show!"
python -m pixelplus_tts say holly --hype "Sit back, relax, and enjoy the show!" -o hype.mp3
python -m pixelplus_tts say --script show_intro.txt -o show_intro.mp3 --music-bed bed.mp3
python -m pixelplus_tts say --list
python -m pixelplus_tts parse show_intro.txt          # -> DjLine JSON
python -m pixelplus_tts serve [--port 7081] [--idle-minutes 10] [-v]
python -m pixelplus_tts download-models [--variant auto|fp32|fp16|int8] [--models-dir DIR]
python -m pixelplus_tts bench                          # load time, RTF, RSS on this device
```
`say` options match fpp-voices: `-o`, `--script`, `--speed` (replaces the voice speed), `--energy`, `--hype`,
`--loudness` (default **-16** here, not -14), and `--no-fx`. It also adds `--pronunciations FILE` and `--music-bed FILE`.

## Configuration (environment)

| Variable | Default | |
|---|---|---|
| `PIXELPLUS_DATA_DIR` | `/var/lib/pixelplus` | models in `tts/models`, cache in `tts/cache` |
| `PIXELPLUS_TTS_HOST` / `_PORT` | `127.0.0.1` / `7081` | loopback only |
| `PIXELPLUS_TTS_MODEL` | `auto` | `fp32`, `fp16` or `int8` |
| `PIXELPLUS_TTS_IDLE_MIN` | `10` | unload the model after N idle minutes; 0 = never |
| `PIXELPLUS_TTS_THREADS` | cores − 1 | onnxruntime intra-op threads |
| `PIXELPLUS_TTS_CACHE_MB` | `200` | render and audition cache size; 0 disables it |
| `PIXELPLUS_TTS_MODELS_DIR`, `PIXELPLUS_TTS_CACHE_DIR` | derived | overrides |
| `PIXELPLUS_TTS_ALLOW_ANY_PATH` | off | allow music beds outside the data dir |

## Layout

```
pixelplus_tts/
  server.py    HTTP service (stdlib ThreadingHTTPServer)
  render.py    request validation, per-line chain, pauses, bed, loudness, cache
  engine.py    Kokoro load/idle-unload, blend styles, synthesize a line
  energy.py    energy/hype prosody (fpp-voices), pure helpers + Praat PSOLA
  audio.py     ffmpeg chains (in-memory pipes), loudness, ducking
  voices.py    base voice catalog, Nick/Holly presets, DjVoice normalization, blending
  pronounce.py pronunciation rules + IPA splicing
  script.py    script parser
  models.py    variants, per-device choice, resumable sha256-verified download
  data/voices.json, data/pronunciations.txt   (from fpp-voices)
install.sh, pixelplus-tts.service
```
