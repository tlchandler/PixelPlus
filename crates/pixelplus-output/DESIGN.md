# pixelplus-output — DPI WS281x engine

This document explains how PixelPlus turns a Raspberry Pi's parallel display
interface (DPI) into a 4-output (difftx, diffsmart) or 60-output (difftxlarge)
WS281x pixel controller. It is the design reference for `crates/pixelplus-output`
and is cited by `docs/ARCHITECTURE.md` §3.4.

The implementation is original. It was written from the WS2811 / WS2812(B) /
WS2815 datasheets, the Linux `panel-dpi` and DRM/KMS documentation, the
Raspberry Pi DPI and `config.txt` documentation, the BCM2835 ARM Peripherals
datasheet and the PixelPlus board documentation. FPP's `DPIPixels`
(CC-BY-ND) was not consulted.

---

## 1. The idea in one paragraph

In 24-bit mode the DPI block clocks one 24-bit word per pixel clock onto
GPIO4..GPIO27 (`DPI_D0..DPI_D23`). If every framebuffer pixel is 26.04 ns long,
a WS281x bit (1.25 µs) is exactly 48 pixels, and each of the 24 data lines is an
independent, perfectly timed WS281x output. The display engine does all the
real-time work in hardware; the CPU only has to *draw* the waveform into a
framebuffer, once per frame, at any time before the next vertical blank.

## 2. Pixel format and bit mapping

* Framebuffer: **XRGB8888**, one little-endian `u32` per DPI pixel (DRM fourcc
  `XR24`, 32 bpp, depth 24).
* Panel bus format: `MEDIA_BUS_FMT_RGB888_1X24` (`0x100a`). The display engine
  sends R on `D23..D16`, G on `D15..D8`, B on `D7..D0`, which is exactly the
  `u32` value `0x00RRGGBB`. Therefore **bit *n* of the `u32` appears on DPI_D*n*
  = GPIO *n + 4*** — the rule in ARCHITECTURE §3.1. Bits 24..31 are ignored.
* Sources for the mapping: the Raspberry Pi DPI documentation's output-format
  table (24-bit RGB: R7..R0 on GPIO27..20, G on GPIO19..12, B on GPIO11..4),
  the vc4 DPI driver (`MEDIA_BUS_FMT_RGB888_1X24` → 24-bit RGB format, RGB
  order) and the RP1 DPI driver's format table (RGB888_1X24: R at bits
  23..16). Confirmed on hardware only by bring-up item 4.
* 32 bpp rather than packed RGB888 because it is the native scan-out format on
  every Pi (no conversion), every pixel is one aligned store, and the encoder
  can fill runs with `slice::fill`.

| Board | Mode | Outputs → DPI bit (GPIO) |
|---|---|---|
| difftx, diffsmart | direct | Port 1 → bit 1 (GPIO5), Port 2 → bit 2 (GPIO6), Port 3 → bit 3 (GPIO7), Port 4 → bit 0 (GPIO4) |
| difftxlarge | latched, 3 banks | output *k*: data bit *k* % 20 (GPIO4..23), bank *k* / 20; LE0 = bit 23 (GPIO27), LE1 = bit 22 (GPIO26), LE2 = bit 21 (GPIO25) |

GPIO0..3 (ID EEPROM, I²C-1) are never touched. On the difftxlarge GPIO24 is
not used by the pixel engine and stays available as a button input.

## 3. Pixel clock: 38.4 MHz

38.4 MHz is chosen because

1. `38.4 MHz / 800 kHz = 48` exactly: every WS281x bit is an integer number of
   pixels with no accumulated drift, and every LED (24 bits) is exactly one
   1152-pixel line.
2. The 26.04 ns resolution is fine enough to hit the narrow window shared by
   all chip families (§4) and to fit three latch time slots of four pixels
   between each pair of bit edges (§7).
3. It is well within every Pi's DPI pixel-clock range and its *average* rate
   can be produced exactly (see below).

The value is not trusted blindly: the backend reads the video mode back from
the kernel and `BitTiming::for_clock` recomputes the pixel counts for whatever
clock is actually programmed, rejecting it if the result leaves the chip
tolerances. But the mode only carries the *requested* rate; what the clock
hardware really produces must be checked once per Pi model (§13, items 1–2).

**How the clock is made (expected, from the Linux `clk-bcm2835` driver; not
yet measured).** On BCM283x/BCM2711 the DPI pixel clock is a peripheral clock
divider with 4 integer and 8 fractional bits and no MASH filter, i.e. a
first-order fractional divider: every pixel is an *integer* number of
source-clock cycles, and the fraction is realised by mixing two lengths.

| Pi | likely source | divider | average | individual pixel |
|---|---|---|---|---|
| Zero 2 W / 3 | PLLD_PER 500 MHz | 13 + 5/256 | 38.404 MHz | 26 or 28 ns |
| 4 / 400 / CM4 | crystal 54 MHz (PLLD_PER 750 MHz would need ÷19.5 > 15.99) | 1 + 104/256 | 38.400 MHz exact | **18.5 or 37 ns** |
| 5 (RP1) | RP1 video PLL, integer divider | – | ≈ 38.4 MHz | 26.04 ns (expected) |

The Pi 4 case is benign for the strings — a 12-pixel T0H is 16 or 17 crystal
cycles = 296–315 ns, a 28-pixel T1H 39 or 40 cycles = 722–741 ns, both inside
the common window of §4 — but it shortens the latch hold pixel of §7 to as
little as 18.5 ns (still ≫ the 573's hold requirement). The pixel counts
never drift: the error of the fractional divider does not accumulate.

## 4. Bit timing

Every bit starts high; a `0` falls after T0H, a `1` after T1H.

| | pixels | time | WS2811 (HS) | WS2812 / 2812B | WS2815 | common window |
|---|---|---|---|---|---|---|
| bit period | 48 | 1250 ns | 1250 ± 600 | 1250 ± 600 | ≥ 1200 | |
| T0H | 12 | 312.5 ns | 100–400 | 250–550 | 220–380 | **250–380** |
| T1H | 28 | 729.2 ns | 450–750 | 650–950 | 580–1000 | **650–750** |
| T1L | 20 | 520.8 ns | ≥ 450 | ≥ 450 | ≥ 300 | |
| reset | | ≥ 300 µs | > 50 µs | > 50 µs (V5: > 280) | > 280 µs | **> 280 µs** |

`Ws281xSpec::COMMON` encodes the common window; `BitTiming::validate` and the
decoder check against it. 312.5 / 729.2 ns sits inside all of them, so one
setting drives mixed strings without a per-output "pixel type" switch. The
margin that matters most is T1H to the 750 ns WS2811 ceiling (21 ns nominal,
~9 ns with the Pi 4 clock dither of §3); buffer/driver/receiver pulse-width
distortion (tPLH − tPHL of the '541, '573, AM26C31 and the receiver) eats into
it, so it is item 3 of the bring-up checklist.

Low times are deliberately *not* held to the datasheets' nominal ±150 ns low
windows: T0L is 937.5 ns, T1L 520.8 ns, and the 24th bit of every LED is
stretched by the 24-pixel h-blank (1562 / 1146 ns). WS281x-family receivers
decode each bit from its high time and treat only a low of several µs as a
reset; the decoder enforces 400 ns ≤ low ≤ 5 µs. This is the same bit layout
the board documentation records for FPP's DPIPixels output (12/28 of 48
pixels at 38.4 MHz, `boardtempinfo/difftxlarge/RESEARCH.md`), which these
boards were designed around.

## 5. Framebuffer geometry

* **Width:** 1152 active pixels = 24 bits × 48. Line *y* carries LED *y* of
  every output.
* **Horizontal blanking:** front porch 8, sync 8, back porch 8 pixels
  (`htotal` 1176, one line = 30.625 µs). Blanking comes after the 24th bit of
  an LED, whose last 20 pixels are low anyway, so it only lengthens that low
  time from 521 ns to 1146 ns. WS281x chips only react to lows of several µs,
  so this is invisible. The decoder verifies every low time stays under 5 µs.
* **Height:** `pixels_per_output + reset_lines` active lines. The reset is
  `ceil(300 µs / 30.625 µs) = 10` lines of low: the last `reset_lines = 7`
  active lines are always zero, plus 1 + 1 + 1 lines of vertical blanking —
  306 µs between frames (≥ 280 µs required). At least one zero line always
  stays inside the active area, so the reset holds even on hardware that
  repeats the last line during blanking.
* **Refresh = maximum frame rate** = 38.4 MHz / (1176 × (N + 10)):

| LEDs per output (N) | refresh | frame time |
|---|---|---|
| 100 | 297 Hz | 3.4 ms |
| 400 | 79.6 Hz | 12.6 ms |
| 800 | **40.3 Hz** | 24.8 ms |
| 1200 | 27.0 Hz | 37.1 ms |
| 1600 | **20.3 Hz** | 49.3 ms |
| 2041 | 15.9 Hz | 62.8 ms (Pi 3 / Zero 2 W maximum: 2048 lines) |

This is the "≈ 1600 px at 20 fps / 800 px at 40 fps" of ARCHITECTURE §3.4, and
it is the same for 4 outputs and for 60. Strings shorter than N simply end
early (their line stays low for the rest of the frame). The mode is fixed at
boot by the overlay (§9), so the daemon chooses N from the longest configured
string (`DpiGeometry::for_pixels` / `for_refresh`) and asks for a reboot when
a longer string is configured. There is no artificial pixel limit; the hard
limit is the display engine's height: 2048 lines on BCM283x, 7680 on BCM2711,
4096 assumed for RP1 until measured.

### Blanking behaviour

On the Pi 3B+/Zero the DPI hardware drives the data pins **low** during
blanking (observed on difftx rev D). Other models may hold the last value.
The layout makes this irrelevant: every blanking interval (h and v) begins
where every output is already low and, in latched mode, where no LE is high.
Holding "low" and driving "low" are the same thing. (This is also why the
difftx rev D port 3 polarity cannot be fixed by inverting a bit: an inverted
line would be *high* at rest but forced low in every blanking interval.)

## 6. Direct mode (difftx, diffsmart)

For each bit of each line:

```
pixel   0 ........ 12 ................ 28 .................... 48
        |  mask     |    data word      |          0           |
```

* `mask` — every output that still has data on this line (so outputs of
  different lengths end cleanly and idle outputs never pulse).
* `data word` — outputs whose current bit is 1.
* `0` — static; written once per buffer.

## 7. Latched mode (difftxlarge)

The difftxlarge has 20 shared data lines (D0..D19) feeding three banks of
74AHCT573 transparent latches (8 per package, 20 outputs per bank), each bank
gated by its own latch-enable (LE) line. A 573 passes D to Q while LE is high
and holds Q when LE falls. Each output's WS281x waveform only has three
events per bit — **rise at 0, conditional fall at T0H, fall at T1H** — so each
bank needs exactly three latch updates per bit, not a continuous stream.

A latch update is a **time slot of 4 pixels** (104 ns):

```
slot pixel      0          1            2            3
data lines   value      value        value        value
LE_b           0          1            1            0
```

* data set-up before LE rises: 26 ns; LE high: 52 ns; data set up before LE
  falls: 78 ns; data held after LE falls: 26 ns (Pi 4 worst case with the
  clock dither of §3: 37 / 55 / 18.5 ns). The board documentation budgets the
  SN74AHCT573 at t_w ≥ 5 ns, t_su ≥ 3.5 ns, t_h ≥ 1.5 ns and finds ~17 ns of
  hold margin after '541 package-to-package skew (VERIFICATION.md "Latch
  timing"; the 74HCT573 is *not* a substitute). The hold after LE falls is the
  tight parameter, and ringing on the long LE bus at LE's falling edge is the
  realistic failure — bring-up item 6.
* The decoder checks this structure pixel by pixel on every simulated frame
  (`DecodedFrame::latch_violations`): a bank's data lines are constant from
  one pixel before its LE rises to one pixel after it falls, and no two LE
  lines are ever high together — so no bank can latch another bank's data.

Bank *b*'s three slots sit at pixel offsets `edge + 4b`:

```
px:  0   4   8  12  16  20  24  28  32  36  40      48
     [B0][B1][B2][B0][B1][B2]    [B0][B1][B2]
      mask (rise)  data (T0H)     0 (T1H)
```

Q changes when LE rises (slot pixel 1). Both edges of a pulse move by the same
`4b + 1` pixels, so each bank's high times are exactly 312.5 / 729.2 ns; the
banks are merely skewed by 104 ns from each other, which is irrelevant to the
strings. Each edge needs a window of `banks × 4` pixels before the next edge:
`T0H ≥ 12`, `T1H − T0H ≥ 12`, `48 − T1H ≥ 12`. With 12/28 this allows **three
banks (60 outputs)**; `BitTiming::validate(clock, banks)` enforces it. A
fourth bank would need a 16-pixel window and T0H/T1H of 417/833 ns — outside
the WS2815/WS2811 windows — so 60 outputs is the right maximum for 38.4 MHz.

During blanking all lines are low, LE included: latches hold (low) and the
waveform is unaffected. Reset lines contain no LE pulses. After the last data
line of a frame every bank has latched 0.

Between the slot windows the data lines are 0. LE pulses keep running on idle
lines (latching zeros); that is harmless and keeps the static regions constant.

## 8. Encoder (`WsEncoder`)

1. For each line and each lane (bank), outputs are processed in groups of
   eight. The eight outputs' R (then G, then B) bytes are packed into a `u64`
   and **bit-transposed** with three delta-swaps (`transpose8`): byte *j* of
   the result holds bit *j* of all eight outputs.
2. A per-group 256-entry table maps such a byte to the board's DPI bits (this
   handles difftx's non-contiguous port order and the bank's bit positions in
   one lookup). The mask uses the same table.
3. Only the *dynamic* pixels are written: direct `[0, 28)` of every bit
   (mask + data), latched the edge-0 and T0H windows. Static regions
   (zeros, T1H latch slots) are written the first time a buffer is used; a
   per-buffer `BufferState` tracks this and how many lines the previous frame
   in that buffer used, so a shorter frame clears only the lines that need it.
4. The mapped DRM buffer is write-combined: the encoder only ever writes it,
   sequentially, never reads it.

   In latched mode the slots are written edge by edge (all banks' edge-0
   slots, then all T0H slots), which is strictly ascending addresses.

Measured (`cargo run --release -p pixelplus-output --example encode_bench`,
Xeon 2.1 GHz, one core): **60 × 1600 LEDs: 0.82 ms per frame** steady state
(5.5 ms for the first, full-template frame); 4 × 1600: 0.54 ms. A Cortex-A72 at
1.5–1.8 GHz is roughly 4–6× slower per core and write-combined memory costs
more than cached memory, so expect ~5–10 ms on a Pi 4 and ~10–15 ms on a Pi 3 /
Zero 2 W — comfortably inside the 25 ms (40 fps) / 50 ms (20 fps) budget.
Measure on hardware with the same example. (The whole test-suite, including
the full 60 × 1600 round trip, also passes for `aarch64-unknown-linux-gnu`
under qemu-user.)

## 9. Device tree overlay and boot configuration

`overlays/pixelplus-dpi.dts` (Pi 0–4, target `&dpi`) and
`overlays/pixelplus-dpi-pi5.dts` (Pi 5, target `&rp1_dpi`) add a `panel-dpi`
node with the timings above and connect it to the DPI block. Every timing is
an overlay parameter (`vactive=`, `hfp=`, `clock-frequency=` …).

**The overlays claim no pins** (`pinctrl-0 = < >`). Consequences:

* The DPI engine may run from boot (the kernel console may even draw into
  it), but nothing reaches the header until `pixelplusd` switches exactly the
  board's pins to the DPI function (`pinmux`): GPIO4–7 for difftx/diffsmart,
  GPIO4–23 + 25–27 for difftxlarge. Only the pins a board needs are claimed,
  and no garbage is ever clocked into pixels.
* On stop — and whenever the daemon is not running — the pins are GPIO
  outputs driven low with pull-downs: the WS281x idle state.
* `config.txt` adds `gpio=4-7=op,dl` (or `4-23,25-27`) so the firmware holds
  the lines low from very early boot, before the kernel. (BCM GPIO0–8 reset
  with pull-ups; the diffsmart's 2 kΩ and difftxlarge's pull-downs cover the
  moment before the firmware runs.)

Pin muxing uses `pinctrl` (raspi-utils, all models) and falls back to direct
`/dev/gpiomem` register access (GPFSEL, GPCLR0, pull control) on
BCM283x/BCM2711 when `pinctrl` is missing. The pixel function is ALT2 on
BCM283x/2711 and `a1` on RP1.

`pi_config::config_txt(board, soc, geometry)` generates the fragment, e.g.
difftxlarge on a Pi 5 with 1600-LED strings:

```ini
[all]
dtparam=i2c_arm=on
dtoverlay=pixelplus-dpi-pi5,vactive=1607
gpio=4-23,25-27=op,dl
dtoverlay=i2c-rtc,ds3231
usb_max_current_enable=1
```

The legacy firmware options (`enable_dpi_lcd`, `dpi_group`, `dpi_mode`,
`dpi_output_format`, `dpi_timings`) only apply to the retired firmware display
driver; Raspberry Pi OS Bookworm/Trixie use KMS, where they are ignored, so
PixelPlus does not emit them.

## 10. Backends

`PixelOutput { start, write_frame, stop, stats }`:

* **`DpiOutput`** — DRM/KMS. Chosen over `/dev/fb0` because fbdev emulation
  binds to whichever connector the kernel prefers (often HDMI), cannot be
  page-flipped reliably, and gives no completion events. The backend scans
  `/dev/dri/card*` for a connector of type DPI (on the Pi 4 the display card
  is usually `card1`; on the Pi 5 RP1 DPI is a separate card), reads its mode,
  derives the `DpiGeometry` from it, allocates **two dumb buffers**, clears
  any CRTC colour management a previous client left (`DEGAMMA_LUT`, `CTM`,
  `GAMMA_LUT` — a LUT would rewrite the waveform bits), sets the mode, muxes
  the pins and then **scans out one complete idle frame on the real pins**
  before returning: on the difftxlarge the 573 latches power up undefined and
  LE is held low until the pins are muxed, so this frame latches 0 into every
  bank and ends with the reset before the first data frame (it also proves
  page flips complete). It then page-flips on vblank with events. `write_frame` waits for the
  previous flip (≤ one refresh), encodes into the back buffer and queues the
  next flip. `stop` sends one all-black frame to every LED, waits for it to be
  scanned out, parks the pins low and restores the previous CRTC state. If the
  device is missing the error says to run `pixelplus config-txt` and reboot.
  DRM master is needed; a desktop session on the same card will make
  `start()` fail with a clear message.
* **`SimOutput`** — keeps the last frame per output in memory (`SimHandle`
  for the UI preview). `SimOutput::verifying(layout, geometry)` additionally
  encodes every frame, runs it through the **independent decoder**
  (`WsDecoder`: replays scan-out including blanking, models the 573 latches,
  measures every pulse against `Ws281xSpec::COMMON`) and fails the frame on
  any data or timing error. The test-suite uses it for randomised round trips
  of all boards, and for a full 60 × 1600 frame.
* **`NullOutput`** — counts and discards.

## 11. Colour pipeline

`PixelPipeline` maps show-order RGB to wire bytes per output: colour order
(`ColorOrder::source_indices`), then one 256-entry LUT per output combining
`gamma`, the output's `brightness` and the player's master brightness
(`round(255 · (x/255)^γ · b% · m%)`). Disabled outputs send black of the same
length (so the string goes dark rather than freezing on its last frame).

## 12. Oscilloscope verification (`ScopePattern`)

`pixelplus test-output --pattern scope --scope <name>`:

| pattern | expect on every data pin |
|---|---|
| `zeros` | 312 ns pulses every 1.25 µs (1.875 µs after each 24th bit) |
| `ones` | 729 ns pulses every 1.25 µs |
| `alternating` | 0xAA: long, short, long, short … |
| `checker` | 24 short then 24 long pulses per LED |
| `identify` | the first byte on output *k* is *k* in binary — verifies the pin map and latch banks |

## 13. Bring-up checklist (needs real hardware)

Nothing in this crate has run on a Pi yet. Everything below is an assumption
that could not be verified by the test-suite; each item says how to check it.
Tools: a ≥ 100 MHz scope (a 24 MHz logic analyser is too slow for 26 ns
pixels), `pixelplus test-output --pattern scope --scope <name>`, root shell.

1. **Real DPI clock and its source, per Pi model** (Zero 2 W, 3B+, 4, 5).
   `sudo cat /sys/kernel/debug/clk/clk_summary | grep -i dpi` (Pi 5: look for
   `clk_dpi` / `pll_video`). Expect 38400000 (Pi 3: 38403xxx). Record the
   parent: on a Pi 4 we expect the 54 MHz crystal (§3).
2. **Pixel quantisation.** Scope any data pin with `alternating`. On a Pi 4
   expect edges on an 18.5 ns grid (pulse widths 296–315 / 722–741 ns); on a
   Pi 3 26–28 ns pixels; on a Pi 5 26.04 ns.
3. **T0H/T1H at the far end of the chain.** With `zeros` and `ones`, measure
   the high time on the Pi pin, at the AM26C31 output (difftx: U1 Y; difftxlarge:
   a driver) and at the receiver's pixel output (diffrx/diffsmart). Pass:
   T0H 250–380 ns and **T1H 650–750 ns at the pixel input**, bit period 1250 ns,
   and the gap after every 24th bit 1.8 µs (`checker`). If T1H exceeds
   ~745 ns at the receiver, note the per-stage distortion and report it.
4. **Bit/GPIO map and byte order** (DESIGN §2, rgb888 bus format). With
   `identify`, the first byte on output *k* must read *k* MSB first.
   difftx/diffsmart: Port 1..4 on GPIO5, 6, 7, 4. difftxlarge: all 60 outputs,
   which also proves bank order (LE0 = GPIO27 → J1–J5, LE1 = GPIO26 → J6–J10,
   LE2 = GPIO25 → J11–J15). **Pi 5 specifically**: check GPIO4, 12 and 20
   (the LSB of B, G and R) carry clean data with `ones` and `zeros` — any RP1
   dithering or colour processing would show up on exactly these pins.
5. **Blanking and mode acceptance.** `dmesg | grep -iE "dpi|vc4|rp1"` shows no
   errors or FIFO underruns at the configured height; on the difftxlarge
   `vactive` up to the model limit (2048 lines Pi 3, test 1607 and a tall mode
   such as 4000 on Pi 4/5 — RP1's limit of 4096 is an assumption). With the
   scope on a data pin during the reset gap, the line must stay low for
   ≥ 280 µs between frames (`zeros`).
6. **difftxlarge latch timing and LE integrity.** Scope LE0–LE2 (GPIO27/26/25)
   at the far latch of each bank (bus test points) together with a data line:
   LE pulse ≈ 52 ns (Pi 4 ≥ 37 ns), clean single edges — **no ringing that
   re-crosses the 573's input threshold within ~26 ns after LE falls**, data
   stable ≥ 10 ns after LE's falling edge at the latch pins. Then run
   `identify` and a full-length `checker` into all 60 outputs. If hold is
   marginal, report it: the slot can be reshaped in software (e.g. data, LE,
   data, data = 26 ns LE, 52 ns hold) without a board change.
7. **Power-up / start-up.** Power the difftxlarge with strings attached and
   watch a string for the first seconds: static levels while the Pi boots are
   expected (latches undefined, LE held low), but **no LED may flash** when
   pixelplusd starts (the idle priming frame of §10 must precede the first
   data). Repeat by `systemctl restart pixelplusd` during a show, and with
   `kill -9 $(pidof pixelplusd)`: `ExecStopPost=pixelplus pins release` must
   park the pins; LEDs may freeze, but must not show console/garbage patterns.
8. **Pins held low by the firmware.** Before pixelplusd starts (e.g. with it
   disabled), `pinctrl get 4-27` must show the pixel pins as `op dl`
   (`gpio=…=op,dl` from config.txt) — **on a Pi 5 in particular**, where RP1
   GPIOs are set up by the firmware differently. After start: `a2` (Pi 0–4) /
   `a1` (Pi 5) with `pd`; after stop: `op dl pd` again. GPIO0–3 and (difftxlarge)
   GPIO24 must never change.
9. **`/dev/gpiomem` fallback** (only used when `pinctrl` is missing): on a Pi 3
   or 4 temporarily `sudo mv /usr/bin/pinctrl{,.bak}`, start/stop output, and
   confirm the same `pinctrl get` states as item 8 (restore pinctrl first).
10. **Overlay on each model.** `dtoverlay -l` lists `pixelplus-dpi` /
    `pixelplus-dpi-pi5`; `ls /sys/class/drm/` shows a `card*-DPI-1` connector;
    `cat /sys/class/drm/card*-DPI-1/modes` shows `1152x<vactive>`. On a Pi 5
    confirm the overlay's `&rp1_dpi` target resolves and that HDMI still works.
11. **DRM master / desktop.** On Raspberry Pi OS Lite `start()` succeeds. With
    a desktop session on the same card (Pi 0–4: HDMI and DPI share vc4) it must
    fail with the "is another program … using the display" error, not hang.
12. **Encoder speed on the slowest Pi.** `cargo run --release -p
    pixelplus-output --example encode_bench` on a Zero 2 W: steady 60 × 1600
    must stay well below 50 ms (expect ~10–15 ms); also watch `lastEncodeUs` /
    `maxEncodeUs` in the output stats during a show.
13. **Conflicting overlays.** The generated fragment warns about w1-gpio
    (GPIO4), SPI0 (GPIO7–11), UART0 (GPIO14/15), I²S (GPIO18–21) and PWM/fan
    pins; confirm none are enabled in `/boot/firmware/config.txt`.

### Board peripherals (`pixelplus-hw`)

14. **EEPROM (AT24C256, 0x50).** `pixelplus eeprom write --board … --rev …`
    then `pixelplus detect`: PPX1 record read back. Test both paths: with the
    kernel `at24` driver (`/sys/bus/i2c/devices/1-0050/eeprom`) and without it
    (`echo 0x50 | sudo tee /sys/bus/i2c/devices/i2c-1/delete_device`, then the
    direct i2c-dev path writes 64-byte pages and polls for the ~5 ms write
    cycle). With the write-protect jumper closed the write must fail with the
    "JP1" message, not succeed silently.
15. **INA226 (difftxlarge, 0x40, 10 mΩ).** With a known load, `12 V input`
    must match a multimeter within ~1 % and `Input current` within ~2 %.
    **Check the sign**: current must read *positive* when the board draws
    power. Negative means IN+/IN− are swapped relative to the current
    direction (then report it; the driver reports the signed value). If the
    kernel `ina2xx` driver is bound instead, its shunt must be 10000 µΩ (the
    driver default).
16. **LM75B (0x48/0x49).** Readings plausible (±2 °C of ambient at idle) and
    in 0.125 °C steps.
17. **DS3231 (0x68).** `pixelplus doctor` / `hwclock -r -f /dev/rtc1` (Pi 5) or
    `/dev/rtc0`; set the time, remove power for a minute with the CR2032 in,
    confirm it kept time and the oscillator-stop flag is clear.
18. **OLED (0x3C).** `pixelplus detect` must list 0x3c (probed with a write,
    since SSD1306 modules need not acknowledge reads). The status screen must
    be upright and not shifted: a module that is really an **SH1106** (132
    columns, no horizontal addressing mode) shows a garbled or 2-pixel-shifted
    image — note the controller printed on the module.
19. **Buttons (GPIO24 on difftxlarge).** Short taps (< 30 ms) and long
    presses must each give exactly one press and one release event.

