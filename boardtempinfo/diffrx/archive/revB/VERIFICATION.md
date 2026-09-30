# Chandler 4D/8P Differential Receiver v1.00 (diffrx rev B) — verification record

**diffrx** is the receiving half of the pair whose transmitter is `../difftx` (the FPP Remote pHAT).
One cat5 in, four WS2811 pixel data outputs, eight power-injection outputs, and a 12 V 30 A distribution
bus with a bolt-down main fuse and a resettable fuse per output — so the board *is* the marine fuse
panel, and the supply cable lands on it directly.

Generated files: `diffrx.kicad_sch` / `diffrx.kicad_pcb` (from `gen_sch.py` and `gen_pcb.py`),
`jlcpcb/` (gerber zip, BOM, CPL from `export_jlc.py`), renders `render_*.png`.
`fetch_parts.py` pulls the JLCPCB parts into `jlc/`; `verify.py` reads the finished board back.

## What changed from rev A

An adversarial review of rev A found four blockers and several smaller errors. Rev B is the answer:

| Rev A | Rev B |
|---|---|
| No main fuse — twelve 4 A polyfuses on a 30 A supply, so nothing protected the supply cable | **F0**, a bolt-down MIDI/AMI 30 A fuse, between the input terminal and the whole bus |
| Copper weight recorded only in this file | Stack-up written into `diffrx.kicad_pcb`, so it reaches the gerber job file too |
| SRV05-4 on the cat5 pairs — clamps to the 5 V rail, i.e. conducts at ~5.7 V of common mode | **PSM712**, −7 V / +12 V asymmetric, which is exactly the RS-485/422 common-mode window |
| 270 Ω series resistors, no rail clamp; a data line pulled to 12 V back-feeds the 5 V rail | 470 Ω plus **D7**, a 6.2 V clamp on the 5 V rail |
| MF-R400, 4 A hold | **MF-R600**, 6 A hold / 12 A trip, sized for the 18 AWG core of a Ray Wu pigtail |
| No keepout at the mounting holes — the +12 V plane ran up to the bare drill | Copper keepout at all four holes |
| Supply ground on one layer | `/GNDIN` and `/12VIN` each on **both** outer layers, stitched |
| RJ45 annular ring below JLCPCB's minimum; `min_connection` 0.0; sub-1 mm silk text | DRC rules tightened to JLCPCB's published 1 oz multilayer limits; board passes against them |
| BOM rows duplicated by value | BOM grouped by LCSC part number — 23 lines |

The board grew from 110 × 100 mm to **130 × 100 mm** to fit the main fuse and the larger MF-R600 bodies.

## Automated checks (KiCad 10.0.6, 2026-09-20)

| Check | Result |
|---|---|
| `kicad-cli sch erc --severity-error --severity-warning` | 0 violations |
| `kicad-cli pcb drc --schematic-parity` against the tightened rules | 0 violations, 0 unconnected, 0 schematic parity issues |
| router | 0 routing failures |
| `verify.py` read-back of the *saved* board (stack-up, signal path, terminal poles, ESD arrays, power entry, zone fills) | matches the tables below; ratsnest 0 |
| silk-against-silk overlap sweep (`scratchpad/silkcheck.py`) | 0 overlapping pairs |
| 3D renders top / bottom | inspected |

The silk sweep is separate because **KiCad's DRC does not check silkscreen against silkscreen**. Two
labels were piled on top of each other in the first rev-B build and passed DRC clean; they are fixed.

## Board

130 × 100 mm, 3 mm corner radius. Four M3 holes, 3.2 mm drill, at (4, 4) (126, 4) (4, 96) (126, 96),
each with a copper keepout so a steel screw cannot reach the +12 V plane. The board is named
**Chandler 4D/8P Differential Receiver v1.00** on the front silkscreen.

**Four layers, 1 oz copper on all four.** This is not JLCPCB's 4-layer default, which is 1 oz outer and
**0.5 oz inner** — and the inner layers are the ones carrying the 30 A. The stack-up is now written into
the board file and into `diffrx-job.gbrjob`, so it travels with the gerbers; confirm it in the order form
anyway.

| Layer | Filled | Role |
|---|---|---|
| F.Cu | — | components, signal routing, and the power islands below |
| In1.Cu | 11819 mm² | solid GND plane |
| In2.Cu | 11838 mm² | solid +12 V plane — this is what feeds the twelve output fuses |
| B.Cu | 10215 mm² | GND fill (thermal relief) plus signal routing |

F.Cu power islands: `12V_top` 824 mm² and `12V_bottom` 1001 mm² under the two fuse rows, in parallel
with the In2 plane; `GND_sources` 252 mm² at the FET sources, stitched to In1 with 51 vias.

Two islands have no inner plane behind them, because they are on the *supply* side of the protection and
every plane is on the protected side. Both are therefore carried on **both outer layers and stitched**:

| Island | F.Cu | B.Cu | stitching vias | carries |
|---|---|---|---|---|
| `/12VIN` | 525 mm² | 525 mm² | 12 | J1 positive → F0 stud |
| `/GNDIN` | 660 mm² | 660 mm² | 16 | J1 negative → the four FET drains |

## Can this really carry 30 A? — the arithmetic

IPC-2221, 1 oz = 35 µm = 1.37 mil, external *k* = 0.048, internal *k* = 0.024:

| Conductor | Geometry | Current for a 10 °C rise | Rise at 30 A |
|---|---|---|---|
| In2 +12 V plane | internal, ~128 mm wide | 40 A | under 10 °C |
| `/12VIN` islands | two external layers, 21 mm wide | ~43 A combined | under 10 °C |
| `/GNDIN` islands | two external layers, 10 mm wide | ~25 A combined | **~15 °C** |

`/GNDIN` is the tightest conductor on the board, and it is the reason it is on two layers instead of one
— on a single layer 30 A would be a 45 °C rise. IPC-2221 assumes an isolated trace in still air; here
both islands sit 0.21 mm above a solid ground plane that spreads the heat, so the real figures are lower.

The In2 plane never actually sees 30 A over any distance: current enters at F0's stud on the right and
leaves at twelve fuse pads spread across the board, so it fans out within the first few millimetres.

## Power budget — where the 30 A goes

A 12 V WS2811 bullet draws about 60 mA (0.72 W) at full white. 800 nodes on one line is 48 A; four such
lines is 192 A, which is six times the supply. **12 V × 30 A = 360 W is the limit**: about 500 nodes at
full white, or roughly 3200 nodes at the 15–20 % average a real sequence runs at.

That 15 % figure on a full 800-node line is 7.2 A — more than one MF-R600 will hold. It does not have
to: each data port has **three** fused feeds (the data terminal plus two injection taps), so 7.2 A
arrives as roughly 2.4 A per fuse, and the port's total fused capacity is 18 A.

Twelve fuses × 6 A = 72 A of nominal capacity on a 30 A supply, so the polyfuses protect the *pigtails*,
not the supply. **F0 is what protects the supply**, and it is why rev A's "keep a real fuse at the power
supply" caveat is gone.

## Fusing

### F0 — main fuse

Bolt-down MIDI/AMI, 30 mm stud spacing, M5, in a hand-built footprint
(`diffrx.pretty/Fuse_MIDI_AMI_M5_30mm.kicad_mod`): two 5.3 mm plated holes with 11 mm pads. It sits
between J1's positive screw and the +12 V bus, so **everything** on the board is behind it.

> F0 is fitted **by hand** after assembly. JLCPCB neither stocks nor places a bolt-down fuse, so the
> footprint carries `exclude_from_pos_files exclude_from_bom` and the part appears in neither the BOM nor
> the CPL. Buy a 30 A MIDI/AMI fuse and two M5 bolts separately. Copper may run under the body; nothing
> else may sit in its courtyard.

### F1–F12 — output fuses

Bourns **MF-R600** PPTC, LCSC C208490. From the MF-R series datasheet (Bourns 05094/R110 7180T):

| I<sub>hold</sub> | I<sub>trip</sub> | V<sub>max</sub> | I<sub>max</sub> | R<sub>min</sub> | R<sub>1max</sub> | 1 h post-trip max | max time to trip | tripped P |
|---|---|---|---|---|---|---|---|---|
| 6.00 A | 12.00 A | 30 V | 40 A | 0.005 Ω | 0.020 Ω | 0.040 Ω | 16.0 s at 30 A | 3.50 W |

Sized for the **18 AWG** core of a three-core Ray Wu pigtail. Three things about this choice, all
deliberate:

- **Hold current derates hard with ambient.** From the datasheet's thermal derating table
  (I<sub>hold</sub> / I<sub>trip</sub>): 6.00 / 12.00 A at 23 °C, 4.98 / 9.96 at 40 °C, 4.62 / 9.24 at
  50 °C, **4.08 / 8.16 at 60 °C**, 3.66 / 7.32 at 70 °C. In a sealed box in the sun, design to the 60 °C
  column.
- **Resistance is a range, not a number, and it grows.** Initial resistance is between R<sub>min</sub>
  0.005 Ω and R<sub>1max</sub> 0.020 Ω; an hour after a trip it can be **0.040 Ω**, which is 0.24 V at
  6 A and shows up as dimming at the far end of that line. (Rev A's "about 10 mΩ cold" quoted a minimum
  as if it were a typical — it is not.)
- **PPTCs are slow.** 16 seconds to trip at 30 A. They protect the wiring, not the pixels.

Mechanically the MF-R600 is 19.3 mm wide (A max) and 31.9 mm tall (B max), on 10.2 mm leads of 0.81 mm
diameter, standing on edge. Terminal columns are 20 mm apart, so **two neighbouring fuses at maximum
body width leave about 0.7 mm between them.** They fit; they are not loose. The footprint
(`FUSE-TH_L19.1-W3.0-P10.20-D1.2-S2.0`) draws the body 0.2 mm narrower than that worst case, which is
why the courtyard check passes.

A 5.08 mm radial PPTC will not drop into this 10.2 mm footprint — going back to a smaller fuse means
changing the footprint as well.

## Signal path (read back from the board, not from the generator)

| Port | RJ45 pins | U1 inputs | U1 output | series R | terminal |
|---|---|---|---|---|---|
| P1 | 1 (+), 2 (−) | 6 = 1A/P1P, 7 = 1B/P1N | 5 → RXD1 | RS1 | J2 pole 2 |
| P2 | 3 (+), 6 (−) | 10 = 3A/P2P, 9 = 3B/P2N | 11 → RXD2 | RS2 | J5 pole 2 |
| P3 | 4 (+), 5 (−) | 14 = 4A/P3P, 15 = 4B/P3N | 13 → RXD3 | RS3 | J8 pole 2 |
| P4 | 7 (+), 8 (−) | 2 = 1A/P4P, 1 = 1B/P4N | 3 → RXD4 | RS4 | J11 pole 2 |

The RJ45 pair assignment and "+ = Y" are identical to difftx, so the two boards plug together with a
straight cat5 patch lead and the path is non-inverting end to end. Which receiver channel handles which
pair is a layout choice only.

U1 pin 4 (G) = +5 V and pin 12 (Ḡ) = GND, i.e. permanently enabled. Pin 8 GND, pin 16 VCC, C6 100 nF
against the VCC pin.

## Output terminals

Twelve KF301-5.0-3P at x = 15, 35, 55, 75, 95, 115, every one reading **+ / D / −** left to right, with
the pole marks printed under the screws and the port name and its fuse (`P1 DATA  F1`) on the line above.
The fuse rows sit 3 mm further inboard than rev A to make that band 6 mm wide; at 1.2 / 1.3 mm the text
is readable with the board in a box, which it was not at 3 mm.

| Row | left → right |
|---|---|
| top (wire entry at the top edge) | J2 P1 DATA, J3 P1 INJ A, J4 P1 INJ B, J5 P2 DATA, J6 P2 INJ A, J7 P2 INJ B |
| bottom (wire entry at the bottom edge) | J11 P4 DATA, J12 P4 INJ A, J13 P4 INJ B, J8 P3 DATA, J9 P3 INJ A, J10 P3 INJ B |

The top row is fitted rotated 180° so its wires leave the board edge; that reverses its *pole numbering*,
which is why the schematic gives the top row pole 3 = fused +12 V and pole 1 = ground, and the bottom row
the other way round. **Physically both rows read + / D / − left to right** — the read-back confirms it,
and that is the only thing you need at bring-up.

The eight **INJ** terminals carry fused +12 V and ground only. **The middle (data) pole is not connected
to anything on the board** — it is there so the injection pigtails can be the same three-core Ray Wu lead
as everything else, which is printed on the silkscreen.

> If an injection pigtail's data core *is* connected at the light end, it becomes an unterminated stub
> hanging off that string's data line. Leave that core cut or unconnected at the light connector, or
> accept the stub.

## Front end

- **RT1–RT4, 120 Ω across each pair.** Cat5 is 100 Ω, but 120 Ω keeps the fail-safe bias above the
  receiver's threshold while dropping driver current, and a 20 % mismatch reflects 9 %.
- **RB1–RB8, 1 kΩ fail-safe bias**, pulling each A low and each B high. With the 120 Ω terminator that
  is 5 × 120 / 2120 = **283 mV**, comfortably past the AM26C32's ±200 mV threshold (TI SLLS104M), so an
  unplugged or unpowered transmitter leaves the receiver output LOW — the WS2811 idle state — instead of
  chattering and painting random colours.
- **D3–D6, one PSM712 per pair.** Pin 1 = I/O 1, pin 2 = I/O 2, pin 3 = GND; the read-back confirms
  `1 = P*P, 2 = P*N, 3 = GND` on all four. The two I/O pins are identical, so which conductor lands on
  which does not matter.

  This part replaced the SRV05-4 for one reason: it is **asymmetric, −7 V / +12 V**, which is precisely
  the RS-485/422 common-mode window the AM26C32 is specified over. Per line (ProTek 05094.R13):

  | direction | V<sub>WM</sub> | V<sub>BR</sub> @ 1 mA | V<sub>C</sub> @ 1 A | I<sub>D</sub> @ V<sub>WM</sub> |
  |---|---|---|---|---|
  | line positive (pin 1→3, 2→3) | 12.0 V | 13.3 V | 19.0 V | 1 µA |
  | line negative (pin 3→1, 3→2) | 7.0 V | 7.5 V | 11.0 V | 20 µA |

  600 W peak pulse per line at 8/20 µs; IEC 61000-4-2 ±15 kV air / ±8 kV contact, 61000-4-4 40 A,
  61000-4-5 24 A. An SRV05-4 clamps to the 5 V rail and would have started conducting at about 5.7 V of
  common mode, throwing away most of the ±7 V the receiver is good for.

  The cost is capacitance: **75 pF typical per line** against the SRV05-4's few pF. Into a 120 Ω
  terminated pair that is a ~5 ns time constant, which is nothing next to a 1.25 µs WS2811 bit.
- **No output buffer, 470 Ω series (RS1–RS4).** The four cases that set this value:
  - output shorted to ground: about 11 mA out of the pin;
  - output pulled to +12 V by a miswired pigtail: (12 − 5.6) / 470 = **13.6 mA** into the pin, against
    the AM26C32's ±25 mA absolute maximum (TI SLLS104M). At rev A's 270 Ω this was 23.7 mA — inside the
    limit on paper, with nothing left over;
  - all four outputs back-fed at once: about 55 mA into the 5 V rail, which **D7** (BZT52C6V2, 6.2 V,
    500 mW) sinks at roughly 0.34 W. That clamp is what makes an unbuffered output safe here;
  - edge rate: 470 Ω into the ~110 pF of a metre of pigtail is a 52 ns time constant, so about 115 ns
    10–90 %, against a WS2811 T0H of 250 ns. Acceptable, not generous — see the open questions below.

  LED1–LED4 hang off the same nodes through 1 kΩ and draw about 1.5 mA each.
- **Common mode.** The cat5 carries four data pairs and no ground, so the only thing holding the two
  boards' grounds together is the power system. Run difftx and diffrx from the same 12 V supply, or add
  a ground wire between them.

## Power entry

J1 is a KF950-9.5-2P, 32 A, 10–22 AWG. **Upper screw = +12 V, lower = supply negative**; 10 AWG is the
right wire for 30 A. Its body runs to x = 129.69, i.e. 0.31 mm short of the board edge, so the wires
leave the board rather than crossing it, and there is no room beside the screws for pole marks — the
main fuse's courtyard is 1.32 mm away on the other side. So the right edge of the board reads, top to
bottom,
`12V 30A` / `INPUT` / `+12V` / *[terminal]* / `GND`, each word on the side of the screw it belongs to.

The path is **J1 positive → `/12VIN` islands → F0 → +12 V bus**, confirmed by read-back:

| pad | net | position |
|---|---|---|
| J1.2 | `/12VIN` | (122.4, 48.25) |
| F0.1 | `/12VIN` | (110.0, 33.0) |
| F0.2 | `+12V` | (80.0, 33.0) |
| J1.1 | `/GNDIN` | (122.4, 57.75) |

Nothing but J1 and F0 touches `/12VIN` — that is the point; every other tap on the board sits behind the
main fuse.

Q1–Q4 are four AOD4184A in parallel in the **ground return** as a reverse-polarity block: about 1.75 mΩ
total (7 mΩ each at V<sub>gs</sub> = 10 V), so 1.6 W at 30 A spread over four TO-252s on a poured island.
Their body diodes conduct in the normal direction, so the board powers up before the gates charge. Gate
from +12 V through R1 10 k, clamped by D1 (BZT52C15) and pulled down by R2 100 k.

D2 (SMDJ15A, 3 kW) sits across the **protected** rails, deliberately not across the input: a
unidirectional TVS ahead of the FETs would be a dead short on a reversed supply. C1 is 470 µF 25 V of
bulk on the 12 V bus.

U3 is a UA78M05 fed from +12 V through R3 10 Ω, so a clamped surge cannot slam its input. Load is about
50 mA — receiver 15 mA, four pairs of bias 11 mA, five LEDs — so under 0.4 W in a SOT-223.

## Assembly and ordering

- 4 layers, 130 × 100 mm, **1 oz on every layer** (not the 0.5 oz inner default), HASL or ENIG.
- **23 BOM lines, 74 placements** — `jlcpcb/diffrx-bom.csv` is grouped by LCSC part number, so the twelve
  terminals are one line and the twelve fuses are another. Everything has an LCSC number.
- **F0 and the four mounting holes are in neither the BOM nor the CPL** by design. F0 is hand-fitted.
- 27 through-hole positions (12 terminals, 12 fuses, the RJ45, the input terminal, and F0). JLCPCB will
  fit them at a higher price than SMT-only; otherwise hand-solder them — the ground pads land on a solid
  inner plane, so use a hot iron.
- Zone pad connection is **solid** on every power zone (the two planes, both fuse-row islands, both
  `/12VIN` islands, both `/GNDIN` islands, `GND_sources`). The B.Cu ground fill is the one exception and
  uses **thermal relief**, so bottom-side pads stay hand-solderable; it is a fill, not a current path —
  ground reaches those pads through the In1 plane.
- Track widths in use: 0.2–0.4 mm signal, 2.4–3.0 mm power. Vias 0.3/0.6 mm signal, 0.4/0.8 mm stitching.
- **Mounting washers: 6 mm OD maximum.** The nearest part body (the end terminals J2, J7, J11, J10) is
  3.48 mm from each hole centre, so anything wider than a 6.96 mm washer fouls it. A standard DIN 125 M3
  washer is 7 mm — use DIN 433 (6 mm) or no washer.

## Still only checkable on hardware or in JLCPCB's preview

- **JLCPCB's rotation convention** for U1, U3, Q1–Q4, D2 and the through-hole parts. The CPL is written
  from the board's own origin (bottom-left corner), the same origin as the gerbers and drill file. Check
  the preview before paying.
- **The RJ45's pin-1 side.** Standard 8P8C convention and the JLC 3D model both put pin 1 at the largest
  x, but the manufacturer drawing could not be fetched. difftx uses the same part, so a mirror error
  would be self-consistent and the boards would still talk to each other — but check continuity from U1
  pin 6 to the contact nearest the board's bottom edge on the first board.
- **Edge rate at 470 Ω** on a long pigtail. 115 ns into a 250 ns T0H is fine on the bench and should be
  fine in the field; if a long injection-heavy line shows data glitches, 220 Ω restores the rev-A edge at
  the cost of a 29 mA fault current, which then needs the clamp to be a bigger part than a BZT52.
- **LED brightness at 1.5 mA.** If they are too dim outdoors, drop RL1–RL5 to 470 Ω.
- **The TO-252 3D body** in the JLCPCB library is about 1 mm off its own pads, so Q1–Q4 use KiCad's
  `Package_TO_SOT_SMD.3dshapes/TO-252-2.step` offset 1.9 mm. That is a render-only change — gerbers,
  drill and the CPL never read the 3D model.
- **Whether 6 A PPTCs hold at your ambient.** Measure the terminal-to-terminal drop under load on a hot
  day and compare against the 60 °C column above.

## Bring-up order

1. **Do not fit F0 yet.** With nothing else connected, put 12 V on J1 (**+ is the upper screw**). Nothing
   should happen — that confirms F0 really is in series with the whole board.
2. Fit F0. LED5 should light and 5 V should appear on C4.
3. Confirm the reverse-polarity block: reverse the supply briefly with a current-limited bench supply.
   Nothing should draw current and nothing should get warm.
4. Check **+12 V at the left screw and ground at the right screw of every output terminal** (both rows
   read + / D / −), and that the drop across each fuse is a few tens of millivolts at a test load.
5. Patch a cat5 lead from difftx. With FPP idle, all four data outputs should sit LOW (fail-safe bias);
   with a sequence running, LED1–LED4 should light.
6. Only then connect pixels, one line at a time, checking polarity before each.
