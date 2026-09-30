# Rev E verification record — FPP Remote pHAT on the Pi Zero 2 W outline (rev D + port 3 polarity fix)

Rev D is rev C shrunk onto the Raspberry Pi Zero 2 W footprint: 65 x 30 mm, R3 corners, the four M2.5 holes on the
Pi's own 58 x 23 mm pattern, so the board stacks exactly over the Pi. The 66 x 66 mm rev C is archived in `archive/revC_66mm/`.

Generated files: `difftx.kicad_sch` / `difftx.kicad_pcb` (from `gen_sch.py` and `gen_pcb.py`), `jlcpcb/` (gerber
zip, BOM, CPL from `export_jlc.py`), `eeprom/difftx-eeprom.bin` (from `make_eeprom.py`), renders `render_*.png`.

## Rev E (2026-09-29): port 3 polarity

**Rev E is rev D with RJ45 port 3 set to the Falcon standard: pin 5 = P3+ (U2 2Y), pin 4 = P3− (U2 2Z).** Rev D had 4 (+) / 5 (−).
On 2026-09-29, a built rev D on a Pi 3B+ drove a PixelController SRx1 v5.01. Ports 1, 2 and 4 worked, but port 3 sparkled
white and ignored data, and a lead with pins 4 and 5 swapped at one end fixed it. (Source for the pinout: Dan Kulp, "blue (5, 4)
is for port 3", https://falconchristmas.com/forum/index.php?topic=15638.0. See also `../CLAUDE.md`.) Rev D is frozen in `archive/revD`.

- **Schematic:** the J1 pin 4 and 5 labels are swapped, and so are the RJ45 symbol's pin names (pin 4 "P3−", pin 5 "P3+"). The title
  and the pinout note say rev E. Edited in place so the UUIDs are kept; the file equals `gen_sch.py`'s output apart from UUIDs.
- **Board** (`patch_revE.py`, mirrored in `gen_pcb.py`): the two port 3 routes swap approaches, so the jack's top-row pads
  are still fed from above on B.Cu and the bottom-row pads from below on F.Cu. 2Z (pin 5) takes a short F.Cu hop to a via at
  (41.2, 12.37), then rev D's B.Cu route to pad 4. 2Y (pin 6) jogs down one pin pitch into the corridor pin 5 used to run along,
  then rev D's F.Cu route around the jack to pad 5. The pad pin functions and the silkscreen (`rev E`, `P3=5,4`) are updated.
  No parts moved.
- **EEPROM:** cape version **1.1** (rev D chips say 1.0). The description says rev E and the strings note gives the pinout;
  the id (`difftx`), the outputs and the default config are unchanged.
- **Checks:** ERC 0. DRC with `--schematic-parity --refill-zones` gives 0 violations, 0 unconnected and 0 parity issues. At
  `--severity-all` there are **no findings at all** (rev D, refilled the same way, had 1 `copper_edge_clearance`).
- **Regenerated:** `jlcpcb/`, `eeprom/difftx-eeprom.bin`, `difftx_sch.pdf`, `render_top.png`, `render_bottom.png`.
  `render_iso.png` and `render_front.png` are still rev D's (the jack copper they show is hidden under the jack anyway).
- **Compatibility:** rev E works with Falcon receivers, diffrx and diffsmart on a straight lead. Rev D boards still need the
  4/5-swapped lead.

## Automated checks (KiCad 10.0.6, 2026-09-19)

| Check | Result |
|---|---|
| `kicad-cli sch erc --severity-all` | 0 errors, 0 warnings |
| `kicad-cli pcb drc --schematic-parity --refill-zones --severity-all` | 0 violations, 0 unconnected, 0 parity issues |
| Read-back of the saved board (pad positions and nets of J3, J1, U2, holes) | matches the tables below |
| 3D renders top / bottom / iso / front | inspected |
| Independent second-agent review (2026-09-20) | no blocker; all should-fix items applied, see below |

## Geometry (board coordinates = Pi Zero coordinates, official Raspberry Pi Zero 2 W mechanical drawing)

- Outline 65 x 30 mm with 3 mm corner radius. Holes H1-H4 at (3.5, 3.5) (61.5, 3.5) (3.5, 26.5) (61.5, 26.5),
  each with a 1 mm copper keepout ring for the standoff.
- J3 socket on the BOTTOM, centre (32.5, 3.5) on the hole line. Pad 1 (Pi pin 1, 3V3) at (8.37, 4.77), the inner row
  at the SD-card end; pad 2 (5V) at (8.37, 2.23). Because flipping a footprint mirrors its numbering, J3 is a
  project footprint (`difftx:PiSocket_2x20_bottom`, a copy of JLC C5124634 with odd/even pad numbers swapped) so pad n
  lies on Pi pin n. Silk "pin 1" marks on both sides.
- Everything else is on top. RJ45 J1 sits on the bottom long edge at the right (x 41-57), opening facing out, 0.3 mm
  in from the edge. 12 V screw terminal J4 on the bottom long edge at the left, wires entering from the edge; "+" is
  the left terminal (pin 1, nearest the fuse), marked on the silk. U4 buck module in the centre, U2 driver right of it.

## Signal path (read back from the board, not from the generator)

| FPP port | Pi pin (DPI bit) | U2 input | U2 outputs | RJ45 pins |
|---|---|---|---|---|
| 1 | P1-29, GPIO5, DPI_D1 | 1A (pin 1) | 1Y pin 2 → P1+, 1Z pin 3 → P1- | 1 (+), 2 (−) |
| 2 | P1-31, GPIO6, DPI_D2 | 3A (pin 9) | 3Y pin 10 → P2+, 3Z pin 11 → P2- | 3 (+), 6 (−) |
| 3 | P1-26, GPIO7, DPI_D3 | 2A (pin 7) | 2Y pin 6 → P3+, 2Z pin 5 → P3- | 4 (+), 5 (−) |
| 4 | P1-7, GPIO4, DPI_D0 | 4A (pin 15) | 4Y pin 14 → P4+, 4Z pin 13 → P4- | 7 (+), 8 (−) |

This assignment differs from rev C (the drivers were re-paired to make the routing fit) and `make_eeprom.py`
was updated to match: the EEPROM strings file lists outputs in the order P1-29, P1-31, P1-26, P1-7. The RJ45 pair
assignment (1/2, 3/6, 4/5, 7/8 = ports 1-4) and Y = "+" are unchanged from the breadboard that drove the SRx1.

Driver enables G (pin 4) and G̅ (pin 12) are tied to GND (enabled). Pin 8 GND. VCC pin 16 = +5V with C1 100 nF.

## Parts and their checks (unchanged from rev C, see `archive/revC_66mm/VERIFICATION.md` for the sources)

AM26C31IDR (TI SLLS103P), AT24C256C (Microchip 8568F: A0-A2 = GND → 0x50, WP high = protected, R1 10 k pull-down,
JP1 to 3V3), K7805-2000R3 (8-36 V in, 5 V 2 A, pin 1 Vin 2 GND 3 Vout - and note LCSC C18212380 is a **YLPTEC**
clone, 10.2 mm tall and switching near 2.4 MHz, NOT the 17.5 mm 400 kHz Mornsun part the datasheet describes;
the pin-out and ratings match, the height and EMI behaviour do not), SS54 (cathode at footprint pad 2,
on the +12V side), 1812 polyfuse 1.5 A hold / 24 V, KF301 screw terminal, Ckmtw RJ45 (pin 1 nearest the edge-most
row, latch up), Samsung / Yageo capacitors rated 50 V (12 V rail) and 25 V (5 V rail). Pi header use: 1 and 17 3V3,
2 and 4 5V, 3 SDA1, 5 SCL1, 7/26/29/31 DPI data, 6/9/14/20/25/30/34/39 GND (each GND pin has an explicit back-layer
stub because the pour is kept out of the pin field).

## Independent reviews

Rev C (66 mm) was reviewed independently against datasheets and the FPP source; that review caught the socket on
the wrong side, fixed there and carried into rev D.

Rev D was reviewed independently on 2026-09-20 (netlist dump, pcbnew read-back of outline, holes, socket pads and
nets at absolute coordinates, zone-island and connectivity check, ERC/DRC re-run, gerber/drill/BOM/CPL contents,
byte-level parse of the EEPROM image, parts against TI SLLS103P, Atmel 8568F, the K78xx datasheet, JLC part
pages, the official Pi Zero 2 W drawing RP-008358 and FPP DPIPixels.cpp / CapeUtils.cpp). Verdict: no blocker; the
socket-side fix is confirmed (pad at Pi pin 1 = 3V3, pin 2 = 5V, every used pad on the right net). Its findings and
what was done:

- 5 V feed to the Pi was a 0.5 mm back-layer trunk through one via: now 0.8 mm with two vias (only the short
  branch to U2's VCC stays 0.5 mm).
- Schematic sheet note and gen_sch.py docstring still quoted the rev C port order: corrected to rev D.
- Ground pour over the Pi Zero 2 W Wi-Fi antenna (bottom edge, between mini-HDMI and the first micro-USB): both
  pours are now kept out of a 10 x 6.7 mm window (x 17-27, y 23-29.7), C3-C5 and the 12 V bus moved right, silk
  "ant" marks it. The antenna outline is not labelled on the drawing, so this window is an informed estimate;
  measure RSSI with the board fitted before a batch.
- Pin-1 square pad had moved to pin 2 with the renumbering: pad 1 is the square again.
- Ground vias touching pad ends (U2 pin 8, C6, U3): moved clear so paste cannot wick into them.
- P1+ track 0.28 mm from an RJ45 peg: moved to 0.58 mm.
- cape-info.json said rev C: now rev D. Gerbers, drill and CPL now share one origin (board bottom-left).

Still only checkable on hardware or in JLCPCB's preview: the jack's pin-1 side (standard 8P8C convention and the
JLC 3D model both put pin 1 at the largest x, but the manufacturer drawing could not be fetched; check continuity
from U2 pin 2 to the contact nearest the right edge on the first board), JLCPCB's rotation convention for D1, U2,
U3, J1 and the bottom-side J3, and the standoff at H1/H2, which has 0.3 mm to the socket body, so use round or
nylon standoffs there.

## Which Raspberry Pi (added 2026-09-20)

**Any 40-pin Raspberry Pi, not just a Zero 2 W.** The 40-pin header sits in the same place relative to the two
header-side mounting holes on every model. Verified against the official Pi 4 mechanical drawing: holes 3.5 mm in
from the edges and 58 mm apart, header centred between them at 32.5, rows therefore at 3.5 +/- 1.27 = 2.23 and
4.77 from the board edge. Those are this board's own numbers, so:

| | difftx | Raspberry Pi 4 / 3B+ / 3A+ |
|---|---|---|
| Socket centre | (32.5, 3.5) | header centred at 32.5, 3.5 from the edge |
| H1, H2 | (3.5, 3.5) and (61.5, 3.5) | the Pi's own two header-side holes |
| Header rows | 2.23 / 4.77 | 2.23 / 4.77 |

So the socket lands on the header and H1/H2 land on the Pi's own holes - two screws plus forty pins. difftx's
65 x 30 outline sits entirely inside an 85 x 56 board, and the tall connectors (Ethernet 13.5 mm, USB 16 mm) are
at x >= 74, outside its span. A full-size Pi also already has its male header soldered on, so you are not hunting
for the scarcer `WH` variant.

**A Zero 2 W remains the neatest fit** (identical outline, all four holes line up). A **3A+** is the closest
substitute - same BCM2837 family, so identical DPI behaviour. FPP's own guidance is to prefer a 3B+ or 4 over a
Pi 5 for string output, since the Pi 5 moved GPIO onto the RP1 southbridge and forced a rewrite of that path.

**An Orange Pi Zero 2W will not work.** Its header is pin-compatible for power, ground and I2C1, but pins
7/29/31/26 land on PI13, PI0, PI15 and PH9 - four unrelated GPIOs in two Allwinner port banks. On a Raspberry Pi
those pins are DPI_D0..D3, four consecutive bits of the parallel display bus, which is the whole mechanism FPP's
DPIPixels output uses. FPP officially supports Raspberry Pi and BeagleBone only.

## BOM Comments corrected (2026-09-20)

The Comment column of `difftx-bom.csv` is the footprint's Value, so it is what a reader sees beside the LCSC code.
Two of the twelve lines named the thing that plugs in rather than the thing being ordered:

| was | now | the part actually ordered |
|---|---|---|
| `Pi Zero 2 W` | `2x20 socket for Pi` | C5124634, a BOOMELE 2x20 **female header** |
| `12V IN` | `terminal 2P 5.0mm` | C474881, a KF301-5.0-2P screw terminal |

JLCPCB cannot fit a single-board computer to a PCBA order, and its parts-library items "cannot be shipped
separately", so the first line read as a request for hardware that is neither in nor orderable with the assembly.
The other ten Comments were checked line by line and are correct. **No copper changed**: the board is hand-routed,
and a geometry hash of every segment, via, pad, footprint position and Edge.Cuts item is identical before and
after (180 items, sha256 398db4f8864ae88267c758d4). Boards already ordered are unaffected.

One thing this board got right that its sibling did not: the K78xx datasheet asks for 22 uF at the input, and
difftx fits **three** 10 uF 50 V X7R 1206 (C89632) in parallel - 30 uF nominal, close to the module.

## Bring-up order

1. Assemble; do not mount on the Pi yet. 12 V on J4 (+ = left terminal), check 5 V on the socket's pins 2/4.
2. Mount on the Pi Zero 2 W (11 mm standoffs). Never power the Pi from USB while 12 V is connected.
3. Boot FPP; `i2cdetect -y 1` should show 50. Write `eeprom/difftx-eeprom.bin` to
   `/sys/bus/i2c/devices/1-0050/eeprom`, reboot, sign the cape in the FPP UI, then close JP1.
4. Channel outputs → four DPIPixels strings; test against the SRx1 (rotary 0).
