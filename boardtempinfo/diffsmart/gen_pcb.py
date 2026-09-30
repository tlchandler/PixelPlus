r"""
diffsmart rev C board generator. Run with KiCad 10's own Python:

  "C:\Program Files\KiCad\10.0\bin\python.exe" gen_pcb.py

Board 130 x 100 mm, R3 corners, FOUR layers, 1 oz on every layer. The stack-up is written into the board
file so the fab cannot substitute the 0.5 oz inner default JLCPCB ships on 4 layers, and so are the DRC
constraints - anything set only in the .kicad_pro is reset to KiCad's defaults the next time the CLI
opens the project, which silently loosened min_connection, the annular ring and the silk text height.

  F.Cu    components, signal routing, the per-fuse +12 V islands and the /12VIN island
  In1.Cu  solid GND plane
  In2.Cu  solid +12 V plane   <- this is what actually carries the 30 A to the twelve fuses
  B.Cu    GND fill plus signal routing

Two inner planes is the whole point of going to four layers: a 130 mm wide 1 oz plane carries 30 A with a
negligible rise, every through-hole fuse and terminal pad lands straight on it, and both outer layers stay
free for the eight cat5 conductors and the four pixel data lines.

Power path: J1 (12 V 30 A in) -> /12VIN islands on both outer layers -> F0, a Keystone 3557-2 ATO blade
fuse holder -> +12 V bus. The supply negative returns through /GNDIN islands, also on both outer layers,
to the four parallel AOD4184A low-side FETs whose common source is board GND; reverse polarity leaves
them off and the board dead, and LED7 across them is then the only thing that lights.

Rev C additions, all of which work with no MCU on the board:
  - a trip LED and a 4.7 k across every output fuse: dark while the fuse conducts, lit once it opens;
  - an NTC divider into both halves of one LM2903 - fan on at about 43 C, OVER TEMP LED at about 65 C;
  - a fused, switched fan output on the left edge (the fan belongs on the enclosure wall, not the board);
  - two LM75B temperature sensors and an I2C header whose VIO pin powers them, so a 3.3 V or a 5 V master
    sets the bus level and no level shifter is ever needed;
  - eight test points, a bleeder across C1 and fourteen 3 mm zip-tie holes.

Vertical layout (y down):
   0.75 -  8.54   six 3-pole output terminals, wire entry at the top edge   (J2..J7)
   8.6  - 14.5    pole marks and the zip-tie holes
  14.5  - 17.5    six polyfuses standing on edge, one per terminal          (F1..F6)
  16.5 - 19.5     one +12 V island per fuse, in parallel with the In2 plane
  19.5 - 28       port labels, trip LEDs, test points, the I2C header and the ambient sensor
  28   - 48       the block the fuse holder freed: F0, the thermal block, the reverse-polarity LED
  48   - 71       middle band: RJ45 + receiver on the left, FET bank and power entry on the right
  71   - 80       fan drive, test points, bottom port labels and trip LEDs
  80.5 - 83.5     one +12 V island per fuse
  82.5 - 85.5     six polyfuses                                             (F7..F12)
  85.5 - 91.4     pole marks and zip-tie holes
  91.46- 99.25    six output terminals, wire entry at the bottom edge       (J8..J13)

The +12 V islands are per-fuse rather than one bar per row for a routing reason: as a full-width bar the
zone was a wall between each trip resistor and its own output's track, and B.Cu underneath is already
full of the cat5 trunk. Per-fuse islands keep the local copper and leave a 6 mm lane between terminals.

Terminal columns are at x = 15, 35, 55, 75, 95, 115 and every terminal reads + / D / - left to right.
Top row: J2 = port 1 data, J3/J4 = port 1 injection, J5 = port 2 data, J6/J7 = port 2 injection.
Bottom row: J11 = port 4 data, J12/J13 = port 4 injection, J8 = port 3 data, J9/J10 = port 3 injection.

This generator strips the silkscreen outline from the terminals, fuses, jack, header and every passive:
those outlines tell an assembler nothing the port labels do not, and rev C needs the room. That is why
lib_footprint_mismatch is expected on about 66 parts - the board differs from the libraries on purpose.
"""
import os, re
import pcbnew
try:                                   # a stray wx assert pops a modal dialog and hangs a headless run
    import wx
    wx.DisableAsserts()
except Exception:
    pass
from pcbnew import VECTOR2I_MM as MM, FromMM

HERE = os.path.dirname(os.path.abspath(__file__))
FPLIB = r"C:\Program Files\KiCad\10.0\share\kicad\footprints"
PROJECT = "diffsmart"
W, H = 160.0, 130.0

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
    sym_fields[r] = {k: v for k, v in re.findall(r'\(property "(Datasheet|Description|LCSC|Value)" "([^"]*)"', b)}
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
# JLCPCB's published limits for a 4-layer board in 1 oz outer copper, held in the board file because
# anything set only in the .kicad_pro is reset to KiCad's defaults the next time the CLI opens it.
ds.m_CopperEdgeClearance = FromMM(0.3); ds.m_MinClearance = FromMM(0.2); ds.m_TrackMinWidth = FromMM(0.2)
ds.m_ViasMinSize = FromMM(0.6); ds.m_MinThroughDrill = FromMM(0.3)
ds.m_MinConn = FromMM(0.2)                 # 0.0 lets a 0.1 mm neck into a pad pass unnoticed
ds.m_ViasMinAnnularWidth = FromMM(0.15)
ds.m_HoleClearance = FromMM(0.30); ds.m_HoleToHoleMin = FromMM(0.25)   # JLCPCB PTH-to-track is 0.28
ds.m_SilkClearance = FromMM(0.15)
ds.m_MinSilkTextHeight = FromMM(1.0); ds.m_MinSilkTextThickness = FromMM(0.15)
# One spoke off the B.Cu shield fill is not a hanging pad: every ground pad also lands on the solid
# In1 plane, and that fill uses thermal relief only so bottom-side pads stay hand-solderable.
ds.m_MinResolvedSpokes = 1
ds.SetAuxOrigin(MM(0.0, H))

nets = {}
def net(name):
    if name not in nets:
        n = pcbnew.NETINFO_ITEM(board, name); board.Add(n); nets[name] = n
    return nets[name]
for name in sorted(set(padnet.values())):
    net(name)

fps = {}
def add_fp(lib, name, ref, value, x, y, rot=0):
    path = {"jlc": os.path.join(HERE, "jlc", "jlc.pretty"),
            PROJECT: os.path.join(HERE, f"{PROJECT}.pretty")}.get(lib, os.path.join(FPLIB, lib + ".pretty"))
    fp = pcbnew.FootprintLoad(path, name)
    assert fp is not None, (lib, name)
    fp.SetReference(ref); fp.SetFPIDAsString(f"{lib}:{name}")
    fp.SetValue(sym_fields.get(ref, {}).get("Value", value))   # parity: the board must echo the schematic
    if ref in sym_uuid:
        fp.SetPath(pcbnew.KIID_PATH(f"/{root_uuid}/{sym_uuid[ref]}"))
        set_field(fp, "Datasheet", sym_fields[ref].get("Datasheet", ""))
        set_field(fp, "Description", sym_fields[ref].get("Description", ""))
        if sym_fields[ref].get("LCSC"):
            set_field(fp, "LCSC", sym_fields[ref]["LCSC"])
            # The jlc footprints carry their own "LCSC Part" property from whichever part the
            # library was built from - every red LED here still claimed the green one's C2297.
            # export_jlc.py reads "LCSC", but EasyEDA and JLC's own importer read "LCSC Part".
            set_field(fp, "LCSC Part", sym_fields[ref]["LCSC"])
        fp.SetExcludedFromBOM(not sym_fields[ref]["in_bom"])
    board.Add(fp)
    fp.SetOrientationDegrees(rot); fp.SetPosition(MM(x, y))
    for pad in fp.Pads():
        n = padnet.get((ref, pad.GetNumber()))
        if n:
            pad.SetNet(net(n))
    fps[ref] = fp
    return fp

def pad(ref, num):
    p = fps[ref].FindPadByNumber(str(num))
    return (round(p.GetPosition().x / 1e6, 4), round(p.GetPosition().y / 1e6, 4))

def trk(layer, netname, pts, w=0.25):
    for (x1, y1), (x2, y2) in zip(pts, pts[1:]):
        t = pcbnew.PCB_TRACK(board); t.SetStart(MM(x1, y1)); t.SetEnd(MM(x2, y2))
        t.SetWidth(FromMM(w)); t.SetLayer(layer); t.SetNet(net(netname)); board.Add(t)
# 0.6/0.3 gives a 0.15 mm annular ring, which is JLCPCB's absolute minimum for a 1 oz multilayer
# board; 0.7/0.3 is their recommended 0.20 mm and costs nothing but a little routing room.
def via(netname, x, y, d=0.7, drill=0.3):
    v = pcbnew.PCB_VIA(board); v.SetPosition(MM(x, y)); v.SetWidth(FromMM(d)); v.SetDrill(FromMM(drill))
    v.SetNet(net(netname)); v.SetLayerPair(F, B); board.Add(v)

# =====================================================================================================
# placement
# =====================================================================================================
TX = [15.0, 35.0, 55.0, 75.0, 95.0, 115.0]
TOP_TERM_Y, TOP_FUSE_Y = 4.5, 16.0
BOT_TERM_Y, BOT_FUSE_Y = 125.5, 114.0
TERM_FP = "CONN-TH_3P-P5.00_KF301-5.0-3P"
FUSE_FP = "FUSE-TH_L19.1-W3.0-P10.20-D1.2-S2.0"     # MF-R600, 19.1 mm disc on a 10.2 mm pitch

# Top row rotated 180 so the wire entry faces the top edge; that reverses its poles, which the schematic
# already accounts for (pole 3 = fused +12 V on the left, pole 1 = ground on the right).
TOP = [("J2", "F1", "P1 DATA"), ("J3", "F2", "P1 INJ A"), ("J4", "F3", "P1 INJ B"),
       ("J5", "F4", "P2 DATA"), ("J6", "F5", "P2 INJ A"), ("J7", "F6", "P2 INJ B")]
BOT = [("J11", "F10", "P4 DATA"), ("J12", "F11", "P4 INJ A"), ("J13", "F12", "P4 INJ B"),
       ("J8", "F7", "P3 DATA"), ("J9", "F8", "P3 INJ A"), ("J10", "F9", "P3 INJ B")]
for x, (j, f, name) in zip(TX, TOP):
    add_fp("jlc", TERM_FP, j, name, x, TOP_TERM_Y, rot=180)
    add_fp("jlc", FUSE_FP, f, "6A", x, TOP_FUSE_Y, rot=180)
for x, (j, f, name) in zip(TX, BOT):
    add_fp("jlc", TERM_FP, j, name, x, BOT_TERM_Y, rot=0)
    add_fp("jlc", FUSE_FP, f, "6A", x, BOT_FUSE_Y, rot=0)

# ---- power entry -----------------------------------------------------------------------------------
# F0 is the main fuse and everything downstream of it is what the board protects. It is a bolt-down
# MIDI/AMI automotive fuse on two M5 studs: the body lies flat above the board, so no component may sit
# in its courtyard but copper runs underneath freely.
# Pads 1/2 are one receptacle and 3/4 the other; 3/4 land inside the /12VIN island, 1/2 reach the
# +12 V bus through the In2 plane. A standard 30 A ATO blade fuse drops in.
add_fp("jlc", "FUSE-TH_4P-L19.8-W6.7_3557-2", "F0", "ATO 30A", 100.0, 33.0, rot=0)
add_fp("jlc", "CONN-TH_P9.50_KF950-9.5-2P", "J1", "12V 30A IN", 152.4, 49.0, rot=90)
QX = [68.0, 76.0, 84.0, 92.0]
for i, x in enumerate(QX):
    q = add_fp("jlc", "TO-252-2_L6.6-W6.1-P4.57-LS9.9-TL-CW", f"Q{i+1}", "AOD4184A", x, 56.0, rot=270)
    # The library's own 3D body sits about 1 mm off its pads, because the mesh is not centred on the
    # axis the footprint is. Use KiCad's TO-252-2 instead, shifted by the 1.9 mm between the two
    # footprints' origins. Cosmetic only - gerbers, drill and the CPL never look at the 3D model.
    q.Models().clear()
    m = pcbnew.FP_3DMODEL()
    m.m_Filename = "${KICAD10_3DMODEL_DIR}/Package_TO_SOT_SMD.3dshapes/TO-252-2.step"
    m.m_Offset = pcbnew.VECTOR3D(1.9, 0.0, 0.0)
    m.m_Scale = pcbnew.VECTOR3D(1.0, 1.0, 1.0)
    m.m_Rotation = pcbnew.VECTOR3D(0.0, 0.0, 0.0)
    q.Models().push_back(m)
add_fp("Resistor_SMD", "R_0603_1608Metric", "R1", "10k", 99.0, 53.5, rot=0)
add_fp("Resistor_SMD", "R_0603_1608Metric", "R2", "100k", 105.0, 53.5, rot=0)
add_fp("jlc", "SOD-123_L2.8-W1.8-LS3.7-RD", "D1", "BZT52C15", 110.5, 53.5, rot=0)
add_fp("jlc", "SMC_L6.9-W5.9-LS7.9-RD", "D2", "SMDJ15A", 70.0, 69.5, rot=0)
add_fp("jlc", "CAP-SMD_BD8.0-L8.3-W8.3-FD", "C1", "470uF 25V", 86.0, 69.5, rot=0)

# ---- 5 V supply ------------------------------------------------------------------------------------
add_fp("jlc", "SOT-223_L6.5-W3.5-P2.30-LS7.0-BR", "U3", "UA78M05", 110.0, 70.0, rot=180)
add_fp("Resistor_SMD", "R_0603_1608Metric", "R3", "10R", 100.0, 68.5, rot=0)
add_fp("Capacitor_SMD", "C_1206_3216Metric", "C2", "10uF", 100.0, 72.0, rot=0)
add_fp("Capacitor_SMD", "C_0805_2012Metric_Pad1.18x1.45mm_HandSolder", "C3", "100nF", 118.0, 67.5, rot=0)
add_fp("Capacitor_SMD", "C_0805_2012Metric_Pad1.18x1.45mm_HandSolder", "C4", "22uF", 118.0, 72.0, rot=0)
add_fp("Capacitor_SMD", "C_0805_2012Metric_Pad1.18x1.45mm_HandSolder", "C5", "100nF", 124.0, 70.0, rot=0)
add_fp("jlc", "SOD-123_L2.8-W1.8-LS3.7-RD", "D7", "BZT52C6V2", 46.0, 72.0, rot=0)
add_fp("Capacitor_SMD", "C_0805_2012Metric_Pad1.18x1.45mm_HandSolder", "C6", "100nF", 41.5, 45.55, rot=0)

# ---- receiver front end ----------------------------------------------------------------------------
j14 = add_fp("jlc", "RJ45-TH_R-RJ45R08P-A004", "J14", "RJ45", 8.36, 50.0, rot=270)
# The jack's two 3.4 mm pegs come out of the library as plated pads with no annular ring. Its signal
# pads go to 1.4 mm: the library's 1.524 mm leaves too little room to escape between the staggered rows,
# and the 1.25 mm this board carried in rev A gave a 0.168 mm annular ring, under JLCPCB's floor.
for p_ in j14.Pads():
    if not p_.GetNumber():
        p_.SetAttribute(pcbnew.PAD_ATTRIB_NPTH)
    else:
        p_.SetSize(pcbnew.F_Cu, MM(1.4, 1.4))
add_fp("jlc", "SOIC-16_L9.9-W3.9-P1.27-LS6.0-BL", "U1", "AM26C32IDR", 36.0, 50.0, rot=270)
# One PSM712 per pair, placed where that pair's trunk already runs.
for ref, x, y in (("D3", 20.0, 58.5), ("D4", 44.0, 58.0), ("D5", 20.0, 70.0), ("D6", 16.0, 38.0)):
    add_fp("jlc", "SOT-23-3_L3.0-W1.7-P0.95-LS2.9-BR", ref, "PSM712", x, y, rot=0)
for ref, x, y in (("RT1", 27.0, 58.5), ("RT2", 52.0, 58.0), ("RT3", 28.0, 70.0), ("RT4", 24.0, 38.0)):
    add_fp("Resistor_SMD", "R_1206_3216Metric", ref, "120R", x, y, rot=0)
BIAS = {"RB1": (34.0, 58.5), "RB2": (39.0, 58.5),      # P1: A to GND, B to +5V
        "RB3": (54.0, 55.0), "RB4": (58.0, 55.0),      # P2
        "RB5": (35.0, 70.0), "RB6": (40.0, 70.0),      # P3
        "RB7": (31.0, 38.0), "RB8": (36.0, 38.0)}      # P4
for ref, (x, y) in BIAS.items():
    add_fp("Resistor_SMD", "R_0603_1608Metric", ref, "1k", x, y, rot=0)

for i, (x, y, rot) in enumerate([(28.0, 50.63, 180), (52.0, 51.9, 0), (52.0, 49.37, 0), (28.0, 48.1, 180)]):
    add_fp("Resistor_SMD", "R_0603_1608Metric", f"RS{i+1}", "470R", x, y, rot=rot)
# one row of indicators along the top of the middle band, where they are visible next to the terminals
LEDPOS = {"LED1": (10.0, 31.0, 0), "LED2": (21.0, 31.0, 0), "LED3": (32.0, 31.0, 0),
          "LED4": (43.0, 31.0, 0), "LED5": (54.0, 31.0, 0)}
RLPOS = {"RL1": (15.0, 31.0, 180), "RL2": (26.0, 31.0, 180), "RL3": (37.0, 31.0, 180),
         "RL4": (48.0, 31.0, 180), "RL5": (59.0, 31.0, 180)}
for ref, (x, y, rot) in LEDPOS.items():
    add_fp("jlc", "LED0805-R-RD", ref, "LED", x, y, rot=rot)
for ref, (x, y, rot) in RLPOS.items():
    add_fp("Resistor_SMD", "R_0603_1608Metric", ref, "1k", x, y, rot=rot)

# ---- fuse-trip indicators -------------------------------------------------------------------------
# One LED and one 4.7 k across each polyfuse, in the band the shrunken +12 V zones freed. Dark while the
# fuse conducts; lit at about 2 mA once it opens and the load drags its downstream side away from the bus.
for x, (j, f, name) in zip(TX, TOP):
    n = f[1:]
    add_fp("jlc", "LED0805-R-RD", f"LF{n}", "red", x, 21.5, rot=0)
    add_fp("Resistor_SMD", "R_0603_1608Metric", f"RF{n}", "4.7k", x, 24.5, rot=0)
for x, (j, f, name) in zip(TX, BOT):
    n = f[1:]
    add_fp("jlc", "LED0805-R-RD", f"LF{n}", "red", x, 108.5, rot=180)
    add_fp("Resistor_SMD", "R_0603_1608Metric", f"RF{n}", "4.7k", x, 105.5, rot=180)

# ---- thermal block, in the space the bolt-down fuse used to take ------------------------------------
# U4 and the NTC sit between the FET bank below and the main fuse to the right - the hot corner.
add_fp("jlc", "SOIC-8_L5.0-W4.0-P1.27-LS6.0-BL", "U4", "LM75BD", 68.0, 26.0, rot=90)
add_fp("Capacitor_SMD", "C_0805_2012Metric_Pad1.18x1.45mm_HandSolder", "C7", "100nF", 60.0, 26.0, rot=0)
add_fp("jlc", "SOIC-8_L4.9-W3.9-P1.27-LS6.0-BL", "U6", "LM2903QDRQ1", 68.0, 38.0, rot=90)
add_fp("Capacitor_SMD", "C_0805_2012Metric_Pad1.18x1.45mm_HandSolder", "C9", "100nF", 62.0, 42.0, rot=0)
add_fp("Resistor_SMD", "R_0805_2012Metric", "RT5", "10k NTC", 82.0, 30.0, rot=0)
for ref, val, x, y in (("R29", "10k", 76.0, 30.0), ("R30", "10k", 76.0, 36.0), ("R31", "4.7k", 82.0, 36.0),
                       ("R32", "100k", 88.0, 36.0), ("R33", "4.7k", 76.0, 42.0), ("R34", "4.7k", 54.0, 34.0),
                       ("R35", "1k", 58.0, 34.0), ("R37", "100k", 82.0, 42.0)):
    add_fp("Resistor_SMD", "R_0603_1608Metric", ref, val, x, y, rot=0)
add_fp("jlc", "LED0805-R-RD", "LED6", "red", 58.0, 38.0, rot=0)
add_fp("Resistor_SMD", "R_0603_1608Metric", "R36", "1k", 52.0, 38.0, rot=0)

# ---- reverse-polarity indicator, beside the input where you would look for it -----------------------
add_fp("jlc", "LED0805-R-RD", "LED7", "red", 101.0, 45.5, rot=0)
add_fp("Resistor_SMD", "R_0603_1608Metric", "R28", "4.7k", 101.0, 49.5, rot=0)
add_fp("Resistor_SMD", "R_0603_1608Metric", "R27", "10k", 93.0, 69.5, rot=0)

# ---- fan output on the left edge, clear of the power section ---------------------------------------
add_fp("jlc", "CONN-TH_3P-P5.00_KF301-5.0-3P", "J16", "FAN 12V", 7.0, 72.0, rot=270)
add_fp("jlc", "SOT-23-3_L2.9-W1.3-P1.90-LS2.4-BR", "Q5", "AO3400A", 23.0, 77.0, rot=0)
add_fp("jlc", "SMA_L4.3-W2.6-LS5.2-RD", "D8", "SS34", 29.0, 77.0, rot=0)
add_fp("Fuse", "Fuse_1206_3216Metric", "F13", "0.35A/30V", 45.0, 77.0, rot=0)

# ---- I2C sensors and header -------------------------------------------------------------------------
# U5 goes in the top-left corner, the coolest part of the board: nothing there dissipates more than the
# receiver's 75 mW. The difference between it and U4 is the number worth reading.
add_fp("jlc", "SOIC-8_L5.0-W4.0-P1.27-LS6.0-BL", "U5", "LM75BD", 20.0, 100.0, rot=0)
add_fp("Capacitor_SMD", "C_0805_2012Metric_Pad1.18x1.45mm_HandSolder", "C8", "100nF", 28.0, 100.0, rot=0)
add_fp("jlc", "HDR-TH_4P-P2.54-V-M", "J15", "I2C", 105.0, 24.0, rot=0)
add_fp("Resistor_SMD", "R_0603_1608Metric", "R39", "4.7k", 99.0, 28.0, rot=0)
add_fp("Resistor_SMD", "R_0603_1608Metric", "R40", "4.7k", 111.0, 28.0, rot=0)

# ---- test points -------------------------------------------------------------------------------------
for ref, x, y in (("TP1", 52.0, 72.0), ("TP2", 56.0, 72.0), ("TP3", 60.0, 72.0),
                  ("TP4", 45.0, 42.0), ("TP5", 52.0, 42.0), ("TP6", 56.0, 42.0),
                  ("TP7", 45.0, 45.0), ("TP8", 88.0, 30.0)):
    add_fp("TestPoint", "TestPoint_Pad_D1.5mm", ref, "TP", x, y, rot=0)

# ---- zip-tie strain relief ---------------------------------------------------------------------------
# Two holes per terminal, clear of the pole marks at TX-5/TX/TX+5. Loop a tie down one and up the other
# and it clamps the pigtail bundle onto the board instead of onto the screws.
ZIP = [(x, y) for y in (11.5, 118.5) for x in (5.0, 25.0, 45.0, 65.0, 85.0, 105.0, 125.0)]
for i, (x, y) in enumerate(ZIP):
    z_ = add_fp(PROJECT, "ZipTie_3.0mm", f"Z{i+1}", "ziptie", x, y)
    # Each of these is drilled through the solid GND and +12 V planes. The default 0.25 mm leaves no
    # margin against drill wander on a 3 mm hole, so push the planes back to 0.5 mm.
    for p_ in z_.Pads():
        p_.SetLocalClearance(FromMM(0.5))

# ---- the Pi column ------------------------------------------------------------------------------------
# The Pi is not on this board. J17 is a 2x20 male header and a ribbon runs to a Pi in the enclosure, so
# every model difftx supports works here - the 65 x 56 and 85 x 56 boards have nowhere to sit on a
# 160 x 130 PCB already carrying twelve screw terminals.
#
# The numbering is STRAIGHT THROUGH. A ribbon maps pin n to pin n, so the swap that was correct for an
# upside-down Pi would now put 5 V on the Pi's 3.3 V pin. Un-mirrored also puts every signal pin on the
# OUTER (west) column, with open board beside it, which is the easy direction to route.
HDR_X, HDR_Y = 132.77, 97.5
def hdrpin(n):
    """Standard 2x20 pin n, in board coordinates. Odd pins west, numbers running south."""
    return (HDR_X - 1.27 if n % 2 else HDR_X + 1.27, HDR_Y - 24.13 + 2.54 * ((n - 1) // 2))

# A project copy of the JLC part with 1.4 mm pads instead of 1.6: at 1.6 the gap between neighbours is
# 0.94 mm, narrower than a track plus its clearance, so the forty pins are a wall with no way in or out.
# 1.4 still leaves a 0.2 mm annular ring on the 1.0 mm drill, which is JLCPCB's recommended figure and
# is ample for a 0.64 mm square header pin.
_src = pcbnew.FootprintLoad(os.path.join(HERE, "jlc", "jlc.pretty"), "HDR-TH_40P-P2.54-V-F-R2-C20-S2.54")
assert _src is not None, "jlc 2x20 header"
for _p in _src.Pads():
    _p.SetSize(pcbnew.VECTOR2I(FromMM(1.4), FromMM(1.4)))
    _p.SetShape(pcbnew.PAD_SHAPE_RECT if _p.GetNumber() == "1" else pcbnew.PAD_SHAPE_CIRCLE)
_src.SetFPIDAsString(f"{PROJECT}:PiRibbon_2x20_P2.54mm")
_src.SetLibDescription(
    "2x20 2.54 mm male header (JLC C50980) for a 40-way ribbon to a Raspberry Pi. Pads are 1.4 mm "
    "rather than the vendor's 1.6 so the pin field can be routed out of; the 1.0 mm drill still leaves "
    "a 0.2 mm annular ring. Numbering is the vendor's own, straight through - NOT the swapped numbering "
    "of the socket this replaced, because a ribbon maps pin n to pin n. The same holes take a shrouded "
    "DC3-40P box header, which is keyed; the board keeps its shroud footprint clear.")
pcbnew.FootprintSave(os.path.join(HERE, f"{PROJECT}.pretty"), _src)
add_fp(PROJECT, "PiRibbon_2x20_P2.54mm", "J17", "2x20 hdr", HDR_X, HDR_Y, rot=270)
# A ribbon is only safe if pad n is where a standard 2x20 puts pin n. Odd on the west column, numbers
# running south, pin 1 at the north end - anything else and the cable is a mirror.
for _n in (1, 2, 3, 4, 39, 40):
    _pp = pad("J17", str(_n)); _ee = hdrpin(_n)
    assert abs(_pp[0] - _ee[0]) < 0.01 and abs(_pp[1] - _ee[1]) < 0.01, ("J17", _n, _pp, _ee)

# The 5 V link, west of the header and clear of the shroud a DC3-40P would need.
add_fp("Connector_PinHeader_2.54mm", "PinHeader_1x02_P2.54mm_Vertical", "JP1", "5V link", 126.0, 76.0, rot=0)

# Mode switch on the left edge: reachable with a finger, and a short hop to both of the things it
# enables - the receiver at (36, 50) and the buffer below it.
add_fp("jlc", "SW-TH_3P-L8.8-W3.9-P2.50_SS12D00G3", "SW1", "SS-12D00-G3", 6.0, 95.0, rot=90)
add_fp("Resistor_SMD", "R_0603_1608Metric", "R45", "10k", 14.0, 95.0, rot=0)

# Everything else that serves the Pi lives in the band the bottom block vacated, south of the two
# supply islands. Those islands are keepouts on both layers, so anything north of them is unreachable
# from the Pi column without a trip round the west side of the board.
add_fp("jlc", "SOIC-14_L8.7-W3.9-P1.27-LS6.0-BL", "U9", "74HCT125D", 80.0, 88.0, rot=0)
# Hard against U9's VCC corner rather than 8.6 mm west of it: this part switches four lines into
# 470 ohms and a cable, and the supply loop was the length of the package.
add_fp("Capacitor_SMD", "C_0805_2012Metric_Pad1.18x1.45mm_HandSolder", "C10", "100nF", 73.0, 85.4, rot=0)
# Each partner sits on the side its own pin faces. Channels 1 and 2 are on pins 2/3 and 5/6, which
# come out of the south face; channels 3 and 4 are on pins 8/9 and 11/12 on the north face. Putting
# all eight resistors in one column either side meant half of every buffer's traffic had to get round
# the package first, and whichever net routed last found the way blocked.
add_fp("Resistor_SMD", "R_0603_1608Metric", "RS5", "470R", 68.0, 100.0, rot=0)   # pin 3, south
add_fp("Resistor_SMD", "R_0603_1608Metric", "RS6", "470R", 76.0, 100.0, rot=0)   # pin 6, south
add_fp("Resistor_SMD", "R_0603_1608Metric", "R41", "10k", 84.0, 100.0, rot=0)    # pin 2, south
add_fp("Resistor_SMD", "R_0603_1608Metric", "R42", "10k", 92.0, 100.0, rot=0)    # pin 5, south
add_fp("Resistor_SMD", "R_0603_1608Metric", "RS7", "470R", 68.0, 76.0, rot=0)   # pin 8, north
add_fp("Resistor_SMD", "R_0603_1608Metric", "RS8", "470R", 76.0, 76.0, rot=0)   # pin 11, north
add_fp("Resistor_SMD", "R_0603_1608Metric", "R43", "10k", 84.0, 76.0, rot=0)    # pin 9, north
add_fp("Resistor_SMD", "R_0603_1608Metric", "R44", "10k", 92.0, 76.0, rot=0)    # pin 12, north
add_fp("jlc", "SOIC-8_L4.9-W3.9-P1.27-LS6.0-BL", "U8", "AT24C256C", 104.0, 88.0, rot=0)
add_fp("Capacitor_SMD", "C_0805_2012Metric_Pad1.18x1.45mm_HandSolder", "C11", "100nF", 104.0, 80.0, rot=0)

# The Pi's 5 V supply, beside the socket it feeds.
add_fp("jlc", "F1812", "F14", "1.5A", 112.0, 88.0, rot=0)
# Both input capacitors sit in the 3.5 mm band between C12 and the module body, so each is about
# 4 mm from pin 1 instead of 7.6 mm away behind the PPTC. The datasheet is explicit that C1 and C2
# must be as close as possible to the terminals.
add_fp("Capacitor_SMD", "C_1206_3216Metric", "C13", "10uF", 117.0, 85.9, rot=0)
add_fp("Capacitor_SMD", "C_1206_3216Metric", "C14", "10uF", 122.5, 85.9, rot=0)
add_fp("jlc", "PWRM-TH_K78XX-2000R3", "U7", "K7805-2000R3", 121.0, 90.0, rot=0)
add_fp("Capacitor_SMD", "C_0805_2012Metric_Pad1.18x1.45mm_HandSolder", "C12", "22uF", 121.0, 83.0, rot=0)

HOLES = [(4.0, 4.0), (156.0, 4.0), (4.0, 126.0), (156.0, 126.0)]
for i, (x, y) in enumerate(HOLES):
    add_fp("MountingHole", "MountingHole_3.2mm_M3", f"H{i+1}", "M3", x, y)
PI_HOLES = []

# =====================================================================================================
# copper planes and the high-current bus
# =====================================================================================================
def zone_rect(layer, x1, y1, x2, y2, netname, name, connection=pcbnew.ZONE_CONNECTION_FULL):
    z = pcbnew.ZONE(board); z.SetNet(net(netname)); z.SetLayer(layer)
    z.SetPadConnection(connection); z.SetLocalClearance(FromMM(0.3)); z.SetMinThickness(FromMM(0.25))
    z.SetThermalReliefGap(FromMM(0.35)); z.SetThermalReliefSpokeWidth(FromMM(0.8)); z.SetZoneName(name)
    z.SetIslandRemovalMode(pcbnew.ISLAND_REMOVAL_MODE_ALWAYS)
    o = z.Outline(); o.NewOutline()
    for x, y in ((x1, y1), (x2, y1), (x2, y2), (x1, y2)):
        o.Append(FromMM(x), FromMM(y))
    board.Add(z); return z

def zone_poly(layer, pts, netname, name, connection=pcbnew.ZONE_CONNECTION_FULL):
    """Same as zone_rect but for an outline that is not a rectangle."""
    z = pcbnew.ZONE(board); z.SetNet(net(netname)); z.SetLayer(layer)
    z.SetPadConnection(connection); z.SetLocalClearance(FromMM(0.3)); z.SetMinThickness(FromMM(0.25))
    z.SetThermalReliefGap(FromMM(0.35)); z.SetThermalReliefSpokeWidth(FromMM(0.8)); z.SetZoneName(name)
    z.SetIslandRemovalMode(pcbnew.ISLAND_REMOVAL_MODE_ALWAYS)
    o = z.Outline(); o.NewOutline()
    for x, y in pts:
        o.Append(FromMM(x), FromMM(y))
    board.Add(z); return z

# the two inner planes are what actually carry the 30 A
zone_rect(I1, 1.0, 1.0, W - 1.0, H - 1.0, "GND", "GND_plane")
zone_rect(I2, 1.0, 1.0, W - 1.0, H - 1.0, "+12V", "12V_plane")
# F.Cu +12 V under each fuse row, paralleling the inner plane
for i, x in enumerate(TX):
    zone_rect(F, x - 6.5, 16.5, x + 6.5, 19.5, "+12V", f"12V_top{i+1}")
    zone_rect(F, x - 6.5, 110.5, x + 6.5, 113.5, "+12V", f"12V_bot{i+1}")
# Supply side of the main fuse: from the input terminal to F0's right-hand stud. Nothing else on the
# board touches this net, which is the point - every other tap sits behind the fuse.
# It is also a 30 A conductor with no inner plane behind it (In2 is +12 V, i.e. the far side of the
# fuse), so like /GNDIN it gets a matching island on B.Cu and the two are stitched together. On one
# 1 oz outer layer 21 mm wide, IPC-2221 puts 30 A at about a 25 C rise; on two it is about half that.
zone_rect(F, 104.0, 30.0, 159.0, 51.0, "/12VIN", "12VIN_top").SetAssignedPriority(1)
zone_rect(B, 104.0, 30.0, 159.0, 51.0, "/12VIN", "12VIN_bottom").SetAssignedPriority(1)
# Supply ground: the input terminal to the drains of the reverse-polarity FETs. It is the one 30 A
# conductor with no inner plane behind it, so it is carried on both outer layers and stitched.
# J1 sits 4 mm north of the rest of the island to clear the Pi's microSD card, so the island reaches up
# to meet its negative pad - but only east of x = 140, which keeps it off the /VG gate bus crossing
# F.Cu at y = 55, and it stops 1 mm short of the /12VIN island above.
GNDIN_OUTLINE = [(62.0, 56.0), (140.0, 56.0), (140.0, 52.0), (158.0, 52.0), (158.0, 66.0), (62.0, 66.0)]
zone_poly(F, GNDIN_OUTLINE, "/GNDIN", "GNDIN_top")
zone_poly(B, GNDIN_OUTLINE, "/GNDIN", "GNDIN_bottom").SetAssignedPriority(1)
# board ground at the FET sources, stitched down to the inner plane
zone_rect(F, 62.0, 43.5, 98.0, 50.5, "GND", "GND_sources")
zone_rect(B, 1.0, 1.0, W - 1.0, H - 1.0, "GND", "GND_bottom", pcbnew.ZONE_CONNECTION_THERMAL)

# ---- copper keepout around the bare mounting holes ---------------------------------------------------
# A steel M3 screw and a metal standoff go through these. Without a keepout the +12 V plane comes within
# the bare 0.25 mm hole-clearance rule of the hole wall, and NPTH tolerance is +/-0.2 mm on top of that.
def hole_keepout(x, y, r=2.6):
    z = pcbnew.ZONE(board)
    z.SetIsRuleArea(True)
    z.SetZoneName(f"keepout_{x:.0f}_{y:.0f}")
    ls = pcbnew.LSET()
    for l in (F, I1, I2, B):
        ls.addLayer(l)
    z.SetLayerSet(ls)
    (getattr(z, "SetDoNotAllowZoneFills", None) or getattr(z, "SetDoNotAllowCopperPour"))(True)
    for meth in ("SetDoNotAllowTracks", "SetDoNotAllowVias", "SetDoNotAllowPads", "SetDoNotAllowFootprints"):
        if hasattr(z, meth):
            getattr(z, meth)(True if meth in ("SetDoNotAllowTracks", "SetDoNotAllowVias") else False)
    o = z.Outline(); o.NewOutline()
    import math
    for k in range(16):
        a = 2 * math.pi * k / 16
        o.Append(FromMM(x + r * math.cos(a)), FromMM(y + r * math.sin(a)))
    board.Add(z)
for hx, hy in HOLES:
    hole_keepout(hx, hy)
for px, py in ((4.93, 43.65), (4.93, 56.35)):        # the RJ45's two locating pegs
    hole_keepout(px, py, 2.7)

# All forty of the socket's holes pass through the solid +12 V plane at the bare hole-clearance rule.
# Twenty-four of them are unconnected Pi pins and the rest are 3V3, 5 V, I2C and GPIO, so one drill
# breakout on one of forty would put 12 V on a Pi GPIO. Both planes are pushed out of the whole field.
def plane_keepout(x1, y1, x2, y2, name):
    z = pcbnew.ZONE(board)
    z.SetIsRuleArea(True); z.SetZoneName(name)
    ls = pcbnew.LSET()
    for l in (I1, I2):
        ls.addLayer(l)
    z.SetLayerSet(ls)
    (getattr(z, "SetDoNotAllowZoneFills", None) or getattr(z, "SetDoNotAllowCopperPour"))(True)
    for meth in ("SetDoNotAllowTracks", "SetDoNotAllowVias", "SetDoNotAllowPads", "SetDoNotAllowFootprints"):
        if hasattr(z, meth):
            getattr(z, meth)(False)
    o = z.Outline(); o.NewOutline()
    for cx, cy in ((x1, y1), (x2, y1), (x2, y2), (x1, y2)):
        o.Append(FromMM(cx), FromMM(cy))
    board.Add(z)
_j17 = fps["J17"].GetBoundingBox(False, False)
plane_keepout(_j17.GetLeft() / 1e6 - 0.4, _j17.GetTop() / 1e6 - 0.4,
              _j17.GetRight() / 1e6 + 0.4, _j17.GetBottom() / 1e6 + 0.4, "pi_socket_plane_keepout")

# ---- board outline, 3 mm corners --------------------------------------------------------------------
def edge(x1, y1, x2, y2):
    s = pcbnew.PCB_SHAPE(board); s.SetShape(pcbnew.SHAPE_T_SEGMENT)
    s.SetStart(MM(x1, y1)); s.SetEnd(MM(x2, y2)); s.SetLayer(pcbnew.Edge_Cuts); s.SetWidth(FromMM(0.1)); board.Add(s)
def arc(cx, cy, sx, sy, angle):
    s = pcbnew.PCB_SHAPE(board); s.SetShape(pcbnew.SHAPE_T_ARC)
    s.SetCenter(MM(cx, cy)); s.SetStart(MM(sx, sy))
    s.SetArcAngleAndEnd(pcbnew.EDA_ANGLE(angle, pcbnew.DEGREES_T), False)
    s.SetLayer(pcbnew.Edge_Cuts); s.SetWidth(FromMM(0.1)); board.Add(s)
R = 3.0
# A plain rectangle. The notch that used to be cut out of this edge was there for the antenna of a Pi
# hanging over the board; with the Pi on a ribbon there is nothing to clear, and the fab does not have
# to rout an internal slot.
edge(R, 0, W - R, 0); edge(W - R, H, R, H); edge(0, H - R, 0, R)
edge(W, R, W, H - R)
arc(W - R, R, W - R, 0, 90); arc(W - R, H - R, W, H - R, 90); arc(R, H - R, R, H, 90); arc(R, R, 0, R, 90)

# ---- the fused output legs: fuse -> terminal, and fuse -> bus ----------------------------------------
# 3.0 mm at 1 oz is 162.8 mil^2, so by IPC-2221 external (k = 0.048) it carries 1.925 * dT^0.44 A:
# a 13 C rise at the fuse's 6 A hold current and 64 C at the 12 A it trips at, which the leg only
# sees for the second or so an MF-R600 takes to open.
for x, (j, f, _) in zip(TX, TOP):
    trk(F, "+12V", [(x + 5.10, 17.0), (x + 5.10, 20.0)], 2.4)
    v = padnet[(f, "2")]
    trk(F, v, [(x - 5.10, 15.0), (x - 5.10, 11.0), (x - 5.0, 8.0), (x - 5.0, 4.5)], 3.0)
for x, (j, f, _) in zip(TX, BOT):
    trk(F, "+12V", [(x - 5.10, 113.0), (x - 5.10, 110.0)], 2.4)
    v = padnet[(f, "2")]
    trk(F, v, [(x + 5.10, 115.0), (x + 5.10, 119.0), (x - 5.0, 122.0), (x - 5.0, 125.5)], 3.0)

# ---- reverse-polarity FETs ---------------------------------------------------------------------------
for x in QX:
    trk(F, "GND", [(x - 2.27, 52.5), (x - 2.27, 48.0)], 2.8)     # source up into the ground island
# gate bus, in the 1.9 mm gap between the FET leads and their tabs
trk(F, "/VG", [(70.27, 55.0), (109.0, 55.0)], 0.3)
for x in QX:
    trk(F, "/VG", [(x + 2.27, 53.2), (x + 2.27, 55.0)], 0.3)
for gx in (99.82, 104.18, 108.81):           # R1.2, R2.1, D1.1 (cathode)
    trk(F, "/VG", [(gx, 53.97), (gx, 55.0)], 0.3)
# input terminal into the two islands it feeds
trk(F, "/GNDIN", [(152.4, 53.75), (152.4, 60.0)], 3.0)
trk(F, "/12VIN", [(152.4, 44.25), (152.4, 41.0)], 3.0)
# Reverse-polarity LED down to the supply-ground island, changing layers because the gate bus runs
# across F.Cu at y = 55 and B.Cu is clear here between the FET tabs and the /12VIN island.
trk(F, "/GNDIN", [(102.05, 45.5), (102.05, 47.0)], 0.3)
via("/GNDIN", 102.05, 47.0)
trk(B, "/GNDIN", [(102.05, 47.0), (102.05, 58.0)], 0.4)

# U9's fourteen pins fan out on a 1.27 mm pitch into the same two channels, and whichever of its nets
# routes last finds the lane taken - the '125 puts each buffer's input and output side by side, while
# the inputs all arrive from the Pi in the east and the outputs all leave for terminals in the west,
# so the tracks want to cross. A straight stub per signal pin, laid before the router starts, gets
# every pin clear of the package; the router then picks the stubs up as that net's existing copper.
# The four OUTPUT stubs are longer than the rest. The inputs all arrive from the socket in the east and
# fan into the package along one east-west lane about 3 mm off its face - which is exactly where a 3 mm
# stub ends, so the first input routed seals the output beside it against both layers at once. Taking
# the outputs 5.4 mm out puts their open ends on the far side of that lane before it exists, and the
# router then has to take the inputs round them.
for _num, _net in (("1", "/MODE"), ("2", "/DPI_D1"), ("3", "/PID1"), ("4", "/MODE"),
                   ("5", "/DPI_D2"), ("6", "/PID2"), ("8", "/PID3"), ("9", "/DPI_D3"),
                   ("10", "/MODE"), ("11", "/PID4"), ("12", "/DPI_D0"), ("13", "/MODE")):
    _px, _py = pad("U9", _num)
    _len = 5.4 if _net.startswith("/PID") else 3.0
    _ey = _py + (_len if _py > 88.0 else -_len)
    # Jog onto the grid column at the pad and then run straight out along it. A SOIC pad is not on the
    # 0.3 mm grid, so a stub that runs out on the pad's own x straddles two columns of cells all the
    # way; the router picks up whichever it likes as this net's existing copper and can then start its
    # path from a cell that has no copper under it, leaving the two ends 0.3 mm apart. On the grid
    # column there is no ambiguity, and the jog is inside the pad's own escape corridor.
    _gx, _gy = round(_px / 0.3) * 0.3, round(_ey / 0.3) * 0.3
    trk(F, _net, [(_px, _py), (_gx, _py), (_gx, _gy)], 0.25)

# stitching: F.Cu ground island to the inner plane, and the two GNDIN islands to each other
for x in [64, 67, 70, 73, 76, 79, 82, 85, 88, 91, 94, 97]:
    via("GND", x, 45.0, d=0.8, drill=0.4)
    via("GND", x, 49.0, d=0.8, drill=0.4)
for x in [64, 68, 72, 76, 80, 84, 88, 92, 96, 100, 104, 108, 112, 116, 120, 124, 128, 132,
          136, 140, 144, 148, 152]:
    via("/GNDIN", x, 64.5, d=0.8, drill=0.4)
# A second row close to the FET drains at y = 59.29. With only the y = 64.5 row, the B.Cu half of the
# island reached the drains through the eight vias nearest them, so each carried about 1.9 A.
for x in [64, 68, 72, 76, 80, 84, 88, 92, 96]:
    via("/GNDIN", x, 61.5, d=0.8, drill=0.4)
for x in (143.0, 147.0, 151.0, 155.0):        # bridge to island, F to B
    via("/GNDIN", x, 54.5, d=0.8, drill=0.4)
for x, y in ([(x, 44.0) for x in (106, 110, 114, 118, 122, 126, 130, 134, 138, 142, 146, 150)] +
             [(x, 47.5) for x in (106, 110, 114, 118, 122, 126, 130, 134, 138, 142, 146)] +
             [(x, 34.0) for x in (124, 132, 140, 148)] +
             [(x, 38.0) for x in (124, 132, 140, 148)]):
    via("/12VIN", x, y, d=0.8, drill=0.4)

# ---- the eight cat5 conductors, routed by hand --------------------------------------------------------
# The AM26C32 takes two pairs on each side. P1 and P4 land on the near column and run straight across on
# B.Cu. P2 and P3 have to get to the far column, and they all go BELOW the part in nested lanes: taking
# some of them over the top would wall off the whole upper half of the band, which is where the
# terminators, bias resistors and ESD arrays have to live. Every crossing here was worked out on paper,
# so the maze router never has to squeeze anything through the 1.27 mm pad pitch.
# The outer conductor of each near-column pair steps away from its partner first, so that neither one
# ends up running 0.4 mm alongside the other's via.
trk(B, "/P4N", [(9.25, 46.43), (12.0, 46.43), (12.0, 44.5), (31.5, 44.5), (31.5, 45.55)])
via("/P4N", 31.5, 45.55); trk(F, "/P4N", [(31.5, 45.55), (33.26, 45.55)])
trk(B, "/P4P", [(7.47, 47.45), (12.0, 47.45)], 0.2)
trk(B, "/P4P", [(12.0, 47.45), (29.0, 47.45), (29.0, 46.83)]); via("/P4P", 29.0, 46.83)
trk(F, "/P4P", [(29.0, 46.83), (33.26, 46.83)])
trk(B, "/P1N", [(9.25, 52.55), (29.0, 52.55), (29.0, 53.17)]); via("/P1N", 29.0, 53.17)
trk(F, "/P1N", [(29.0, 53.17), (33.26, 53.17)])
trk(B, "/P1P", [(7.47, 53.57), (9.4, 55.5), (31.5, 55.5), (31.5, 51.9)])
via("/P1P", 31.5, 51.9); trk(F, "/P1P", [(31.5, 51.9), (33.26, 51.9)])

trk(B, "/P2N", [(9.25, 48.47), (43.0, 48.47), (43.0, 54.45)]); via("/P2N", 43.0, 54.45)
trk(F, "/P2N", [(43.0, 54.45), (38.74, 54.45)])
# nested lanes under the part: the conductor that leaves the jack lowest drops first and runs deepest
trk(F, "/P2P", [(7.47, 51.53), (11.0, 51.53)], 0.2)
trk(F, "/P2P", [(11.0, 51.53), (11.0, 64.0), (49.0, 64.0), (49.0, 53.17), (38.74, 53.17)])
# Port 3 is the Falcon 5(+)/4(-) (difftx rev D had it reversed): P3P drops through a via and runs on B.Cu
# between pads 4 and 6 to pad 5; P3N hops to pad 4 on F.Cu. Same as patch_port3.py.
via("/P3P", 13.0, 50.51); trk(B, "/P3P", [(13.0, 50.51), (11.98, 49.49), (7.47, 49.49)], 0.2)
trk(F, "/P3P", [(13.0, 50.51), (13.0, 62.5), (47.0, 62.5)]); via("/P3P", 47.0, 62.5)
trk(B, "/P3P", [(47.0, 62.5), (47.0, 48.1)]); via("/P3P", 47.0, 48.1)
trk(F, "/P3P", [(47.0, 48.1), (38.74, 48.1)])
trk(F, "/P3N", [(9.25, 50.51), (10.98, 50.51), (12.0, 49.49)])
trk(F, "/P3N", [(12.0, 49.49), (15.0, 49.49), (15.0, 61.0), (45.0, 61.0)]); via("/P3N", 45.0, 61.0)
trk(B, "/P3N", [(45.0, 61.0), (45.0, 46.83)]); via("/P3N", 45.0, 46.83)
trk(F, "/P3N", [(45.0, 46.83), (38.74, 46.83)])


# ---- ground and 5 V for the pins that have nowhere to go ---------------------------------------------
# The AM26C32's ground and enable sit in the middle of a 1.27 mm pad column, so their only way out is
# straight sideways; placing those two by hand, before anything else claims the room, is the difference
# between the router finding a way and boxing itself in. The PSM712 arrays no longer need this - their
# ground is an end pin with open board beside it, unlike the SRV05-4 they replaced.
# U1 pin 12 is ~G, tied high so the enable is G alone; pin 4 is G, now the mode line. Both are
# boxed in by the 1.27 mm pad pitch, so each gets a stub out to somewhere the router can reach.
trk(F, "+5V", [(38.74, 50.63), (41.6, 50.63)], 0.35); via("+5V", 41.6, 50.63)   # U1 pin 12 (~G)
trk(F, "/MODE", [(33.26, 49.37), (30.6, 49.5)], 0.25)                           # U1 pin 4 (G)
HAND_STITCHED = {("U1", "12"), ("U1", "4")}

# =====================================================================================================
# routing
# =====================================================================================================
from grid_router import Grid

netid = {n: k + 1 for k, n in enumerate(sorted(set(padnet.values())))}
G = Grid(board, fps, padnet, netid, trk, via, W, H, F, B)
G.add_pads()
# Bus copper is no-go for signals; the B.Cu ground fill yields to tracks so it is not an obstacle, but
# the supply-ground island on B.Cu is a real zone and must be.
for r in [(104.0, 30.0, 159.0, 51.0), (62.0, 56.0, 158.0, 66.0), (62.0, 43.5, 98.0, 50.5)]:
    G.add_zone_keepout(F, *r)
for x in TX:
    G.add_zone_keepout(F, x - 6.5, 16.5, x + 6.5, 19.5)
    G.add_zone_keepout(F, x - 6.5, 110.5, x + 6.5, 113.5)
G.add_zone_keepout(B, 62.0, 56.0, 158.0, 66.0)
for L_ in (F, B):
    G.add_zone_keepout(L_, 140.0, 52.0, 158.0, 56.0)
G.add_zone_keepout(B, 104.0, 30.0, 159.0, 51.0)
# The mounting holes and the RJ45's locating pegs are KiCad rule areas. The maze router does not
# read those, so without this it will happily route straight through a 3.2 mm hole.
for _hx, _hy in HOLES:
    for _L in (F, B):
        G.add_zone_keepout(_L, _hx - 2.6, _hy - 2.6, _hx + 2.6, _hy + 2.6)
for _hx, _hy in ((4.93, 43.65), (4.93, 56.35)):
    for _L in (F, B):
        G.add_zone_keepout(_L, _hx - 2.7, _hy - 2.7, _hx + 2.7, _hy + 2.7)
G.add_existing_tracks()

# Most constrained first: the receiver's own pins are boxed in by its 1.27 mm pad pitch, so they get
# the board to themselves before the long cat5 and pixel-data runs claim the space around it.
# The eight cat5 conductors already have their trunk routed by hand above; listing them here just
# hangs each pair's terminator, bias resistors and ESD array off that existing copper.
# Ground and +12 V on the remaining SMD pads first, while there is still room for a via beside each one.
for ref, fp in sorted(fps.items()):
    if ref in ("Q1", "Q2", "Q3", "Q4"):     # FET sources already reach the island on a wide track
        continue
    for p in fp.Pads():
        if (p.GetNetname() in ("GND", "+12V") and p.GetAttribute() == pcbnew.PAD_ATTRIB_SMD
                and (ref, p.GetNumber()) not in HAND_STITCHED):
            G.stitch(ref, p.GetNumber(), p.GetNetname())

ORDER = ["/RXD1", "/RXD2", "/RXD3", "/RXD4",
         "/P1P", "/P1N", "/P2N", "/P2P", "/P3N", "/P3P", "/P4N", "/P4P",
         *[f"/V{k+1}" for k in range(12)],
         "/MODE",
         "/NTC", "/VREFAN", "/VREFOT", "/FANDRV", "/OTA", "/OT",
         "/LK3", "+5V", "/DATA1", "/DATA2", "/DATA3", "/DATA4",
         "/VG", "/VREG", "/LK1", "/LK2", "/LK4", "/LK5",
         # rev C. /GNDIN is deliberately absent: it is a zone net, the FET drains join it by sitting in
         # the fill, and the one pad outside the island is hand-routed above.
         "/FANM", "/FAN12",
         # The Pi side, most constrained first. The socket's signal pins sit on the INNER column, so
         # each one escapes east into the 14 mm strip between the socket and the notch and then has to
         # come back west through the single channel north of the pin field. They get it before
         # anything else. /SDA and /SCL run the whole length of the board, from the socket at the
         # bottom right to the sensors at the top left, detouring round two supply islands that block
         # both layers, so they take their lanes next, ahead of the short local nets.
         # ...and among themselves, north to south. They all leave through the same channel above the
         # pin field, so they stack up in it in the order they are routed; a southern pin routed first
         # takes the innermost lane and seals in every pin north of it.
         # GPIO7 is the one Pi signal on an EVEN pin, so J17.26 is the only one of these on the outer
         # column and the only one that escapes straight west. It takes that lane first, because the
         # inner-column nets come round the north channel and then run south down the same strip of
         # board, on both layers, right across its nose.
         "/DPI_D3", "/PID3",
         "/P3V3", "/SDA", "/SCL",
         # Each buffer channel's two halves route together. U9's input and output for one channel are
         # adjacent pins sharing one crowded escape, so doing all four inputs and then all four outputs
         # - in either order - leaves the second group's stubs walled in by the first group's tracks.
         "/DPI_D0", "/PID4", "/DPI_D1", "/PID1", "/DPI_D2", "/PID2",
         "/RP", "/PI5V", "/HDR5V", "/PI12"] + [f"/LT{k+1}" for k in range(12)]
import time, sys
fail = 0
for n in ORDER:
    t0 = time.time()
    # A Pi Zero 2 W peaks near 1.2 A and 0.25 mm of 1 oz copper is good for 0.88 A by IPC-2221, so the
    # Pi's supply nets are routed wide. 0.5 mm carries 1.44 A at a 10 C rise.
    WIDE = {"/PI5V": 0.5, "/HDR5V": 0.5, "/PI12": 0.4, "/P3V3": 0.3, "/VG": 0.3, "/VREG": 0.3, "+5V": 0.3}
    # Only the nets carrying hand-routed escape stubs have genuinely disjoint existing copper.
    STUBBED = {"/MODE", "/DPI_D0", "/DPI_D1", "/DPI_D2", "/DPI_D3",
               "/PID1", "/PID2", "/PID3", "/PID4"}
    ok = G.route(n, WIDE.get(n, 0.25), split_pre=(n in STUBBED))
    print(f"  {n:8s} {'ok' if ok else 'FAIL'}  {time.time()-t0:6.1f}s", flush=True)
    fail += 0 if ok else 1
print("routing failures:", fail, flush=True)

# =====================================================================================================
# silkscreen
# =====================================================================================================
def text(s, x, y, size=1.0, layer=pcbnew.F_SilkS, angle=0, just=None):
    tx = pcbnew.PCB_TEXT(board); tx.SetText(s); tx.SetPosition(MM(x, y)); tx.SetLayer(layer)
    size = max(size, 1.0)                      # JLCPCB will not reliably print below 1.0 mm
    tx.SetTextSize(MM(size, size)); tx.SetTextThickness(FromMM(max(size * 0.15, 0.15)))
    tx.SetTextAngleDegrees(angle); tx.SetMirrored(layer == pcbnew.B_SilkS)
    if just:
        tx.SetHorizJustify(just)
    board.Add(tx); return tx

# The terminal, fuse and jack outlines are wider than the parts are pitched, or run off the board edge,
# and none of them tell an assembler anything this board's own labelling does not. The FETs keep theirs:
# that outline carries the only pin-1 marker on four polarity-critical parts.
SILK_KEEP = ("Q1", "Q2", "Q3", "Q4", "Q5", "U1", "U3", "U4", "U5", "U6",
             "D1", "D2", "D7", "D8", "F0", "J1", "J16")
STRIP = ([j for j, _, _ in TOP + BOT] + [f for _, f, _ in TOP + BOT] + ["J14"] +
         [r for r in fps if r not in SILK_KEEP and r[0] in ("R", "C", "L", "T", "Z", "D", "F")]
         + ["J15", "J17"])
for ref in STRIP:
    fp_ = fps[ref]
    for g in list(fp_.GraphicalItems()):
        if g.GetLayer() == pcbnew.F_SilkS:
            fp_.Remove(g)

KEEP_REF = ("J", "U", "Q", "H")
for ref, fp in fps.items():
    fp.Value().SetVisible(False)
    keep = (ref[0] in KEEP_REF and ref not in [j for j, _, _ in TOP + BOT]) or ref in ("D2", "F0")
    r = fp.Reference()
    r.SetVisible(keep)
    if keep:
        r.SetTextSize(MM(1.0, 1.0)); r.SetTextThickness(FromMM(0.15)); r.SetLayer(pcbnew.F_SilkS)
for ref, (x, y, ang) in {"J1": (140.0, 27.0, 0), "J14": (10.0, 60.5, 0), "U1": (36.0, 43.0, 0),
                         "U3": (110.0, 65.5, 0), "F0": (92.0, 41.5, 0),
                         "U4": (62.0, 22.5, 0), "U5": (20.0, 95.0, 0), "U6": (68.0, 33.5, 0),
                         "J15": (105.0, 22.0, 0), "J16": (17.0, 63.5, 0),
                         "U7": (121.0, 100.0, 0), "JP1": (121.0, 76.0, 0), "U8": (104.0, 93.5, 0), "U9": (88.0, 88.0, 0),
                         "J17": (139.0, 127.0, 0), "SW1": (6.0, 86.0, 0), "Q5": (25.0, 82.0, 0),
                         "D2": (70.0, 74.0, 0),
                         "Q1": (68.0, 64.0, 0), "Q2": (76.0, 64.0, 0),
                         "Q3": (84.0, 64.0, 0), "Q4": (92.0, 64.0, 0)}.items():
    r = fps[ref].Reference(); r.SetPosition(MM(x, y)); r.SetTextAngleDegrees(ang)
for ref in [r for r in fps if r[0] == "H"]:
    fps[ref].Reference().SetVisible(False)

# Output terminals: pole marks right under the screws, port name and its fuse on the line above, in the
# 6 mm band the fuse rows were moved back to make.
for x, (j, f, name) in zip(TX, TOP):
    for dx, mark in ((-5, "+"), (0, "D"), (5, "-")):
        text(mark, x + dx, 9.9, 1.2, just=pcbnew.GR_TEXT_H_ALIGN_CENTER)
    text(f"{name}  {f}", x, 19.5, 1.3, just=pcbnew.GR_TEXT_H_ALIGN_CENTER)
for x, (j, f, name) in zip(TX, BOT):
    for dx, mark in ((-5, "+"), (0, "D"), (5, "-")):
        text(mark, x + dx, 120.4, 1.2, just=pcbnew.GR_TEXT_H_ALIGN_CENTER)
    text(f"{name}  {f}", x, 110.5, 1.3, just=pcbnew.GR_TEXT_H_ALIGN_CENTER)

text("RX = cat5 drives the outputs    PI = the Pi on J17 drives them", 76.0, 48.0, 1.0,
     just=pcbnew.GR_TEXT_H_ALIGN_CENTER)
text("Chandler 4D/8P Smart Receiver  v1.00", 72.0, 45.0, 1.4,
     just=pcbnew.GR_TEXT_H_ALIGN_CENTER)
text("INJ terminals: power only, D pole not connected", 42.0, 60.5, 1.0,
     just=pcbnew.GR_TEXT_H_ALIGN_CENTER)
text("12V ONLY - check polarity before powering", 42.0, 63.0, 1.0,
     just=pcbnew.GR_TEXT_H_ALIGN_CENTER)

# main fuse and input terminal
text("MAIN FUSE - ATO 30A BLADE", 100.0, 38.7, 1.2, just=pcbnew.GR_TEXT_H_ALIGN_CENTER)
# J1 moved 30 mm east when the Pi column was cut in; this block did not, so the only terminal on the
# board that carries 30 A had nothing written next to it while "+12V" floated in the middle of its own
# copper island and "GND" sat against the J17 designator, reading as a label for the Pi socket. The pole
# marks are placed against J1's actual pads, west of its body so they clear its outline.
_jx, _jy12 = pad("J1", "2")
_jx, _jyg = pad("J1", "1")
text("+12V", _jx - 10.4, _jy12, 1.2, just=pcbnew.GR_TEXT_H_ALIGN_CENTER)
text("GND", _jx - 10.4, _jyg, 1.2, just=pcbnew.GR_TEXT_H_ALIGN_CENTER)
text("12V INPUT", 140.0, 30.0, 1.2, just=pcbnew.GR_TEXT_H_ALIGN_CENTER)
text("30A MAX", 140.0, 32.5, 1.2, just=pcbnew.GR_TEXT_H_ALIGN_CENTER)
text("24A CONT", 140.0, 35.0, 1.2, just=pcbnew.GR_TEXT_H_ALIGN_CENTER)

text("CAT5 IN from difftx", 9.5, 34.0, 1.0, just=pcbnew.GR_TEXT_H_ALIGN_CENTER)

# Which way the mode switch goes, printed against the actual pads so the labels cannot end up on the
# wrong ends: pin 1 is the +5 V throw, which enables the receiver.
_x1, _y1 = pad("SW1", "1")
_x3, _y3 = pad("SW1", "3")
text("RX", _x1 + 5.0, _y1, 1.2, just=pcbnew.GR_TEXT_H_ALIGN_CENTER)
text("PI", _x3 + 5.0, _y3, 1.2, just=pcbnew.GR_TEXT_H_ALIGN_CENTER)
text("MODE", 6.0, 105.0, 1.0, just=pcbnew.GR_TEXT_H_ALIGN_CENTER)

# The header field, and the two things a ribbon lets you get wrong. A plain 2x20 male header has no
# key, so an IDC socket will seat either way round, and reversed it puts 5 V on the Pi's GPIO2. Pin 1 is
# marked from the pad's own position so the legend cannot drift away from the copper. The box is also
# the shroud outline: fit a keyed DC3-40P in these holes instead and it lands inside this rectangle.
_p1x, _p1y = pad("J17", "1")
_p40x, _p40y = pad("J17", "40")
_hx0, _hx1 = min(_p1x, _p40x) - 2.2, max(_p1x, _p40x) + 2.2
_hy0, _hy1 = min(_p1y, _p40y) - 2.2, max(_p1y, _p40y) + 2.2
for _a, _b in (((_hx0, _hy0), (_hx1, _hy0)), ((_hx1, _hy0), (_hx1, _hy1)),
               ((_hx1, _hy1), (_hx0, _hy1)), ((_hx0, _hy1), (_hx0, _hy0))):
    _s = pcbnew.PCB_SHAPE(board); _s.SetShape(pcbnew.SHAPE_T_SEGMENT)
    _s.SetStart(MM(*_a)); _s.SetEnd(MM(*_b)); _s.SetLayer(pcbnew.F_SilkS)
    _s.SetWidth(FromMM(0.15)); board.Add(_s)
# A filled triangle north of the field, pointing at pin 1's column.
_tri = pcbnew.PCB_SHAPE(board); _tri.SetShape(pcbnew.SHAPE_T_POLY)
_tri.SetPolyPoints([MM(_p1x - 1.3, _hy0 - 3.1), MM(_p1x + 1.3, _hy0 - 3.1), MM(_p1x, _hy0 - 1.5)])
_tri.SetLayer(pcbnew.F_SilkS); _tri.SetFilled(True); _tri.SetWidth(FromMM(0.15)); board.Add(_tri)
text("1", _p1x - 3.4, _hy0 - 2.3, 1.2, just=pcbnew.GR_TEXT_H_ALIGN_CENTER)

# Everything else goes in the 23 mm strip east of the field, which is now empty board.
for _i, _s in enumerate(("PI GPIO", "40 WAY RIBBON", "ANY 40-PIN PI", "",
                         "PIN 1 AT TOP", "REVERSED PUTS", "5V ON GPIO2", "",
                         "GPIO 4-7 =", "DATA 1-4", "",
                         "5V LINK IN:", "BOARD POWERS", "PI, 2A MAX", "",
                         "LINK OUT: PI", "USES OWN PSU")):
    if _s:
        text(_s, 148.0, 74.0 + 2.6 * _i, 1.0, just=pcbnew.GR_TEXT_H_ALIGN_CENTER)
text("5V LINK", 124.5, 81.0, 1.0, just=pcbnew.GR_TEXT_H_ALIGN_CENTER)

# rev C labelling
text("I2C  G 3V3 SDA SCL   3.3V ONLY - NOT 5V", 105.0, 26.0, 1.0, just=pcbnew.GR_TEXT_H_ALIGN_CENTER)
text("OVER TEMP", 57.0, 35.5, 1.0, just=pcbnew.GR_TEXT_H_ALIGN_CENTER)
text("REVERSED", 101.0, 43.0, 1.0, just=pcbnew.GR_TEXT_H_ALIGN_CENTER)
text("FAN 12V", 17.0, 66.0, 1.0, just=pcbnew.GR_TEXT_H_ALIGN_CENTER)
for dy, mark in ((-5.0, "+"), (0.0, "-"), (5.0, "G")):
    text(mark, 12.5, 72.0 + dy, 1.2, just=pcbnew.GR_TEXT_H_ALIGN_CENTER)
text("TIE", 5.0, 14.0, 1.0, just=pcbnew.GR_TEXT_H_ALIGN_CENTER)
text("TIE", 5.0, 86.0, 1.0, just=pcbnew.GR_TEXT_H_ALIGN_CENTER)
text("P1=1/2  P2=3/6", 7.0, 36.5, 1.0, just=pcbnew.GR_TEXT_H_ALIGN_CENTER)
text("P3=5/4  P4=7/8", 7.0, 39.0, 1.0, just=pcbnew.GR_TEXT_H_ALIGN_CENTER)
for i, x in enumerate((10.0, 21.0, 32.0, 43.0)):
    text(f"P{i+1}", x, 28.0, 1.0, just=pcbnew.GR_TEXT_H_ALIGN_CENTER)
text("PWR", 54.0, 28.0, 1.0, just=pcbnew.GR_TEXT_H_ALIGN_CENTER)

filler = pcbnew.ZONE_FILLER(board)
filler.Fill(board.Zones())
out = os.path.join(HERE, f"{PROJECT}.kicad_pcb")
pcbnew.SaveBoard(out, board)

# ---- stack-up -----------------------------------------------------------------------------------
# The 30 A numbers depend entirely on copper weight, and rev A recorded it only in a markdown file -
# order it on JLCPCB's 4-layer default (1 oz outer / 0.5 oz inner) and the supply-ground island runs
# three times hotter than designed. Writing it into the board means it also reaches the gerber job file.
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
    (layer "B.Mask" (type "Top Solder Mask") (thickness 0.01))
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
