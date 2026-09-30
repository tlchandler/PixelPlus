"""
Rev D board generator: FPP Remote pHAT for Pi Zero 2 W, on the Pi Zero's own outline. Run with KiCad 10's Python:

  "C:\Program Files\KiCad\10.0\bin\python.exe" gen_pcb.py

Board 65 x 30 mm, R3 corners, 2 layers. Board coordinates = Pi Zero 2 W coordinates (official mechanical drawing):
  holes at (3.5, 3.5) (61.5, 3.5) (3.5, 26.5) (61.5, 26.5); header centre line y = 3.5, pin 1 at (8.37, 4.77)
  (inner row, SD-card end), pin 2 at (8.37, 2.23).  The Pi sits underneath, face up; J3 is a socket on the
  BOTTOM of this board whose pad numbers equal Pi pin numbers (project footprint with odd/even pads swapped
  because flipping mirrors the numbering).
Layout (top view): EEPROM cluster left, 12 V input bottom-left, buck module centre, AM26C31 (U2) right of it,
RJ45 (J1) on the bottom edge at the right with its opening facing +y.
"""
import os, re
import pcbnew
from pcbnew import VECTOR2I_MM as MM, FromMM

HERE = os.path.dirname(os.path.abspath(__file__))
FPLIB = r"C:\Program Files\KiCad\10.0\share\kicad\footprints"
PROJECT = "difftx"

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
for b in blocks(net_txt[net_txt.find("(nets"):], "(net\n"):
    name = re.search(r'\(name "([^"]*)"', b).group(1)
    for ref, pin in re.findall(r'\(ref "([^"]+)"\)\s*\(pin "([^"]+)"\)', b):
        padnet[(ref, pin)] = name
sch_txt = open(os.path.join(HERE, f"{PROJECT}.kicad_sch"), encoding="utf-8").read()
root_uuid = re.search(r'\(uuid "([^"]+)"\)', sch_txt).group(1)
sym_uuid, sym_fields = {}, {}
for b in blocks(sch_txt, "\n\t(symbol\n"):
    u = re.search(r'\(uuid "([^"]+)"\)', b).group(1)
    r = re.search(r'\(property "Reference" "([^"]+)"', b).group(1)
    sym_uuid[r] = u
    sym_fields[r] = {k: v for k, v in re.findall(r'\(property "(Datasheet|Description|LCSC)" "([^"]*)"', b)}
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
ds = board.GetDesignSettings()
ds.m_CopperEdgeClearance = FromMM(0.3); ds.m_MinClearance = FromMM(0.2); ds.m_TrackMinWidth = FromMM(0.2)
ds.m_ViasMinSize = FromMM(0.6); ds.m_MinThroughDrill = FromMM(0.3)
ds.SetAuxOrigin(MM(0.0, 30.0))                     # bottom-left corner: shared origin for gerbers, drill and CPL

nets = {}
def net(name):
    if name not in nets:
        n = pcbnew.NETINFO_ITEM(board, name); board.Add(n); nets[name] = n
    return nets[name]
for name in sorted(set(padnet.values())):
    net(name)

fps = {}
def add_fp(lib, name, ref, value, x, y, want=None, bottom=False):
    path = {"jlc": os.path.join(HERE, "jlc", "jlc.pretty"), PROJECT: os.path.join(HERE, f"{PROJECT}.pretty")}.get(lib, os.path.join(FPLIB, lib + ".pretty"))
    fp = pcbnew.FootprintLoad(path, name)
    assert fp is not None, (lib, name)
    fp.SetReference(ref); fp.SetValue(value); fp.SetFPIDAsString(f"{lib}:{name}")
    if ref in sym_uuid:
        fp.SetPath(pcbnew.KIID_PATH(f"/{root_uuid}/{sym_uuid[ref]}"))
        set_field(fp, "Datasheet", sym_fields[ref].get("Datasheet", ""))
        set_field(fp, "Description", sym_fields[ref].get("Description", ""))
        if sym_fields[ref].get("LCSC"):
            set_field(fp, "LCSC", sym_fields[ref]["LCSC"])
        fp.SetExcludedFromBOM(not sym_fields[ref]["in_bom"])
    board.Add(fp)
    fp.SetPosition(MM(x, y))
    if bottom:
        fp.SetLayerAndFlip(pcbnew.B_Cu)      # body under the board, pads mirrored
    if want:
        for ang in (0, 90, 180, 270, -90):
            fp.SetOrientationDegrees(ang); fp.SetPosition(MM(x, y))
            if all(abs(fp.FindPadByNumber(pn).GetPosition().x / 1e6 - px) < 0.02 and abs(fp.FindPadByNumber(pn).GetPosition().y / 1e6 - py) < 0.02 for pn, (px, py) in want.items()):
                break
        else:
            raise SystemExit(f"could not orient {ref}: " + str([(pn, fp.FindPadByNumber(pn).GetPosition()) for pn in want]))
    for pad in fp.Pads():
        n = padnet.get((ref, pad.GetNumber()))
        if n:
            pad.SetNet(net(n))
    fps[ref] = fp
    return fp

# ---- placement (mm, Pi Zero coordinates, y down) ------------------------------------------------------------------
W, H = 65.0, 30.0
PIH = {"H1": (3.5, 3.5), "H2": (61.5, 3.5), "H3": (3.5, 26.5), "H4": (61.5, 26.5)}
def pipin(n):                       # Pi header pin n: odd = inner row (y 4.77), even = outer row (y 2.23)
    return (8.37 + 2.54 * ((n - 1) // 2), 4.77 if n % 2 else 2.23)

# socket on the bottom: project copy of the JLC socket with odd/even pad numbers swapped (see docstring)
_src = pcbnew.FootprintLoad(os.path.join(HERE, "jlc", "jlc.pretty"), "HDR-TH_40P-P2.54-V-F-R2-C20-S2.54-2")
for _p in _src.Pads():
    _n = int(_p.GetNumber()); _p.SetNumber(str(_n + 1 if _n % 2 else _n - 1))
    _p.SetShape(pcbnew.PAD_SHAPE_RECT if _p.GetNumber() == "1" else pcbnew.PAD_SHAPE_CIRCLE)
_src.SetFPIDAsString(f"{PROJECT}:PiSocket_2x20_bottom")
_src.SetLibDescription("2x20 2.54 mm socket (JLC C5124634) for the underside of a pHAT: pad numbers = Pi header pins once the footprint is on B.Cu")
pcbnew.FootprintSave(os.path.join(HERE, f"{PROJECT}.pretty"), _src)
add_fp(PROJECT, "PiSocket_2x20_bottom", "J3", "2x20 socket for Pi", 32.5, 3.5,
       {"1": pipin(1), "2": pipin(2), "39": pipin(39), "40": pipin(40)}, bottom=True)

# EEPROM cluster (left)
add_fp("jlc", "SOIC-8_L4.9-W3.9-P1.27-LS6.0-BL", "U3", "AT24C256C-SSHL-T", 12.0, 12.0,
       {"5": (9.4, 10.1), "8": (9.4, 13.9), "4": (14.6, 10.1), "1": (14.6, 13.9)})      # SDA/SCL/WP/VCC on the left column
U3L, U3R = 9.4, 14.6
SDA_Y, SCL_Y, WP_Y, VCC_Y = 10.1, 11.37, 12.64, 13.9
add_fp("Jumper", "SolderJumper-2_P1.3mm_Open_Pad1.0x1.5mm", "JP1", "WP", 7.4, 13.27, {"1": (7.4, 12.62), "2": (7.4, 13.92)})
add_fp("Resistor_SMD", "R_0603_1608Metric", "R1", "10k", 5.0, 11.5, {"1": (5.0, 10.675), "2": (5.0, 12.325)})
add_fp("Capacitor_SMD", "C_0805_2012Metric_Pad1.18x1.45mm_HandSolder", "C2", "100nF", 10.5, 16.1, {"1": (9.462, 16.1), "2": (11.538, 16.1)})
# 12 V input chain (bottom-left)
add_fp("jlc", "CONN-TH_P5.00_KF301-5.0-2P", "J4", "terminal 2P 5.0mm", 12.5, 25.9, {"1": (10.0, 25.9), "2": (15.0, 25.9)})
add_fp("jlc", "F1812", "F1", "1.5A", 12.5, 19.6, {"1": (10.65, 19.6), "2": (14.35, 19.6)})
add_fp("jlc", "SMA_L4.4-W2.8-LS5.4-R-RD", "D1", "SS54", 20.4, 21.0, {"1": (17.9, 21.0), "2": (22.9, 21.0)})   # A left, K right
for i, x in enumerate((28.2, 31.4, 34.6)):
    add_fp("Capacitor_SMD", "C_1206_3216Metric", f"C{3+i}", "10uF", x, 24.8, {"1": (x, 23.325), "2": (x, 26.275)})
# buck module (centre), body toward +y
add_fp("jlc", "PWRM-TH_K78XX-2000R3", "U4", "K7805-2000R3", 26.0, 12.4, {"1": (23.46, 12.4), "2": (26.0, 12.4), "3": (28.54, 12.4)})
add_fp("Capacitor_SMD", "C_0805_2012Metric_Pad1.18x1.45mm_HandSolder", "C6", "22uF", 26.0, 8.7, {"1": (27.038, 8.7), "2": (24.962, 8.7)})
# driver: columns vertical, pins 1-8 up the right column, 9-16 down the left column
add_fp("jlc", "SOIC-16_L9.9-W3.9-P1.27-LS6.0-BL", "U2", "AM26C31IDR", 36.9, 13.0,
       {"1": (39.64, 17.45), "8": (39.64, 8.55), "16": (34.16, 17.45), "9": (34.16, 8.55)})
U2L, U2R = 34.16, 39.64
def u2y(n):                         # pad y for pin n (1..8 bottom->top on the right, 9..16 top->bottom on the left)
    return 17.45 - 1.27 * (n - 1) if n <= 8 else 8.55 + 1.27 * (n - 9)
add_fp("Capacitor_SMD", "C_0805_2012Metric_Pad1.18x1.45mm_HandSolder", "C1", "100nF", 33.0, 20.0, {"1": (33.0, 18.962), "2": (33.0, 21.038)})
# RJ45 on the bottom edge, opening facing +y
add_fp("jlc", "RJ45-TH_R-RJ45R08P-A004", "J1", "R-RJ45R08P-A004", 49.0, 21.04, {"1": (52.57, 21.93), "2": (51.55, 20.15), "8": (45.43, 20.15)})
RJ = {n: (52.57 - 1.02 * (n - 1), 21.93 if n % 2 else 20.15) for n in range(1, 9)}
for ref, (hx, hy) in PIH.items():
    add_fp("MountingHole", "MountingHole_2.7mm_M2.5", ref, "M2.5", hx, hy)
    for pad in fps[ref].Pads():
        pad.SetLocalClearance(FromMM(1.0))   # ~4.7 mm copper-free circle under the standoff
for pad in fps["J1"].Pads():
    if pad.GetNumber() == "":
        pad.SetAttribute(pcbnew.PAD_ATTRIB_NPTH)
pcbnew.FootprintSave(os.path.join(HERE, "jlc", "jlc.pretty"), fps["J1"])

def ref_at(ref, x, y, size=0.8, angle=0):
    t = fps[ref].Reference(); t.SetPosition(MM(x, y)); t.SetTextSize(MM(size, size)); t.SetTextThickness(FromMM(0.12))
    t.SetLayer(pcbnew.F_SilkS); t.SetVisible(True); t.SetTextAngleDegrees(angle); t.SetMirrored(False)
for ref, (x, y) in {"J3": (32.5, 7.2), "U3": (12.0, 8.9), "JP1": (7.4, 10.9), "R1": (3.0, 10.4), "C2": (7.7, 16.1),
                    "J4": (5.4, 21.0), "F1": (12.5, 17.5), "D1": (25.4, 21.2), "C3": (28.2, 28.2), "C4": (31.4, 28.2), "C5": (34.6, 28.2),
                    "U4": (26.0, 20.0), "C6": (26.0, 6.9), "U2": (36.9, 19.8), "C1": (35.5, 21.0), "J1": (49.0, 12.0)}.items():
    ref_at(ref, x, y)
for ref in fps:
    fps[ref].Value().SetVisible(False)
for ref in PIH:
    fps[ref].Reference().SetVisible(False)

# ---- outline: Pi Zero rectangle with R3 corners --------------------------------------------------------------------
def edge(x1, y1, x2, y2):
    s = pcbnew.PCB_SHAPE(board); s.SetShape(pcbnew.SHAPE_T_SEGMENT)
    s.SetStart(MM(x1, y1)); s.SetEnd(MM(x2, y2)); s.SetLayer(pcbnew.Edge_Cuts); s.SetWidth(FromMM(0.1)); board.Add(s)
def arc(cx, cy, sx, sy, angle):
    s = pcbnew.PCB_SHAPE(board); s.SetShape(pcbnew.SHAPE_T_ARC)
    s.SetCenter(MM(cx, cy)); s.SetStart(MM(sx, sy)); s.SetArcAngleAndEnd(pcbnew.EDA_ANGLE(angle, pcbnew.DEGREES_T), False)
    s.SetLayer(pcbnew.Edge_Cuts); s.SetWidth(FromMM(0.1)); board.Add(s)
R = 3.0
edge(R, 0, W - R, 0); edge(W, R, W, H - R); edge(W - R, H, R, H); edge(0, H - R, 0, R)
arc(W - R, R, W - R, 0, 90); arc(W - R, H - R, W, H - R, 90); arc(R, H - R, R, H, 90); arc(R, R, 0, R, 90)

F, B = pcbnew.F_Cu, pcbnew.B_Cu
def track(layer, netname, pts, w=0.25):
    for (x1, y1), (x2, y2) in zip(pts, pts[1:]):
        t = pcbnew.PCB_TRACK(board); t.SetStart(MM(x1, y1)); t.SetEnd(MM(x2, y2)); t.SetWidth(FromMM(w)); t.SetLayer(layer); t.SetNet(net(netname)); board.Add(t)
def via(netname, x, y, d=0.8):
    v = pcbnew.PCB_VIA(board); v.SetPosition(MM(x, y)); v.SetWidth(FromMM(d)); v.SetDrill(FromMM(0.4)); v.SetNet(net(netname)); v.SetLayerPair(F, B); board.Add(v)

# ---- EEPROM ----------------------------------------------------------------------------------------------------------
track(F, "/SDA1", [pipin(3), (10.91, 8.6), (U3L, SDA_Y)])
track(F, "/SCL1", [pipin(5), (13.45, 7.6), (11.6, 9.4), (11.6, SCL_Y), (U3L, SCL_Y)])
track(F, "/WP", [(U3L, WP_Y), (7.4, 12.62), (6.2, 11.5), (5.0, 10.675)])                                   # pin 7 -> JP1.1 -> R1
track(F, "GND", [(5.0, 12.325), (3.9, 12.325)]); via("GND", 3.9, 12.325)
track(F, "+3V3", [(U3L, VCC_Y), (7.4, 13.92)])                                                 # VCC -> JP1.2
track(F, "+3V3", [(U3L, VCC_Y), (9.462, 16.1)])                    # VCC -> C2
track(F, "GND", [(11.538, 16.1), (12.9, 17.4)]); via("GND", 12.9, 17.4)
track(F, "GND", [(U3R, 10.1), (U3R, 13.9), (16.3, 13.9)]); via("GND", 16.3, 13.9)              # A0-A2, GND
track(B, "+3V3", [pipin(17), (28.69, 7.2), (17.5, 7.2), (17.5, 14.85)]); via("+3V3", 17.5, 14.85)
track(B, "+3V3", [pipin(1), (8.37, 6.4), (28.69, 6.4)]) # 3V3 from Pi pin 17
track(F, "+3V3", [(17.5, 14.85), (9.462, 14.85), (9.462, 16.1)])

# ---- 12 V in -> 5 V ---------------------------------------------------------------------------------------------------
track(F, "/12V_IN", [(10.0, 25.9), (10.0, 22.6), (10.65, 21.95), (10.65, 19.6)], 0.8)          # J4 + -> F1
track(F, "/12V_F", [(14.35, 19.6), (15.75, 21.0), (17.9, 21.0)], 0.8)                                          # F1 -> D1 anode
track(F, "+12V", [(22.9, 21.0), (26.9, 21.0), (28.2, 22.3), (28.2, 23.325), (34.6, 23.325)], 0.8)   # D1 K -> C3..C5, clear of the antenna window                          # D1 cathode -> C3..C5
track(F, "+12V", [(23.46, 12.4), (23.46, 11.0), (20.0, 11.0), (20.0, 19.3), (21.7, 21.0), (22.9, 21.0)], 0.8)   # -> U4 Vin
track(F, "+5V", [(28.54, 12.4), (28.54, 10.4), (27.64, 9.5), (27.038, 8.7)], 0.8)             # Vout -> C6
track(F, "GND", [(24.962, 8.7), (23.7, 9.9)]); via("GND", 23.7, 9.9)
via("+5V", 28.3, 8.7, 0.8); via("+5V", 28.3, 9.8, 0.8); track(F, "+5V", [(28.3, 8.7), (28.3, 9.8)], 0.8); track(B, "+5V", [(28.3, 9.8), (28.3, 8.7)], 0.8)                                                                    # C6 pad 2 -> back layer trunk
track(F, "+5V", [(27.038, 8.7), (28.3, 8.7)], 0.8)
track(B, "+5V", [(28.3, 8.7), (21.5, 8.7), (21.5, 16.0), (6.7, 16.0), (6.7, 1.0), (10.91, 1.0)], 0.8)   # to Pi pins 2/4
track(B, "+5V", [(8.37, 1.0), pipin(2)], 0.8); track(B, "+5V", [(10.91, 1.0), pipin(4)], 0.8)
track(F, "+5V", [(28.54, 12.4), (28.54, 17.5), (34.4, 17.5), (U2R - 5.48, u2y(16))], 0.5)      # Vout -> U2 VCC
track(F, "+5V", [(33.0, 17.5), (33.0, 18.962)], 0.5)                                            # -> C1
track(F, "GND", [(33.0, 21.038), (30.6, 21.6), (30.0, 21.6)]); via("GND", 30.0, 21.6)

# ---- DPI inputs (Pi -> U2) --------------------------------------------------------------------------------------------
track(F, "/DPI_D0", [pipin(7), (15.99, 7.5), (31.2, 7.5), (31.2, u2y(15)), (U2L, u2y(15))])                     # P1-7  -> 4A
track(F, "/DPI_D2", [pipin(31), (46.47, 6.9), (U2L, 6.9), (U2L, u2y(9))])                                       # P1-31 -> 3A
track(B, "/DPI_D3", [pipin(26), (40.12, 3.5), (40.12, 6.9), (41.8, 6.9), (41.8, u2y(7))]); via("/DPI_D3", 41.8, u2y(7))       # P1-26 -> 2A
track(F, "/DPI_D3", [(41.8, u2y(7)), (U2R, u2y(7))])
track(B, "/DPI_D1", [pipin(29), (43.93, 6.0), (52.8, 6.0), (52.8, u2y(1))]); via("/DPI_D1", 52.8, u2y(1))       # P1-29 -> 1A
track(F, "/DPI_D1", [(52.8, u2y(1)), (U2R, u2y(1))])

# ---- driver enables / ground --------------------------------------------------------------------------------------------
track(F, "GND", [(U2R, u2y(4)), (42.9, u2y(4))]); via("GND", 42.9, u2y(4))          # G
track(F, "GND", [(U2L, u2y(12)), (32.2, u2y(12))]); via("GND", 32.2, u2y(12))       # ~G
track(F, "GND", [(U2R, u2y(8)), (41.05, u2y(8))]); via("GND", 41.05, u2y(8))          # GND pin 8
track(B, "GND", [pipin(25), (38.85, 6.1)]); via("GND", 38.85, 6.1)                   # Pi pin 25 island

# ---- outputs -> RJ45. Even pins (back row, y 20.45) from above on the back layer; odd pins (front row, y 22.23)
#      from below through the pocket between the jack's pegs. Port mapping: driver1->port1, driver2->port3,
#      driver3->port2, driver4->port4 (matches the EEPROM strings file).
# 1Y -> pin 1 (P1+)
track(F, "/P1+", [(U2R, u2y(2)), (57.75, u2y(2)), (57.75, 27.0), RJ[1][0:1] + (27.0,), RJ[1]])
# 1Z -> pin 2 (P1-)
track(F, "/P1-", [(U2R, u2y(3)), (42.2, u2y(3))]); via("/P1-", 42.2, u2y(3))
track(B, "/P1-", [(42.2, u2y(3)), (42.2, 17.0), (RJ[2][0], 17.0), RJ[2]])
# 2Y -> pin 4 (P3+)
track(F, "/P3+", [(U2R, u2y(6)), (41.2, u2y(6))]); via("/P3+", 41.2, u2y(6))
track(B, "/P3+", [(41.2, u2y(6)), (41.2, 17.6), (RJ[4][0], 17.6), RJ[4]])
# 2Z -> pin 5 (P3-)
track(F, "/P3-", [(U2R, u2y(5)), (58.35, u2y(5)), (58.35, 27.45), (RJ[5][0], 27.45), RJ[5]])
# 3Y -> pin 3 (P2+)
track(F, "/P2+", [(U2L, u2y(10)), (38.1, u2y(10)), (38.1, 27.0), (47.5, 27.0)]); via("/P2+", 47.5, 27.0)
track(B, "/P2+", [(47.5, 27.0), (RJ[3][0], 27.0), RJ[3]])
# 3Z -> pin 6 (P2-)
track(F, "/P2-", [(U2L, u2y(11)), (36.9, u2y(11))]); via("/P2-", 36.9, u2y(11))
track(B, "/P2-", [(36.9, u2y(11)), (36.9, 18.2), (RJ[6][0], 18.2), RJ[6]])
# 4Y -> pin 7 (P4+)
track(F, "/P4+", [(U2L, u2y(14)), (37.65, u2y(14)), (37.65, 27.9), (43.0, 27.9)]); via("/P4+", 43.0, 27.9)
track(B, "/P4+", [(43.0, 27.9), (RJ[7][0], 27.9), RJ[7]])
# 4Z -> pin 8 (P4-)
track(F, "/P4-", [(U2L, u2y(13)), (35.9, u2y(13))]); via("/P4-", 35.9, u2y(13))
track(B, "/P4-", [(35.9, u2y(13)), (35.9, 18.8), (RJ[8][0], 18.8), RJ[8]])

# ---- ground stitching -----------------------------------------------------------------------------------------------------
for (x, y) in [(3.0, 9.5), (3.0, 19.5), (15.5, 28.6), (31.0, 28.9), (36.5, 29.0), (44.7, 29.2),
               (60.5, 12.0), (60.5, 20.5), (62.8, 15.5), (56.5, 8.5), (23.5, 17.0), (16.0, 23.3), (2.5, 15.5),
               (54.6, 13.6)]:   # last one ties the pour pocket around U2 pin 4 (G) to the main ground
    via("GND", x, y)

# ---- zones -------------------------------------------------------------------------------------------------------------------
def zone_rect(layer, x1, y1, x2, y2, netname, name):
    z = pcbnew.ZONE(board); z.SetNet(net(netname)); z.SetLayer(layer)
    z.SetPadConnection(pcbnew.ZONE_CONNECTION_THERMAL); z.SetLocalClearance(FromMM(0.3)); z.SetMinThickness(FromMM(0.25))
    z.SetThermalReliefGap(FromMM(0.4)); z.SetThermalReliefSpokeWidth(FromMM(0.5)); z.SetZoneName(name)
    z.SetIslandRemovalMode(pcbnew.ISLAND_REMOVAL_MODE_ALWAYS)   # slivers between header pins would otherwise stay as islands
    o = z.Outline(); o.NewOutline()
    for x, y in ((x1, y1), (x2, y1), (x2, y2), (x1, y2)):
        o.Append(FromMM(x), FromMM(y))
    board.Add(z); return z
# no pour inside the header pin field (it would only make slivers between pins); the GND pins get explicit stubs
def keepout(layers, x1, y1, x2, y2, name):
    z = pcbnew.ZONE(board); z.SetIsRuleArea(True); z.SetZoneName(name)
    ls = pcbnew.LSET(); [ls.addLayer(l) for l in layers]; z.SetLayerSet(ls)
    (getattr(z, "SetDoNotAllowZoneFills", None) or getattr(z, "SetDoNotAllowCopperPour"))(True)
    for m in ("SetDoNotAllowTracks", "SetDoNotAllowVias", "SetDoNotAllowPads", "SetDoNotAllowFootprints"):
        if hasattr(z, m):
            getattr(z, m)(False)                                  # only the pour is excluded
    o = z.Outline(); o.NewOutline()
    for x, y in ((x1, y1), (x2, y1), (x2, y2), (x1, y2)):
        o.Append(FromMM(x), FromMM(y))
    board.Add(z)
keepout((F, B), 6.9, 1.3, 58.1, 5.7, "header_no_pour")
keepout((F, B), 17.0, 23.0, 27.0, 29.7, "wifi_antenna_window")   # Pi Zero 2 W antenna sits below here, between mini-HDMI and USB
for n in (6, 14, 20, 30, 34):                                           # outer-row GND pins -> strip above the header
    track(B, "GND", [pipin(n), (pipin(n)[0], 0.9)])
track(B, "GND", [pipin(9), (17.26, 3.5), (17.26, 0.9)])                 # inner-row GND pins
track(B, "GND", [pipin(39), (56.63, 6.3)])
zone_rect(F, 0.3, 0.3, W - 0.3, H - 0.3, "GND", "GND_top")
zone_rect(B, 0.3, 0.3, W - 0.3, H - 0.3, "GND", "GND_bottom")

def text(s, x, y, size=0.8, layer=pcbnew.F_SilkS, angle=0):
    t = pcbnew.PCB_TEXT(board); t.SetText(s); t.SetPosition(MM(x, y)); t.SetLayer(layer)
    t.SetTextSize(MM(size, size)); t.SetTextThickness(FromMM(0.12)); t.SetTextAngleDegrees(angle)
    t.SetMirrored(layer == pcbnew.B_SilkS); board.Add(t)
text("pin 1", 8.4, 7.2, 0.8)
text("pin 1", 8.4, 7.2, 0.8, layer=pcbnew.B_SilkS)
text("FPP RS-422 pHAT rev D", 53.5, 8.4, 0.8)
text("+", 6.6, 24.2, 0.8)
text("-", 19.0, 24.2, 0.8)
text("ant", 22.0, 26.6, 0.8)
text("NO USB PWR", 2.6, 19.5, 0.8, angle=90)
text("P1=1,2 P2=3,6 P3=4,5 P4=7,8", 53.5, 10.2, 0.8)

filler = pcbnew.ZONE_FILLER(board); filler.Fill(board.Zones())
out = os.path.join(HERE, f"{PROJECT}.kicad_pcb"); pcbnew.SaveBoard(out, board)
for ref in ("J3", "U3", "J1", "U2", "U4", "D1", "F1", "J4", "JP1", "R1"):
    fp = fps[ref]
    print(ref, "rot", fp.GetOrientationDegrees(), fp.IsFlipped(), [(p.GetNumber(), round(p.GetPosition().x / 1e6, 2), round(p.GetPosition().y / 1e6, 2), p.GetNetname()) for p in fp.Pads() if p.GetNumber() in ("1", "2", "3", "8", "9", "16", "39", "40")])
print("saved", out)
