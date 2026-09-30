r"""
difftxlarge rev A board generator. Run with KiCad 10's own Python (after gen_sch.py and the netlist export):

  "C:\Program Files\KiCad\10.0\bin\python.exe" gen_pcb.py

Board 310 x 206 mm (639 cm^2, under JLCPCB's 650 cm^2 "Large Size" line), R3 corners, FOUR layers, 1 oz:
  F.Cu    components and signal routing
  In1.Cu  solid GND
  In2.Cu  +12V under the power section (x >= 95, y < 62), 5V_DRV everywhere else
  B.Cu    the 23-line bus, signal routing, GND fill

Floorplan (y down; everything from the ribbon header down is DY = 6 mm lower than listed here):
    3 -  59   Raspberry Pi on M2.5 standoffs, rotated so its ports face the top and left edges and its
              header faces down the board; power, I2C, audio and fan along the top edge to the right
   64         J16 box header straight below the Pi's own header (a short ribbon, no twist)
   78 -  84   three AHCT541 buffers and their 33 R series resistors
   89 - 104   the bus: D19 (top) ... D0, LE2, LE1, LE0 (bottom), 0.7 mm pitch on B.Cu
  111         nine AHCT573 latches, three per bank, each centred over the jack(s) it feeds
  122 - 146   per jack: activity LEDs, AM26C31, four PSM712s
  149 - 165   fifteen RJ45s on a 20 mm pitch, openings facing the bottom edge
  165 - 200   the cable band: each plug and cable lies on the board, and a tie through the slot pair
              27 mm in front of the jack clamps the jacket to the board behind the boot

Routing is built from the board's repetition: one jack cell is routed on its own grid and copied fifteen
times, one bank's latch-to-driver wiring is routed and copied three times, the bus and its taps are drawn
directly, and the maze router (grid_router.py) does what is left.
"""
import os, re, sys, math, time
import pcbnew
try:                                   # a stray wx assert pops a modal dialog and hangs a headless run
    import wx
    wx.DisableAsserts()
except Exception:
    pass
from pcbnew import VECTOR2I_MM as MM, FromMM

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
FPLIB = r"C:\Program Files\KiCad\10.0\share\kicad\footprints"
PROJECT = "difftxlarge"
W, H = 310.0, 206.0
# Everything from the ribbon header down sits DY below its first layout, so the header is 6 mm clear of the
# Pi's edge and standoffs: an IDC plug is taller than the shroud and rises past the Pi's board, and the ribbon
# needs room to fold. The board grew by the same 6 mm (310 x 206 = 639 cm^2, still under JLC's 650).
DY = 6.0
PLACE_ONLY = "--place-only" in sys.argv

def blocks(s, key):
    out = []; i = 0
    while True:
        j = s.find(key, i)
        if j < 0:
            return out
        d = 0; k = j
        while True:
            c = s[k]
            if c == '(':
                d += 1
            elif c == ')':
                d -= 1
                if d == 0:
                    break
            k += 1
        out.append(s[j:k + 1]); i = k + 1

net_txt = open(os.path.join(HERE, f"{PROJECT}.net"), encoding="utf-8").read()
padnet = {}
UNC = {}
FULL = {}            # short net name (D0) -> the netlist's own name (/D0); the board keeps the full names
for b in blocks(net_txt[net_txt.find("(nets"):], "(net\n"):
    name = re.search(r'\(name "([^"]*)"', b).group(1)
    if name.startswith("unconnected-"):
        for ref, pin in re.findall(r'\(ref "([^"]+)"\)\s*\(pin "([^"]+)"\)', b):
            UNC[(ref, pin)] = name                   # a no-connect pin: keeps its net name, never routed
        continue
    FULL[name.lstrip("/")] = name
    for ref, pin in re.findall(r'\(ref "([^"]+)"\)\s*\(pin "([^"]+)"\)', b):
        padnet[(ref, pin)] = name.lstrip("/")
def nn(obj):
    return obj.GetNetname().lstrip("/")
sch_txt = open(os.path.join(HERE, f"{PROJECT}.kicad_sch"), encoding="utf-8").read()
root_uuid = re.search(r'\(uuid "([^"]+)"\)', sch_txt).group(1)
sym_uuid, sym_fields = {}, {}
for b in blocks(sch_txt, "\n\t(symbol\n"):
    u = re.search(r'\(uuid "([^"]+)"\)', b).group(1)
    r = re.search(r'\(property "Reference" "([^"]+)"', b).group(1)
    sym_uuid[r] = u
    sym_fields[r] = {k: v for k, v in re.findall(r'\(property "(Datasheet|Description|LCSC|Value|Footprint)" "([^"]*)"', b)}
    sym_fields[r]["in_bom"] = "(in_bom yes)" in b

def set_field(fp, name, value):
    std = {"Datasheet": pcbnew.FIELD_T_DATASHEET, "Description": pcbnew.FIELD_T_DESCRIPTION}
    f = fp.GetField(std[name]) if name in std else next((g for g in fp.GetFields() if g.GetName() == name), None)
    if f is None:
        nf = pcbnew.PCB_FIELD(fp, std.get(name, pcbnew.FIELD_T_USER), name)
        nf.SetText(value); nf.SetVisible(False); nf.SetLayer(pcbnew.F_Fab); fp.Add(nf)
        f = next(g for g in fp.GetFields() if g.GetName() == name)
    f.SetText(value); f.SetVisible(False)

board = pcbnew.BOARD()
board.SetCopperLayerCount(4)
F, I1, I2, B = pcbnew.F_Cu, pcbnew.In1_Cu, pcbnew.In2_Cu, pcbnew.B_Cu
ds = board.GetDesignSettings()
# JLCPCB 4-layer 1 oz limits, held in the board file (the .kicad_pro is reset by the CLI). Vias 0.7/0.3 =
# JLCPCB's recommended 0.2 mm annular ring, not the 0.15 mm absolute minimum.
ds.m_CopperEdgeClearance = FromMM(0.3); ds.m_MinClearance = FromMM(0.2); ds.m_TrackMinWidth = FromMM(0.2)
ds.m_ViasMinSize = FromMM(0.7); ds.m_MinThroughDrill = FromMM(0.3)
ds.m_MinConn = FromMM(0.2)
ds.m_ViasMinAnnularWidth = FromMM(0.2)
ds.m_HoleClearance = FromMM(0.30); ds.m_HoleToHoleMin = FromMM(0.25)
ds.m_SilkClearance = FromMM(0.15)
ds.m_MinSilkTextHeight = FromMM(1.0); ds.m_MinSilkTextThickness = FromMM(0.15)
ds.m_MinResolvedSpokes = 1
ds.SetAuxOrigin(MM(0.0, H))

nets = {}
def net(name):
    name = FULL.get(name, name)
    if name not in nets:
        n = pcbnew.NETINFO_ITEM(board, name); board.Add(n); nets[name] = n
    return nets[name]
for name in sorted(set(padnet.values())):
    net(name)

fps = {}
def add_fp(ref, x, y, rot=0):
    """Footprint from the schematic's own Footprint field, so the board cannot drift from it."""
    lib, name = sym_fields[ref]["Footprint"].split(":")
    path = {"jlc": os.path.join(HERE, "jlc", "jlc.pretty"),
            PROJECT: os.path.join(HERE, f"{PROJECT}.pretty")}.get(lib, os.path.join(FPLIB, lib + ".pretty"))
    fp = pcbnew.FootprintLoad(path, name)
    assert fp is not None, (ref, lib, name)
    fp.SetReference(ref); fp.SetFPIDAsString(f"{lib}:{name}")
    fp.SetValue(sym_fields[ref].get("Value", ""))
    fp.SetPath(pcbnew.KIID_PATH(f"/{root_uuid}/{sym_uuid[ref]}"))
    set_field(fp, "Datasheet", sym_fields[ref].get("Datasheet", ""))
    set_field(fp, "Description", sym_fields[ref].get("Description", ""))
    if sym_fields[ref].get("LCSC"):
        set_field(fp, "LCSC", sym_fields[ref]["LCSC"])
        set_field(fp, "LCSC Part", sym_fields[ref]["LCSC"])
    fp.SetExcludedFromBOM(not sym_fields[ref]["in_bom"])
    if not sym_fields[ref]["in_bom"]:
        fp.SetExcludedFromPosFiles(True)
    board.Add(fp)
    fp.SetOrientationDegrees(rot); fp.SetPosition(MM(x, y))
    for p in fp.Pads():
        n = padnet.get((ref, p.GetNumber()))
        if n:
            p.SetNet(net(n))
        elif (ref, p.GetNumber()) in UNC:
            p.SetNet(net(UNC[(ref, p.GetNumber())]))
        elif p.GetAttribute() == pcbnew.PAD_ATTRIB_PTH and p.GetNumber() == "":
            # easyeda2kicad imports locating pegs as plated, unnumbered pads (PJ-3270); a peg is a plain hole
            p.SetAttribute(pcbnew.PAD_ATTRIB_NPTH)
            p.SetLayerSet(pcbnew.PAD.UnplatedHoleMask())
            p.SetSize(p.GetDrillSize())
    fps[ref] = fp
    return fp

def pad(ref, num):
    p = fps[ref].FindPadByNumber(str(num))
    return (round(p.GetPosition().x / 1e6, 4), round(p.GetPosition().y / 1e6, 4))

made = []            # (kind, layer, net, pts / pos, width) of everything drawn, for the cell/bank copies
def trk(layer, netname, pts, w=0.25):
    for (x1, y1), (x2, y2) in zip(pts, pts[1:]):
        if abs(x1 - x2) < 1e-6 and abs(y1 - y2) < 1e-6:
            continue
        t = pcbnew.PCB_TRACK(board); t.SetStart(MM(x1, y1)); t.SetEnd(MM(x2, y2))
        t.SetWidth(FromMM(w)); t.SetLayer(layer); t.SetNet(net(netname)); board.Add(t)
        made.append(("t", layer, netname, ((x1, y1), (x2, y2)), w))
def via(netname, x, y, d=0.7, drill=0.3):
    v = pcbnew.PCB_VIA(board); v.SetPosition(MM(x, y)); v.SetWidth(FromMM(d)); v.SetDrill(FromMM(drill))
    v.SetNet(net(netname)); v.SetLayerPair(F, B); board.Add(v)
    made.append(("v", None, netname, (x, y), d))

# =====================================================================================================
# placement
# =====================================================================================================
JX = [15.0 + 20.0 * j for j in range(15)]          # jack centres
JY = H - 35.0 - 8.62                                # jack origin: its front face (+8.62) 35 mm from the edge
TIE_Y = JY + 8.62 + 27.0                            # slot pair, behind a booted plug
LED_Y, RA_Y, DRV_Y, ESD_Y = 120.3 + DY, 124.3 + DY, 134.0 + DY, 145.2 + DY
LATCH_Y = 111.0 + DY
LATCH_DX = {0: 25.0, 1: 68.0, 2: 97.0}             # within a bank; banks are 100 mm apart (the middle one sits
                                                    # 3 mm right of its jack pair to leave x 50-61 to the buffer taps)
BUS_Y0, BUS_P = 104.4 + DY, 0.7                          # bottom line (LE0) and pitch, going up
# bus line order from the bottom: LE0, LE1, LE2, D0 ... D19
BUS_ORDER = ["LE0", "LE1", "LE2"] + [f"D{i}" for i in range(20)]
BUS_Y = {n: round(BUS_Y0 - BUS_P * k, 3) for k, n in enumerate(BUS_ORDER)}
PORT_X = [6.0, 2.0, -2.0, -6.0]                     # LED / resistor columns: ports 4, 3, 2, 1 left to right, like the jack's pairs
ESD_X = [-6.3, -2.1, 2.1, 6.3]

for j in range(15):
    xc = JX[j]
    add_fp(f"J{j+1}", xc, JY, 0)
    add_fp(f"U{j+1}", xc, DRV_Y, 180)
    add_fp(f"C{13+j}", xc + 7.6, DRV_Y + 1.6, 90)
    for p in range(4):
        o = 4 * j + p + 1
        add_fp(f"LED{o}", xc + PORT_X[p], LED_Y, 90)
        add_fp(f"RA{o}", xc + PORT_X[p], RA_Y, 90)
    # port 4 (RJ45 7/8) is the leftmost pair on the jack and port 1 (1/2) the rightmost, so the ESD row
    # reads port 4, 3, 2, 1 from the left
    for p, dx in zip((3, 2, 1, 0), ESD_X):
        add_fp(f"D{4*j+p+1}", xc + dx, ESD_Y, 270)
    add_fp(f"Z{2*j+1}", xc - 5.5, TIE_Y, 0)
    add_fp(f"Z{2*j+2}", xc + 5.5, TIE_Y, 0)

for b in range(3):
    for m in range(3):
        x = 100.0 * b + LATCH_DX[m]
        add_fp(f"U{16 + 3*b + m}", x, LATCH_Y, 180)
        add_fp(f"C{4 + 3*b + m}", x + 5.0, LATCH_Y + 2.2, 90)

# Pi: rotated 180 degrees, occupying x 8..93, y 3..59. Its holes (Pi coords (3.5,3.5) (61.5,3.5) (3.5,52.5)
# (61.5,52.5), header along y = 3.5 centred on x = 32.5) land at:
PI_X0, PI_Y0 = 8.0, 3.0
def pi_xy(px, py):
    return (PI_X0 + 85.0 - px, PI_Y0 + 56.0 - py)
PI_HOLES = [pi_xy(3.5, 3.5), pi_xy(61.5, 3.5), pi_xy(3.5, 52.5), pi_xy(61.5, 52.5)]
PI_HDR = pi_xy(32.5, 3.5)                           # (60.5, 55.5)
for i, (hx, hy) in enumerate(PI_HOLES):
    add_fp(f"H{7+i}", hx, hy)
J16_Y = 64.0 + DY
add_fp("J16", PI_HDR[0], J16_Y, 180)                # pin 1 at the right end, odd row on top, as on the Pi

# Buffer channels are handed out in header order (see gen_sch.py): SLOT[line] = position left to right.
HDR = {4: 7, 5: 29, 6: 31, 7: 26, 8: 24, 9: 21, 10: 19, 11: 23, 12: 32, 13: 33, 14: 8, 15: 10, 16: 36, 17: 11,
       18: 12, 19: 35, 20: 38, 21: 40, 22: 15, 23: 16, 24: 18, 25: 22, 26: 37, 27: 13}
LINES = [f"D{i}" for i in range(20)] + ["LE0", "LE1", "LE2"]
LINE_GPIO = {f"D{i}": 4 + i for i in range(20)} | {"LE0": 27, "LE1": 26, "LE2": 25}
ORDER_BY_X = sorted(range(23), key=lambda k: (-((HDR[LINE_GPIO[LINES[k]]] - 1) // 2), HDR[LINE_GPIO[LINES[k]]] % 2))
SLOT = {LINES[k]: s for s, k in enumerate(ORDER_BY_X)}
BUF_X = [40.0, 56.0, 79.0]
BUF_Y, RS_Y = 78.0 + DY, 87.0 + DY
# pull-downs: a row between the header and the buffers, one above each buffer input in the same order, so
# every Pi line runs one way only - header, pull-down, buffer - instead of branching up and down
for k, name in enumerate(LINES):
    u, t = divmod(SLOT[name], 8)
    add_fp(f"RP{k+1}", BUF_X[u] + 1.6 * (t - 3.5), 70.2 + DY, 270)
for u, x in enumerate(BUF_X):
    add_fp(f"U{25+u}", x, BUF_Y, 180)
    add_fp(f"C{1+u}", x - 5.2, BUF_Y - 1.6, 90)
# series resistors, one per bus line, straight under the buffer output that drives it
RS_OF = {}
for k, name in enumerate(LINES):
    u, t = divmod(SLOT[name], 8)
    RS_OF[name] = (f"RS{k+1}", u, t)
    add_fp(f"RS{k+1}", BUF_X[u] - 0.65 + 1.6 * (t - 3.5), RS_Y, 270)

# ---- top edge connectors and power ------------------------------------------------------------------
add_fp("J18", 102.0, 4.86, 180)                     # USB-C power out, mouth at the top edge
add_fp("R30", 96.0, 11.0, 0)
add_fp("R31", 108.0, 11.0, 0)
add_fp("J19", 118.0, 9.0, 0)                        # 3.5 mm jack, nose at the edge. The PJ-3270-4A drawing gives
                                                    # an 11.6 mm body + 2.5 mm nose with the sleeve pin 2.5 mm behind
                                                    # the body front, so the nose tip is 9.35 mm ahead of pad 1
                                                    # (local -4.35). Its 3D model is shifted +3.5 mm in the
                                                    # footprint to match the drawing.
add_fp("R46", 126.5, 14.0, 90)
add_fp("R47", 129.0, 14.0, 90)
add_fp("J20", 134.0, 4.5, 180)                      # line out
# OLED socket inside the board, not at the edge: a 0.96 inch SSD1306 module (27.3 x 27.8 mm) plugs in and lies
# face up over a clear, outlined area below it, the way the Pi has its own space
OLED_X, OLED_Y = 266.0, 33.0
add_fp("J22", OLED_X, OLED_Y, 0)
add_fp("J21", 170.0, 4.5, 180)                      # fan
add_fp("J17", 294.0, 4.5, 180)                      # 12 V in

# 12 V input chain, right to left along the top
add_fp("F1", 276.0, 13.0, 0)
add_fp("Q1", 259.0, 13.0, 270)
add_fp("R1", 259.5, 20.5, 0)
add_fp("D61", 252.5, 20.5, 90)
add_fp("D62", 252.0, 26.0, 0)
add_fp("R2", 245.0, 13.0, 180)
add_fp("U30", 245.0, 21.0, 0)
add_fp("C62", 241.2, 21.5, 90)
add_fp("C32", 232.0, 13.0, 0)
add_fp("R3", 232.0, 20.0, 0)
add_fp("LED61", 292.0, 20.0, 0); add_fp("D63", 292.0, 23.5, 0); add_fp("R4", 292.0, 27.0, 0)
add_fp("LED62", 284.0, 20.0, 0); add_fp("R5", 284.0, 23.5, 0)

# The two TPS56637 bucks share one layout (see buck_copper() in the routing section). Chip at (x, y):
#   input caps rot 90 in a row above-right, pin 1 (+12V) down onto the VIN pour, pin 2 (GND) up
#   C_HF 100 nF straddling the VIN bar and the PGND bar, SW pour below to the inductor (rot 270, pin 1 up),
#   output pour under the inductor with three 22 uF hanging off it; FB / EN parts to the left.
BUCKS = [("U28", "5V_PI", 112.0, 28.0, 40, 10, "L1"), ("U29", "5V_DRV", 112.0, 57.0, 50, 20, "L2")]
def place_buck(u, rail, x, y, cb, rb, lref):
    add_fp(u, x, y, 0)
    for i in range(3):
        add_fp(f"C{cb+1+i}", x + 3.2 + 2.4 * i, y - 2.5, 90)       # input 10 uF: pin 1 (+12V) bottom
    add_fp(f"C{cb+4}", x + 0.4, y - 0.6 - 2.0, 180)                # input HF: pin 1 (+12V) right, pin 2 left
    add_fp(f"C{cb}", x + 1.8, y + 3.0, 270)                         # boot: pin 1 (BT) up, pin 2 (SW) down
    add_fp(lref, x + 3.2, y + 9.5, 270)                             # inductor: pin 1 (SW) up
    for i in range(3):
        add_fp(f"C{cb+5+i}", x + 7.6 + 3.0 * i, y + 15.8, 270)     # output: pin 1 (rail) up
    add_fp(f"C{cb+8}", x - 4.5, y + 2.5, 0)                         # feed-forward C
    add_fp(f"R{rb}", x - 4.5, y + 4.5, 0)                           # FB top
    add_fp(f"R{rb+1}", x - 4.5, y + 0.5, 0)                         # FB bottom
    add_fp(f"R{rb+2}", x - 4.5, y + 6.5, 0)                         # feed-forward R
    add_fp(f"R{rb+3}", x - 4.5, y - 3.0, 0)                         # EN top
    add_fp(f"R{rb+4}", x - 4.5, y - 1.0, 0)                         # EN bottom
for u, rail, x, y, cb, rb, lref in BUCKS:
    place_buck(u, rail, x, y, cb, rb, lref)
add_fp("LED63", 100.0, 20.0, 0); add_fp("R32", 100.0, 23.0, 0)
add_fp("LED64", 130.0, 68.0, 0); add_fp("R33", 130.0, 71.0, 0)

# I2C: EEPROM beside J16, RTC + battery, second LM75 at the power section, pull-ups
add_fp("R34", 96.0, 60.0, 90); add_fp("R35", 98.0, 60.0, 90)
add_fp("U35", 100.0, 68.0, 0); add_fp("C60", 104.8, 68.0, 90)
add_fp("R36", 100.0, 74.5, 0); add_fp("JP1", 96.0, 74.5, 0)
add_fp("U31", 200.0, 50.0, 0); add_fp("C61", 204.8, 50.0, 90)
add_fp("BT1", 200.0, 34.0, 0)
add_fp("U33", 226.0, 30.0, 0); add_fp("C64", 230.8, 30.0, 90)
add_fp("U32", 9.0, LATCH_Y, 0); add_fp("C63", 9.0, LATCH_Y - 5.5, 0)

# thermostat and fan
add_fp("U34", 175.0, 30.0, 0); add_fp("C65", 179.8, 30.0, 90)
add_fp("RT1", 135.0, 42.0, 0)                        # NTC between the two bucks, the warmest spot
for k, ref in enumerate(("R37", "R38", "R39", "R40", "R41", "R42", "R43", "R45", "R48")):
    add_fp(ref, 165.0 + 2.4 * k, 38.0, 90)
add_fp("R44", 186.0, 24.0, 0); add_fp("LED65", 186.0, 20.5, 0)
add_fp("R49", 190.0, 24.0, 0)                        # OT pull-up, beside R44 and clear of BT1's 33 mm outline
add_fp("Q2", 176.0, 14.0, 0); add_fp("F2", 182.0, 14.0, 0); add_fp("D64", 164.0, 14.0, 0)

# 5V_DRV bulk, test points
add_fp("C28", 111.0, LATCH_Y, 90)
add_fp("C31", 88.0, BUF_Y, 90)
TPX = [120, 128, 136, 144, 152, 160, 168, 176, 184, 192, 200, 208]
TP_BUS = {7: ("LE0", 107.0), 8: ("LE1", 109.6), 9: ("LE2", 112.2), 10: ("D0", 114.8)}   # sit right on the bus
TP_POS = {}
for i, x in enumerate(TPX):
    if i + 1 in TP_BUS:
        TP_POS[i + 1] = (TP_BUS[i + 1][1], 87.2 + DY)
    elif i == 0:
        TP_POS[1] = (140.0, 58.0)                  # +12V: inside the In2 +12V area, so it simply stitches down
    else:
        TP_POS[i + 1] = (float(x), 80.0)
    add_fp(f"TP{i+1}", *TP_POS[i + 1])

# mounting holes
for i, (x, y) in enumerate([(4.0, 4.0), (306.0, 4.0), (4.0, H - 4.0), (306.0, H - 4.0), (105.0, H - 4.0), (205.0, H - 4.0)]):
    add_fp(f"H{i+1}", x, y)
# two more on the top edge, between the terminals and the fuse holder, which take screwdriver force
add_fp("H11", 152.0, 4.0); add_fp("H12", 212.0, 4.0)

missing = sorted(set(sym_fields) - set(fps) - {r for r in sym_fields if r.startswith("#")})
assert not missing, f"not placed: {missing}"

# =====================================================================================================
# outline, planes, keepouts
# =====================================================================================================
def edge(x1, y1, x2, y2):
    s = pcbnew.PCB_SHAPE(board); s.SetShape(pcbnew.SHAPE_T_SEGMENT)
    s.SetStart(MM(x1, y1)); s.SetEnd(MM(x2, y2)); s.SetLayer(pcbnew.Edge_Cuts); s.SetWidth(FromMM(0.1)); board.Add(s)
def arc(cx, cy, sx, sy, angle):
    s = pcbnew.PCB_SHAPE(board); s.SetShape(pcbnew.SHAPE_T_ARC)
    s.SetCenter(MM(cx, cy)); s.SetStart(MM(sx, sy))
    s.SetArcAngleAndEnd(pcbnew.EDA_ANGLE(angle, pcbnew.DEGREES_T), False)
    s.SetLayer(pcbnew.Edge_Cuts); s.SetWidth(FromMM(0.1)); board.Add(s)
R_ = 3.0
edge(R_, 0, W - R_, 0); edge(W - R_, H, R_, H); edge(0, H - R_, 0, R_); edge(W, R_, W, H - R_)
arc(W - R_, R_, W - R_, 0, 90); arc(W - R_, H - R_, W, H - R_, 90); arc(R_, H - R_, R_, H, 90); arc(R_, R_, 0, R_, 90)

def zone_poly(layer, pts, netname, name, connection=pcbnew.ZONE_CONNECTION_FULL, prio=0):
    z = pcbnew.ZONE(board); z.SetNet(net(netname)); z.SetLayer(layer)
    z.SetPadConnection(connection); z.SetLocalClearance(FromMM(0.3)); z.SetMinThickness(FromMM(0.25))
    z.SetThermalReliefGap(FromMM(0.35)); z.SetThermalReliefSpokeWidth(FromMM(0.5)); z.SetZoneName(name)
    z.SetIslandRemovalMode(pcbnew.ISLAND_REMOVAL_MODE_ALWAYS); z.SetAssignedPriority(prio)
    o = z.Outline(); o.NewOutline()
    for x, y in pts:
        o.Append(FromMM(x), FromMM(y))
    board.Add(z); return z
def rect(x1, y1, x2, y2):
    return [(x1, y1), (x2, y1), (x2, y2), (x1, y2)]

PWR12 = (95.0, 1.0, W - 1.0, 62.0)                  # In2 +12V region; everything else on In2 is 5V_DRV
zone_poly(I1, rect(0.8, 0.8, W - 0.8, H - 0.8), "GND", "GND_plane")
zone_poly(I2, rect(*PWR12), "+12V", "12V_plane", prio=1)
zone_poly(I2, rect(0.8, 0.8, W - 0.8, H - 0.8), "5V_DRV", "5VDRV_plane")
zone_poly(B, rect(0.8, 0.8, W - 0.8, H - 0.8), "GND", "GND_bottom")

def keepout(x, y, r, layers=(F, I1, I2, B), name=None):
    z = pcbnew.ZONE(board); z.SetIsRuleArea(True); z.SetZoneName(name or f"keepout_{x:.1f}_{y:.1f}")
    ls = pcbnew.LSET()
    for l in layers:
        ls.addLayer(l)
    z.SetLayerSet(ls)
    (getattr(z, "SetDoNotAllowZoneFills", None) or getattr(z, "SetDoNotAllowCopperPour"))(True)
    for meth, v in (("SetDoNotAllowTracks", True), ("SetDoNotAllowVias", True), ("SetDoNotAllowPads", False),
                    ("SetDoNotAllowFootprints", False)):
        if hasattr(z, meth):
            getattr(z, meth)(v)
    o = z.Outline(); o.NewOutline()
    for k in range(16):
        a = 2 * math.pi * k / 16
        o.Append(FromMM(x + r * math.cos(a)), FromMM(y + r * math.sin(a)))
    board.Add(z)
HOLES = [(fps[f"H{i}"].GetPosition().x / 1e6, fps[f"H{i}"].GetPosition().y / 1e6, 2.8 if 7 <= i <= 10 else 3.2)
         for i in range(1, 13)]
for x, y, r in HOLES:
    keepout(x, y, r)                                 # standoff / screw head + nut, no copper at all

if PLACE_ONLY:
    filler = pcbnew.ZONE_FILLER(board); filler.Fill(board.Zones())
    pcbnew.SaveBoard(os.path.join(HERE, f"{PROJECT}.kicad_pcb"), board)
    print("placement only: saved")
    sys.exit(0)

# =====================================================================================================
# routing, phase 1: the bus and its taps (drawn directly)
# =====================================================================================================
from grid_router import Grid
netid = {n: k + 1 for k, n in enumerate(sorted(set(padnet.values())))}
DONE = set()                          # (ref, pad) already connected by hand, so later stitching skips them
BUS_W = 0.2
DATA_X = (12.0, 302.0)
LINE_X = {n: DATA_X for n in BUS_ORDER}

def tap(netname, px, py, tx, y_line, y_bend1, y_bend2):
    """Pad -> straight to y_bend1 -> diagonal to (tx, y_bend2) -> straight to the bus line -> via."""
    trk(F, netname, [(px, py), (px, y_bend1), (tx, y_bend2), (tx, y_line)], 0.2)
    via(netname, tx, y_line)

def under_body_via(netname, ref, pins, vx, vy):
    for pn in pins:
        px, py = pad(ref, pn)
        trk(F, netname, [(px, py), (px, vy), (vx, vy)] if abs(px - vx) > 0.01 else [(px, py), (vx, vy)], 0.25)
        DONE.add((ref, str(pn)))
    via(netname, vx, vy)

# latches: inputs fan from 0.65 to 1.0 mm pitch on the way up to the bus
for b in range(3):
    for m in range(3):
        ref = f"U{16 + 3*b + m}"
        x0 = 100.0 * b + LATCH_DX[m]
        top = LATCH_Y - 2.87
        for i in range(8):
            pn = str(9 - i)
            px, py = pad(ref, pn)
            n = padnet[(ref, pn)]
            if n == "GND":
                continue
            tap(n, px, py, x0 + 1.0 * (i - 3.5), BUS_Y[n], top - 1.4, top - 2.9)
            DONE.add((ref, pn))
        # unused inputs of the third latch (bits 20-23 do not exist): tie them to pin 1 along the pad row
        unused = [str(9 - i) for i in range(8) if padnet[(ref, str(9 - i))] == "GND"]
        if unused:
            xs = [pad(ref, p)[0] for p in unused] + [pad(ref, "1")[0]]
            trk(F, "GND", [(min(xs), top), (max(xs), top)], 0.25)
            for p in unused:
                DONE.add((ref, p))
        # GND pins 1 and 10 (top corners) -> vias under the body; VCC pin 20 -> via under the body
        under_body_via("GND", ref, ["10"], x0 - 2.6, LATCH_Y)
        under_body_via("GND", ref, ["1"], x0 + 2.6, LATCH_Y - 0.9)
        p20 = pad(ref, "20")
        trk(F, "5V_DRV", [p20, (p20[0], LATCH_Y + 0.9), (x0 + 1.2, LATCH_Y + 0.9)], 0.25)
        via("5V_DRV", x0 + 1.2, LATCH_Y + 0.9); DONE.add((ref, "20"))
        c = f"C{4 + 3*b + m}"
        trk(F, "5V_DRV", [p20, pad(c, "1")], 0.3); DONE.add((c, "1"))
        # LE (pin 11, bottom-left): out to the left, up past the fan, into its own bus line
        le = padnet[(ref, "11")]
        p11 = pad(ref, "11")
        lx = x0 - 5.5
        trk(F, le, [p11, (p11[0] - 0.9, p11[1]), (lx, p11[1] - 1.6), (lx, BUS_Y[le])], 0.2)
        via(le, lx, BUS_Y[le]); DONE.add((ref, "11"))

# buffers: GND and VCC under the body, outputs fanned out to their series resistors, resistors into the bus
for u, x0 in enumerate(BUF_X):
    ref = f"U{25+u}"
    under_body_via("GND", ref, ["10"], x0 - 2.6, BUF_Y)
    under_body_via("GND", ref, ["1", "19"], x0 + 2.6, BUF_Y)
    p20 = pad(ref, "20")
    trk(F, "5V_DRV", [p20, (p20[0] + 1.2, p20[1]), (p20[0] + 1.2, p20[1] - 1.2)], 0.25)
    via("5V_DRV", p20[0] + 1.2, p20[1] - 1.2); DONE.add((ref, "20"))
    cref = f"C{1+u}"
    for t in range(8):
        pn = str(11 + t)
        n = padnet.get((ref, pn))
        if not n:
            continue
        name = n[2:]                                   # B_D3 -> D3
        rs = RS_OF[name][0]
        px, py = pad(ref, pn)
        r1 = pad(rs, "1"); r2 = pad(rs, "2")
        if r1[1] > r2[1]:
            r1, r2 = r2, r1                            # r1 = the upper pad
        trk(F, n, [(px, py), (px, py + 0.9), (r1[0], r1[1] - 0.8), r1], 0.2)
        DONE.add((ref, pn))
        up = padnet[(rs, "1")] if pad(rs, "1") == r1 else padnet[(rs, "2")]
        assert up == n, (rs, up, n)
        bus_net = name
        trk(F, bus_net, [r2, (r2[0], BUS_Y[bus_net])], 0.2)
        via(bus_net, r2[0], BUS_Y[bus_net])
        DONE.add((rs, "1")); DONE.add((rs, "2"))
# the bus lines themselves, each from its first tap to its last (test points included)
for n in BUS_ORDER:
    xs = [geo[0] for kind, _, nm, geo, _ in made if kind == "v" and nm == n and abs(geo[1] - BUS_Y[n]) < 0.01]
    xs += [tx for ln, tx in TP_BUS.values() if ln == n]
    trk(B, n, [(min(xs), BUS_Y[n]), (max(xs), BUS_Y[n])], BUS_W)
print("phase 1: bus and taps drawn", flush=True)

# =====================================================================================================
# power copper: pours drawn as zones. Each one is also handed to the maze router as area OWNED by its
# net, so same-net routes may enter it (to reach a pad inside) and nothing else may cross it.
# =====================================================================================================
POURS = []           # (layer, rect, net) - rectangles, as the router sees them; a zone may be several
def pour(layer, x1, y1, x2, y2, netname, prio=5, draw=True):
    if draw:
        zone_poly(layer, rect(x1, y1, x2, y2), netname, f"pour_{netname}_{x1:.0f}_{y1:.0f}", prio=prio)
    POURS.append((layer, (x1, y1, x2, y2), netname))
def pour_poly(layer, pts, netname, rects, prio=5):
    """One zone for an L-shaped pour (two overlapping same-net zones are a DRC error), plus its rectangles
    for the router."""
    zone_poly(layer, pts, netname, f"pour_{netname}_{pts[0][0]:.0f}_{pts[0][1]:.0f}", prio=prio)
    for r_ in rects:
        pour(layer, *r_, netname, draw=False)
def in_pour(layer, x, y, netname):
    return any(L == layer and n == netname and r[0] <= x <= r[2] and r[1] <= y <= r[3] for L, r, n in POURS)

def buck_copper(u, rail, x, y, cb, rb, lref):
    sw, bt = f"SW_{rail}", f"BT_{rail}"
    pour_poly(F, [(x + 0.8, y - 2.4), (x + 9.8, y - 2.4), (x + 9.8, y + 3.0), (x + 5.7, y + 3.0), (x + 5.7, y + 0.2),
                  (x + 0.8, y + 0.2)], "+12V",                              # VIN: bar, fingers, cap pin 1s
              [(x + 0.8, y - 2.4, x + 9.8, y + 0.2), (x + 5.7, y + 0.2, x + 9.8, y + 3.0)])
    pour_poly(F, [(x - 0.75, y - 6.2), (x + 9.8, y - 6.2), (x + 9.8, y - 2.9), (x + 0.35, y - 2.9), (x + 0.35, y + 0.6),
                  (x - 0.75, y + 0.6)], "GND",                              # cap pin 2s and down the PGND bar
              [(x - 0.75, y - 6.2, x + 9.8, y - 2.9), (x - 0.75, y - 2.9, x + 0.35, y + 0.6)])
    pour(F, x - 0.3, y + 0.6, x + 5.2, y + 7.4, sw)                       # SW: pad -> inductor pin 1
    pour(F, x + 1.0, y + 11.3, x + 17.0, y + 15.0, rail)                  # output: inductor pin 2 -> caps
    for vx in (x - 0.2, x + 2.0, x + 4.4, x + 6.8, x + 9.2):
        via("GND", vx, y - 5.3)
    for vx, vy in ((x + 6.6, y + 1.2), (x + 8.8, y + 1.2), (x + 6.6, y + 2.3), (x + 8.8, y + 2.3), (x + 9.3, y - 0.4)):
        via("+12V", vx, vy)
    # BOOT pad -> bootstrap cap pin 1, a short hand track (it sits inside the SW pour, which clears it)
    trk(F, bt, [pad(u, "7"), (pad(u, "7")[0] + 0.3, pad(f"C{cb}", "1")[1]), pad(f"C{cb}", "1")], 0.25)
    DONE.add((u, "7")); DONE.add((f"C{cb}", "1")); DONE.add((f"C{cb}", "2"))
    # AGND (pin 3) joins the PGND pour on a short stub
    p3 = pad(u, "3")
    trk(F, "GND", [p3, (x - 0.6, p3[1])], 0.25); DONE.add((u, "3"))
    for ref in [u, lref] + [f"C{cb+k}" for k in range(1, 8)]:
        for p in fps[ref].Pads():
            px_, py_ = p.GetPosition().x / 1e6, p.GetPosition().y / 1e6
            if p.GetAttribute() == pcbnew.PAD_ATTRIB_SMD and in_pour(F, px_, py_, nn(p)):
                DONE.add((ref, p.GetNumber()))

for args in BUCKS:
    buck_copper(*args)
# U29's output pour sits in the 5V_DRV half of In2: stitch it down
for vx in (122.0, 124.5, 127.0):
    via("5V_DRV", vx, 57.0 + 12.4)
# 5V_PI: U28's output pour -> a 3 mm B.Cu strip -> the USB-C receptacle's two VBUS pads
X5 = 98.2
pour_poly(B, [(X5 - 1.5, 8.2), (104.3, 8.2), (104.3, 10.3), (X5 + 1.5, 10.3), (X5 + 1.5, 41.0), (124.0, 41.0),
              (124.0, 44.2), (X5 - 1.5, 44.2)], "5V_PI",
          [(X5 - 1.5, 8.2, X5 + 1.5, 44.2), (X5 - 1.5, 8.2, 104.3, 10.3), (X5 - 1.5, 41.0, 124.0, 44.2)])
for vx, vy in ((116.0, 42.6), (118.5, 42.6), (121.0, 42.6)):
    via("5V_PI", vx, vy)
# Each VBUS pad drops straight down on a stub as wide as the pad to two vias into the B.Cu pour: four vias for
# up to 3 A, where the first layout had one (independent review, 2026-09-25). The CC lines run down between
# the two stubs. Each GND pad gets its own 0.5 mm stub and via; the four shell pads are plated GND too.
a9, b9 = pad("J18", "A9"), pad("J18", "B9")
for p_ in (a9, b9):
    trk(F, "5V_PI", [p_, (p_[0], 9.7)], 0.7)
    via("5V_PI", p_[0], 8.7); via("5V_PI", p_[0], 9.7)
for gp in ("A12", "B12"):
    q_ = pad("J18", gp)
    trk(F, "GND", [q_, (q_[0], 8.0)], 0.5); via("GND", q_[0], 8.0)
DONE.update({("J18", "A9"), ("J18", "B9"), ("J18", "A12"), ("J18", "B12")})

# 12 V input chain: pours on F.Cu, J17 -> F1 -> Q1 -> R2
pour(F, 280.5, 1.2, 305.0, 17.5, "12VIN")
pour(F, 260.7, 8.0, 271.0, 17.5, "12VF")
pour_poly(F, [(246.9, 8.0), (257.4, 8.0), (257.4, 16.6), (250.2, 16.6), (250.2, 27.8), (246.9, 27.8)], "12VP",
          [(246.9, 8.0, 257.4, 16.6), (246.9, 16.6, 250.2, 27.8)])
# the TVS's return: a GND pour under its anode with four vias to In1, not a 0.25 mm stub
pour(F, 253.9, 23.6, 258.8, 29.8, "GND")
for vx in (254.5, 255.7, 256.9, 258.1):
    via("GND", vx, 29.0)
DONE.add(("D62", "2"))
pour(F, 237.5, 9.5, 243.2, 16.6, "+12V")
for vx, vy in ((238.5, 10.5), (240.0, 10.5), (238.5, 15.6), (240.0, 15.6), (242.2, 10.5), (242.2, 15.6)):
    via("+12V", vx, vy)
# the gate: Q1 pin 4 -> R1 (pull-down) and D61's anode
g = pad("Q1", "4")
trk(F, "QG", [g, (g[0], 18.9), (pad("D61", "2")[0], 18.9), pad("D61", "2")], 0.3)
trk(F, "QG", [(g[0], 18.9), (pad("R1", "1")[0], 18.9), pad("R1", "1")], 0.3)
for k in (("Q1", "4"), ("D61", "2"), ("R1", "1")):
    DONE.add(k)
k1 = pad("D61", "1"); k62 = pad("D62", "1")
trk(F, "12VP", [k1, (k1[0] - 2.0, k1[1]), (k1[0] - 2.0, 16.3)], 0.5)
DONE.update({("D61", "1"), ("D62", "1")})
for ref, pins in (("J17", "1"), ("F1", "1234"), ("Q1", "1235678"), ("R2", "12")):
    for pn in pins:
        DONE.add((ref, pn))
# INA226 GND (pin 7) is boxed in by VS and VBUS; it and the two address pins (A0, A1 = GND) meet at one via
# under the body
ux, uy = fps["U30"].GetPosition().x / 1e6, fps["U30"].GetPosition().y / 1e6
p7, p1, p2 = pad("U30", "7"), pad("U30", "1"), pad("U30", "2")
trk(F, "GND", [p7, (p7[0], uy - 0.9), (ux, uy)], 0.25)
trk(F, "GND", [p2, (p2[0], uy + 0.9), (ux, uy)], 0.25)
trk(F, "GND", [p1, p2], 0.25)
via("GND", ux, uy)
DONE.update({("U30", "7"), ("U30", "1"), ("U30", "2")})
# test points on the bus: a straight F.Cu drop onto the line's own via
for k, (ln, tx) in TP_BUS.items():
    trk(F, ln, [(tx, 87.2 + DY), (tx, BUS_Y[ln])], 0.25); via(ln, tx, BUS_Y[ln]); DONE.add((f"TP{k}", "1"))
# INA226: VBUS and IN- (+12V, pins 8/9) up-left into the +12V pour; IN+ (12VP, pin 10) down through a via
# and across on B.Cu to the 12VP pour, because on F.Cu the two would have to cross
p8, p9, p10 = pad("U30", "8"), pad("U30", "9"), pad("U30", "10")
trk(F, "+12V", [p8, p9], 0.25)
trk(F, "+12V", [p9, (p9[0], 17.4), (242.5, 15.4), (242.5, 14.3)], 0.3)   # IN- / VBUS onto R2's +12V pad
trk(F, "12VP", [p10, (p10[0], 20.5)], 0.25); via("12VP", p10[0], 20.5)
trk(B, "12VP", [(p10[0], 20.5), (p10[0] - 0.8, 21.3), (p10[0] - 0.8, 25.4), (246.3, 25.4), (246.3, 15.6),
                 (247.5, 15.6)], 0.3)
via("12VP", 247.5, 15.6)                                 # IN+ comes up right under R2's 12VP pad (Kelvin)
trk(F, "12VP", [(247.5, 15.6), (247.5, 14.3)], 0.3)
# SDA / SCL out of the 0.5 mm pin row and clear of D62: SDA ends high, SCL passes under it, so they never cross
q4, q5 = pad("U30", "4"), pad("U30", "5")
trk(F, "SDA", [q4, (q4[0], q4[1] + 1.9), (q4[0] - 1.3, q4[1] + 3.2), (q4[0] - 1.3, q4[1] + 5.9)], 0.2)
trk(F, "SCL", [q5, (q5[0], q5[1] + 7.4), (q5[0] - 2.4, q5[1] + 7.4)], 0.2)
DONE.update({("U30", "8"), ("U30", "9"), ("U30", "10"), ("U30", "4"), ("U30", "5")})
# the REVERSED indicator's resistor into the 12VIN pour, round the right of LED61 / D63
r4 = pad("R4", "2")
trk(F, "12VIN", [r4, (295.6, r4[1]), (295.6, 17.0)], 0.3); DONE.add(("R4", "2"))
# buck EN and FB: short stubs out of the 0.5 mm pin field, EN up and FB down, before any router runs
for u, rail, x, y, cbase, rb, lref in BUCKS:
    pe, pf = pad(u, "1"), pad(u, "2")
    trk(F, f"EN_{rail}", [pe, (x - 2.5, pe[1]), (x - 2.5, y - 1.9)], 0.2)
    trk(F, f"FB_{rail}", [pf, (x - 2.1, pf[1]), (x - 2.1, y + 1.7)], 0.2)
    DONE.update({(u, "1"), (u, "2")})
print("power pours drawn", flush=True)

# =====================================================================================================
# routing, phase 2: one jack cell on its own grid, copied fifteen times
# =====================================================================================================
def new_grid(refs, walls=()):
    Gx = Grid(board, {r: fps[r] for r in refs}, padnet, netid, trk, via, W, H, F, B)
    for L, (x1, y1, x2, y2), n in POURS:           # pours first, so pads and their clearance rings win
        Gx.mark_force(L, x1, y1, x2, y2, netid[n])
    Gx.add_pads()
    for L, x1, y1, x2, y2 in walls:
        Gx.add_zone_keepout(L, x1, y1, x2, y2)
    for hx, hy, r in HOLES:
        for L in (F, B):
            Gx.add_zone_keepout(L, hx - r, hy - r, hx + r, hy + r)
    for fp in fps.values():                         # every bare hole on the board is a hard obstacle
        for p in fp.Pads():
            if p.GetAttribute() == pcbnew.PAD_ATTRIB_NPTH:
                bb = p.GetBoundingBox()
                for L in (F, B):
                    Gx.add_zone_keepout(L, bb.GetLeft() / 1e6, bb.GetTop() / 1e6, bb.GetRight() / 1e6, bb.GetBottom() / 1e6)
    Gx.add_existing_tracks()
    return Gx

def smd_power_pads(refs, nets_):
    out = []
    for ref in refs:
        for p in fps[ref].Pads():
            if (nn(p) in nets_ and p.GetAttribute() == pcbnew.PAD_ATTRIB_SMD
                    and (ref, p.GetNumber()) not in DONE):
                out.append((ref, p.GetNumber(), nn(p)))
    return out

def cell_refs(j):
    return ([f"J{j+1}", f"U{j+1}", f"C{13+j}"] + [f"LED{4*j+p+1}" for p in range(4)]
            + [f"RA{4*j+p+1}" for p in range(4)] + [f"D{4*j+p+1}" for p in range(4)])

def rename(n, dj=0, db=0):
    m = re.match(r"^(O|LA)(\d+)$", n)
    if m:
        return f"{m.group(1)}{int(m.group(2)) + 4 * dj + 20 * db}"
    m = re.match(r"^J(\d+)_(\d)([PN])$", n)
    if m:
        return f"J{int(m.group(1)) + dj}_{m.group(2)}{m.group(3)}"
    return n

def copy_items(items, dx, fn):
    for kind, layer, n, geo, w in items:
        if kind == "t":
            (x1, y1), (x2, y2) = geo
            trk(layer, fn(n), [(x1 + dx, y1), (x2 + dx, y2)], w)
        else:
            via(fn(n), geo[0] + dx, geo[1], d=w)

xc0 = JX[0]
CELL_TOP = 118.0 + DY
# The cell is routed on a scratch board of its own, 20 mm wide, with the router on a 0.1 mm grid and a
# 0.25 mm clearance (DRC is 0.2). On the full board the same router has to run at 0.3 mm / 0.38 mm to fit in
# memory, and at that resolution it cannot use the gaps between 1.27 mm SOIC pins or between the RJ45's
# 1.02 mm rows - which is exactly where a jack cell has to route. The board edges of the scratch board are
# the cell's own walls, so whatever is routed there can be copied into every column without collisions.
CX0, CY0 = xc0 - 10.0, 114.0 + DY                      # scratch origin in board coordinates
CW, CH_ = 20.0, 56.0
cb = pcbnew.BOARD(); cb.SetCopperLayerCount(4)
cnets = {}
def cnet(name):
    name = FULL.get(name, name)
    if name not in cnets:
        n = pcbnew.NETINFO_ITEM(cb, name); cb.Add(n); cnets[name] = n
    return cnets[name]
cfps = {}
for ref in cell_refs(0):
    src = fps[ref]
    lib, name = sym_fields[ref]["Footprint"].split(":")
    path = {"jlc": os.path.join(HERE, "jlc", "jlc.pretty")}.get(lib, os.path.join(FPLIB, lib + ".pretty"))
    fp = pcbnew.FootprintLoad(path, name); fp.SetReference(ref); cb.Add(fp)
    fp.SetOrientationDegrees(src.GetOrientationDegrees())
    fp.SetPosition(MM(src.GetPosition().x / 1e6 - CX0, src.GetPosition().y / 1e6 - CY0))
    for p in fp.Pads():
        n = padnet.get((ref, p.GetNumber()))
        if n:
            p.SetNet(cnet(n))
    cfps[ref] = fp
cell_items = []
def ctrk(layer, netname, pts, w=0.25):
    for (x1, y1), (x2, y2) in zip(pts, pts[1:]):
        if abs(x1 - x2) > 1e-6 or abs(y1 - y2) > 1e-6:
            cell_items.append(("t", layer, netname, ((x1 + CX0, y1 + CY0), (x2 + CX0, y2 + CY0)), w))
def cvia(netname, x, y, d=0.7, drill=0.3):
    cell_items.append(("v", None, netname, (x + CX0, y + CY0), d))
Gc = Grid(cb, cfps, padnet, netid, ctrk, cvia, CW, CH_, F, B, grid=0.1, clear=0.3, via_extra=0.25)
Gc.add_pads()
for p in cfps[f"J1"].Pads():                      # the jack's locating pegs
    if p.GetAttribute() == pcbnew.PAD_ATTRIB_NPTH:
        bb = p.GetBoundingBox()
        for L in (F, B):
            Gc.add_zone_keepout(L, bb.GetLeft() / 1e6, bb.GetTop() / 1e6, bb.GetRight() / 1e6, bb.GetBottom() / 1e6)
# keep the top 4 mm (y < 118 on the board) free: that is where the bank routing brings the latch outputs in
for L in (F, B):
    Gc.add_zone_keepout(L, 0.0, 0.0, CW, CELL_TOP - CY0 - 0.4)
# Most constrained first: inputs and pairs, then the plane stitches, then the LED returns, which have room.
CELL_NETS = ["O1", "J1_1P", "J1_1N", "O4", "J1_4P", "J1_4N", "O2", "J1_2P", "J1_2N", "O3", "J1_3P", "J1_3N"]
fail = 0
for n in CELL_NETS:
    ok = Gc.route(n, 0.2)
    fail += 0 if ok else 1
    if not ok:
        print("  cell route FAILED", n)
for ref, pn, n in smd_power_pads(cell_refs(0), ("GND", "5V_DRV")):
    if not Gc.stitch(ref, pn, n):
        fail += 1
        print("  cell stitch failed", ref, pn, n)
for n in ["LA1", "LA2", "LA3", "LA4"]:
    ok = Gc.route(n, 0.2)
    fail += 0 if ok else 1
    if not ok:
        print("  cell route FAILED", n)
del Gc, cb
print(f"phase 2: cell routed ({len(cell_items)} items, {fail} failures)", flush=True)
for j in range(15):
    copy_items(cell_items, JX[j] - xc0, lambda n, j=j: rename(n, dj=j))
if "--cell-only" in sys.argv:
    filler = pcbnew.ZONE_FILLER(board); filler.Fill(board.Zones())
    pcbnew.SaveBoard(os.path.join(HERE, f"{PROJECT}.kicad_pcb"), board); sys.exit(0)

# =====================================================================================================
# windowed routing: the fine-grid router, on a scratch board holding just one rectangle of this board
# =====================================================================================================
def pad_extent(fp_):
    bbs = [p.GetBoundingBox() for p in fp_.Pads()]
    if not bbs:
        return None
    return (min(b.GetLeft() for b in bbs) / 1e6, min(b.GetTop() for b in bbs) / 1e6,
            max(b.GetRight() for b in bbs) / 1e6, max(b.GetBottom() for b in bbs) / 1e6)

def plane_for(net_, x, y):
    in12 = PWR12[0] <= x <= PWR12[2] and PWR12[1] <= y <= PWR12[3]
    if net_ == "GND":
        return True
    if net_ == "+12V":
        return in12
    if net_ == "5V_DRV":
        return not in12
    return False

def _route_window(x0, y0, x1, y1, nets_, stitch_nets, grid, clear, skip_refs):
    """Route nets_ inside the rectangle on a scratch board at a fine grid; the rectangle's edges are walls.
    Footprints wholly inside take part; footprints straddling the edge contribute their inside pads as
    obstacles. Existing copper and the power pours inside the window are carried over. Returns the new
    copper (board coordinates) and adds it to the board unless apply is False."""
    sb = pcbnew.BOARD(); sb.SetCopperLayerCount(4)
    snets = {}
    def snet(name):
        name = FULL.get(name, name)
        if name not in snets:
            n = pcbnew.NETINFO_ITEM(sb, name); sb.Add(n); snets[name] = n
        return snets[name]
    sfps, border = {}, []
    for ref, src in fps.items():
        e = pad_extent(src)
        if e is None or e[2] < x0 or e[0] > x1 or e[3] < y0 or e[1] > y1:
            continue
        if e[0] >= x0 + 0.4 and e[2] <= x1 - 0.4 and e[1] >= y0 + 0.4 and e[3] <= y1 - 0.4 and ref not in skip_refs:
            lib, name = sym_fields[ref]["Footprint"].split(":")
            path = {"jlc": os.path.join(HERE, "jlc", "jlc.pretty"),
                    PROJECT: os.path.join(HERE, f"{PROJECT}.pretty")}.get(lib, os.path.join(FPLIB, lib + ".pretty"))
            fp_ = pcbnew.FootprintLoad(path, name); fp_.SetReference(ref); sb.Add(fp_)
            fp_.SetOrientationDegrees(src.GetOrientationDegrees())
            fp_.SetPosition(MM(src.GetPosition().x / 1e6 - x0, src.GetPosition().y / 1e6 - y0))
            for p in fp_.Pads():
                n = padnet.get((ref, p.GetNumber()))
                if n and (ref, p.GetNumber()) not in DONE:     # connected by hand already: an obstacle, not a target
                    p.SetNet(snet(n))
                elif p.GetAttribute() == pcbnew.PAD_ATTRIB_PTH and p.GetNumber() == "":
                    p.SetAttribute(pcbnew.PAD_ATTRIB_NPTH)
            sfps[ref] = fp_
        else:
            for p in src.Pads():
                bb = p.GetBoundingBox()
                border.append((bb.GetLeft() / 1e6 - x0, bb.GetTop() / 1e6 - y0, bb.GetRight() / 1e6 - x0, bb.GetBottom() / 1e6 - y0))
    for t in board.GetTracks():
        if isinstance(t, pcbnew.PCB_VIA):
            px, py = t.GetPosition().x / 1e6, t.GetPosition().y / 1e6
            if x0 - 1 <= px <= x1 + 1 and y0 - 1 <= py <= y1 + 1:
                v = pcbnew.PCB_VIA(sb); v.SetPosition(MM(px - x0, py - y0)); v.SetWidth(FromMM(0.7))
                v.SetDrill(t.GetDrillValue()); v.SetNet(snet(nn(t))); v.SetLayerPair(F, B); sb.Add(v)
        else:
            s, e_ = t.GetStart(), t.GetEnd()
            sx, sy, ex, ey = s.x / 1e6, s.y / 1e6, e_.x / 1e6, e_.y / 1e6
            if max(sx, ex) < x0 - 1 or min(sx, ex) > x1 + 1 or max(sy, ey) < y0 - 1 or min(sy, ey) > y1 + 1:
                continue
            tt = pcbnew.PCB_TRACK(sb); tt.SetStart(MM(sx - x0, sy - y0)); tt.SetEnd(MM(ex - x0, ey - y0))
            tt.SetWidth(t.GetWidth()); tt.SetLayer(t.GetLayer()); tt.SetNet(snet(nn(t))); sb.Add(tt)
    items = []
    def rtrk(layer, netname, pts, w=0.25):
        for (a1, b1), (a2, b2) in zip(pts, pts[1:]):
            if abs(a1 - a2) > 1e-6 or abs(b1 - b2) > 1e-6:
                items.append(("t", layer, netname, ((a1 + x0, b1 + y0), (a2 + x0, b2 + y0)), w))
    def rvia(netname, x, y, d=0.7, drill=0.3):
        items.append(("v", None, netname, (x + x0, y + y0), d))
    Gw = Grid(sb, sfps, padnet, netid, rtrk, rvia, x1 - x0, y1 - y0, F, B, grid=grid, clear=clear, via_extra=0.25)
    for L, (a1, b1, a2, b2), n in POURS:                   # pours first, so pads and their rings win
        if a2 >= x0 and a1 <= x1 and b2 >= y0 and b1 <= y1:
            Gw.mark_force(L, a1 - x0, b1 - y0, a2 - x0, b2 - y0, netid[n])
    Gw.add_pads()
    # a window may reach past the board edge (so parts hard against it are wholly inside); nothing may be
    # routed out there, nor within the 0.5 mm copper-to-edge band
    for L in (F, B):
        if y0 < 0.5:
            Gw.add_zone_keepout(L, 0.0, 0.0, x1 - x0, 0.5 - y0 - 0.4)
        if x1 > W - 0.5:
            Gw.add_zone_keepout(L, W - 0.5 - x0 + 0.4, 0.0, x1 - x0, y1 - y0)
    for a1, b1, a2, b2 in border:
        for L in (F, B):
            Gw.add_zone_keepout(L, a1, b1, a2, b2)
    for hx, hy, r in HOLES:
        if x0 - r <= hx <= x1 + r and y0 - r <= hy <= y1 + r:
            for L in (F, B):
                Gw.add_zone_keepout(L, hx - r - x0, hy - r - y0, hx + r - x0, hy + r - y0)
    for fp_ in sfps.values():
        for p in fp_.Pads():
            if p.GetAttribute() == pcbnew.PAD_ATTRIB_NPTH:
                bb = p.GetBoundingBox()
                for L in (F, B):
                    Gw.add_zone_keepout(L, bb.GetLeft() / 1e6, bb.GetTop() / 1e6, bb.GetRight() / 1e6, bb.GetBottom() / 1e6)
    Gw.add_existing_tracks()
    fails, stitched = [], set()
    # signals first, then the plane stitches (a stitch via dropped early sits exactly where a boxed-in
    # pin needs to escape), then the supply nets, which can reach any stitch or pour of their own
    for n in [n for n in nets_ if n not in PWR]:
        if not Gw.route(n, 0.2):
            fails.append(n)
    for ref in sorted(sfps):
        for p in sfps[ref].Pads():
            n = nn(p)
            if (n in stitch_nets and p.GetAttribute() == pcbnew.PAD_ATTRIB_SMD and (ref, p.GetNumber()) not in DONE):
                ax, ay = p.GetPosition().x / 1e6 + x0, p.GetPosition().y / 1e6 + y0
                if in_pour(F, ax, ay, n) or not plane_for(n, ax, ay):
                    continue
                if Gw.stitch(ref, p.GetNumber(), n):
                    stitched.add((ref, p.GetNumber()))
                else:
                    fails.append(f"stitch {ref}.{p.GetNumber()}")
    for n in [n for n in nets_ if n in PWR]:
        if not Gw.route(n, 0.2):
            fails.append(n)
    del Gw, sb
    return items, fails, stitched

def route_window(x0, y0, x1, y1, nets_, stitch_nets=(), grid=0.1, clear=0.3, label="", apply=True, skip_refs=(),
                 attempts=5):
    """_route_window, retried when nets fail: a failed net is usually one walled in by nets routed before it,
    so each retry starts from scratch with EVERY net that has failed so far moved to the front, in the order
    they failed (moving only the latest failures forward just trades one walled-in net for another). Nothing
    from a failed attempt is kept - neither its copper nor its stitched pads."""
    order, first = list(nets_), []
    for k in range(attempts):
        items, fails, stitched = _route_window(x0, y0, x1, y1, order, stitch_nets, grid, clear, skip_refs)
        failed_nets = [n for n in fails if not n.startswith("stitch ")]
        if not failed_nets or k == attempts - 1:
            break
        first += [n for n in failed_nets if n not in first]
        print(f"window {label}: attempt {k + 1} failed {failed_nets}, retrying with {first} first", flush=True)
        order = first + [n for n in nets_ if n not in first]
    DONE.update(stitched)
    if apply:
        copy_items(items, 0.0, lambda n: n)
    print(f"window {label}: {len(items)} items, failures: {fails}", flush=True)
    return items, fails

PWR = ("GND", "+12V", "5V_DRV")
ALL_FAILS = []
def RW(*a, **k):
    items, f = route_window(*a, **k)
    ALL_FAILS.extend(f)
    return items

# =====================================================================================================
# routing, phase 3: bank 1's latch outputs to its five jack cells, copied to banks 2 and 3
# =====================================================================================================
# each latch's outputs from the outside in, so an inner pin never walls in an outer one
BANK_ORDER = [4, 3, 2, 1, 5, 6, 7, 8, 12, 11, 10, 9, 13, 14, 15, 16, 20, 19, 18, 17]
bank_items, f = route_window(0.5, 104.9 + DY, 104.5, 122.8 + DY, [f"O{k}" for k in BANK_ORDER], stitch_nets=("GND",),
                             label="bank 1", apply=False, skip_refs=("U32", "C63"))
ALL_FAILS.extend(f)
# the stitch vias of U32 / C63 / C30 are bank-1-only parts; they were skipped above, so the copies are clean
for b in (0, 1, 2):
    copy_items(bank_items, 100.0 * b, lambda n, b=b: rename(n, db=b))
for ref in [f"U{16+k}" for k in range(9)] + [f"C{4+k}" for k in range(9)]:
    for p in fps[ref].Pads():
        DONE.add((ref, p.GetNumber()))
for j in range(15):
    for ref in cell_refs(j):
        for p in fps[ref].Pads():
            DONE.add((ref, p.GetNumber()))

# =====================================================================================================
# routing, phase 4: the rest, window by window, then the long runs on the coarse full-board grid
# =====================================================================================================
PWR = ("GND", "+12V", "5V_DRV")
PI_NETS = [f"PI_{n}" for n in LINES]
# the Pi lines from both ends of the header towards the middle, then I2C (short, leaving to the right)
_by_slot = sorted(LINES, key=lambda n: SLOT[n])
_pi_order = []
while _by_slot:
    _pi_order.append(_by_slot.pop(0))
    if _by_slot:
        _pi_order.append(_by_slot.pop())
_pi_order.remove("D19"); _pi_order.insert(0, "D19")      # the middle column's line: last in, walled off
RW(26.0, 40.0, 105.8, 89.9 + DY, [f"PI_{n}" for n in _pi_order] + ["SDA", "SCL", "P3V3", "WP"], stitch_nets=PWR,
   label="Pi header + buffers")
RW(0.5, 104.9 + DY, 24.0, 118.0 + DY, ["P3V3", "SDA", "SCL"], stitch_nets=PWR, label="U32")
for u, rail, x, y, cbase, rb, lref in BUCKS:
    RW(97.8, y - 9.5, 133.0, y + 19.2, [f"EN_{rail}", f"FB_{rail}", f"FF_{rail}", "LKPI", "LKDRV", rail, "+12V"],
      stitch_nets=PWR, label=f"buck {u}")
# windows along the top edge start past it: a part hard against the edge must still be wholly inside the
# window, or the window treats it as an obstacle and its nets silently go unrouted
RW(94.0, -1.0, 150.0, 19.0, ["CC1", "CC2", "5V_PI", "AUD_L_IN", "AUD_R_IN", "AUD_L", "AUD_R"], stitch_nets=PWR,
  label="USB-C + audio")
RW(133.0, -1.0, 311.0, 61.5, ["12VIN", "12VP", "RPA", "RPK", "LK12", "VBAT", "P3V3", "SDA", "SCL",
                            "NTC", "VREFAN", "VREFOT", "FANDRV", "OT", "OTA", "FAN12", "FANM", "5V_DRV", "+12V"],
  stitch_nets=PWR, grid=0.15, clear=0.35, label="power section")

G = new_grid(list(fps))
G.skip = DONE
nstitch = 0
for ref in sorted(fps):
    for p in fps[ref].Pads():
        n = nn(p)
        if n not in PWR or p.GetAttribute() != pcbnew.PAD_ATTRIB_SMD or (ref, p.GetNumber()) in DONE:
            continue
        x, y = p.GetPosition().x / 1e6, p.GetPosition().y / 1e6
        if in_pour(F, x, y, n) or not plane_for(n, x, y):
            continue
        if G.stitch(ref, p.GetNumber(), n):
            nstitch += 1
        else:
            print("  stitch failed", ref, p.GetNumber(), n)
print(f"phase 4: {nstitch} leftover plane stitches", flush=True)
GLOBAL = ["P3V3", "SDA", "SCL", "5V_DRV", "5V_PI"]
fail = []
for n in GLOBAL:
    # split_pre: pieces routed in different windows are separate islands until this pass joins them
    if not G.route(n, 0.25, split_pre=True):
        fail.append(n)
print("window failures:", ALL_FAILS, flush=True)
print("global failures:", fail, flush=True)

# =====================================================================================================
# silkscreen
# =====================================================================================================
CC = pcbnew.GR_TEXT_H_ALIGN_CENTER
def text(s, x, y, size=1.0, layer=pcbnew.F_SilkS, angle=0, just=CC, bold=False):
    tx = pcbnew.PCB_TEXT(board); tx.SetText(s); tx.SetPosition(MM(x, y)); tx.SetLayer(layer)
    size = max(size, 1.0)                      # JLCPCB will not reliably print below 1.0 mm
    tx.SetTextSize(MM(size, size)); tx.SetTextThickness(FromMM(max(size * (0.2 if bold else 0.15), 0.15)))
    tx.SetTextAngleDegrees(angle); tx.SetHorizJustify(just)
    board.Add(tx); return tx
def line(x1, y1, x2, y2, w=0.15, layer=pcbnew.F_SilkS):
    s = pcbnew.PCB_SHAPE(board); s.SetShape(pcbnew.SHAPE_T_SEGMENT)
    s.SetStart(MM(x1, y1)); s.SetEnd(MM(x2, y2)); s.SetLayer(layer); s.SetWidth(FromMM(w)); board.Add(s)
def box(x1, y1, x2, y2, w=0.15):
    for a, b_ in (((x1, y1), (x2, y1)), ((x2, y1), (x2, y2)), ((x2, y2), (x1, y2)), ((x1, y2), (x1, y1))):
        line(*a, *b_, w=w)

# Passives, LEDs, ESD arrays and the jack bodies lose their silk outlines: at this density they only
# collide with each other and with the labels, and they tell an assembler nothing the labels below do not.
KEEP_OUTLINE = {f"U{k}" for k in range(1, 36)} | {"J16", "J20", "J21", "F1", "BT1", "Q1", "Q2"}
for ref, fp_ in fps.items():
    if ref not in KEEP_OUTLINE:
        for g in list(fp_.GraphicalItems()):
            if g.GetLayer() == pcbnew.F_SilkS:
                fp_.Remove(g)
    fp_.Value().SetVisible(False)
    r = fp_.Reference()
    show = ref in ("J18", "F1", "BT1", "JP1") or (ref.startswith("U") and int(ref[1:]) >= 28)
    r.SetVisible(show)
    if show:
        r.SetTextSize(MM(1.0, 1.0)); r.SetTextThickness(FromMM(0.15))

fps["U30"].Reference().SetPosition(MM(245.0, 26.0))    # clear of R2, which sits over the default spot
fps["U32"].Reference().SetPosition(MM(9.0, LATCH_Y + 4.4))
fps["U35"].Reference().SetPosition(MM(100.0, 63.3))

# Polarity marks. The outlines above are stripped, and the two LED parts number their pads opposite ways
# (green: pad 1 = anode; red: pad 1 = cathode), so without a mark a part turned 180 degrees would not show in
# the JLCPCB preview or on inspection. A short bar just past the cathode pad, and a "+" by C32's positive pad.
def pad_mark(ref, pn, plus=False):
    fp_ = fps[ref]; c = fp_.GetPosition(); p_ = fp_.FindPadByNumber(pn)
    px, py = p_.GetPosition().x / 1e6, p_.GetPosition().y / 1e6
    ux, uy = px - c.x / 1e6, py - c.y / 1e6
    n = (ux * ux + uy * uy) ** 0.5; ux, uy = ux / n, uy / n
    bb = p_.GetBoundingBox(); bw, bh = bb.GetWidth() / 1e6, bb.GetHeight() / 1e6
    along = (bw if abs(ux) > abs(uy) else bh) / 2.0
    across = (bh if abs(ux) > abs(uy) else bw) / 2.0 + 0.2
    if plus:
        text("+", px + ux * (along + 0.9), py + uy * (along + 0.9), 1.0)
        return
    mx, my = px + ux * (along + 0.3), py + uy * (along + 0.3)
    line(mx - uy * across, my + ux * across, mx + uy * across, my - ux * across, w=0.2)
for ref in list(fps):
    if ref.startswith("LED"):
        pad_mark(ref, "1" if ref in ("LED61", "LED65") else "2")     # red: pad 1 = cathode; green: pad 2
for ref in ("D61", "D62", "D63", "D64"):
    pad_mark(ref, "1")                                                 # pad 1 = cathode on all four
pad_mark("C32", "1", plus=True)

# jacks: name, the FPP outputs it carries, the LED order, a write-on box between cables, the tie slots
for j in range(15):
    xc = JX[j]
    text(f"J{j+1}", xc, JY + 11.5, 2.5, bold=True)
    text(f"OUT {4*j+1}-{4*j+4}", xc, JY + 14.8, 1.2)
    for p in range(4):
        text(str(p + 1), xc + PORT_X[p], LED_Y - 2.6, 1.0)
    if j < 14:
        box(xc + 6.8, JY + 10.5, xc + 13.2, JY + 25.5)
    text("TIE", xc, TIE_Y + 4.3, 1.0)
text("WRITE THE STRING NAMES IN THE BOXES BETWEEN THE JACKS", 255.0, 77.0, 1.2)
text("ZIP-TIE EACH CABLE THROUGH ITS SLOT PAIR, BEHIND THE PLUG BOOT", 255.0, 79.5, 1.2)
for b in range(3):
    text(f"BANK {b+1}: J{5*b+1}-J{5*b+5}  (LATCH P1-{('13', '37', '22')[b]})", 55.0 + 100.0 * b, LATCH_Y - 5.8 - 1.2, 1.0)
text("LEDS: PORT 4 3 2 1 (LEFT TO RIGHT), LIT = DATA", 155.0, LED_Y - 4.2 - 0.4, 1.0)

# the Pi and its ribbon
box(PI_X0, PI_Y0, PI_X0 + 85.0, PI_Y0 + 56.0, w=0.2)
text("RASPBERRY PI 3B+ / 4 / 5", PI_X0 + 42.5, PI_Y0 + 20.5, 2.0, bold=True)
text("FACE UP ON M2.5 STANDOFFS, 10 MM OR TALLER", PI_X0 + 42.5, PI_Y0 + 24.0, 1.2)
text("POWER THE PI FROM J18 (USB-C) - THE RIBBON CARRIES NO 5 V", PI_X0 + 42.5, PI_Y0 + 27.5, 1.2)
text("PI 5: SET usb_max_current_enable=1 IN config.txt", PI_X0 + 42.5, PI_Y0 + 30.5, 1.2)
text("AUDIO: PI JACK (3B+/4) OR USB SOUND CARD (5) -> J19", PI_X0 + 42.5, PI_Y0 + 33.5, 1.0)
p1x, p1y = pad("J16", "1")
tri = pcbnew.PCB_SHAPE(board); tri.SetShape(pcbnew.SHAPE_T_POLY)
tri.SetPolyPoints([MM(p1x + 7.4, p1y - 1.3), MM(p1x + 7.4, p1y + 1.3), MM(p1x + 5.8, p1y)])
tri.SetLayer(pcbnew.F_SilkS); tri.SetFilled(True); tri.SetWidth(FromMM(0.15)); board.Add(tri)
text("1", p1x + 8.8, p1y, 1.2)
text("J16 PI RIBBON - PIN 1 = PI PIN 1 (3V3)", PI_HDR[0], PI_Y0 + 44.0, 1.0)

# connectors along the top edge
_a, _b = pad("J17", "1"), pad("J17", "2")
text("+", _a[0], _a[1] + 4.6, 1.6, bold=True); text("-", _b[0], _b[1] + 4.6, 1.6, bold=True)
text("12V IN", 294.0, 13.2, 1.4, bold=True)
text("FUSE: 5A ATO BLADE", 274.0, 4.2, 1.0)
text("REVERSED", 292.0, 30.0, 1.0); text("12V", 284.0, 26.6, 1.0)
text("PI POWER OUT", 102.0, 13.5, 1.0); text("5V 3A USB-C", 102.0, 15.2, 1.0)
text("5V PI", 100.0, 25.5, 1.0); text("5V DRV", 130.0, 73.5, 1.0)
text("AUDIO IN", 118.0, 19.2, 1.0)
for pn, lab in (("1", "L"), ("2", "R"), ("3", "G")):
    x_, y_ = pad("J20", pn); text(lab, x_, y_ + 4.6, 1.2, bold=True)
text("LINE OUT", 134.0, 11.8, 1.0)
for pn, lab in (("1", "G"), ("2", "V"), ("3", "C"), ("4", "D")):
    x_, y_ = pad("J22", pn); text(lab, x_, y_ + 2.3, 1.0)
box(OLED_X - 13.65, OLED_Y - 2.3, OLED_X + 13.65, OLED_Y + 25.5, w=0.2)
text("0.96 IN OLED", OLED_X, OLED_Y + 11.0, 1.2)
text("SSD1306 I2C 0x3C", OLED_X, OLED_Y + 13.6, 1.0)
text("PLUGS IN FACE UP", OLED_X, OLED_Y + 16.0, 1.0)
for pn, lab in (("1", "+"), ("2", "-"), ("3", "G")):
    x_, y_ = pad("J21", pn); text(lab, x_, y_ + 4.6, 1.2, bold=True)
text("FAN 12V", 170.0, 11.8, 1.0); text("OVER TEMP", 186.0, 17.8, 1.0)
text("CR2032 +", 214.5, 25.0, 1.0)
text("WP", 96.0, 76.6, 1.0)
for i, lab in enumerate(("12V", "5VPI", "5VDRV", "3V3", "GND", "GND", "LE0", "LE1", "LE2", "D0", "SDA", "SCL")):
    tx_, ty_ = TP_POS[i + 1]
    text(lab, tx_, ty_ - 2.0 if (i + 1) in TP_BUS else ty_ + 2.6, 1.0)
text("difftxlarge rev A", 250.0, 66.0, 2.5, bold=True)
text("60-OUTPUT FPP DPIPIXELS CAPE - 15 x RJ45 FALCON DIFFERENTIAL", 250.0, 70.0, 1.2)
text("RJ45 PAIRS (+/-): P1 = 1/2   P2 = 3/6   P3 = 5/4   P4 = 7/8", 250.0, 73.0, 1.2)

# =====================================================================================================
# fill, save, stack-up
# =====================================================================================================
filler = pcbnew.ZONE_FILLER(board)
filler.Fill(board.Zones())
from board_fixups import find_dangling_stubs
_dead = find_dangling_stubs(board)
for _t in _dead:
    board.Remove(_t)
removed = len(_dead)
print(f"removed {removed} dangling stubs", flush=True)
filler.Fill(board.Zones())
# DIFFTXLARGE_OUT lets a verification run write elsewhere instead of over the board in the project
out = os.environ.get("DIFFTXLARGE_OUT") or os.path.join(HERE, f"{PROJECT}.kicad_pcb")
pcbnew.SaveBoard(out, board)
STACKUP = """		(stackup
    (layer "F.SilkS" (type "Top Silk Screen"))
    (layer "F.Paste" (type "Top Solder Paste"))
    (layer "F.Mask" (type "Top Solder Mask") (thickness 0.01))
    (layer "F.Cu" (type "copper") (thickness 0.035))
    (layer "dielectric 1" (type "prepreg") (thickness 0.2104) (material "FR4") (epsilon_r 4.5) (loss_tangent 0.02))
    (layer "In1.Cu" (type "copper") (thickness 0.035))
    (layer "dielectric 2" (type "core") (thickness 1.065) (material "FR4") (epsilon_r 4.5) (loss_tangent 0.02))
    (layer "In2.Cu" (type "copper") (thickness 0.035))
    (layer "dielectric 3" (type "prepreg") (thickness 0.2104) (material "FR4") (epsilon_r 4.5) (loss_tangent 0.02))
    (layer "B.Cu" (type "copper") (thickness 0.035))
    (layer "B.Mask" (type "Bottom Solder Mask") (thickness 0.01))
    (layer "B.Paste" (type "Bottom Solder Paste"))
    (layer "B.SilkS" (type "Bottom Silk Screen"))
    (copper_finish "HASL lead free")
    (dielectric_constraints no)
  )
"""
raw = open(out, encoding="utf-8").read()
if "(stackup" not in raw:
    key = "\n\t(setup\n"
    i = raw.index(key) + len(key)
    raw = raw[:i] + STACKUP + raw[i:]
    open(out, "w", encoding="utf-8", newline="\n").write(raw)
print("saved", out)
