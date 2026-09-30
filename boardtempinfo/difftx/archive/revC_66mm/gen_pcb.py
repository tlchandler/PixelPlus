"""
Rev C board generator (FPP Remote pHAT for Pi Zero 2 W). Run with KiCad 10's Python:

  "C:\Program Files\KiCad\10.0\bin\python.exe" gen_pcb.py

Board 66 x 66 mm, 2 layers, mounts on top of a Raspberry Pi Zero 2 W:
  H1-H4 M2.5 on the Pi Zero hole pattern (58 x 23 mm, 3.5 mm from the Pi edges), Pi under the board,
  J3 2x20 socket over the Pi header: pin 1 = 4.87 mm right of the left hole centre, inner row
  (per the KiCad Raspberry_Pi_Zero footprint derived from Raspberry Pi's mechanical drawing).
Bottom band: J1 RJ45 at the left edge, U2 driver, U3 EEPROM (top-left over the Pi area),
12 V input on the bottom-right: J4 -> F1 -> D1 -> U4 -> 5 V to the Pi header.
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

# ---- placement (mm, board origin top-left, Pi under the board) ---------------
W, H = 66.0, 66.0
PIH = {"H1": (4.0, 4.0), "H2": (62.0, 4.0), "H3": (4.0, 27.0), "H4": (62.0, 27.0)}     # Pi Zero holes
# The socket goes on the BOTTOM (the Pi is underneath and its pins must enter the socket body). Flipping the
# JLC footprint mirrors its pad numbering, so make a library copy with odd/even pad numbers swapped: after the
# flip, pad n lands exactly where Pi pin n is.
_src = pcbnew.FootprintLoad(os.path.join(HERE, "jlc", "jlc.pretty"), "HDR-TH_40P-P2.54-V-F-R2-C20-S2.54-2")
for _p in _src.Pads():
    _n = int(_p.GetNumber()); _p.SetNumber(str(_n + 1 if _n % 2 else _n - 1))
_src.SetFPIDAsString(f"{PROJECT}:PiSocket_2x20_bottom")
_src.SetLibDescription("2x20 2.54 mm socket (JLC C5124634) for the underside of a pHAT: pad numbers = Pi header pins once the footprint is on B.Cu")
pcbnew.FootprintSave(os.path.join(HERE, f"{PROJECT}.pretty"), _src)
add_fp(PROJECT, "PiSocket_2x20_bottom", "J3", "Pi Zero 2 W", 33.0, 4.0,
       {"1": (8.87, 5.27), "2": (8.87, 2.73), "39": (57.13, 5.27), "40": (57.13, 2.73)}, bottom=True)
def pipin(n):                       # header pin n -> (x, y); odd = inner row (y 5.27), even = outer (y 2.73)
    return (8.87 + 2.54 * ((n - 1) // 2), 5.27 if n % 2 else 2.73)
add_fp("jlc", "SOIC-8_L4.9-W3.9-P1.27-LS6.0-BL", "U3", "AT24C256C-SSHL-T", 22.6, 20.9,
       {"1": (25.2, 22.81), "4": (25.2, 19.0), "5": (20.0, 19.0), "8": (20.0, 22.81)})   # SDA/SCL/WP/VCC face the header
add_fp("Capacitor_SMD", "C_0805_2012Metric_Pad1.18x1.45mm_HandSolder", "C2", "100nF", 20.0, 25.5, {"1": (20.0, 24.462), "2": (20.0, 26.538)})
add_fp("Resistor_SMD", "R_0603_1608Metric", "R1", "10k", 16.0, 26.15, {"1": (16.0, 25.325), "2": (16.0, 26.975)})
add_fp("Jumper", "SolderJumper-2_P1.3mm_Open_Pad1.0x1.5mm", "JP1", "WP", 13.05, 25.325, {"1": (13.7, 25.325), "2": (12.4, 25.325)})
add_fp("jlc", "RJ45-TH_R-RJ45R08P-A004", "J1", "R-RJ45R08P-A004", 8.82, 50.0, {"1": (7.93, 53.57), "2": (9.71, 52.55), "8": (9.71, 46.43)})
add_fp("jlc", "SOIC-16_L9.9-W3.9-P1.27-LS6.0-BL", "U2", "AM26C31IDR", 27.0, 47.5,
       {"1": (24.26, 43.05), "8": (24.26, 51.94), "16": (29.74, 43.05), "9": (29.74, 51.94)})
add_fp("Capacitor_SMD", "C_0805_2012Metric_Pad1.18x1.45mm_HandSolder", "C1", "100nF", 33.8, 41.54, {"1": (33.8, 40.5), "2": (33.8, 42.58)})
add_fp("jlc", "PWRM-TH_K78XX-2000R3", "U4", "K7805-2000R3", 54.0, 38.0, {"1": (51.46, 38.0), "2": (54.0, 38.0), "3": (56.54, 38.0)})
add_fp("Capacitor_SMD", "C_0805_2012Metric_Pad1.18x1.45mm_HandSolder", "C6", "22uF", 58.5, 33.8, {"1": (57.462, 33.8), "2": (59.538, 33.8)})
for i, x in enumerate((49.0, 52.2, 55.4)):
    add_fp("Capacitor_SMD", "C_1206_3216Metric", f"C{3+i}", "10uF", x, 50.0, {"1": (x, 48.525), "2": (x, 51.475)})
add_fp("jlc", "SMA_L4.4-W2.8-LS5.4-R-RD", "D1", "SS54", 53.0, 54.5, {"1": (55.5, 54.5), "2": (50.5, 54.5)})
add_fp("jlc", "F1812", "F1", "1.5A", 59.5, 54.5, {"1": (61.35, 54.5), "2": (57.65, 54.5)})
add_fp("jlc", "CONN-TH_P5.00_KF301-5.0-2P", "J4", "12V IN", 57.0, 61.5, {"1": (54.5, 61.5), "2": (59.5, 61.5)})
for ref, (hx, hy) in PIH.items():
    add_fp("MountingHole", "MountingHole_2.7mm_M2.5", ref, "M2.5", hx, hy)
add_fp("MountingHole", "MountingHole_3.2mm_M3", "H5", "M3", 3.5, 62.5)
add_fp("MountingHole", "MountingHole_3.2mm_M3", "H6", "M3", 30.0, 62.5)
for ref in list(PIH) + ["H5", "H6"]:
    for pad in fps[ref].Pads():
        pad.SetLocalClearance(FromMM(1.4))   # ~5.5 mm copper-free circle for the screw head / standoff

# JLC RJ45 pegs as plain holes; keep the library copy in sync
for pad in fps["J1"].Pads():
    if pad.GetNumber() == "":
        pad.SetAttribute(pcbnew.PAD_ATTRIB_NPTH)
pcbnew.FootprintSave(os.path.join(HERE, "jlc", "jlc.pretty"), fps["J1"])

def ref_at(ref, x, y, size=1.0):
    t = fps[ref].Reference(); t.SetPosition(MM(x, y)); t.SetTextSize(MM(size, size)); t.SetTextThickness(FromMM(0.15))
    t.SetLayer(pcbnew.F_SilkS); t.SetVisible(True); t.SetTextAngleDegrees(0); t.SetMirrored(False)
for ref, (x, y) in {"J3": (33.0, 8.0), "U3": (27.5, 20.9), "C2": (22.4, 25.5), "R1": (17.9, 26.15), "JP1": (13.05, 23.5),
                    "J1": (8.8, 38.9), "U2": (27.0, 39.6), "C1": (36.1, 41.54), "U4": (46.5, 41.0), "C6": (58.5, 31.6),
                    "C3": (49.0, 46.9), "C4": (52.2, 46.9), "C5": (55.4, 46.9), "D1": (47.0, 54.5), "F1": (63.8, 54.5), "J4": (64.0, 58.0)}.items():
    ref_at(ref, x, y)
for ref in fps:
    fps[ref].Value().SetVisible(False)
for ref in list(PIH) + ["H5", "H6"]:
    fps[ref].Reference().SetVisible(False)

def edge(x1, y1, x2, y2):
    s = pcbnew.PCB_SHAPE(board); s.SetShape(pcbnew.SHAPE_T_SEGMENT)
    s.SetStart(MM(x1, y1)); s.SetEnd(MM(x2, y2)); s.SetLayer(pcbnew.Edge_Cuts); s.SetWidth(FromMM(0.1)); board.Add(s)
edge(0, 0, W, 0); edge(W, 0, W, H); edge(W, H, 0, H); edge(0, H, 0, 0)

F, B = pcbnew.F_Cu, pcbnew.B_Cu
def track(layer, netname, pts, w=0.25):
    for (x1, y1), (x2, y2) in zip(pts, pts[1:]):
        t = pcbnew.PCB_TRACK(board); t.SetStart(MM(x1, y1)); t.SetEnd(MM(x2, y2)); t.SetWidth(FromMM(w)); t.SetLayer(layer); t.SetNet(net(netname)); board.Add(t)
def via(netname, x, y, d=0.8):
    v = pcbnew.PCB_VIA(board); v.SetPosition(MM(x, y)); v.SetWidth(FromMM(d)); v.SetDrill(FromMM(0.4)); v.SetNet(net(netname)); v.SetLayerPair(F, B); board.Add(v)

# ---- DPI inputs from the Pi header to the driver -------------------------------
track(B, "/DPI_D0", [pipin(7), (16.49, 8.5), (22.6, 8.5), (22.6, 32.5)]); via("/DPI_D0", 22.6, 32.5)          # P1-7 -> 1A
track(F, "/DPI_D0", [(22.6, 32.5), (22.6, 43.05), (24.26, 43.05)])
track(F, "/DPI_D3", [pipin(26), (40.62, 4.0), (40.62, 36.0), (25.0, 36.0), (25.0, 37.7)]); via("/DPI_D3", 25.0, 37.7)   # slips between pins 25/27; via("/DPI_D3", 25.0, 37.7)           # P1-26 -> 4A via interior
track(B, "/DPI_D3", [(25.0, 37.7), (25.0, 39.9), (27.8, 39.9), (27.8, 44.32)]); via("/DPI_D3", 27.8, 44.32)
track(F, "/DPI_D3", [(27.8, 44.32), (29.74, 44.32)])
track(F, "/DPI_D1", [pipin(29), (44.43, 37.0), (41.0, 37.0), (41.0, 51.94), (29.74, 51.94)])                   # P1-29 -> 3A
track(F, "/DPI_D2", [pipin(31), (46.97, 36.5), (45.5, 36.5), (45.5, 53.5), (22.9, 53.5), (22.9, 50.67), (24.26, 50.67)])  # P1-31 -> 2A

# ---- driver outputs -> RJ45 (rev B topology shifted +3.5 mm) ---------------------
track(F, "/P4-", [(24.26, 45.59), (20.8, 45.59), (20.8, 46.43), (9.71, 46.43)])
track(F, "/P1-", [(24.26, 48.13), (21.4, 48.13), (21.4, 52.55), (9.71, 52.55)])
track(F, "/P1+", [(24.26, 49.4), (22.0, 49.4), (22.0, 58.8), (3.1, 58.8), (3.1, 53.57), (7.93, 53.57)])
track(F, "/P4+", [(24.26, 44.32), (22.7, 44.32)]); via("/P4+", 22.7, 44.32)
track(B, "/P4+", [(22.7, 44.32), (22.7, 41.2), (1.8, 41.2), (1.8, 47.45), (7.93, 47.45)])
track(F, "/P2+", [(29.74, 45.59), (27.8, 45.59)]); via("/P2+", 27.8, 45.59)
track(B, "/P2+", [(27.8, 45.59), (27.8, 59.3), (2.5, 59.3), (2.5, 51.53), (7.93, 51.53)])
track(F, "/P2-", [(29.74, 46.86), (26.2, 46.86)]); via("/P2-", 26.2, 46.86)
track(B, "/P2-", [(26.2, 46.86), (24.6, 48.47), (9.71, 48.47)])
track(F, "/P3-", [(29.74, 49.4), (26.2, 49.4)]); via("/P3-", 26.2, 49.4)
track(B, "/P3-", [(26.2, 49.4), (27.0, 48.6), (27.0, 40.7), (1.0, 40.7), (1.0, 49.49), (7.93, 49.49)])
track(F, "/P3+", [(29.74, 50.67), (26.2, 50.67)]); via("/P3+", 26.2, 50.67)
track(B, "/P3+", [(26.2, 50.67), (26.04, 50.51), (9.71, 50.51)])
track(F, "GND", [(24.26, 46.86), (22.9, 46.86)]); via("GND", 22.9, 46.86)      # G low
track(F, "GND", [(29.74, 48.13), (31.4, 48.13)]); via("GND", 31.4, 48.13)      # ~G low
track(F, "GND", [(24.26, 51.94), (26.2, 51.94)]); via("GND", 26.2, 51.94)      # U2 GND
track(F, "GND", [(33.8, 42.58), (33.8, 44.0)]); via("GND", 33.8, 44.0)         # C1
track(F, "+5V", [(33.8, 40.5), (31.5, 40.5), (31.5, 43.05), (29.74, 43.05)], 0.4)   # C1 -> U2 VCC

# ---- EEPROM (I2C on the back layer straight from the header pins) -----------------
track(B, "/SDA1", [pipin(3), (11.41, 19.0), (19.0, 19.0)]); via("/SDA1", 19.0, 19.0); track(F, "/SDA1", [(19.0, 19.0), (20.0, 19.0)])
track(F, "/SCL1", [pipin(5), (13.95, 20.27), (20.0, 20.27)])
track(F, "+3V3", [pipin(17), (29.19, 27.0), (22.0, 27.0), (22.0, 24.462), (20.0, 24.462), (20.0, 22.81)], 0.4)   # pin 17 -> C2 -> VCC
track(F, "+3V3", [(22.0, 27.0), (22.0, 29.5), (12.4, 29.5), (12.4, 25.325)], 0.4)                                # ties the JP1 rail to it
track(F, "GND", [(39.35, 5.27), (39.35, 7.5)]); via("GND", 39.35, 7.5)                                            # pin 25 island
track(F, "GND", [(20.0, 26.538), (20.0, 27.8)]); via("GND", 20.0, 27.8)
track(F, "GND", [(25.2, 19.0), (25.2, 22.81), (25.2, 24.3)]); via("GND", 25.2, 24.3)    # A0-A2, GND
track(F, "/WP", [(20.0, 21.54), (18.9, 21.54)]); via("/WP", 18.9, 21.54)
track(B, "/WP", [(18.9, 21.54), (18.9, 23.9), (16.0, 23.9)]); via("/WP", 16.0, 23.9)
track(F, "/WP", [(16.0, 23.9), (16.0, 25.325), (13.7, 25.325)])
track(F, "GND", [(16.0, 26.975), (16.0, 28.2)]); via("GND", 16.0, 28.2)
track(F, "+3V3", [pipin(1), (8.87, 25.325), (12.4, 25.325)], 0.4)

# ---- 12 V input -> 5 V ------------------------------------------------------------
track(F, "/12V_IN", [(54.5, 61.5), (54.5, 59.2), (61.35, 59.2), (61.35, 54.5)], 0.8)   # J4 -> F1
track(F, "/12V_F", [(57.65, 54.5), (55.5, 54.5)], 0.8)                                  # F1 -> D1 anode
track(F, "+12V", [(50.5, 54.5), (50.5, 48.525)], 0.8)                                    # D1 cathode -> 12 V bus
track(F, "+12V", [(49.0, 48.525), (55.4, 48.525)], 0.8)                                   # bus through C3-C5
track(F, "+12V", [(51.46, 38.0), (51.46, 48.525)], 0.8)                                  # bus -> U4 Vin
track(F, "+5V", [(56.54, 38.0), (56.54, 33.8), (57.462, 33.8)], 0.8)                    # U4 Vout -> C6
track(F, "GND", [(59.538, 33.8), (60.8, 33.8)], 0.5); via("GND", 60.8, 33.8)
# 5 V distribution on the back layer: C6 -> along y=34 -> left edge -> Pi pins 2 and 4, branch to C1
track(F, "+5V", [(57.462, 33.8), (57.462, 32.2)], 0.8); via("+5V", 57.462, 32.2)
track(B, "+5V", [(57.462, 32.2), (57.462, 34.0), (7.2, 34.0), (7.2, 1.5), (8.87, 1.5), (8.87, 2.73)], 0.8)
track(B, "+5V", [(8.87, 1.5), (11.41, 1.5), (11.41, 2.73)], 0.5)
track(B, "+5V", [(35.0, 34.0), (35.0, 38.2)], 0.5); via("+5V", 35.0, 38.2)
track(F, "+5V", [(35.0, 38.2), (35.0, 39.3), (33.8, 40.5)], 0.4)

# ---- ground stitching ------------------------------------------------------------------
for (x, y) in [(56.0, 20.0), (61.5, 45.0), (44.0, 48.5), (47.5, 55.5), (20.0, 38.7), (15.0, 43.0), (20.0, 43.5), (25.0, 56.0),
               (43.0, 20.0), (33.0, 10.0), (10.0, 30.0), (36.0, 58.0), (12.0, 62.0), (45.0, 62.0)]:
    via("GND", x, y)

# ---- zones ---------------------------------------------------------------------------------
def zone_rect(layer, x1, y1, x2, y2, netname, name):
    z = pcbnew.ZONE(board); z.SetNet(net(netname)); z.SetLayer(layer)
    z.SetPadConnection(pcbnew.ZONE_CONNECTION_THERMAL); z.SetLocalClearance(FromMM(0.3)); z.SetMinThickness(FromMM(0.25))
    z.SetThermalReliefGap(FromMM(0.4)); z.SetThermalReliefSpokeWidth(FromMM(0.5)); z.SetZoneName(name)
    o = z.Outline(); o.NewOutline()
    for x, y in ((x1, y1), (x2, y1), (x2, y2), (x1, y2)):
        o.Append(FromMM(x), FromMM(y))
    board.Add(z); return z
zone_rect(F, 0.5, 0.5, W - 0.5, H - 0.5, "GND", "GND_top")
zone_rect(B, 0.5, 0.5, W - 0.5, H - 0.5, "GND", "GND_bottom")

def text(s, x, y, size=1.0, layer=pcbnew.F_SilkS, angle=0):
    t = pcbnew.PCB_TEXT(board); t.SetText(s); t.SetPosition(MM(x, y)); t.SetLayer(layer)
    t.SetTextSize(MM(size, size)); t.SetTextThickness(FromMM(0.15 if size <= 1 else 0.2)); t.SetTextAngleDegrees(angle); t.SetMirrored(layer == pcbnew.B_SilkS); board.Add(t)
text("FPP RS-422 pHAT rev C  (Pi Zero 2 W underneath, SD card <-)", 33.0, 12.0, 1.0)
text("pin 1", 8.9, 8.2, 0.8)
text("pin 1", 8.9, 8.2, 0.8, layer=pcbnew.B_SilkS)
text("latch up", 8.8, 40.9, 0.8)
text("12V IN  + -", 46.0, 62.0, 0.8)
text("NO USB POWER ON PI WHEN 12V IS CONNECTED", 26.0, 64.9, 0.8)
text("RJ45 pin 1 at bottom", 33.0, 56.9, 0.8)
text("P1=1,2 P2=3,6 P3=4,5 P4=7,8", 33.0, 58.6, 0.8)
text("JP1 close = EEPROM write protect", 13.0, 29.6, 0.8)
text("DPI D0/D3/D1/D2 = P1-7/26/29/31", 46.0, 30.0, 0.8)

filler = pcbnew.ZONE_FILLER(board); filler.Fill(board.Zones())
out = os.path.join(HERE, f"{PROJECT}.kicad_pcb"); pcbnew.SaveBoard(out, board)
for ref in ("J3", "U3", "J1", "U2", "U4", "D1", "F1", "J4", "JP1"):
    fp = fps[ref]
    print(ref, "rot", fp.GetOrientationDegrees(), [(p.GetNumber(), round(p.GetPosition().x / 1e6, 2), round(p.GetPosition().y / 1e6, 2), p.GetNetname()) for p in fp.Pads() if p.GetNumber() in ("1", "2", "3", "8", "9", "16", "40")])
print("saved", out)
