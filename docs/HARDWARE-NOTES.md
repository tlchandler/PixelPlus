# Hardware notes: timing and sync (for the next board revisions)

Summary of the sync research (2026-09) as far as it concerns the boards
(difftx, diffsmart, difftxlarge). Short version: **no board change is needed
for good sync**; add one cheap 3-pin **SYNC header** on the next revision of
every board to keep a GPS-PPS upgrade and a hardware timing test point open.

How sync works in software is described in `docs/ARCHITECTURE.md` §7.4.

## Where the timing error comes from

| Source | Size | Fixed by |
|---|---|---|
| Network clock estimate (Wi-Fi) | 0.1–1 ms with power save off; 20–100 µs on Ethernet | software (4-timestamp exchange, kernel receive stamps, offset + drift fit) |
| Frame vs. vblank phase (per controller) | ±R/2: ±12 ms at 800 px/output (40 Hz), ±6 ms at 400 px, ±1.7 ms at 100 px | only a higher refresh, i.e. **shorter strings per output** (split long strings across more ports) |
| WS281x latch delay (string length) | L × 30.6 µs + 0.3 ms: 24.5 ms at 800 px, 49 ms at 1600 px | software (presentation time + latch alignment) |
| Wi-Fi power save | 50–1000 ms spikes | OS configuration (image) + runtime warning |

The vblank grid is a free-running clock set by the DPI pixel clock; its phase
cannot be steered without a modeset (a visible glitch), so ±R/2 per controller
is the floor for DPI output. Everything else is solved in software.

## Recommended: 3-pin SYNC header (every board, next revision)

```
 SYNC  1  3V3
       2  GND
       3  GPIO24 ── 330 Ω ──┬── pin 3
                            ├── ESD TVS diode to GND (e.g. PESD3V3 / TPD1E10B06)
                            └── (optional) 10 kΩ pull-down, DNP by default
```

* **difftxlarge:** GPIO24 (header P1-18) is the only free GPIO — every other
  one is a DPI data or latch-enable line, and the UART (GPIO14/15) and both SPI
  buses are DPI pins there. GPIO24 is DPI D20, but `pinmux.rs` muxes only the
  board's own pins, so it stays a normal GPIO (`OutputLayout` never claims it).
* **difftx / diffsmart:** only GPIO4–7 are used for ports (plus I²C), so any
  free GPIO works; use **GPIO24 as well** so software has one convention.
* Cost ≈ $0.20 (header, resistor, TVS). Place it near the board edge, away from
  the differential drivers; a keyed 3-pin JST-XH or a plain 2.54 mm header.

It serves two purposes:

1. **GPS PPS input (future option).** A GPS module with a PPS output
   (ATGM336H ~$4–8, u-blox M8/M10 ~$10–25 plus a patch antenna) gives every
   controller the same second to < 1 µs, independent of Wi-Fi. Only PPS, 3V3
   and GND are needed — no NMEA: chrony numbers the pulses from the network
   time (`refclock PPS /dev/pps0` without `lock`; the clock must already be
   within ±0.5 s). Software: `dtoverlay=pps-gpio,gpiopin=24`, chrony, and the
   daemon using the PPS-disciplined clock when chrony reports lock (not
   implemented; worth it only if field data shows clock errors above ~2 ms,
   e.g. congested 2.4 GHz, many Zero 2 Ws, or separate yards on mesh Wi-Fi).
   It needs a sky view (fine through plastic, not metal or a garage roof).
   It does **not** fix the ±R/2 output quantisation.
2. **Timing test point.** In a self-test mode the output can drive GPIO24
   (a spare DPI bit, D20) high on the first line of every Nth frame. It is
   scanned out by the same hardware as the pixels, so a two-channel scope or a
   $10 USB logic analyser (sigrok/PulseView) across two controllers shows their
   exact frame-to-frame offset with no software jitter (and lets us verify the
   DRM vblank timestamp against real scan-out, DESIGN.md §13 item 15).

## Options considered and not recommended for sync

| Option | Why not |
|---|---|
| DS3231M SQW → GPIO (difftxlarge RTC) | An independent oscillator per board: gives a ±5 ppm frequency reference, no common phase between controllers. The software drift estimate already measures frequency against the leader. |
| Better RTC (RV-3032-C7) / TCXO / OCXO | Cannot replace the SoC crystal; helps wall-clock time only when offline. |
| Wired sync line (RS-485 PPS/timecode) | The Falcon differential RJ45 pinout uses all 4 pairs for ports — needs its own cable. If you run a cable, run Ethernet (20–100 µs in software, ns with hardware PTP on Pi 5/CM4). |
| PTP-capable PHY/NIC on the board | Pi 5 and CM4 already have hardware PTP; µs are invisible for lights; no free SPI on difftxlarge. |
| 433 MHz / LoRa sync pulses | Interference, FCC §15.231 limits periodic transmissions; LoRa needs SPI (not free on difftxlarge). Solves what software already solves. |
| Dedicated MCU (e.g. RP2040 PIO) timing the pixel output | Would remove the ±R/2 vblank quantisation (the only thing software cannot fix) but means a new output architecture (20+ data lines, latches). Revisit only if ±R/2 is visible on long strings after splitting them. |

## Operating recommendations (no hardware change)

* **Wi-Fi power save off** (the image does it; the Controllers page and the
  health check warn when a controller still has it on).
* Prefer **5 GHz** where available (Pi 3B+/4/5); the Zero 2 W is 2.4 GHz only —
  pick a clean channel (1/6/11), or a dedicated access point/SSID for big shows.
  Ethernet is best for fixed controllers.
* Keep strings per output short where a board has spare ports: refresh and
  therefore alignment scale with the longest string of the controller.
* Calibrate the sound delay (Settings → Audio → **Sync lights to sound**) where
  the audience listens: FM transmitters, TVs and Bluetooth speakers add delay,
  and sound travels ~3 ms per metre.
