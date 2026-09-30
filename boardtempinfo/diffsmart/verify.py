r"""
Independent read-back of diffsmart.kicad_pcb. Run with KiCad 10's Python:

  "C:\Program Files\KiCad\10.0\bin\python.exe" verify.py

Nothing here reads the generator: it opens the saved board, walks the connectivity KiCad itself built,
and prints the tables that VERIFICATION.md quotes. If the generator and this disagree, this is right.
"""
import os
import pcbnew

HERE = os.path.dirname(os.path.abspath(__file__))
board = pcbnew.LoadBoard(os.path.join(HERE, "diffsmart.kicad_pcb"))
fps = {f.GetReference(): f for f in board.GetFootprints()}

def padnet(ref, num):
    p = fps[ref].FindPadByNumber(str(num))
    return p.GetNetname(), round(p.GetPosition().x / 1e6, 2), round(p.GetPosition().y / 1e6, 2)

raw = open(os.path.join(HERE, "diffsmart.kicad_pcb"), encoding="utf-8").read()
print("stackup copper layers (this is what the 30 A numbers assume):")
for l in raw.splitlines():
    if '(type "copper")' in l:
        print("  " + l.strip())

bb = board.GetBoardEdgesBoundingBox()
print(f"outline      {bb.GetWidth()/1e6:.1f} x {bb.GetHeight()/1e6:.1f} mm, "
      f"{board.GetCopperLayerCount()} copper layers")
print("layers      ", ", ".join(board.GetLayerName(l) for l in
                                (pcbnew.F_Cu, pcbnew.In1_Cu, pcbnew.In2_Cu, pcbnew.B_Cu)))

print("\ncat5 pair -> receiver input -> receiver output -> series R -> terminal")
JACK = {"P1": ("1", "2"), "P2": ("3", "6"), "P3": ("4", "5"), "P4": ("7", "8")}
U1IN = {"P1": ("6", "7"), "P2": ("10", "9"), "P3": ("14", "15"), "P4": ("2", "1")}
U1OUT = {"P1": "5", "P2": "11", "P3": "13", "P4": "3"}
RS = {"P1": "RS1", "P2": "RS2", "P3": "RS3", "P4": "RS4"}
TERM = {"P1": "J2", "P2": "J5", "P3": "J8", "P4": "J11"}
for port in ("P1", "P2", "P3", "P4"):
    jp, jn = JACK[port]
    up, un = U1IN[port]
    print(f"  {port}: RJ45 {jp}(+)={padnet('J14', jp)[0]:6s} {jn}(-)={padnet('J14', jn)[0]:6s}"
          f" -> U1 {up}/{un} = {padnet('U1', up)[0]:6s}/{padnet('U1', un)[0]:6s}"
          f" -> U1 {U1OUT[port]} = {padnet('U1', U1OUT[port])[0]:6s}"
          f" -> {RS[port]} = {padnet(RS[port], 1)[0]:6s}/{padnet(RS[port], 2)[0]:7s}"
          f" -> {TERM[port]} pole2 = {padnet(TERM[port], 2)[0]}")

print("\nreceiver housekeeping")
for pin, what in (("4", "G (enable, high)"), ("12", "~G (enable, low)"),
                  ("8", "GND"), ("16", "VCC")):
    print(f"  U1.{pin:<2s} {padnet('U1', pin)[0]:6s}  {what}")

print("\noutput terminals: fuse -> + pole, data pole, - pole")
for j in ["J2", "J3", "J4", "J5", "J6", "J7", "J8", "J9", "J10", "J11", "J12", "J13"]:
    poles = {n: padnet(j, n) for n in ("1", "2", "3")}
    v = [p for p in poles.values() if p[0].startswith("/V")]
    fuse = next((f for f in fps if f.startswith("F") and
                 padnet(f, "2")[0] == v[0][0]), "?") if v else "?"
    order = sorted(poles.values(), key=lambda p: p[1])
    print(f"  {j:4s} {fps[j].GetValue():9s} left->right: " +
          "  ".join(f"{p[0]}" for p in order) + f"   (fed by {fuse})")

print("\nESD arrays (PSM712: pins 1 and 2 are the pair, 3 is GND - nothing ties to 5 V)")
for ref in ("D3", "D4", "D5", "D6"):
    print(f"  {ref}: " + "  ".join(f"{n}={padnet(ref, n)[0]}" for n in ("1", "2", "3")))

print("\npower entry: input terminal -> main fuse -> reverse-polarity FETs -> bus")
for ref, num, what in (("J1", "2", "input terminal, supply positive"),
                       ("F0", "3", "ATO holder, supply-side receptacle"),
                       ("F0", "1", "ATO holder, bus-side receptacle"),
                       ("J1", "1", "input terminal, supply negative"),
                       ("Q1", "2", "Q1 drain"), ("Q1", "3", "Q1 source"), ("Q1", "1", "Q1 gate"),
                       ("D2", "1", "TVS cathode"), ("D2", "2", "TVS anode"),
                       ("C1", "1", "bulk +"), ("C1", "2", "bulk -"),
                       ("D7", "1", "5 V clamp cathode"), ("D7", "2", "5 V clamp anode")):
    n, x, y = padnet(ref, num)
    print(f"  {ref}.{num:<2s} {n:8s} at ({x}, {y})   {what}")

print("\nfuse-trip indicators (dark while the fuse conducts, lit once it opens)")
for k in range(12):
    lf, rf = f"LF{k+1}", f"RF{k+1}"
    print(f"  F{k+1:<2d} {lf}: anode={padnet(lf, '1')[0]:6s} cathode={padnet(lf, '2')[0]:7s}"
          f"  -> {rf}: {padnet(rf, '1')[0]:7s}/{padnet(rf, '2')[0]}")

print("\nreverse-polarity indicator (across the FET bank, so it is dark in normal operation)")
for ref, num, what in (("LED7", "1", "anode, supply-side ground"), ("LED7", "2", "cathode"),
                       ("R28", "1", "series resistor"), ("R28", "2", "board ground")):
    print(f"  {ref}.{num} {padnet(ref, num)[0]:8s}  {what}")

print("\nthermostat: NTC divider -> LM2903 -> fan FET and over-temp LED")
for ref, num, what in (("R29", "1", "+5 V"), ("R29", "2", "divider node"), ("RT5", "1", "NTC top"),
                       ("RT5", "2", "NTC bottom"),
                       ("U6", "2", "IN1- (NTC)"), ("U6", "3", "IN1+ (fan reference)"),
                       ("U6", "1", "OUT1 -> fan gate"), ("U6", "5", "IN2+ (NTC)"),
                       ("U6", "6", "IN2- (over-temp reference)"), ("U6", "7", "OUT2 -> LED"),
                       ("U6", "4", "V-"), ("U6", "8", "V+"),
                       ("Q5", "1", "fan gate"), ("Q5", "2", "fan source"), ("Q5", "3", "fan drain"),
                       ("F13", "1", "fan fuse in"), ("F13", "2", "fan fuse out"),
                       ("D8", "1", "flyback, see pin name below"), ("D8", "2", "flyback, see pin name below"),
                       ("J16", "1", "fan +"), ("J16", "2", "fan -"), ("J16", "3", "fan ground")):
    print(f"  {ref}.{num:<2s} {padnet(ref, num)[0]:9s}  {what}")

# Diode orientation, taken from the footprint's own pin names rather than from a hand-written label:
# the previous version of this file asserted D8 pin 1 was the anode and cheerfully confirmed a
# backwards flyback diode.
print("\ndiode orientation, read from each footprint's pin names")
# The board's pads carry no pin functions, so take the names from the netlist, which is generated
# from the symbol library and is therefore independent of the placement.
import re as _re
_net = open(os.path.join(HERE, "diffsmart.net"), encoding="utf-8").read()
_fn = {}
for _m in _re.finditer(r'\(ref "?([A-Za-z0-9]+)"?\)\s*\(pin "?([A-Za-z0-9]+)"?\)\s*\(pinfunction "?([^")]*)', _net):
    _fn[(_m.group(1), _m.group(2))] = _m.group(3)
for ref, expect in (("D1", "cathode on the gate clamp"), ("D2", "cathode on +12V"),
                    ("D7", "cathode on +5V"), ("D8", "cathode on the supply, anode on the switch node")):
    print(f"  {ref}: " + "  ".join(
        f"{n}={_fn.get((ref, n), '?')}:{padnet(ref, n)[0]}" for n in ("1", "2"))
        + f"   (expect {expect})")
_k = next((n for n in ("1", "2") if _fn.get(("D8", n), "").upper().startswith("K")), None)
_cn = padnet("D8", _k)[0] if _k else "?"
print("  D8 cathode (pin %s) is on %s%s" % (
    _k, _cn, "  OK - correct low-side flyback" if _cn == "/FAN12"
    else "  *** WRONG: forward diode across the fan supply ***"))

print("\nI2C sensors, powered from the header's VIO and not from the board's 5 V rail")
for ref in ("U4", "U5"):
    print(f"  {ref}: " + "  ".join(f"{n}={padnet(ref, n)[0]}" for n in ("1", "2", "4", "7", "8")))
print("  J15: " + "  ".join(f"{n}={padnet('J15', n)[0]}" for n in ("1", "2", "3", "4")))
print("  +5V on either sensor? " +
      ("YES - WRONG" if any(padnet(r, n)[0] == "+5V" for r in ("U4", "U5") for n in "12345678")
       else "no, correct - they float when nothing is plugged in"))

# KiCad does not put DRC constraints in the .kicad_pcb - they live in the project file, and LoadBoard
# quietly loads the bound project, so reading them off the BOARD object tells you nothing about where
# they came from. Read the JSON, which is what kicad-cli pcb drc actually enforces.
import json
print("\nmode select - the single line that swaps the two sources")
for ref, num, what in (("SW1", "1", "throw toward RECEIVER (+5V)"), ("SW1", "2", "common -> /MODE"),
                       ("SW1", "3", "throw toward PI (GND)"), ("R45", "1", "default pull-up"),
                       ("U1", "4", "AM26C32 G   - HIGH enables the receiver"),
                       ("U1", "12", "AM26C32 ~G  - tied high so the enable is G alone"),
                       ("U9", "1", "74HCT125 ~1OE"), ("U9", "4", "74HCT125 ~2OE"),
                       ("U9", "10", "74HCT125 ~3OE"), ("U9", "13", "74HCT125 ~4OE")):
    print(f"  {ref}.{num:<2s} {padnet(ref, num)[0]:9s}  {what}")
_g, _gb = padnet("U1", "4")[0], padnet("U1", "12")[0]
_oe = {padnet("U9", n)[0] for n in ("1", "4", "10", "13")}
print("  " + ("OK - one net drives the receiver's G and all four buffer ~OE, and ~G is tied high"
              if _g == "/MODE" and _gb == "+5V" and _oe == {"/MODE"}
              else "*** WRONG: the two sources are not mutually exclusive ***"))

print("\nthe four outputs, and the two things that can drive them")
for i in (1, 2, 3, 4):
    print(f"  DATA{i}: receiver RS{i} {padnet(f'RS{i}', '1')[0]:7s}->{padnet(f'RS{i}', '2')[0]:7s}"
          f"   Pi RS{i+4} {padnet(f'RS{i+4}', '1')[0]:7s}->{padnet(f'RS{i+4}', '2')[0]:7s}"
          f"   LED{i} on {padnet(f'LED{i}', '1')[0]}")

print("\nPi interface (GPIO map is identical to difftx, so FPP needs no change)")
for pin, what in (("1", "3V3"), ("2", "5V"), ("3", "SDA1"), ("4", "5V"), ("5", "SCL1"),
                  ("7", "GPIO4"), ("26", "GPIO7"), ("29", "GPIO5"), ("31", "GPIO6")):
    print(f"  J17.{pin:<2s} {padnet('J17', pin)[0]:9s}  Pi pin {pin} ({what})")
print("  supply: F14 " + padnet("F14", "1")[0] + " -> " + padnet("F14", "2")[0] +
      " -> U7 " + padnet("U7", "1")[0] + " -> " + padnet("U7", "3")[0])
print("  EEPROM U8 at 0x50: " + "  ".join(f"{n}={padnet('U8', n)[0]}" for n in ("1", "2", "3", "5", "6", "7", "8")))
# D9 is gone: it was meant to stop the J15 header back-feeding the Pi's 3.3 V rail, but the Pi's own
# 1.8 k pull-ups on GPIO2/GPIO3 are a path around it, and it cost the LM75s 0.3-0.4 V of headroom.
assert "D9" not in fps, "D9 is supposed to be deleted"
print("  I2C rail: J15.2 " + padnet("J15", "2")[0] + ", LM75 U4.8 " + padnet("U4", "8")[0] +
      ", U5 A0 " + padnet("U5", "7")[0] + " (A0 high -> 0x49)")

print("\nthe ribbon header, read back from J17's pads")
# A 40-way ribbon is a straight 1:1 map, so board pad n must be where a standard 2x20 puts pin n AND
# must carry the net for Pi pin n. The socket this replaced was deliberately mirrored; this must not be.
_p1 = padnet("J17", "1")[1:]; _p2 = padnet("J17", "2")[1:]; _p3 = padnet("J17", "3")[1:]
_ex = (_p2[0] - _p1[0], _p2[1] - _p1[1])       # pin 1 -> pin 2, across the two rows
_ey = (_p3[0] - _p1[0], _p3[1] - _p1[1])       # pin 1 -> pin 3, one pitch along
_cross = _ex[0] * _ey[1] - _ex[1] * _ey[0]
print(f"  pin 1 {_p1}   pin 2 {_p2}   pin 3 {_p3}")
print(f"  1->2 {_ex} (2.54 across)   1->3 {_ey} (2.54 along)   handedness {_cross:+.2f}")
print("  " + ("OK - straight through, so a standard ribbon lands pin n on pin n"
              if abs(_cross - 6.4516) < 0.01 else
              f"*** MIRRORED ({_cross:+.2f}): a ribbon would deliver pin 1 to pin 2 - 5 V into the Pi's 3V3 ***"))

# The Raspberry Pi 40-pin header, for every pin this board connects. Anything not listed is left open.
PI_HEADER = {"1": ("3V3", "/P3V3"), "2": ("5V", "/HDR5V"), "3": ("GPIO2 SDA1", "/SDA"),
             "4": ("5V", "/HDR5V"), "5": ("GPIO3 SCL1", "/SCL"), "6": ("GND", "GND"),
             "7": ("GPIO4", "/DPI_D0"), "9": ("GND", "GND"), "14": ("GND", "GND"),
             "17": ("3V3", "/P3V3"), "20": ("GND", "GND"), "25": ("GND", "GND"),
             "26": ("GPIO7", "/DPI_D3"), "29": ("GPIO5", "/DPI_D1"), "30": ("GND", "GND"),
             "31": ("GPIO6", "/DPI_D2"), "34": ("GND", "GND"), "39": ("GND", "GND")}
_bad = []
for _n, (_fn, _want) in PI_HEADER.items():
    _got = padnet("J17", _n)[0]
    _ok = _got == _want
    if not _ok:
        _bad.append(_n)
    print(f"  J17.{_n:<2s} {_got:9s} Pi pin {_n:<2s} = {_fn:11s} {'' if _ok else '*** expected ' + _want + ' ***'}")
_open = [p.GetNumber() for p in fps["J17"].Pads() if not p.GetNetname() or "unconnected" in p.GetNetname()]
print(f"  {len(_open)} pins left open, {len(PI_HEADER)} connected"
      + ("" if not _bad else f"   *** WRONG NET ON {_bad} ***"))

print("\nthe 5 V link - the board's 2 A buck reaches the ribbon only through it")
print("  U7 out " + padnet("U7", "3")[0] + " -> JP1.1 " + padnet("JP1", "1")[0] +
      "  |  JP1.2 " + padnet("JP1", "2")[0] + " -> J17.2/4 " + padnet("J17", "2")[0])
print("  " + ("OK - pulling the shunt isolates the Pi's 5 V from this board entirely"
              if padnet("JP1", "1")[0] == "/PI5V" and padnet("JP1", "2")[0] == "/HDR5V"
              and padnet("J17", "2")[0] == "/HDR5V" and padnet("U7", "3")[0] == "/PI5V"
              else "*** the link does not break the 5 V path ***"))
print("  ground and signals are unaffected by the link: J17.6 " + padnet("J17", "6")[0] +
      ", J17.1 " + padnet("J17", "1")[0] + " (the Pi's own 3V3 still runs the sensors)")

print("\nPi supply track widths (the board feeds the Pi only with JP1 fitted, 2 A ceiling)")
import collections as _c
_w = _c.defaultdict(set)
for _t in board.GetTracks():
    if _t.GetClass() == "PCB_TRACK" and _t.GetNetname() in ("/PI5V", "/HDR5V", "/PI12", "/P3V3"):
        _w[_t.GetNetname()].add(round(_t.GetWidth() / 1e6, 2))
for _n in ("/PI5V", "/HDR5V", "/PI12", "/P3V3"):
    print(f"  {_n:8s} widths in use: {sorted(_w[_n])} mm")

print("\nDRC constraints, read from diffsmart.kicad_pro (this is what DRC enforces)")
rules = json.load(open(os.path.join(HERE, "diffsmart.kicad_pro"), encoding="utf-8"))[
    "board"]["design_settings"]["rules"]
WANT = {"min_clearance": 0.2, "min_connection": 0.2, "min_track_width": 0.2, "min_via_diameter": 0.6,
        "min_via_annular_width": 0.15, "min_through_hole_diameter": 0.3,
        "min_copper_edge_clearance": 0.3, "min_hole_clearance": 0.3, "min_hole_to_hole": 0.25,
        "min_silk_clearance": 0.15, "min_text_height": 1.0, "min_text_thickness": 0.15,
        "min_resolved_spokes": 1}
bad = 0
for k, want in WANT.items():
    got = rules.get(k)
    ok = got == want
    bad += 0 if ok else 1
    print(f"  {k:28s} {got}" + ("" if ok else f"   *** expected {want} ***"))
print("  " + ("all as intended" if not bad else f"{bad} constraint(s) NOT as intended"))

print("\nzones")
for z in board.Zones():
    print(f"  {z.GetZoneName():14s} {board.GetLayerName(z.GetLayer()):7s} net {z.GetNetname():8s} "
          f"filled={z.IsFilled()} area={z.GetFilledArea()/1e12:7.1f} mm2")

conn = board.GetConnectivity()
print(f"\nratsnest (unconnected) count: {conn.GetUnconnectedCount(True)}")
