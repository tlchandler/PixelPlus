# Chandler 4D/8P Differential Receiver v1.00 (diffrx rev C) — verification record

**diffrx** is the receiving half of the pair whose transmitter is `../difftx` (the FPP Remote pHAT).
One cat5 in, four WS2811 pixel data outputs, eight power-injection outputs, and a 12 V 30 A distribution
bus with a main fuse and a resettable fuse per output — so the board *is* the marine fuse panel, and the
supply cable lands on it directly.

**Rev C adds everything the board can do without an MCU on it**: a trip LED across every fuse, an
NTC/comparator fan thermostat with an over-temperature alarm, a reverse-polarity LED, two I²C
temperature sensors on a header, test points, a bleeder and zip-tie strain relief. Nothing added needs
firmware, and nothing added is in the path of the pixel data.

Generated files: `diffrx.kicad_sch` / `diffrx.kicad_pcb` (from `gen_sch.py` and `gen_pcb.py`),
`jlcpcb/` (gerber zip, BOM, CPL from `export_jlc.py`), renders `render_*.png`.
`fetch_parts.py` pulls the JLCPCB parts into `jlc/`; `verify.py` reads the finished board back;
`silkcheck.py` sweeps the silkscreen.

## Port 3 polarity fix (2026-09-29)

**J14 port 3 is now pin 5 (+) / pin 4 (−), the Falcon standard.** Before this fix, the board had 4 (+) / 5 (−), copied from difftx
rev D. On 2026-09-29, a PixelController SRx1 v5.01 fed by difftx rev D showed that assignment was reversed: port 3 sparkled
white and ignored data, and a cable with pins 4 and 5 swapped at one end fixed it. (Source for the pinout: Dan Kulp,
"blue (5, 4) is for port 3", https://falconchristmas.com/forum/index.php?topic=15638.0. See also the repo `CLAUDE.md`.)

- Schematic: the J14 pin 4 and 5 labels are swapped (pin 4 = P3N, pin 5 = P3P), and the notes are updated.
- Board (`patch_port3.py`, mirrored in `gen_pcb.py`): P3P drops through a new via at (13.0, 50.51) and runs on B.Cu
  between pads 4 and 6 to pad 5 (0.2 mm track, 0.22 mm to each pad). P3N takes an F.Cu hop from (12.0, 49.49) to pad 4.
  The silkscreen now reads `P3=5/4`.
- Checks: ERC 0; DRC with `--schematic-parity --refill-zones` gives 0 violations, 0 unconnected and 0 parity issues.
  All-severity results are unchanged apart from the expected `lib_footprint_mismatch` warnings. `jlcpcb/` has been re-exported.
- **Compatibility:** this board now matches Falcon transmitters. Paired with a **difftx rev D**, port 3 is inverted, so use a
  lead with pins 4 and 5 swapped at one end (the same lead that makes rev D work with Falcon receivers).
- A backup of the board before this fix is in `../_backups/`.

## What changed from rev B

| Rev B | Rev C |
|---|---|
| Bolt-down MIDI main fuse, hand-fitted, in neither BOM nor CPL | **Keystone 3557-2 ATO holder** (C352820), machine-placed, takes a standard 30 A ATO blade fuse |
| Nothing said which output had tripped | A **red LED and a 4.7 k across every fuse** — dark in normal operation, lit at ~2 mA once it opens |
| No thermal sensing of any kind | **NTC + LM2903**: fan on at ~43 °C, **OVER TEMP** LED at ~65 °C. No firmware |
| No fan provision | Fused, switched **12 V fan output** on the left edge |
| No way to read board temperature | Two **LM75B** sensors on an I²C header, powered from the header's **VIO** |
| A reversed supply just looked dead | **REVERSED** LED across the FET bank |
| No test points, no bleeder | Eight test points and a 10 k bleeder across C1 |
| Pigtails pulled directly on the screws | **Fourteen 3 mm zip-tie holes** |
| Two full-width F.Cu +12 V bars | One island per fuse, which opens a 6 mm routing lane between terminals |
| Vias at 0.15 mm annular ring (JLCPCB's *absolute* minimum) | All 211 vias at **0.20 mm**, their recommended figure |
| `min_hole_clearance` 0.25 mm, looser than the fab | **0.30 mm**, stricter than JLCPCB's 0.28 mm |
| Red LEDs carried the green LED's `LCSC Part` property from the library | Both LCSC fields now agree on every part |

The board stays **130 × 100 mm**. Part count goes from 79 to 152 footprints (126 of them placed) and the BOM from 23 to 34 lines.

## Automated checks (KiCad 10.0.6, 2026-09-20)

| Check | Result |
|---|---|
| `kicad-cli sch erc --severity-error --severity-warning` | **0 violations** |
| `kicad-cli pcb drc --schematic-parity` | **0 unconnected, 0 schematic parity issues**; 66 `lib_footprint_mismatch` warnings, all expected (see below) |
| router | **0 routing failures** |
| `verify.py` read-back of the *saved* board | matches every table below; ratsnest 0 |
| `silkcheck.py` (silk vs silk, silk vs pads, silk vs board edge) | **0 / 0 / 0** |
| 3D renders top / bottom | inspected |
| adversarial design review | 1 blocker, 1 wrong footprint, 4 false claims in this file — all fixed |

### The adversarial review, and the blocker it found

Rev C went through a second skeptical review. It found **one blocker that would have killed the fan
outright**, plus a wrong part footprint and four false claims in this file. All are fixed below.

**BLOCKER — D8, the fan flyback diode, was fitted backwards.** The SS34 symbol has **pin 1 = cathode**,
and the footprint's polarity band is on the pad-1 side. The board had the cathode on the switch node and
the anode on the fused +12 V. That is not merely a useless flyback — it is a **forward-biased diode
straight across the fan supply every time Q5 turns on**. The loop is F13 (0.30 Ω) + the diode (~0.5 V) +
the FET (<32 mΩ), so roughly 20–35 A of inrush, past the AO3400A's 30 A pulsed rating, and F13 trips in
single-digit milliseconds and stays tripped. The fan would never have run — and it would have failed at
precisely the moment the board wanted cooling, with nothing to indicate why.

Every other diode in the generator (D1, D2, D7) is correctly wired pin 1 = cathode, so this was a
transcription slip rather than a misunderstanding.

It survived the first round because **`verify.py` was wrong in the same direction**: it hard-coded
`("D8", "1", "flyback anode")`, so the read-back printed a confident "D8.1 /FANM flyback anode" and
confirmed the bug. A check that can agree with the fault it exists to catch is worse than no check at
all. `verify.py` now reads the pin *names* out of the netlist and asserts the cathode is on `/FAN12`,
so it cannot make the same mistake twice.

**Also fixed:** R27 was C25804, a **0603** part, placed on an 0805 land — the same class of error as the
R34 one found earlier, and a tombstone risk. It is now on the 0603 land and merges into the existing
10 k line, taking the BOM from 34 rows to 33.

### Two things the first pass found that were wrong before

**The DRC rules were not being enforced.** `gen_sch.py` writes the tightened JLCPCB limits into
`diffrx.kicad_pro`. `min_connection` had gone back from 0.2 to **0.0**, `min_via_annular_width` from
0.15 to **0.1**, and `min_text_height` from 1.0 to **0.8**, so several DRC runs passed against looser
rules than intended.

The culprit is **`pcbnew.SaveBoard()` at the end of `gen_pcb.py`**, which writes the *bound project file*
from the board's in-memory design settings — overwriting whatever `gen_sch.py` had put there with
KiCad's defaults for every constraint the board object did not carry. Exactly the three that survived
(clearance, track width, edge clearance) were the three `gen_pcb.py` was already setting. The fix is
therefore to set **all** of them on `board.GetDesignSettings()` before the save, which is now done.

Two corrections to what an earlier draft of this file claimed:

- It is **not** `kicad-cli` that resets the project. Running `pcb drc` and `sch erc` and diffing the
  rules dict shows no change in KiCad 10.0.6.
- The constraints do **not** "live in the board file". KiCad does not serialise them into
  `.kicad_pcb` at all — load the board alone in an empty directory and every one reads back as a
  default. They live in `.kicad_pro`; setting them on the board object matters only because
  `SaveBoard()` writes that file. `verify.py` now reads the JSON directly and compares it against the
  intended values, so it is checking the thing it claims to check:

| Constraint | mm | | Constraint | mm |
|---|---|---|---|---|
| MinClearance | 0.200 | | HoleClearance | 0.250 |
| MinConn | 0.200 | | HoleToHoleMin | 0.250 |
| TrackMinWidth | 0.200 | | SilkClearance | 0.150 |
| ViasMinSize | 0.600 | | MinSilkTextHeight | 1.000 |
| ViasMinAnnularWidth | 0.150 | | MinSilkTextThickness | 0.150 |
| MinThroughDrill | 0.300 | | CopperEdgeClearance | 0.300 |

`min_hole_clearance` is **0.30 mm**, deliberately stricter than JLCPCB's own 0.28 mm PTH-to-track, so
the board cannot pass something the fab would reject.

`MinResolvedSpokes` is deliberately 1. It guards against a pad hanging off a zone by a single thermal
spoke, which matters when that zone is the pad's only connection. Here it is not: every ground pad also
lands on the solid In1 plane, and the B.Cu fill uses thermal relief purely so bottom-side pads stay
hand-solderable.

**The maze router could finish a track short of a round pad.** Its target-cell test used the pad's
*bounding box*, and a round pad's bbox corners lie outside the copper by (√2−1)·r — 0.37 mm on the
1.8 mm through-hole fuse pads, more than a grid step. A path allowed to end on a corner cell stopped in
bare laminate while the router believed it had arrived. It now asks the pad itself (`pad.HitTest`).
This is fixed in `grid_router.py` and applies to the transmitter board too.

### The 66 expected DRC warnings

`lib_footprint_mismatch` fires because this board deliberately strips the silkscreen outline from the
terminals, fuses, jack, I²C header and every passive. Those outlines tell an assembler nothing the port
labels do not, and rev C needs the room. The gerbers come from the board file, so the libraries not
matching has no manufacturing effect. It cannot be silenced from the project file, because — as above —
`kicad-cli` resets that file.

## Board

130 × 100 mm, 3 mm corner radius. Four M3 holes, 3.2 mm, at (4, 4) (126, 4) (4, 96) (126, 96), each with
a copper keepout so a steel screw cannot reach the +12 V plane.

**Four layers, 1 oz copper on all four.** This is not JLCPCB's 4-layer default, which is 1 oz outer and
**0.5 oz inner** — and the inner layers carry the 30 A. The stack-up is in the board file and in
`diffrx-job.gbrjob`; confirm it on the order form anyway.

| Layer | Filled | Role |
|---|---|---|
| F.Cu | — | components, signal routing, and the power islands below |
| In1.Cu | 11789 mm² | solid GND plane |
| In2.Cu | 11701 mm² | solid +12 V plane — this is what feeds the twelve output fuses |
| B.Cu | 9765 mm² | GND fill (thermal relief) plus signal routing |

F.Cu power islands: twelve `12V_top*` / `12V_bot*` islands of 38.6 mm², one per fuse, in parallel with
the In2 plane; `GND_sources` 252 mm² at the FET sources, stitched to In1 with **24** vias (an earlier
draft said 51 — that was the count of every GND via on the board, not the ones in this island). 30 A
over 24 vias is 1.25 A each.

Two islands have no inner plane behind them, because they are on the *supply* side of the protection and
every plane is on the protected side. Both are carried on **both outer layers and stitched**:

| Island | F.Cu | B.Cu | stitching vias | carries |
|---|---|---|---|---|
| `/12VIN` | 525 mm² | 525 mm² | 12 | J1 positive → F0 |
| `/GNDIN` | 660 mm² | 660 mm² | 25 | J1 negative → the four FET drains |

`/GNDIN`'s stitching used to be a single row at y = 64.5, which meant the B.Cu half of the island
reached the FET drains at y = 59.29 through only the eight vias nearest them, at about 1.9 A each. A
second row of nine at y = 61.5 now sits directly under the drains.

## Can this really carry 30 A? — the arithmetic

IPC-2221, 1 oz = 35 µm = 1.37 mil, external *k* = 0.048, internal *k* = 0.024:

| Conductor | Geometry | Current for a 10 °C rise | Rise at 30 A |
|---|---|---|---|
| In2 +12 V plane | internal, ~128 mm wide | 40 A | under 10 °C |
| `/12VIN` islands | two external layers, 21 mm wide | ~43 A combined | under 10 °C |
| `/GNDIN` islands | two external layers, 10 mm wide | ~25 A combined | **~15 °C** |

`/GNDIN` is the tightest conductor on the board and the reason it is on two layers — on one, IPC-2221
gives 30 A as a **70.6 °C** rise (an earlier draft said 45 °C, which was wrong). IPC-2221 assumes an
isolated trace in still air; both islands sit 0.21 mm above a solid ground plane, so the real figures
are lower. Note that these are raw IPC-2221 numbers throughout — no plane-proximity correction has been
applied to any of them.

## Power budget

A 12 V WS2811 bullet draws about 60 mA (0.72 W) at full white. 800 nodes on one line is 48 A; four such
lines is 192 A, six times the supply. **12 V × 30 A = 360 W is the limit**: about 500 nodes at full
white, or roughly 3200 at the 15–20 % average a real sequence runs at.

That 15 % on a full 800-node line is 7.2 A — more than one MF-R600 will hold. It does not have to: each
data port has **three** fused feeds (the data terminal plus two injection taps), so 7.2 A arrives as
roughly 2.4 A per fuse, and the port's total fused capacity is 18 A.

## Fusing

### F0 — main fuse

**Keystone 3557-2** (LCSC C352820), a pre-assembled two-receptacle PCB holder with four solder pins,
30 A / 500 V, ~$0.96 at quantity, ~4,950 in stock, and JLCPCB wave-solders it. It takes a **standard ATO
blade fuse** — the ones in every auto parts shop.

> **Fit a 30 A ATO fuse, and keep the real continuous load under about 24 A.** The holder is rated 30 A,
> so at a full 30 A it is at 100 % of rating, and automotive practice de-rates a continuously loaded
> blade fuse to 70–80 % or it nuisance-blows. Any realistic sequence is well under this — the 30 A is
> the supply's ceiling, not its working current — but a rig that genuinely draws 30 A all evening wants
> the bolt-down MIDI that rev B used.

Worth recording why this part and not something better: **JLCPCB has nothing better.** The only other
"30 A" PCB fuse holder in their library, C142933 (Littelfuse MINI FL1), is rated **22 A continuous**
despite accepting 30 A fuses, and the proper 30 A ATO holder (C207060, Littelfuse 178.6165.0001) is out
of stock. Most boards in this class do not fuse 30 A on-board at all — they split into 5–10 A branches,
which is exactly what the twelve MF-R600s do.

### F1–F12 — output fuses

Bourns **MF-R600** PPTC, LCSC C208490, from the MF-R series datasheet:

| I<sub>hold</sub> | I<sub>trip</sub> | V<sub>max</sub> | I<sub>max</sub> | R<sub>min</sub> | R<sub>1max</sub> | 1 h post-trip max | max time to trip | tripped P |
|---|---|---|---|---|---|---|---|---|
| 6.00 A | 12.00 A | 30 V | 40 A | 0.005 Ω | 0.020 Ω | 0.040 Ω | 16.0 s at 30 A | 3.50 W |

Sized for the **18 AWG** core of a three-core Ray Wu pigtail. Three things about this choice:

- **Hold current derates hard with ambient** (I<sub>hold</sub> / I<sub>trip</sub>): 6.00 / 12.00 A at
  23 °C, 4.98 / 9.96 at 40 °C, 4.62 / 9.24 at 50 °C, **4.08 / 8.16 at 60 °C**, 3.66 / 7.32 at 70 °C.
  This is the single best reason the board now measures its own temperature.
- **Resistance is a range and it grows.** 0.005–0.020 Ω initially, up to **0.040 Ω** an hour after a
  trip, which is 0.24 V at 6 A and shows as dimming at the far end of that line.
- **PPTCs are slow.** 16 seconds at 30 A. They protect the wiring, not the pixels.

Mechanically the MF-R600 is 19.3 mm wide (A max) on 10.2 mm leads. Terminal columns are 20 mm apart, so
**two neighbouring fuses at maximum body width leave about 0.7 mm between them.** They fit; they are not
loose.

### Trip indicators (new)

A **KT-0805R red LED and a 4.7 k** across each polyfuse. In normal operation the fuse drops a few tens
of millivolts and the LED is dark. When it trips, the load drags the downstream side away from the bus,
about 12 V appears across the fuse, and the LED lights at **2.1 mA**. That current is far too small to
hold the fuse open or to power anything downstream. With the output unplugged there is no current, no
drop and no light — correct, because nothing is wrong.

Read back from the board: every `LF{n}` anode is on `+12V`, its cathode on `/LT{n}`, and `RF{n}` runs
`/LT{n}` → `/V{n}`, for all twelve.

## Thermal: fan thermostat and over-temperature alarm (new)

No MCU. An NTC divider feeds both halves of one comparator.

- **RT5**, Vishay NTCS0805E3103FHT (C3195213): 10 kΩ at 25 °C, B25/85 = 3940 K, ±1 %, −40…+150 °C. It
  sits between the FET bank and the main fuse — the hot corner.
- **U6**, TI LM2903QDRQ1 (C475499), SOIC-8, **AEC-Q100 grade 1, −40…+125 °C**. This matters: the
  ordinary LM393 parts JLCPCB stocks are 0…70 °C commercial, and a box that can reach 70 °C is the wrong
  place for a comparator that stops being specified there.

With a 10 k top resistor the divider node falls as the board heats: **1.60 V at 43 °C**, **0.88 V at
65 °C**.

| Half | Reference | Trips at | Drives |
|---|---|---|---|
| A (pins 1–3) | 10 k / 4.7 k loaded by the 100 k hysteresis leg | **on 44.2 °C, off 40.9 °C** (3.2 °C) | `/FANDRV` → Q5 gate |
| B (pins 5–7) | 4.7 k / 1 k = 0.877 V | **64.6 °C**, no hysteresis | **OVER TEMP** LED |

The fan numbers are *not* the 1.60 V the unloaded divider would give: R32, the hysteresis resistor, sits
on the reference node and pulls it to **1.552 V** with the output low and **1.693 V** with it released.
So TP8 crosses 1.55 V on the way up, not 1.60 V, and the hysteresis is 3.2 °C rather than the 2.4 °C an
earlier draft claimed.

Half A is wired reference-to-IN+ and NTC-to-IN− so its open-drain output releases when hot, and the
4.7 k pull-up then takes the gate high. Half B is the other way round, so its output pulls low when hot
and sinks the LED.

**U6 runs from +12 V, not the 5 V rail.** Its outputs are open-drain, so R33 and R36 still pull up to
+5 V and drive the gate and the LED at 5 V — only the supply pin moved. The reason is the input
common-mode range: at V_CC = 5 V the ceiling is V_CC − 1.5 V (25 °C) or V_CC − 2 V over temperature,
and the NTC node climbs to 3.85 V at 0 °C and 4.88 V at −40 °C. TI SLCS141M's footnote does permit
this — *"the upper end of the input voltage range is V_CC − 1.5 V for one input, and the other input can
exceed the V_CC level; the comparator provides a proper output state"* — and our reference input sits at
1.55 V, comfortably in range. So it was never a defect. But an outdoor board would have spent much of
every year relying on a footnote whose wording differs between datasheet revisions. At V_CC = 12 V the
input window is 0–10 V and the question does not arise, for about 1 mA.

Two honest limitations:

- **R37 (100 k) does not hold the fan off if U6 is unpopulated.** R33 (4.7 k) is pulling the same node
  up to +5 V, so the divider sits at **4.78 V** and Q5 is hard on. An earlier draft of this file claimed
  the opposite. In practice this fails in the safe direction — no comparator means the fan simply runs —
  but do not rely on R37 for anything. It only pulls the released output from 5.00 V down to 4.64 V.
- **Half B has no hysteresis**, so the OVER TEMP LED will hover for a few seconds as the board crawls
  through 65 °C. This is not easily fixable: hysteresis needs positive feedback to the *non-inverting*
  input, and half B's non-inverting input is the NTC node itself, which is shared with half A. Feeding
  the output back there would drag half A's threshold around too. A flickering alarm LED is the cheaper
  outcome.

> **An open NTC fails silently and unsafely.** RT5 is the bottom leg, so if it goes open — cracked
> chip, bad joint, cut trace — R29 pulls the node to +5 V, which both halves read as "very cold". The
> fan stays off and the alarm stays dark. A *short* fails safe and visible (fan on, alarm on). The
> divider cannot be turned round to fix this: with the NTC on top, open reads as 0 V, which the
> reversed comparator sense would also read as "cold". **If TP8 reads 5.0 V, the NTC is open — it does
> not mean the board is cold.** A board that has never once run its fan is the symptom.

## Fan output (new)

**The fan belongs on the enclosure wall, not on the board.** What matters is air moving across the two
fuse rows, which sit at the board edges; a board-mounted fan blows down on one spot. So this is an
output, not a mount.

J16 is a KF301-5.0-3P on the left edge marked **+ − G**. Feed: `+12V` → **F13**, a Fuzetec
FSMD035-30-1206R PPTC (0.35 A hold / 30 V) → `/FAN12` → J16 pole 1. Return: J16 pole 2 → `/FANM` →
**Q5** (AO3400A, 30 V 5.7 A) to ground, with **D8** (SS34) as flyback. Pole 3 is a spare ground.

> **Keep the fan under about 200 mA** — a 40 or 60 mm fan, not an 80 mm. That is the polyfuse, which
> derates with ambient like any other.
>
> And the obvious caveat: venting an outdoor enclosure invites water and insects. A metal backplate or a
> bigger box is the cheaper first move; the fan is the fallback.

Total board dissipation at a sustained full 30 A is **~8–10 W** (polyfuses 1.5 W, FETs 1.6 W, `/GNDIN`
copper 1.5 W, other copper 0.9 W, main fuse ~2–3 W, regulator 0.35 W). A typical sequence at 15–20 %
average is nearer 3 W.

## I²C temperature sensors (new)

Two **NXP LM75BD** (C34565), ±2 °C, SOIC-8:

| Sensor | Address | Placed at | Nearest heat source |
|---|---|---|---|
| U4 | 0x48 (A2/A1/A0 all low) | (68, 26) | top fuse row, 12.2 mm; FET bank, 30 mm; F0, 25.8 mm |
| U5 | 0x49 (A0 high) | (7, 24) | F1's pad, 9.5 mm |

> **Known limitation — these are not a "hot spot vs ambient" pair, whatever you might want them to be.**
> U4 ended up 26–30 mm from both the FET bank and the main fuse, and U5, nominally the cool corner, is
> actually *closer* to a dissipating polyfuse than U4 is to anything. On top of that, both sit on the
> same two solid copper planes spanning the whole board, which makes it close to isothermal laterally.
> Expect a few degrees between them at most, against **±2 °C per sensor** — so up to ±4 °C on the
> difference. Read them as two board-temperature measurements, which is genuinely useful against the
> MF-R600 derating table, and do not read much into the difference. A real hot-spot sensor would need
> U4 within ~5 mm of F0's pads, which is a v1.01 change.

The sensor that actually matters for control is **RT5**, the NTC, at (82, 30) — 11.3 mm from F0's pad
and the closest thing on the board to the power section.

**They run from VIO on the header, not from the board's 5 V rail.** Whatever you plug in — a 3.3 V ESP32
or a 5 V Pi — supplies VIO, so the sensors and the bus sit at the master's own logic level and **no level
shifter is ever needed**. With nothing plugged in they are simply unpowered, which costs nothing because
there is no master on the board to read them. The fan and the alarm LED do not depend on them at all.

J15 is marked `I2C  G VIO SDA SCL   VIO=IN`. Read-back confirms no +5 V reaches either sensor.

## Signal path (read back from the board, not from the generator)

| Port | RJ45 pins | U1 inputs | U1 output | series R | terminal |
|---|---|---|---|---|---|
| P1 | 1 (+), 2 (−) | 6 = 1A/P1P, 7 = 1B/P1N | 5 → RXD1 | RS1 | J2 pole 2 |
| P2 | 3 (+), 6 (−) | 10 = 3A/P2P, 9 = 3B/P2N | 11 → RXD2 | RS2 | J5 pole 2 |
| P3 | 5 (+), 4 (−) | 14 = 4A/P3P, 15 = 4B/P3N | 13 → RXD3 | RS3 | J8 pole 2 |
| P4 | 7 (+), 8 (−) | 2 = 1A/P4P, 1 = 1B/P4N | 3 → RXD4 | RS4 | J11 pole 2 |

The Falcon standard (port 3 = 5+/4−). Identical to difftx except port 3, which difftx rev D has reversed (see
"Port 3 polarity fix" above). U1 pin 4 (G) = +5 V, pin 12 (Ḡ) = GND — permanently enabled.

## Output terminals

Twelve KF301-5.0-3P at x = 15, 35, 55, 75, 95, 115, every one reading **+ / D / −** left to right.

| Row | left → right |
|---|---|
| top | J2 P1 DATA, J3 P1 INJ A, J4 P1 INJ B, J5 P2 DATA, J6 P2 INJ A, J7 P2 INJ B |
| bottom | J11 P4 DATA, J12 P4 INJ A, J13 P4 INJ B, J8 P3 DATA, J9 P3 INJ A, J10 P3 INJ B |

The top row is fitted rotated 180° so its wires leave the board edge; that reverses its *pole numbering*
in the schematic, which is why the top row's fused +12 V lands on pole 3 and the bottom row's on pole 1.
**Physically both rows read + / D / − left to right.**

Rev C moves the port label (`P1 DATA  F1`) off the outer band and onto the line above its own trip LED,
which freed the outer band for the pole marks and the zip-tie holes.

The eight **INJ** terminals carry fused +12 V and ground only; **the middle pole is not connected on the
board**, so an injection pigtail's data core never becomes an unterminated stub on the string.

## Strain relief (new)

Fourteen **3.0 mm unplated holes**, seven per row at x = 5, 25, 45, 65, 85, 105, 125, in the band between
the screws and the fuses. Each terminal uses the pair that flanks it: loop a tie down one and up the
other and it clamps the pigtail bundle onto the board instead of onto the screws.

They are *between* terminals, not beside each one, because the fused output tracks are 3 mm wide and run
at x = TX ∓ 5.1 — a hole at TX ± 8 ate into that copper. Between terminals the gap is 7 mm.

## Front end

- **RT1–RT4, 120 Ω** across each pair; **RB1–RB8, 1 kΩ fail-safe bias** giving **283 mV**, past the
  AM26C32's ±200 mV threshold, so an unplugged transmitter leaves the outputs LOW — the WS2811 idle
  state.
- **D3–D6, one PSM712 per pair.** Asymmetric **−7 V / +12 V**, which is exactly the RS-485/422
  common-mode window the AM26C32 is specified over. Per line: line-positive V<sub>WM</sub> 12.0 V,
  V<sub>BR</sub> 13.3 V, V<sub>C</sub> 19.0 V at 1 A; line-negative 7.0 / 7.5 / 11.0 V. 600 W at
  8/20 µs; IEC 61000-4-2 ±15 kV air. Cost is 75 pF per line, ~5 ns into a terminated pair — nothing
  against a 1.25 µs WS2811 bit.
- **470 Ω series (RS1–RS4)**, set by four cases: short to ground ≈ 11 mA out; pulled to +12 V
  **13.6 mA** in, against a ±25 mA absolute maximum; all four back-fed at once ≈ 55 mA into the 5 V rail,
  which **D7** (BZT52C6V2, 500 mW) sinks at ~0.34 W; and an edge of ~115 ns into a metre of pigtail
  against a 250 ns T0H.
- **Common mode.** Cat5 carries no ground. Run difftx and diffrx from the same supply, or add a ground
  wire.

## Power entry

J1, a KF950-9.5-2P, 32 A, 10–22 AWG. **Upper screw = +12 V, lower = supply negative**; 10 AWG is right
for 30 A. Its body runs to x = 129.69, 0.31 mm short of the edge, so wires leave the board rather than
crossing it. The right edge reads top to bottom `12V 30A` / `INPUT` / `+12V` / *[terminal]* / `GND`.

Path, confirmed by read-back: **J1.2 `/12VIN` (122.4, 48.25) → F0.3 `/12VIN` (106.74, 31.3) → F0.1
`+12V` (93.26, 31.3)**, with J1.1 `/GNDIN` at (122.4, 57.75). Nothing but J1 and F0 touches `/12VIN`.

Q1–Q4 are four AOD4184A in parallel in the **ground return**: about 1.75 mΩ total, 1.6 W at 30 A over
four TO-252s on a poured island. Their body diodes conduct in the normal direction, so the board powers
up before the gates charge. Gate from +12 V through R1 10 k, clamped by D1 and pulled down by R2 100 k.

**LED7 (REVERSED)** sits across the FET bank — not across the input. Normally the FETs are on and drop
~50 mV, so the LED is dark and never sees a reverse volt. With the supply backwards the FETs block, the
full 12 V stands across them, and this is the only thing on the board that does anything.

D2 (SMDJ15A) is across the **protected** rails, deliberately not the input: a unidirectional TVS ahead of
the FETs would be a dead short on a reversed supply. C1 is 470 µF of bulk, now with **R27, a 10 k
bleeder**, so it does not sit charged. U3 is a UA78M05 fed through R3 10 Ω, under 0.4 W.

## Test points

TP1 +12 V, TP2 GND, TP3 +5 V, TP4–TP7 DATA1–4, **TP8 on the NTC divider node** — that last one lets you
read the board temperature with a multimeter and the table above, with no I²C master at all.

## Assembly and ordering

- 4 layers, 130 × 100 mm, **1 oz on every layer** (not the 0.5 oz inner default), HASL or ENIG.
- **33 BOM lines, 126 placements.** `jlcpcb/diffrx-bom.csv` is grouped by LCSC part number, and every
  line has been checked against LCSC for value, package and availability.
- **Build quantity is limited by the MF-R600**: ~1,400 in stock and twelve per board, so about 115
  boards. Roughly half the lines are JLCPCB *Extended* parts, each carrying a feeder fee.
- **Everything is machine-placed.** Rev B's hand-fitted main fuse is gone. Only the four mounting holes
  and the fourteen zip-tie holes are excluded, and those are holes, not parts.
- 29 through-hole positions (13 terminals including the fan, 12 fuses, the RJ45, the input terminal,
  the I²C header and F0). JLCPCB wave-solders these at extra cost.
- Zone pad connection is **solid** on every power zone. The B.Cu ground fill is the one exception and
  uses **thermal relief** so bottom-side pads stay hand-solderable; it is a fill, not a current path.
- Drill tools in use: 0.3 mm (0.7 mm pad) and 0.4 mm (0.8 mm pad) vias, 0.914 mm (RJ45), 1.1 mm (I²C header), 1.2 mm (fuses), 1.4 mm
  (terminals), 1.8 mm (F0), 2.0 and 3.4 mm (input terminal), **3.0 mm × 14 (zip ties)**, 3.2 mm × 4 (M3).
- **Mounting washers: 6 mm OD maximum.** The end terminals are 3.48 mm from each hole centre; a standard
  DIN 125 M3 washer is 7 mm and fouls them. Use DIN 433 (6 mm) or none.

## Still only checkable on hardware or in JLCPCB's preview

- **JLCPCB's rotation convention** for the ICs, FETs, diodes and through-hole parts. The CPL is written
  from the board's own origin, the same origin as the gerbers and drill. Check the preview before paying.
- **The RJ45's pin-1 side.** difftx uses the same part, so a mirror error would be self-consistent — but
  check continuity from U1 pin 6 to the contact nearest the board's bottom edge on the first board.
- **The thermostat thresholds.** 43 °C and 65 °C are computed from the NTC's B value and 1 % resistors,
  not measured. Expect a couple of degrees either way; if the fan runs too eagerly, raise R31.
- **Edge rate at 470 Ω** on a long pigtail. 115 ns into a 250 ns T0H is fine on the bench; if a long
  injection-heavy line glitches, 220 Ω restores the rev-A edge at the cost of a 29 mA fault current,
  which would then need a bigger clamp than a BZT52.
- **Green activity LED brightness, worst case.** LED1–LED4 are KT-0805G with V_F up to 3.0 V, driven
  through 1 kΩ from an AM26C32 output guaranteed only to V_OH ≥ 3.8 V at −6 mA. Worst case that is
  **0.8 mA**, not the 1.5–3 mA quoted below; typically it is ~2.1 mA. The red and yellow LEDs have far
  more headroom. Drop RL1–RL4 to 470 Ω if they are too dim.
- **LED brightness.** The trip and indicator LEDs run at 1.5–3 mA. If they are too dim outdoors, drop
  RL1–RL5 and the RF resistors.
- **Whether 6 A PPTCs hold at your ambient.** Now you can answer this properly: read U4 and U5, or put a
  meter on TP8.
- **The TO-252 3D body** in the JLCPCB library is about 1 mm off its own pads, so Q1–Q4 use KiCad's
  `TO-252-2.step` offset 1.9 mm. Render-only — gerbers, drill and CPL never read the 3D model.

## Bring-up order

1. **Do not fit the ATO fuse yet.** Put 12 V on J1 (**+ is the upper screw**). Nothing should happen —
   that confirms F0 really is in series with the whole board.
2. Fit a 30 A ATO fuse. LED5 should light and 5 V appear on TP3.
3. Confirm the reverse-polarity block: reverse the supply briefly on a current-limited bench supply.
   Nothing should draw current, nothing should get warm, and **REVERSED** should light.
4. Check **+12 V at the left screw and ground at the right screw of every output terminal** (both rows
   read + / D / −), and that the drop across each fuse is a few tens of millivolts at a test load. All
   twelve trip LEDs should be dark.
5. Pull one output's load with a deliberate overload and watch that port's LED light. Let it cool.
6. Warm the board (a hairdryer on the FET corner) and check the fan output switches on near **44 °C**
   and back off near **41 °C**, and that **OVER TEMP** lights near **65 °C**. TP8 falls as it heats:
   **1.55 V ≈ 44 °C, 0.88 V ≈ 65 °C**. If TP8 reads a steady 5.0 V, the NTC is open, not cold.
   Check the fan actually spins — a reversed flyback diode would trip F13 instead, and that was a real
   bug in the first cut of this revision.
7. Patch a cat5 lead from difftx. With FPP idle, all four data outputs should sit LOW (fail-safe bias);
   with a sequence running, LED1–LED4 should light.
8. Only then connect pixels, one line at a time, checking polarity before each.
