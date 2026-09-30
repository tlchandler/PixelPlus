# Rev C verification record — FPP Remote pHAT (Pi Zero 2 W, 12 V in)

Generated files: `difftx.kicad_sch` / `difftx.kicad_pcb` (from `gen_sch.py` and `gen_pcb.py`),
`jlcpcb/` (gerbers zip, BOM, CPL from `export_jlc.py`), `eeprom/difftx-eeprom.bin` (from `make_eeprom.py`).

## Automated checks (KiCad 10.0.6, 2026-09-19)

| Check | Result |
|---|---|
| `kicad-cli sch erc --severity-all` | 0 errors, 0 warnings |
| `kicad-cli pcb drc --schematic-parity --refill-zones --severity-all` | 0 violations, 0 unconnected, 0 parity issues |
| 3D render top / bottom / iso | inspected, all parts on the board, RJ45 flush with the left edge, nothing over the Pi hole pattern |

## What each element was checked against

**Pi Zero 2 W mechanical.** Official Raspberry Pi Zero 2 W mechanical drawing (datasheets.raspberrypi.com):
holes 3.5 mm from each edge on a 58 x 23 mm grid, header along the long edge with its rows 1.27 mm either side of
the hole line, square pin-1 pad at the SD-card end on the inner row, 4.87 mm from the hole. On this board: holes
H1-H4 at (4,4) (62,4) (4,27) (62,27), J3 centre y = 4.0, pin 1 at (8.87, 5.27), pin 2 at (8.87, 2.73). The Pi sits
underneath, face up, so its handedness matches the top view and the SD card ends up on the left edge (silk arrow).
The KiCad `Raspberry_Pi_Zero_Socketed_THT_FaceDown` footprint has the same distances but is the mirror image (it is
for a Pi mounted face-down on top of a carrier), so it was used only for the spacings, not for chirality.

**J3 socket is on the BOTTOM of the board** (independent review caught it on top in the first cut). The Pi's male
header enters the 8.5 mm socket body under the board; 11 mm standoffs. Because flipping a footprint mirrors its pad
numbering, J3 uses a project copy of the JLC socket footprint (`difftx:PiSocket_2x20_bottom`) with odd/even pad
numbers swapped, so that on the back layer pad n lies exactly on Pi pin n. Pad positions were read back from the
board after generation: pad 1 (8.87, 5.27) +3V3, pad 2 (8.87, 2.73) +5V, pad 39 (57.13, 5.27) GND. The CPL lists J3
as a Bottom-side part; JLCPCB charges extra for two-sided assembly, so you can also untick J3 in their BOM step and
solder the socket yourself. The mounting holes have a 1.4 mm copper keepout ring (about 5.5 mm clear) for the
standoffs.

**Header pin use** (Raspberry Pi GPIO numbering, `pinout.xyz` / Pi documentation): 1 3V3, 2 and 4 5V, 3 SDA1, 5 SCL1,
6/9/14/20/25/30/34/39 GND, 7 GPIO4, 26 GPIO7, 29 GPIO5, 31 GPIO6, 17 3V3.

**FPP DPIPixels pin map** (`src/channeloutput/DPIPixels.cpp`, `GetDPIPinBitPosition`): P1-7 = DPI_D0 = bit 0,
P1-29 = DPI_D1, P1-31 = DPI_D2, P1-26 = DPI_D3. The EEPROM string file lists outputs in FPP port order:
port 1 = P1-31 → U2 2A → 2Y/2Z → RJ45 1/2; port 2 = P1-26 → 4A → 4Y/4Z → 3/6; port 3 = P1-29 → 3A → 4/5;
port 4 = P1-7 → 1A → 7/8.

**AM26C31IDR** (TI datasheet, SOIC-16): 1 1A, 2 1Y, 3 1Z, 4 G, 5 2Z, 6 2Y, 7 2A, 8 GND, 9 3A, 10 3Y, 11 3Z, 12 G̅,
13 4Z, 14 4Y, 15 4A, 16 VCC. Outputs enabled when G is high or G̅ is low; both are tied to GND here (G̅ low
enables), which is the configuration that was proven on the breadboard. Y = non-inverting = RJ45 "+".

**Falcon differential receiver RJ45.** Pair-to-port assignment confirmed by a Falcon developer post
(falconchristmas.com forum topic 15638): pins 1/2 = port 1, 3/6 = port 2, 4/5 = port 3, 7/8 = port 4. Which pin of
each pair is "+" is not stated in any Falcon document we could reach; the board uses Y (non-inverting) on 1/3/4/7
and Z on 2/6/5/8, which is the exact wiring that ran the SRx1 v5.01 successfully on the breadboard in this project
(Phase 1). If you ever change the driver or jack, re-check that on hardware first. RJ45 footprint is JLC C385834's
own (easyeda2kicad); the jack is latch-up, contact 1 at the bottom of the opening as placed; silk says so.

**AT24C256C-SSHL-T** (Microchip datasheet, SOIC-8): 1 A0, 2 A1, 3 A2, 4 GND, 5 SDA, 6 SCL, 7 WP, 8 VCC.
A0-A2 = GND → address 0x50, which is what FPP's `fppcapedetect` probes on i2c-1 with the `24c256` driver.
WP: 10 k pull-down (R1) so the part is writable by default; JP1 shorts WP to 3V3 to protect it. The Pi's own
1.8 k I2C pull-ups on SDA1/SCL1 serve the bus, so none are added here (HAT spec only requires pull-ups on
ID_SD/ID_SC, which this board does not use). Decoupling C2 100 nF at VCC.

**K7805-2000R3** (Mornsun datasheet, SIP-3): 1 Vin, 2 GND, 3 Vout; 8-36 V in, 5 V / 2 A out, no minimum load.
Input caps C3-C5 3 × 10 µF 1206 (rated 25 V or higher: C89632 is 50 V), output C6 22 µF. Output is a "power output"
pin in the schematic, so the +5V net carries no PWR_FLAG (that duplicate flag was the one ERC error, now removed).

**Input protection.** J4 KF301 5.08 mm screw terminal (pin 1 = +, pin 2 = GND, silk "+ −" in that order) →
F1 1812 polyfuse 1.5 A hold (C22392774) → D1 SS54 Schottky in series (anode toward the fuse, cathode = +12V bus;
JLC SMA footprint pad 2 is the cathode, symbol pin 2 = K). 0.8 mm tracks on the 12 V and 5 V paths.

**Back-powering the Pi.** 5 V from U4 goes to header pins 2 and 4 on a 0.8 mm bottom-layer track. A Pi Zero has
no diode between its USB PWR input and the header 5 V pins, hence the silk and schematic warning never to plug USB
power in while 12 V is connected.

**EEPROM image** (`make_eeprom.py`, format from FPP `docs/EEPROM.txt` and `www/fppEEPROM.php`): header
`FPP02` + name + version + serial, record code 98 location `0` (any), record code 2 tar.gz of the cape directory
(`cape-info.json`, `strings/difftx.json`, `defaults/config/co-pixelStrings.json`), end record `0`. Written unsigned;
FPP signs it in place through its UI after you enter the key or voucher, which needs JP1 open (WP low).
Re-parsed independently after generation: all three files recovered, driver DPIPixels, 4 outputs, 400 px each.

**JLCPCB parts** (all in the JLC library, footprints and 3D models pulled with easyeda2kicad):
C34923 AM26C31IDR, C6482 AT24C256C, C385834 RJ45, C5124634 2x20 socket, C18212380 K7805-2000R3, C474881 KF301-2P,
C22392774 1812 polyfuse, C22452 SS54, C49678 100 nF 0805, C89632 10 µF 1206, C45783 22 µF 0805, C25804 10 k 0603.
JP1 and the mounting holes are excluded from BOM and CPL. CPL origin is the exact board corner.

## Independent review (2026-09-19)

A separate reviewer re-derived everything above from the netlist, the board file, the datasheets and the FPP source
tree, and re-ran ERC/DRC. It found one blocker, the socket side, fixed as described under J3. Items it flagged as
not verifiable from documents: RJ45 +/− polarity within a pair (covered by the breadboard test), and JLCPCB's own
rotation convention for D1/U2/U3/J1, which must be eyeballed in JLCPCB's assembly preview before paying (check the
diode band is toward C3-C5, and the SOIC pin-1 dots match the silk). Design choices it noted but that are not
errors: no TVS/series resistors on the RS-422 pairs (AM26C31 is ±2 kV HBM only), and that until the EEPROM is
signed FPP licenses only 2 outputs and warns above 50 pixels.

## Bring-up order

1. Assemble, do not mount on the Pi yet. Apply 12 V to J4, confirm 5 V on header pins 2/4 and 3.3 V absent (comes
   from the Pi).
2. Mount on the Pi Zero 2 W (11 mm standoffs). Boot FPP, `i2cdetect -y 1` should show `50`.
3. Write `eeprom/difftx-eeprom.bin` to `/sys/bus/i2c/devices/1-0050/eeprom`, reboot, FPP should list the cape
   "difftx". Sign it from the FPP UI (voucher from diy@falconplayer.com or a purchased key). Then close JP1.
4. Channel outputs → the 4 DPIPixels strings appear; run FPP's test mode against the SRx1 (rotary at 0).
