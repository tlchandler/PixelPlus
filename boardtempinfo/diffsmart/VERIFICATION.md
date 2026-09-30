# Chandler 4D/8P Smart Receiver v1.00 (diffsmart) — verification record

**diffsmart** is the dual-mode member of the family. It is the whole of `../diffrx/archive/revC_release` — one
cat5 in, four WS2811 data outputs, eight power-injection outputs, a 12 V 30 A fused distribution bus,
an NTC fan thermostat and two I²C temperature sensors — **plus the brains of `../difftx`**: a 40-pin
Raspberry Pi header, a 2 A supply for the Pi and an EEPROM on its I²C bus.

The Pi connects by **40-way ribbon** and lives in the enclosure, not on the board, so **any 40-pin
Raspberry Pi works** — Zero 2 W, 3A+, 3B+, 4 — the same set `../difftx` accepts.

One slide switch decides which of the two drives the four pixel outputs:

| Switch | `/MODE` | AM26C32 `G` | 74HCT125 `~OE` | The board is |
|---|---|---|---|---|
| **RX** | HIGH | enabled | Hi-Z | a differential receiver — the cat5 link drives the outputs |
| **PI** | LOW | Hi-Z | enabled | a standalone smart controller — the Pi on J17 drives them |

Generated from `gen_sch.py` and `gen_pcb.py` exactly as the other two boards; `verify.py` reads the
finished board back, `silkcheck.py` sweeps the silkscreen, `export_jlc.py` writes `jlcpcb/`.

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

## Automated checks (KiCad 10.0.6, 2026-09-20)

| Check | Result |
|---|---|
| `kicad-cli sch erc` | **0 violations** |
| `kicad-cli pcb drc --schematic-parity` | **0 unconnected, 0 schematic parity issues** |
| router | **0 routing failures** |
| `verify.py` read-back | mode logic, both data sources, Pi interface and supply widths all as designed |
| `silkcheck.py` (silk vs silk / pads / edge) | **0 / 0 / 0** |
| 3D renders | inspected |

Remaining DRC output is 83 `lib_footprint_mismatch` (expected — this board strips footprint silkscreen
outlines, see `gen_pcb.py`) and 7 `track_dangling`, discussed under *Known issues*.

`verify.py` also now reports the Pi's real geometry, solved out of the socket's pad positions rather
than taken from the generator's intent — see *The Pi*. That check exists because the first build of this
board had the socket mirrored and every other check passed.

## Why one switch is enough

This was the piece of luck that kept the design cheap. TI's AM26C32 function table (SLLS104M Table 7-1)
puts the outputs in high-impedance **only when `G` = L AND `~G` = H**. Tie `~G` permanently high and the
enable collapses to `G` alone — and the 74HCT125's `~OE` wants the *same* polarity:

```
/MODE = H  ->  G = H : receiver drives      ~OE = H : buffer in Hi-Z
/MODE = L  ->  G = L : receiver in Hi-Z     ~OE = L : buffer drives
```

So a plain SPDT slide switch does it, with no inverter and no second pole. Each source keeps **its own
470 Ω** into the shared node (RS1–4 for the receiver, RS5–8 for the Pi), so even if both were somehow
enabled the contention is about 10 mA rather than a short.

`R45` (10 k to +5 V) defaults the board to **receiver** mode if the switch is ever open mid-travel.

Read back from the board: `SW1.2`, `U1.4` and all four of `U9`'s `~OE` pins are on `/MODE`; `U1.12` is
on `+5V`.

## The Pi

**The Pi is not on this board.** J17 is a **2×20 male header** and a 40-way ribbon runs to a Pi mounted
in the enclosure. That is what makes this board work with **every model difftx supports** — Zero 2 W,
3A+, 3B+, 4 — rather than only the one that is hard to buy.

The earlier design had a Zero 2 WH plugged upside down into a socket here. It could never have grown
past that one model:

| Pi | Size | Footprint on the board, measured from the socket |
|---|---|---|
| Zero 2 W | 65 × 30 | x 129.3–159.3, y 65–130 — fits exactly, and only exactly |
| 3A+ | 65 × 56 | x 129.3–**185.3** — 25 mm off the east edge |
| 3B+ / 4 | 85 × 56 | that, **and** y 65–**150**, 20 mm off the south edge |

There is no 65 × 56 clear area on a 160 × 130 board carrying twelve screw terminals, let alone 85 × 56.
A bigger Pi would hang unsupported off two edges with its mounting holes in mid-air, over the bottom
terminal row.

**The numbering un-swapped, and that is the safety-critical part.** The socket's odd/even pads were
deliberately mirrored, because a Pi presented upside down shows the board its solder side. A ribbon is a
straight 1:1 pin map, so that same mirror would now deliver pin 1 to pin 2 — **5 V into the Pi's 3.3 V
rail**, which is precisely the fault the swap was introduced to fix, reappearing from the other
direction. `verify.py` asserts the handedness with the sign flipped, and then checks all eighteen
connected pins against the published Raspberry Pi header:

```
  1->2 (2.54, 0.0) (2.54 across)   1->3 (0.0, 2.54) (2.54 along)   handedness +6.45
  OK - straight through, so a standard ribbon lands pin n on pin n
  22 pins left open, 18 connected
```

**What goes with the Pi:** the notch in the east edge, both M2.5 standoff holes, the Pi outline on the
silk, and the whole class of mechanical risk that came with a computer hanging 11 mm over the copper —
antenna clearance, microSD ejection, the buck module's height, what is underneath it. The outline is a
plain 160 × 130 rectangle again, so the fab no longer routs an internal slot. It also routes more
easily: un-mirrored puts every signal pin on the **outer** column with open board beside it, which is
the arrangement that never had a dead end.

**The one thing a ribbon can get wrong is orientation.** A plain 2×20 male header has no key, so an IDC
socket will seat either way round, and reversed it hands 5 V to GPIO2. The board carries a filled
triangle and a `1` drawn from pad 1's own position, a box around the field, and `PIN 1 AT TOP /
REVERSED PUTS 5V ON GPIO2` beside it. **The same forty holes take a shrouded DC3-40P box header**, which
is keyed — the silk box is its shroud outline, so there is clearance if you would rather have the key.

**The GPIO map is unchanged**, so an existing FPP configuration needs no edit:

| Pi pin | GPIO | net | buffer | output |
|---|---|---|---|---|
| 7 | 4 | `/DPI_D0` | U9 4A → 4Y | DATA4 |
| 29 | 5 | `/DPI_D1` | U9 1A → 1Y | DATA1 |
| 31 | 6 | `/DPI_D2` | U9 2A → 2Y | DATA2 |
| 26 | 7 | `/DPI_D3` | U9 3A → 3Y | DATA3 |

These four are **DPI_D0–D3**, four consecutive bits of the Broadcom parallel display bus, which is what
FPP's DPIPixels output clocks a framebuffer out of. That is why it is these pins and not any others, and
it is also why an Orange Pi Zero 2W cannot substitute: its header is pin-compatible, but those four pins
land on PI13, PH9, PI0 and PI15 — four unrelated GPIOs in two different Allwinner port banks.

**R41–R44 are 2 k, not 10 k.** They hold all four DPI lines low, so an unpopulated header leaves the
outputs at the WS2811 idle state. The case that actually sets the value is a *populated, booting* Pi:
GPIO4–7 are four of the nine BCM pins that come out of reset with the **internal pull-up enabled**, and
it stays enabled for the whole boot and again through shutdown. Against 10 k that divider sits at 0.55 V
typical and **0.77 V** if the Pi's pull-up is at the low end of the published spread — against a
74HCT125 V_IL of **0.8 V flat over the whole temperature range**. Thirty millivolts, with all four
inputs parked near threshold drawing crowbar current. 2 k gives 0.19 V worst case.

**U9 is a 74HCT125**, not an HC part: HCT inputs read 3.3 V logic reliably from a 5 V rail, and the
outputs swing to 5 V for the pixels.

## Power

**Two 5 V rails, deliberately.**

- **U7, a K7805-2000R3** (12 V → 5 V, 2 A) feeds *only* the Pi, and only through **JP1**. A Zero 2 W
  peaks near 1.4 A at 5 V, which the existing UA78M05 in a SOT-223 cannot do.
- **JP1 is the 5 V link, and it exists because 2 A is not enough for every Pi.** Raspberry Pi ask for
  2.5 A on a 3A+/3B+ and 3 A on a Pi 4. Fit the shunt and the board powers the Pi off the 12 V bus as
  before — a Zero 2 W or a 3A+. Pull it for anything larger and the Pi runs from its own supply, with
  the two 5 V rails never meeting and nothing to back-feed. Ground and all four data lines are
  unaffected either way, and the Pi's own 3V3 still arrives on pin 1 to run the sensors, so *only* the
  supply is broken. **Never fit the link and plug a supply into the Pi at the same time.**
- **F14, a 1.5 A PPTC, is on the 12 V side of U7, not the 5 V side.** At the Pi's peak that node carries
  only 0.67 A, so F14 is not sized for the Pi — it is sized to open on a shorted module, which is the
  fault that would otherwise sit straight across the 30 A bus. (A 1.5 A PPTC on the *5 V* side would
  have been a nuisance-trip candidate: ~1.0 A hold at 60 °C against a 1.46 A peak.)
- **Two 10 µF/50 V input capacitors, C13 and C14, both beside U7 pin 1.** The K78xx-2000R3 datasheet
  calls for 22 µF at the input and says both capacitors must be as close as possible to the module's
  terminals; one 10 µF 7.6 mm away, behind the PPTC, was a third of that. 20 µF nominal is still 2 µF
  short of the figure — recorded here rather than papered over — but it is the same dielectric under the
  same DC bias, with an order of magnitude less loop area.
- **The linear UA78M05 stays** for the board's own logic. The RS-422 receiver resolves a ±200 mV
  differential against a threshold referenced to that rail, and it is not worth putting buck ripple
  underneath it for the sake of one part.

The two rails share ground and are never tied together.

**Track widths matter here and DRC caught it.** The router's default 0.25 mm of 1 oz copper carries
0.88 A at a 10 °C rise by IPC-2221 — not enough for a 1.2 A peak. The Pi supply nets are routed wide:

| net | width | IPC-2221 at 10 °C rise |
|---|---|---|
| `/PI5V` | 0.5 mm | 1.44 A |
| `/PI12` | 0.4 mm | 1.23 A |
| `/P3V3` | 0.3 mm | 0.98 A (load is a few mA) |

## The Pi can read the sensors

The Pi's own I²C (pins 3 and 5) joins the board's `/SDA` and `/SCL`, so **a fitted Pi reads both LM75
temperature sensors natively** — no external master, no extra wiring. The HAT EEPROM (U8, AT24C256C)
sits on the same bus at **0x50**, clear of the sensors at **0x48** and **0x49**.

**D9 is gone and `/VIO` is now just `/P3V3`.** D9 was a Schottky from the Pi's 3V3 to the sensor rail,
there to stop the J15 header back-feeding the Pi. It could not do that: the Pi carries **physical 1.8 kΩ
pull-ups from GPIO2 and GPIO3 to its own 3V3 rail**, and `/SDA` and `/SCL` go straight to those pins —
about 3.25 kΩ in parallel with the diode, which is the classic partial-power-through-a-GPIO condition.
Meanwhile it cost real headroom: at −20 °C, with the Pi's 3V3 3 % low, its forward drop put the sensor
rail at **2.78 V against the LM75B's 2.80 V floor**. On an outdoor Christmas-light controller that is an
ordinary December night, and the LM75B is rated to −55 °C, so it would be alive and out of spec rather
than simply off.

> **J15's supply pin is 3.3 V now, and the silkscreen says `3.3V ONLY - NOT 5V`.** On the receiver-only
> board this pin took 2.8–5.5 V and the master set the bus level. That is not available here, because
> SDA and SCL are hard-wired to GPIO2 and GPIO3, which are **not 5 V tolerant** — the old legend, with a
> Pi fitted, invited someone to destroy two GPIOs. This is a deliberate loss of a rev C feature.

**U8 is a user EEPROM, not a HAT ID EEPROM.** It sits at 0x50 on **GPIO2/GPIO3 (pins 3 and 5, i2c-1)**.
The Pi's firmware reads the HAT ID EEPROM **only** on ID_SD/ID_SC, pins 27 and 28, which this socket
leaves unconnected. So there is no device-tree overlay, no automatic GPIO configuration and no HAT
detection of any kind — read it with `i2cdump -y 1 0x50`. WP is tied low so it can be flashed in place
with difftx's `make_eeprom.py`. It runs from the Pi's own 3V3, so with no Pi it is absent from the bus.
(difftx carries the same mislabelling and has not been changed.)

## What moved, and what did not

The board grew from 130 × 100 to **160 × 130 mm**. All four edges of the receiver were already
committed — terminals top and bottom, RJ45 and fan left, 12 V input right — so the Pi could not simply
be dropped in; it needed an edge of its own.

- Everything in the original 130 × 100 keeps its **x** position.
- The **bottom block** (terminals, fuses, trip LEDs, zip-tie holes, labelling) moved down 30 mm.
- **J1 moved 30 mm right and 4 mm up** onto the new edge. Right, to make room for the Pi column; up,
  to clear the Pi's microSD card, which ejects northward across y = 65 at x 136.4–148.4 and wanted the
  same space as the terminal's body. Clearance is now 1.61 mm. Because J1's negative pad then sits
  north of the `/GNDIN` island, that island is an **L** rather than a rectangle — it reaches up to
  y = 52 in its eastern 18 mm only, which keeps it clear of the `/VG` gate bus crossing F.Cu at y = 55.
  The islands are longer but not narrower, so their temperature rise is unchanged; the extra drop is
  about 20 mV at 30 A.
- The band the bottom block vacated (y 76–104) now holds the Pi's support circuitry. It had to go
  there: the two supply islands are keepouts on **both** outer layers across y 30–66, so anything in
  the Pi column below them cannot reach anything above without a trip round the west side.
- The **east edge is a plain rectangle again** and H4 is back to an M3 corner: the notch and the two
  M2.5 standoff holes existed only for a Pi sitting over the board.
- The activity LEDs **moved onto the shared output nodes** (through 2 k) so they indicate in *both*
  modes rather than only in receiver mode. 2 k, not the 4.7 k first fitted: the LED is a KT-0805G at
  2.6 V minimum, not the 2.4 V an earlier note assumed, so 4.7 k in series with the port's own 470 Ω
  gave **0.46 mA** — a tenth of the test current, and only while data is high, which on a WS2811 frame
  is about a fifth of the time. Invisible outdoors. 2 k gives 0.97 mA for 0.46 V of output swing,
  leaving ~4.5 V at the terminal against a WS2812B threshold of 3.5 V.
- **U5**, the second temperature sensor, moved to (20, 100). It was jammed against the left board edge
  with three nets trying to leave the same two columns. This also partly answers the rev C review's
  point that it was not actually in a cool spot — though see *Known issues*.

## Five bugs this build turned up

**1. The Pi socket was mirrored — and the first fix for it was wrong.** The symptom was a cluster of
unroutable socket pins, and the first diagnosis was a rotation error, "corrected" from `rot=90` to
`rot=270`. That cannot be right: **a rotation never changes handedness**, so both values are equally
unmatable. An adversarial review caught it, and two independent checks confirm it:

- *Against this board's own notch.* Both generators define the Pi's rows the same way — even pins
  2.23 mm from the header edge, odd pins 4.77 mm. In the mirrored build the **even** column sat at
  x = 134.04 and the odd at 131.5, which puts the Pi's header edge on the east and its body extending
  **west** to x = 106.27. The silk outline, the notch and the keepout all assume it extends east.
- *Against difftx.* difftx carries the Pi below it, right way up, socket on B.Cu; this board carries it
  above, upside down, socket on F.Cu. Those two cases need **opposite** handedness in board
  coordinates. Both boards read the same way, and difftx is the one that already does the odd/even
  pad-number swap a mirror needs.

Had it been built, a Pi fitted as the silkscreen directs would have taken `/PI5V` on pin 1 — **5 V into
its 3.3 V rail**, and the same into GPIO2. The other physically matable orientation shorts pin 1 to
ground. There was no orientation in which that board worked.

Two more blockers fell out of the same fix, because the Pi's *whole position* was 30 mm wrong: its
antenna sat over the solid planes and the buck converter's input loop rather than over the notch, and
U7 and J10 were underneath it, with the buck's clone module (a 10.2 mm YLPTEC part, not the 17.5 mm
Mornsun the datasheet describes) about 0.3 mm from the Pi's RP3A0. With the socket corrected the antenna
lands at x 154.57–159.27 — inside the notch, overhanging free air by 9.3 mm — and nothing is under the
Pi but its own two mounting holes.

**And then it inverted.** When the Pi moved onto a ribbon the correct numbering became the *un*-swapped
one, because a cable maps pin n to pin n. The same two pads, the same 5 V onto the same 3.3 V rail — and
the fix for one arrangement is the bug in the other. The lesson is not "swap the pads"; it is that the
mating geometry decides the handedness and has to be derived, every time, from how the two parts
actually meet.

**The reason it survived the first round is worth recording:** `verify.py` checked that every net was on
the pad the generator meant to put it on, and it was. The pads were in the wrong places. The check now
runs the other way — it solves the socket's measured pad positions for the Pi-to-board mapping and
asserts the determinant is −1.

**2. The router did not know the notch exists.** The grid is built from the board's bounding rectangle,
so it was routing into a region with no board. The notch is now registered as a keepout on both layers.
The mounting holes and the RJ45's locating pegs had the same problem — they are KiCad *rule areas*,
which the maze router never read, and `/SDA` was being routed straight through a 3.2 mm hole.

**3. A real router bug.** `route()` seeded its tree with *all* of a net's pre-existing copper at once,
which silently assumes that copper is one connected piece. That held while the only hand-routed copper
was the cat5 trunk. It does not hold for the escape stubs this board adds: four separate stubs on
`/MODE` were assumed joined, so the net "routed" and DRC then reported the ones never connected.

The fix is **opt-in**, and that matters. Seeding from connected components unconditionally made diffrx
*worse* — 27 dangling tracks where it had none — because most disjoint-looking fragments are in fact
joined by a zone the router does not model, so it dutifully routed between them. `route()` now takes
`split_pre`, and only the nine nets carrying escape stubs set it. Regenerating diffrx with the updated
router gives back exactly its previous result, which is how I know the change is contained.

**4. Two hand-routed tracks still wired the receiver's old enables** — pin 12 to GND and pin 4 to +5 V.
Pin 12 is now +5 V, so the GND track was shorting the 5 V rail to ground through U1's own pad.

**5. A same-net zone at a higher priority silently emptied every island.** Reaching J1's moved pad with
a second `/GNDIN` rectangle overlapped the first, which DRC calls `zones_intersect` unless the
priorities differ — and giving the bridge the higher priority made KiCad's filler drop **both** islands
and `/12VIN` as well, to zero area: 69 unconnected items, 67 dangling vias. The fill has to be one
polygon, so `zone_poly()` now exists alongside `zone_rect()`. Worth knowing generally: a zone with no
copper in it is not reported as an error by anything except connectivity.

There was also a mechanical near-miss: the Pi's corner originally overlapped J1's body, and J1 is a
~10 mm screw terminal against an 8.5 mm socket — the Pi would have sat on it. The Pi moved 2 mm south.

## Escape stubs

U9's fourteen pins fan out on a 1.27 mm pitch into the same two channels, and the '125 puts each
buffer's input and output side by side while the inputs all arrive from the Pi in the east and the
outputs all leave for terminals in the west — so every channel's two tracks want to cross, and whichever
net routed last found the lane taken. `gen_pcb.py` lays a short straight stub on each signal pin before
the router starts; the router then picks them up as that net's existing copper. U1's two enable pins get
the same treatment.

Stubs **must run along a router grid column**, not merely end on a node. A SOIC pad is not on the
0.3 mm grid, so a stub that runs straight out on the pad's own x straddles two columns of cells the
whole way; the router picks up whichever it likes as that net's existing copper and can then start its
path from a cell with no copper under it, leaving the two ends 0.3 mm apart — connected in the
generator's mind, unconnected in DRC's. Each stub now jogs onto the grid column at the pad, inside the
pad's own escape corridor, and runs out along it.

**The four output stubs are 5.4 mm, the rest 3.0 mm.** The inputs all arrive from the socket in the east
and fan into the package along a single east-west lane about 3 mm off its face — exactly where a 3 mm
stub ends — so the first input routed sealed the output beside it against both layers at once. Taking
the outputs past that lane before it exists forces the inputs around them. Routing order matters for the
same reason and is now explicit in `ORDER`: the socket's inner-column pins go **north to south**,
because they all leave through one channel and stack up in it in the order they are routed, and each
buffer channel's input and output route **together**.

## Assembly and ordering

- 4 layers, **160 × 130 mm**, 1 oz on every layer, plain rectangle with R3 corners.
- **41 BOM lines, 147 placements** — up from 33 and 126.
- New parts, all JLCPCB-assemblable: 2×20 socket **C2977589**, K7805-2000R3 **C18212380**, AT24C256C
  **C6482**, 74HCT125D **C5962**, SS-12D00-G3 slide switch **C22355741**, 1.5 A PPTC **C22392774**.
- The Pi header uses a **project footprint**, not the vendor one. At the vendor's 1.6 mm pads the gap
  between neighbours is 0.94 mm — narrower than a track plus its clearance, so the forty pins form a
  wall nothing can be routed in or out of. `diffsmart.pretty/PiRibbon_2x20_P2.54mm.kicad_mod` uses
  1.4 mm pads, which still leaves a 0.2 mm annular ring on the 1.0 mm drill and is ample for a 0.64 mm
  square header pin. Numbering is the vendor's own, unmodified.
- **You supply, and JLCPCB cannot**: a Raspberry Pi (any 40-pin model — Zero 2 WH, 3A+, 3B+, 4), a
  **40-way IDC ribbon**, a jumper shunt for JP1, and a 30 A ATO blade fuse. Keep the ribbon short —
  200 mm or less — since it carries 3.3 V CMOS edges from the Pi to the buffer inputs.

  JLCPCB's PCBA line solders parts onto the board, so a single board computer that plugs into a socket
  is not something it can fit — and you would not want it soldered, since the Pi has to come off for
  the SD card. Their part pages also state that components bought through the parts library are *"for
  PCBA orders only, and cannot be shipped separately"*, so a BOM line cannot be used as a shopping list
  for loose items either.

  Four BOM Comments were corrected for exactly this reason. Each named the thing that plugs in rather
  than the thing being ordered, which is how a line ends up read as a request for hardware that is not
  in the assembly:

  | was | now | the part actually ordered |
  |---|---|---|
  | `Pi Zero 2 W` | `2x20 socket for Pi` | C2977589, a female header |
  | `ATO 30A` | `ATO fuse holder` | C352820, the holder — **the fuse is not included** |
  | `12V 30A IN` | `terminal 2P 9.5mm` | C475106, a screw terminal |
  | `I2C` | `header 1x4` | C124378, a pin header |

  **`../difftx` has the same fault** — its BOM line reads `Pi Zero 2 W` against C5124634, which is a
  BOOMELE 2×20 female header. Worth correcting there too.

## Known issues

- **7 `track_dangling` warnings.** They are the unused remainder of an escape stub, where the router
  joined the stub partway along instead of at its end. Shortening the stubs breaks the escape they
  exist for, so they stay. Connectivity verifies clean and a ≤3 mm open tail on a net that is either
  static or switching at 800 kHz is electrically irrelevant.
- **A plain header can be plugged in backwards.** The silk marks pin 1 three ways, but if you want it
  made impossible, fit a shrouded DC3-40P in the same holes — the footprint has clearance for the
  shroud. Reversed, the ribbon puts 5 V on GPIO2.
- **The ribbon carries 3.3 V logic, unterminated.** Four lines at WS2811 rates plus I²C, over a cable,
  into HCT inputs with 2 k pulldowns at this end. Fine at 800 kHz and the standard Pi ribbon interleaves
  grounds, but keep it short and do not run it alongside a pixel output.
- **J1 still sits 4 mm north of where it started**, which was to clear the microSD card of a Pi that is
  no longer there. It costs nothing and the `/GNDIN` island is verified in its L shape, so it stays.
- **The 12 V plane still runs through the socket's 40 holes** on the outer layers; both *inner* planes
  are now pulled out of the connector field, which is where the 30 A bus actually is.
- **The two temperature sensors still are not a hot/ambient pair.** U5 moved somewhere better but the
  board has two solid copper planes spanning it, so it is close to isothermal laterally and the
  difference between the sensors remains inside their own ±2 °C tolerance. Read them as two board
  temperatures.
- **`silkcheck.py`'s edge test is a bounding box.** That was actively wrong while the board had a notch;
  on a plain rectangle it is now correct, but trust DRC for edge clearance regardless.

## Bring-up

1. **Receiver mode first, no Pi fitted.** Slide the switch to **RX**. Bring the board up exactly as
   `../diffrx/archive/revC_release/VERIFICATION.md` describes — main fuse last, check the reverse-polarity
   block, check every terminal's poles, then patch cat5 from difftx. All four ports should behave
   exactly as the receiver does, because in RX mode this *is* that board.
2. **Then Pi mode, still no Pi.** Slide to **PI**. All four data outputs should sit LOW — that is
   R41–R44 holding the buffer inputs down with nothing driving them.
3. **Ring the ribbon out before it touches a Pi** — power off, continuity meter, board pin 1 to cable
   pin 1 to Pi pin 1, and board pin 2 to Pi pin 2. This board's first build had that mirrored and
   nothing but geometry caught it; a reversed cable puts 5 V on GPIO2.
4. **Decide JP1 before you connect anything.** Zero 2 W or 3A+: fit the shunt and the board powers the
   Pi. 3B+, 4 or 5: leave it out and give the Pi its own supply. Check 5 V on J17 pins 2/4 with the
   shunt in and no Pi attached, and check it is *absent* with the shunt out.
5. **Connect the ribbon and boot the Pi.**
6. `i2cdetect -y 1` should show **0x48, 0x49 and 0x50** — both temperature sensors and the EEPROM.
   Note that 0x50 is a plain user EEPROM here, not a HAT ID EEPROM: nothing auto-configures.
7. Configure FPP with the same GPIO 4/5/6/7 string output difftx uses, and check the activity LEDs.
8. Only then connect pixels, one line at a time.
