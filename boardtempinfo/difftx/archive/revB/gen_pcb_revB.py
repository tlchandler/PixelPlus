"""
Rev B (JLCPCB assembly) board generator. Run with KiCad 10's Python:

  "C:\Program Files\KiCad\10.0\bin\python.exe" gen_pcb.py

Reads difftx.net (pad -> net) and difftx.kicad_sch (symbol uuids, fields) so
the board matches the schematic exactly (DRC schematic parity).

Board 62 x 58 mm, 2 layers.
  U1  Raspberry Pi Pico 2 W, reflowed on its castellations (JLC C42394205)
  J1  Ckmtw R-RJ45R08P-A004 right-angle RJ45, opening at the left edge (C385834)
  U2  AM26C31IDR SOIC-16 (C34923), vertical, pins 1-8 on the left column
  C1  100nF 0805 (C49678), C2 10uF 0805 (C15850), J2 1x2 header (C32713268)
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
    f = None
    if name in std:
        f = fp.GetField(std[name])
    else:
        for g in fp.GetFields():
            if g.GetName() == name:
                f = g
    if f is None:
        fid = std.get(name, pcbnew.FIELD_T_USER)
        nf = pcbnew.PCB_FIELD(fp, fid, name)
        nf.SetText(value); nf.SetVisible(False); nf.SetLayer(pcbnew.F_Fab)
        fp.Add(nf)
        for g in fp.GetFields():
            if g.GetName() == name:
                f = g
    f.SetText(value)
    f.SetVisible(False)

board = pcbnew.BOARD()
ds = board.GetDesignSettings()
ds.m_CopperEdgeClearance = FromMM(0.3)
ds.m_MinClearance = FromMM(0.2)
ds.m_TrackMinWidth = FromMM(0.2)
ds.m_ViasMinSize = FromMM(0.6)
ds.m_MinThroughDrill = FromMM(0.3)

nets = {}
def net(name):
    if name not in nets:
        n = pcbnew.NETINFO_ITEM(board, name); board.Add(n); nets[name] = n
    return nets[name]
for name in sorted(set(padnet.values())):
    net(name)

fps = {}
def add_fp(lib, name, ref, value, x, y, want=None):
    path = os.path.join(HERE, "jlc", "jlc.pretty") if lib == "jlc" else (os.path.join(HERE, f"{PROJECT}.pretty") if lib == PROJECT else os.path.join(FPLIB, lib + ".pretty"))
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

# ---- placement -----------------------------------------------------------
add_fp("jlc", "COMM-SMD_L51.0-W21.0-P2.54_PICOW", "U1", "Pico2W", 27.35, 20.11,
       {"1": (3.3, 29.8), "20": (51.56, 29.8), "40": (3.3, 10.42), "21": (51.56, 10.42)})
add_fp("jlc", "RJ45-TH_R-RJ45R08P-A004", "J1", "R-RJ45R08P-A004", 8.82, 46.5,
       {"1": (7.93, 50.07), "2": (9.71, 49.05), "8": (9.71, 42.93)})
add_fp("jlc", "SOIC-16_L9.9-W3.9-P1.27-LS6.0-BL", "U2", "AM26C31IDR", 27.0, 44.0,
       {"1": (24.26, 39.55), "8": (24.26, 48.44), "16": (29.74, 39.55), "9": (29.74, 48.44)})
add_fp("Capacitor_SMD", "C_0805_2012Metric_Pad1.18x1.45mm_HandSolder", "C1", "100nF", 33.8, 38.04,
       {"1": (33.8, 37.0), "2": (33.8, 39.08)})
add_fp("Capacitor_SMD", "C_0805_2012Metric_Pad1.18x1.45mm_HandSolder", "C2", "10uF", 36.2, 38.04,
       {"1": (36.2, 37.0), "2": (36.2, 39.08)})
add_fp("jlc", "HDR-TH_2P-P2.54-V-M-3", "J2", "SCOPE", 53.0, 34.0, {"1": (51.73, 34.0), "2": (54.27, 34.0)})
HOLES = {"H1": (3.5, 3.5), "H2": (58.5, 3.5), "H3": (58.5, 54.3), "H4": (31.0, 54.3)}
for ref, (hx, hy) in HOLES.items():
    add_fp("MountingHole", "MountingHole_3.2mm_M3", ref, "M3", hx, hy)

# --- fix-ups on the imported JLC footprints ---------------------------------
# RJ45 board-lock pegs are plain holes (JLC draws them as plated pads with no annulus)
for pad in fps["J1"].Pads():
    if pad.GetNumber() == "":
        pad.SetAttribute(pcbnew.PAD_ATTRIB_NPTH)
        pad.SetLayerSet(pcbnew.LSET.AllCuMask() if False else pad.GetLayerSet())
# Pico courtyard from JLC is self-intersecting: replace it with a clean rectangle
u1 = fps["U1"]
for item in list(u1.GraphicalItems()):
    if item.GetLayer() == pcbnew.F_CrtYd:
        u1.Remove(item)
cy = pcbnew.PCB_SHAPE(u1); cy.SetShape(pcbnew.SHAPE_T_RECT)
cy.SetStart(MM(27.35 - 26.9, 20.11 - 10.8)); cy.SetEnd(MM(27.35 + 25.8, 20.11 + 10.8))
cy.SetLayer(pcbnew.F_CrtYd); cy.SetWidth(FromMM(0.05)); u1.Add(cy)
# keep the library copies in sync with these fix-ups (silences lib mismatch warnings)
for ref in ("J1", "U1"):
    c = fps[ref].Duplicate(False) if hasattr(fps[ref], "Duplicate") else None
    pcbnew.FootprintSave(os.path.join(HERE, "jlc", "jlc.pretty"), fps[ref])

def ref_at(ref, x, y, size=1.0):
    t = fps[ref].Reference()
    t.SetPosition(MM(x, y)); t.SetTextSize(MM(size, size)); t.SetTextThickness(FromMM(0.15))
    t.SetLayer(pcbnew.F_SilkS); t.SetVisible(True); t.SetTextAngleDegrees(0)
ref_at("U1", 27.0, 7.3)
ref_at("U2", 27.0, 35.9)
ref_at("J1", 8.8, 36.9)
ref_at("C1", 33.8, 42.2)
ref_at("C2", 36.2, 42.2)
ref_at("J2", 53.0, 31.6)
for ref in fps:
    fps[ref].Value().SetVisible(False)
for ref in HOLES:
    fps[ref].Reference().SetVisible(False)

# ---- outline -------------------------------------------------------------
W, H = 62.0, 58.0
def edge(x1, y1, x2, y2):
    s = pcbnew.PCB_SHAPE(board); s.SetShape(pcbnew.SHAPE_T_SEGMENT)
    s.SetStart(MM(x1, y1)); s.SetEnd(MM(x2, y2)); s.SetLayer(pcbnew.Edge_Cuts); s.SetWidth(FromMM(0.1)); board.Add(s)
edge(0, 0, W, 0); edge(W, 0, W, H); edge(W, H, 0, H); edge(0, H, 0, 0)

# ---- tracks & vias ---------------------------------------------------------
F, B = pcbnew.F_Cu, pcbnew.B_Cu
def track(layer, netname, pts, w=0.25):
    for (x1, y1), (x2, y2) in zip(pts, pts[1:]):
        t = pcbnew.PCB_TRACK(board)
        t.SetStart(MM(x1, y1)); t.SetEnd(MM(x2, y2)); t.SetWidth(FromMM(w)); t.SetLayer(layer); t.SetNet(net(netname))
        board.Add(t)
def via(netname, x, y):
    v = pcbnew.PCB_VIA(board)
    v.SetPosition(MM(x, y)); v.SetWidth(FromMM(0.8)); v.SetDrill(FromMM(0.4)); v.SetNet(net(netname))
    v.SetLayerPair(F, B); board.Add(v)

# inputs (front), ordered so nothing crosses in the corridor under the Pico
track(F, "/GP2",  [(10.92, 29.8), (10.92, 33.25), (22.6, 33.25), (22.6, 39.55), (24.26, 39.55)])            # 1A
track(F, "/GP3",  [(13.46, 29.8), (13.46, 32.75), (25.0, 32.75), (25.0, 34.2)]); via("/GP3", 25.0, 34.2)     # 4A via interior
track(B, "/GP3",  [(25.0, 34.2), (25.0, 36.4), (27.8, 36.4), (27.8, 40.82)]); via("/GP3", 27.8, 40.82)
track(F, "/GP3",  [(27.8, 40.82), (29.74, 40.82)])
track(F, "/GP4",  [(16.0, 29.8), (16.0, 32.25), (38.5, 32.25), (38.5, 48.44), (29.74, 48.44)])              # 3A
track(F, "/GP5",  [(18.54, 29.8), (18.54, 31.75), (39.5, 31.75), (39.5, 50.0), (22.9, 50.0), (22.9, 47.17), (24.26, 47.17)])  # 2A
track(F, "/GP15", [(51.56, 29.8), (51.56, 33.83), (51.73, 34.0)])

# 5 V: VBUS -> under the Pico -> right side -> via -> C2, C1, U2 pin 16
track(F, "+5V", [(3.3, 10.42), (3.3, 13.3), (42.67, 13.3), (42.67, 36.5)], 0.5); via("+5V", 42.67, 36.5)
track(B, "+5V", [(42.67, 36.5), (37.1, 36.5), (36.2, 35.6)], 0.5); via("+5V", 36.2, 35.6)
track(F, "+5V", [(36.2, 35.6), (36.2, 37.0)], 0.5)
track(F, "+5V", [(36.2, 35.6), (33.8, 35.6), (33.8, 37.0)], 0.5)
track(F, "+5V", [(33.8, 37.0), (31.5, 37.0), (31.5, 39.55), (29.74, 39.55)], 0.4)

# left-column outputs on the front
track(F, "/P4-", [(24.26, 42.09), (20.8, 42.09), (20.8, 42.93), (9.71, 42.93)])                 # 1Z -> pin 8
track(F, "/P1-", [(24.26, 44.63), (21.4, 44.63), (21.4, 49.05), (9.71, 49.05)])                 # 2Z -> pin 2
track(F, "/P1+", [(24.26, 45.90), (22.0, 45.90), (22.0, 55.3), (3.1, 55.3), (3.1, 50.07), (7.93, 50.07)])  # 2Y -> pin 1
track(F, "/P4+", [(24.26, 40.82), (22.7, 40.82)]); via("/P4+", 22.7, 40.82)                     # 1Y via
track(B, "/P4+", [(22.7, 40.82), (22.7, 37.7), (1.8, 37.7), (1.8, 43.95), (7.93, 43.95)])       # 1Y -> pin 7

# right-column outputs: stubs into the SOIC interior, vias, back layer to the jack
track(F, "/P2+", [(29.74, 42.09), (27.8, 42.09)]); via("/P2+", 27.8, 42.09)                     # 4Y
track(B, "/P2+", [(27.8, 42.09), (27.8, 55.8), (2.5, 55.8), (2.5, 48.03), (7.93, 48.03)])        # 4Y -> pin 3
track(F, "/P2-", [(29.74, 43.36), (26.2, 43.36)]); via("/P2-", 26.2, 43.36)                     # 4Z
track(B, "/P2-", [(26.2, 43.36), (24.6, 44.97), (9.71, 44.97)])                                 # 4Z -> pin 6
track(F, "/P3-", [(29.74, 45.90), (26.2, 45.90)]); via("/P3-", 26.2, 45.90)                     # 3Z
track(B, "/P3-", [(26.2, 45.90), (27.0, 45.1), (27.0, 37.2), (1.0, 37.2), (1.0, 45.99), (7.93, 45.99)])  # 3Z -> pin 5
track(F, "/P3+", [(29.74, 47.17), (26.2, 47.17)]); via("/P3+", 26.2, 47.17)                     # 3Y
track(B, "/P3+", [(26.2, 47.17), (26.04, 47.01), (9.71, 47.01)])                                 # 3Y -> pin 4

# ground: enables, U2 GND, capacitor returns -> vias into the back pour
track(F, "GND", [(24.26, 43.36), (22.9, 43.36)]); via("GND", 22.9, 43.36)      # G (pin 4) low
track(F, "GND", [(29.74, 44.63), (31.4, 44.63)]); via("GND", 31.4, 44.63)      # ~G (pin 12) low
track(F, "GND", [(24.26, 48.44), (26.2, 48.44)]); via("GND", 26.2, 48.44)      # pin 8
track(F, "GND", [(33.8, 39.08), (33.8, 40.5)]); via("GND", 33.8, 40.5)         # C1
track(F, "GND", [(36.2, 39.08), (36.2, 40.5)]); via("GND", 36.2, 40.5)         # C2
for x in (8.38, 21.08, 33.78, 46.48):                                          # pads 3, 8, 13, 18
    yv = 27.7 if x > 43 else 27.2                                               # stay out of the antenna keep-out
    track(F, "GND", [(x, 29.8), (x, yv)]); via("GND", x, yv)
for x in (8.38, 33.78, 46.48):                                                 # pads 38, 28, 23
    track(F, "GND", [(x, 10.42), (x, 8.0)]); via("GND", x, 8.0)
for (x, y) in [(56.0, 20.0), (56.0, 45.0), (1.6, 34.0), (44.0, 45.0), (20.0, 35.2), (45.0, 52.0),
               (25.0, 52.5), (15.0, 39.5), (20.0, 40.0)]:                     # last three bridge pour islands
    via("GND", x, y)                                                            # stitching

# ---- zones -----------------------------------------------------------------
def zone_rect(layer, x1, y1, x2, y2, netname=None, keepout=False, name=""):
    z = pcbnew.ZONE(board)
    if netname:
        z.SetNet(net(netname))
    z.SetLayer(layer)
    if keepout:
        z.SetIsRuleArea(True)
        ls = pcbnew.LSET(); ls.addLayer(pcbnew.F_Cu); ls.addLayer(pcbnew.B_Cu); z.SetLayerSet(ls)
        for fn in ("SetDoNotAllowZoneFills", "SetDoNotAllowCopperPour"):
            if hasattr(z, fn):
                getattr(z, fn)(True)
        z.SetDoNotAllowTracks(True); z.SetDoNotAllowVias(True); z.SetDoNotAllowPads(False); z.SetDoNotAllowFootprints(False)
    else:
        z.SetPadConnection(pcbnew.ZONE_CONNECTION_THERMAL)
        z.SetLocalClearance(FromMM(0.3)); z.SetMinThickness(FromMM(0.25))
        z.SetThermalReliefGap(FromMM(0.4)); z.SetThermalReliefSpokeWidth(FromMM(0.5))
    z.SetZoneName(name)
    o = z.Outline(); o.NewOutline()
    for x, y in ((x1, y1), (x2, y1), (x2, y2), (x1, y2)):
        o.Append(FromMM(x), FromMM(y))
    board.Add(z)
    return z
zone_rect(F, 43.2, 13.2, 54.0, 27.0, keepout=True, name="antenna_keepout")
zone_rect(F, 0.5, 0.5, W - 0.5, H - 0.5, "GND", name="GND_top")
zone_rect(B, 0.5, 0.5, W - 0.5, H - 0.5, "GND", name="GND_bottom")

# ---- silkscreen text ---------------------------------------------------------
def text(s, x, y, size=1.0, layer=pcbnew.F_SilkS, angle=0):
    t = pcbnew.PCB_TEXT(board); t.SetText(s); t.SetPosition(MM(x, y)); t.SetLayer(layer)
    t.SetTextSize(MM(size, size)); t.SetTextThickness(FromMM(0.15 if size <= 1 else 0.2)); t.SetTextAngleDegrees(angle); board.Add(t)
text("PICO 2W RS-422 PIXEL DRIVER  rev B", 31, 3.2, 1.2)
text("USB", 11.5, 6.0, 1.0)
text("<- antenna, keep clear", 48.5, 6.0, 0.8)
text("RJ45 pin 1 at bottom: P1=1,2 P2=3,6 P3=4,5 P4=7,8", 21.0, 57.0, 0.8)
text("GP15", 51.2, 36.4, 0.8)
text("GND", 55.2, 36.4, 0.8)
text("JLCPCB rev B", 50.0, 50.0, 0.8)

filler = pcbnew.ZONE_FILLER(board); filler.Fill(board.Zones())
out = os.path.join(HERE, f"{PROJECT}.kicad_pcb")
pcbnew.SaveBoard(out, board)
for ref in ("U1", "J1", "U2", "C1", "C2", "J2"):
    fp = fps[ref]
    print(ref, "rot", fp.GetOrientationDegrees(), [(p.GetNumber(), round(p.GetPosition().x / 1e6, 2), round(p.GetPosition().y / 1e6, 2), p.GetNetname()) for p in fp.Pads() if p.GetNumber() in ("1", "2", "8", "16")])
print("saved", out)
