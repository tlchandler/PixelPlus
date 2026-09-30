# difftxlarge rev A — verification record

**difftxlarge** is the 60-output sibling of `../difftx` (the FPP Remote pHAT). It has fifteen RJ45 jacks with 4 RS-422
pixel ports each, in the Falcon differential pinout. They land on `../diffrx` / `../diffsmart`, the same as difftx's
single jack. A Raspberry Pi drives it through FPP's DPIPixels output in **latch mode**: 20 data lines and 3 latch
banks. It also carries:

- an RTC;
- a 12 V voltage and current monitor;
- two temperature sensors;
- a fan thermostat;
- line-level audio out;
- an OLED header;
- a USB-C output that powers the Pi.

The design choices and their sources are in `DESIGN.md` and `RESEARCH.md`.

Board: **310 × 206 mm = 639 cm²**, under JLCPCB's 650 cm² "Large Size" line. It is four layers with 1 oz copper,
and all 389 parts sit on the top side.

## Port 3 polarity fix (2026-09-29)

**Every jack's port 3 is now pin 5 (+) / pin 4 (−), the Falcon standard.** Before this fix, the board had 4 (+) / 5 (−), copied
from difftx rev D. On 2026-09-29, a PixelController SRx1 v5.01 fed by difftx rev D showed that assignment was reversed: port 3
sparkled white, and a cable with pins 4 and 5 swapped at one end fixed it. (Source: Dan Kulp, "blue (5, 4) is for port 3",
https://falconchristmas.com/forum/index.php?topic=15638.0. See also the repo `CLAUDE.md`.)

- `CH` in `gen_sch.py` and `check_netlist.py`: port 3 is now `("7", "6", "5", "5", "4")`, so 2Y goes to pin 5 and 2Z goes to pin 4.
  The notes, the J-descriptions, the silkscreen legend (`P3 = 5/4`), `DESIGN.md` and the EEPROM strings notes all match.
- **The board is a full `gen_pcb.py` regeneration** (the jack cell had to be re-routed: 0 cell failures, 0 window or global failures,
  about 8 minutes). `compare_boards.py` against the previous board: footprints (444) are identical and the via count (816)
  is unchanged. **Every track and via difference is on a `Jn_3P` / `Jn_3N` net**, and the only silk difference is the legend.
- Checks: ERC 0; `check_netlist.py` 0 errors; DRC with `--schematic-parity --refill-zones` gives 0 violations, 0 unconnected and 0 parity issues.
  All-severity results are the same as before (4 `track_dangling`, 199 `lib_footprint_mismatch`). `jlcpcb/` and
  `eeprom/difftxlarge-eeprom.bin` have been regenerated.
- Pairing with a difftx rev D is not an issue (both are transmitters). This board now works with Falcon receivers, diffrx and diffsmart
  on a straight lead.
- A backup of the folder before this fix is in `../_backups/`.

## Files and how they are made

| Step | Command | Output |
|---|---|---|
| Parts | `python fetch_parts.py` | `jlc/` symbols, footprints, 3D |
| Schematic | `python gen_sch.py` | `difftxlarge.kicad_sch` / `.kicad_pro` / `.kicad_sym`, lib tables |
| Netlist | `kicad-cli sch export netlist -o difftxlarge.net difftxlarge.kicad_sch` | `difftxlarge.net` |
| Trace check | `python check_netlist.py` | every output traced from Pi pin to RJ45 pin |
| Board | KiCad Python `gen_pcb.py` (30+ min) | `difftxlarge.kicad_pcb` |
| Fab files | KiCad Python `export_jlc.py` | `jlcpcb/` gerber zip, BOM, CPL |
| Cape EEPROM | `python make_eeprom.py` | `eeprom/difftxlarge-eeprom.bin` |

**The board as it stands is exactly what `gen_pcb.py` produces** (the last full run, after the ribbon-header
move). `patch_board.py` and `_2` … `_5` are a record of earlier in-place edits to previous versions of the board;
everything they did is in `gen_pcb.py` or `board_fixups.py`, so they don't need running again.

**How `gen_pcb.py` routes.**

- The bus and its taps, the power pours, and a handful of awkward pins are drawn directly.
- One jack cell is routed on a 20 mm scratch board at a 0.1 mm grid, then copied to all fifteen columns.
- Bank 1's latch-to-LED/driver wiring is routed, then copied to banks 2 and 3.
- The rest is routed in fine-grid windows: Pi header + buffers, each buck, USB-C + audio, and the power section.
  A window with failures is re-routed from scratch with every net that has failed so far moved to the front (up to
  five attempts). The Pi header window needs four.
- A coarse full-board pass then joins the few long I²C and supply runs between windows.

## Automated checks (KiCad 10.0.6, 2026-09-25)

| Check | Result |
|---|---|
| `kicad-cli sch erc --severity-error --severity-warning` | **0** errors, **0** warnings |
| `check_netlist.py`: all 60 outputs traced Pi pin → pull-down → buffer → 33 R → bus → latch bit (right bank) → driver + LED → PSM712 → RJ45 pin; driver/latch supply pins; ribbon 5 V pins open | **0** errors |
| `kicad-cli pcb drc --schematic-parity --refill-zones --severity-error` | **0** violations, **0** unconnected pads, **0** parity issues |
| Same, `--severity-all` | 199 `lib_footprint_mismatch` (expected: the generator strips silkscreen outlines from passives, LEDs, jacks and terminals); 4 `track_dangling` (0.3 mm router stubs next to pads of their own net; the nets are fully connected) |
| BOM / CPL | 47 lines, 389 placed parts, **every line has an LCSC code**, 0 bottom-side parts |
| Reproducibility: full `gen_pcb.py` run to a scratch file, compared by geometry (`compare_boards.py`) | Footprints (442), vias (814) and silk text (160) **identical**. Tracks differ in 3 vs 17 short segments out of ~45,360: the 3 router links that patch 2 put back sit at slightly different coordinates, and the regeneration keeps 14 sub-1 mm stubs that patch 1 removed. The regenerated board is also clean on its own (0 violations, 0 unconnected, 0 parity; 4 `track_dangling` warnings) |

Design rules are written into the board file: 0.2 mm track and clearance, 0.7/0.3 mm vias (0.2 mm annular ring,
JLCPCB's recommended figure), 0.5 mm copper-to-edge (the project's setting, stricter than JLCPCB's 0.3 mm), 0.3 mm hole clearance and 1.0 mm silk text.

## Revisions after the first review (2026-09-25)

| Comment | Change |
|---|---|
| Audio jack set back from the edge | The jack itself was at the edge; its **3D model** sat 3.5 mm back from where the manufacturer's drawing puts the part (11.6 mm body + 2.5 mm nose, sleeve pin 2.5 mm behind the body front). The model is now offset +3.5 mm in the footprint, and J19 is at y 9.0 with the nose at the edge. (An intermediate fix that moved the jack itself forward had put the real nose 2.6 mm past the edge; the independent review caught it.) |
| RJ45s sunk into the board in 3D | Not a placement error. 342 model paths in this and the other designs pointed at `Documents/jumperless/...`, which no longer exists, so KiCad showed stale cached models. All repointed to `${KIPRJMOD}/jlc/jlc.3dshapes` (`../repoint_3d_models.py`); the jacks now sit on the board |
| 12 V in looked like bare holes | Same cause: it is a KF301 5 mm screw terminal (C474881), and now renders as one |
| OLED header crowded between two terminals at the edge | J22 is now a 1x4 **female** socket (C2718488) inside the board at (266, 33), with a 27.3 x 27.8 mm outline below it that is kept clear. A 0.96 inch SSD1306 module plugs in face up and lies over that area. No mounting holes are drilled: their spacing varies between module vendors |
| U30 legend under the shunt | Moved clear |
| Ribbon header J16 too close to the Pi | J16 and everything below it moved 6 mm down (board 310 × 200 → 310 × 206 mm, 639 cm²). The shroud is now about 6.4 mm from the Pi's edge and about 7.4 mm from its two header-side standoffs, so a bulky IDC plug and the ribbon's fold both clear the Pi |
| Found on the way | A routing window that a part overhangs treats the part as an obstacle, and its nets silently go unrouted. Windows along the board edge now extend past it (with the off-board part kept out). A window with failures is re-routed with every failed net first |

## Independent adversarial review (2026-09-25) and what was done

A separate agent reviewed the design to break it: its own netlist and pad trace of all 60 outputs, DRC/ERC,
footprint-vs-library comparison, and sub-reviews of power, FPP software and BOM/mechanical against the datasheets
and the FPP source. **No blockers.** A follow-up pass re-traced all 60 outputs on the regenerated board and confirmed the fixes below. Its findings and the changes made:

| Finding | Change |
|---|---|
| All 5V_PI current (up to 3 A) reached J18 through one via; J18's GND pads on 0.25 mm stubs | Each VBUS pad now has a full-width stub with two vias into the B.Cu 5V_PI pour (four in all); each GND pad a 0.5 mm stub and its own via |
| D62 (SMDJ15A TVS) returned to ground through a 0.25 mm stub and one via | D62's anode sits in an F.Cu GND pour with four vias; its cathode is inside the 12VP pour |
| Polarity marks stripped from the LEDs, D61/D63/D64 and C32 (and the two LED parts number their pads opposite ways) | A bar beside every cathode pad and a "+" by C32's positive pad |
| No mounting holes along the top edge, where the terminals and fuse take screwdriver force | H11 (152, 4) and H12 (212, 4), M3 |
| J19's nose overhung the edge by about 2.6 mm (the documentation said otherwise) | Back at the edge; see the first table |
| Bring-up step 4 would fail: FPP deletes the i2c device at every boot, so the sysfs `eeprom` file isn't there | Step 4 now registers the device first |
| Latch timing mis-stated here; the HCT573 fallback is limited by hold time, not pulse width | Corrected below; the HCT573 is no longer suggested |
| Stock doubts (LCSC pages) | JLCPCB's own stock checked: see Cost notes |
| INA226 sense lines landed in the pours a few mm from R2 | IN− and IN+ now land on R2's own pads (Kelvin) |
| 5V_DRV at about 100 µF effective, the top of TI's range | Two of the four 22 µF bulk capacitors removed (C29, C30) |
| OVER TEMP comparator had no hysteresis | R48 330 k from its output to the NTC node (IN+B), with R49 10 k pulling the open-collector output up: alarm on at ~65.4 °C, off at ~64.4 °C; the fan's switch-on point moves only from ~43.1 to ~43.8 °C. (The first try, 100 k with no pull-up, left LED65 glowing faintly on R48's ~10 µA; the follow-up review caught it.) |
| `lm75` driver gives 0.5 °C steps | The cape registers the sensors as `lm75b` (0.125 °C) |
| "U32" and "U35" legends on pads | Moved |

## Signal path

| | |
|---|---|
| Output *k* (FPP port *k*+1, *k* = 0…59) | bank *b* = *k* // 20, data line D(*k* % 20) = GPIO(4 + *k* % 20), jack J(*k*//4 + 1), port *k* % 4 + 1 |
| Latch enables | LE0 = GPIO27 (P1-13) → J1–J5, LE1 = GPIO26 (P1-37) → J6–J10, LE2 = GPIO25 (P1-22) → J11–J15. The order of `"latches"` in the EEPROM |
| Buffers | 3 × SN74AHCT541 (3.3 V in, 5 V out). Channels are assigned in header-pin order for routing (a buffer doesn't care) |
| Latches | 9 × SN74AHCT573, three per bank. Each group of four bits is laid in reverse so the outputs meet the LEDs in jack order |
| Latch timing | Each bank's slot is 4 framebuffer pixels of 26.04 ns: data, data + LE, data + LE, data (`WriteLatchedDataAtPosition`, DPIPixels.h). So LE is high **52 ns**, data is set up **78 ns** before LE falls and held only **26 ns** after (the next slot changes it). The SN74AHCT573 needs tW ≥ 5 ns, tsu ≥ 3.5 ns, th ≥ 1.5 ns: about 17 ns of hold margin remains even with worst-case skew between two '541 packages. The 74HCT573 (th 9 / 11 / 15 ns at 25 / 85 / 125 °C) would be marginal hot and is not a substitute |
| Drivers | 15 × AM26C31, G and ~G tied low (enabled) as on difftx. Port 1 = ch4, 2 = ch1, 3 = ch2, 4 = ch3 (a layout choice); Y = +, Z = − |
| RJ45 | (+/−) port 1 = pins 1/2, port 2 = 3/6, **port 3 = 5/4**, port 4 = 7/8: the Falcon standard, same as diffrx and diffsmart. difftx rev D has port 3 reversed (4/5) |
| Per output | PSM712 TVS pair (+12/−7 V stand-off); green activity LED on the latch output through 2 k (~1 mA) |
| I2C (i2c-1, the Pi's 3V3) | AT24C256 cape EEPROM 0x50, DS3231M RTC 0x68, INA226 0x40, LM75B 0x48 (driver row) and 0x49 (power), OLED socket 0x3C |

## Power

- **Input chain:** J17 12 V → F1 (ATO holder, fit a **5 A** blade) → Q1 AO4407A high-side P-FET (reverse polarity) → R2 10 mΩ (INA226) → +12V.
  - An SMDJ15A TVS sits behind the FET, with a 470 µF bulk capacitor.
  - A red REVERSED LED is lit only when the supply is backwards; a 1N4148W keeps 12 V off it in normal use.
- **5V_PI (U28 TPS56637, 5.10 V):** feeds J18 USB-C, advertising 3 A with 10 k Rp on each CC pin.
  - A short C-to-C cable powers the Pi through its own input.
  - **The ribbon carries no 5 V:** pins 2 and 4 are only two 28 AWG conductors at about 1 A each.
- **5V_DRV (U29 TPS56637):** the buffers, latches, drivers and LEDs, about 1.5 A with 60 terminated pairs. It feeds In2 under the driver half of the board.
- **Both bucks** follow TI's 5 V design:
  - 3.3 µH, 3 × 22 µF, 20 k + 100 pF feed-forward, 75 k / 10 k divider;
  - EN divider 100 k / 15 k, so they start at about 8.9 V;
  - VIN / PGND / SW / output as F.Cu pours, stitched to the planes.

## FPP software (from the FPP source, master 98bf09e, about 10.1.x)

- **Licensing:** latches work **without** a license. DPIPixels treats an unlicensed cape as 2 licensed outputs before it checks for latches.
  - What an unsigned EEPROM gets you: **outputs 1–2 at full length, and 3–60 capped at 50 pixels**.
  - Signing needs an FPP license covering ≥ 60 outputs. Do it from Cape Info → EEPROM Signature with **JP1 open** (WP low); FPP writes the signed image back in place.
- `cape-sensors.json` (the temperatures, 12 V and current on FPP's status page) also only loads from a signed EEPROM. Until then, copy its entries into `config/sensors.json` by hand.
- **RTC:** the cape sets `piRTC = 2` and registers `ds3231 0x68` itself, which a Pi 5 needs because its own RTC is rtc0.
- **OLED:** status display only. Every GPIO except 0–3 and 24 (P1-18, unused) is a pixel output, so there are no button inputs.

## Still only checkable on hardware or in JLCPCB's preview

1. **Ribbon orientation.** J16 is laid out as a mirror of the Pi's own header: pin 1 at the right end, odd row on top, straight-through numbering. With a standard 40-way IDC cable, check continuity from J16 pin 1 to Pi pin 1 (3V3) before powering, and confirm the box header's key suits your cable. **Reversed puts 3V3 and GND on GPIO pins.**
2. **JLCPCB rotations** for the TSSOPs, SOICs, VQFN, SOT-23s, USB-C and the through-hole parts: check the preview before paying. The CPL uses the board's own origin (bottom-left), which the gerbers share.
3. **TPS56637 footprint (C841386) and the inductor footprint (C2962881).**
   - Compare the TPS56637 against TI's RPA land pattern.
   - The inductor's import is named after the 0803H. The datasheet puts the 0803 and 0805 in one land-pattern row (same 8.5 × 8.0 body), but confirm it.
4. **PJ-3270-4A pin mapping** (1 sleeve, 2 tip, 3 ring, 4 switch) is from the part's drawing by position. Check L and R on the first board. Its two locating pegs are converted from plated to plain holes by the generator.
5. **CR2032 polarity:** pad 1 is + per the holder drawing and its silk "+". Check before fitting a cell.
6. **Latch-enable signal integrity.** The 52 ns LE pulses (26 ns data hold) cross a bus up to about 280 mm long with 33 Ω source resistors. Scope LE0–LE2 at the far latch (test points on the bus) before trusting all 60 outputs.
7. **Power-up.** Latch outputs are undefined until FPP's first LE pulse. That leaves static levels on the cables, which carry no pixel data.
8. **Thermostat thresholds** (about 43.8 °C fan, 65.4 / 64.4 °C alarm) are computed from the NTC's B value, as on diffsmart, not measured.
9. **RJ45 pin-1 side:** the same part and orientation as difftx. Check continuity from U1 pin 14 (port 1 +) to J1 pin 1 on the first board.

**Not yet done:** difftx rev D had an independent second review before ordering; this board hasn't. One is recommended.

## Cost notes

- **About 28 of the 46 BOM lines are JLCPCB Extended parts**, with a loading fee each per order (about $3 each, estimate).
- **Stock at JLCPCB (2026-09-25):** SN74AHCT573PWR C141311 **1,672** (9 per board, so about 18 boards); 22 µF 1206
  C12891 781,000 (Basic); CR2032 holder C70377 39,963. LCSC's own pages showed less; JLCPCB's stock is what assembly uses.
  Stock moves: confirm it in JLCPCB's order tool at checkout.
- **Latch second source:** TI **SN74AHC573PWR (C132992)**, 2,287 in stock, same TSSOP-20 pinout. The non-T part
  needs CMOS input levels, which it gets here: the latches are driven by the 5 V AHCT541 buffers, not by the Pi.
  Not the 74HCT573 (see Latch timing).

## Bring-up order

1. **12 V only, no Pi.** 12 V on J17 (+ at the right terminal). Check 5.1 V at TP 5VPI and 5VDRV and the 12V / 5V PI / 5V DRV LEDs. REVERSED must stay dark.
2. **Mount the Pi.** Face up on the M2.5 standoffs, ribbon to J16 (check pin 1), C-to-C cable from J18 to the Pi's USB-C. On a Pi 5, add `usb_max_current_enable=1` to config.txt.
3. **Check the I2C devices.** Boot FPP; `i2cdetect -y 1` should show 40, 48, 49, 50, 68 (and 3c with an OLED); addresses FPP has bound a driver to show as `UU`.
4. **Program the cape EEPROM.** FPP releases the i2c device at every boot, so register it first, then write it:
   `echo 24c256 0x50 | sudo tee /sys/bus/i2c/devices/i2c-1/new_device`, then
   `sudo dd if=difftxlarge-eeprom.bin of=/sys/bus/i2c/devices/1-0050/eeprom`. Reboot, sign in the FPP UI, then close JP1.
5. **Test outputs.** Run a test pattern on J1 port 1 into a diffrx, then scope LE0–LE2 (see 6 above), then all 60.
