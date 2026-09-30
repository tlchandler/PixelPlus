"""Rev D -> rev E (2026-09-29): RJ45 port 3 becomes the Falcon standard, pin 5 = P3+ (2Y), pin 4 = P3- (2Z).

Rev D had 4(+)/5(-); a PixelController SRx1 v5.01 showed port 3 inverted (see ../CLAUDE.md). The two port 3 routes
swap approaches, so the jack's top-row pads are still fed from above on B.Cu and the bottom-row pads from below on F.Cu:
  2Z (U2 pin 5, P3-): short F.Cu hop, via at (41.2, u2y(5)), then rev D's B.Cu route down x = 41.2 and along y = 17.6 to pad 4.
  2Y (U2 pin 6, P3+): jogs down one pin pitch into the corridor pin 5 used to run along, then rev D's F.Cu route
                      around the jack (x = 58.35, y = 27.45) up to pad 5.
Silkscreen: "rev D" -> "rev E", "P3=4,5" -> "P3=5,4". Mirrored in gen_pcb.py.
Run with KiCad's Python from this folder:  python patch_revE.py
"""
import pcbnew
from pcbnew import VECTOR2I_MM as MM, FromMM, ToMM

PATH = "difftx.kicad_pcb"
board = pcbnew.LoadBoard(PATH)
F, B = pcbnew.F_Cu, pcbnew.B_Cu
net = lambda n: board.FindNet(n)

U2R = 39.64
def u2y(n):
    return 17.45 - 1.27 * (n - 1)
RJ = {n: (52.57 - 1.02 * (n - 1), 21.93 if n % 2 else 20.15) for n in range(1, 9)}

# ---- queries first (KiCad 10 SWIG returns untyped objects after the first Remove) ----
j1 = board.FindFootprintByReference("J1")
pads = {p.GetNumber(): p for p in j1.Pads()}
assert (pads["4"].GetNetname(), pads["5"].GetNetname()) == ("/P3+", "/P3-"), "already rev E?"
u2 = {p.GetNumber(): p for p in board.FindFootprintByReference("U2").Pads()}
assert (u2["6"].GetNetname(), u2["5"].GetNetname()) == ("/P3+", "/P3-")
old = [t for t in board.GetTracks() if t.GetNetname() in ("/P3+", "/P3-")]
n_vias = sum(isinstance(t, pcbnew.PCB_VIA) for t in old)
assert n_vias == 1 and len(old) == 9, (n_vias, len(old))           # rev D: 1 + via + 3 on P3+, 4 on P3-
texts = [d for d in board.GetDrawings() if isinstance(d, pcbnew.PCB_TEXT)
         and d.GetText() in ("FPP RS-422 pHAT rev D", "P1=1,2 P2=3,6 P3=4,5 P4=7,8")]
assert len(texts) == 2

for t in old:
    board.Remove(t)
pads["4"].SetNet(net("/P3-")); pads["4"].SetPinFunction("P3-")
pads["5"].SetNet(net("/P3+")); pads["5"].SetPinFunction("P3+")
for d in texts:
    d.SetText({"FPP RS-422 pHAT rev D": "FPP RS-422 pHAT rev E",
               "P1=1,2 P2=3,6 P3=4,5 P4=7,8": "P1=1,2 P2=3,6 P3=5,4 P4=7,8"}[d.GetText()])

def track(layer, netname, pts, w=0.25):
    for (x1, y1), (x2, y2) in zip(pts, pts[1:]):
        t = pcbnew.PCB_TRACK(board); t.SetStart(MM(x1, y1)); t.SetEnd(MM(x2, y2))
        t.SetWidth(FromMM(w)); t.SetLayer(layer); t.SetNet(net(netname)); board.Add(t)

def via(netname, x, y, d=0.8, drill=0.4):
    v = pcbnew.PCB_VIA(board); v.SetPosition(MM(x, y)); v.SetWidth(FromMM(d)); v.SetDrill(FromMM(drill))
    v.SetNet(net(netname)); v.SetLayerPair(F, B); board.Add(v)

# 2Y -> pin 5 (P3+)
track(F, "/P3+", [(U2R, u2y(6)), (42.0, u2y(6)), (43.27, u2y(5)), (58.35, u2y(5)), (58.35, 27.45), (RJ[5][0], 27.45), RJ[5]])
# 2Z -> pin 4 (P3-)
track(F, "/P3-", [(U2R, u2y(5)), (41.2, u2y(5))]); via("/P3-", 41.2, u2y(5))
track(B, "/P3-", [(41.2, u2y(5)), (41.2, 17.6), (RJ[4][0], 17.6), RJ[4]])

pcbnew.ZONE_FILLER(board).Fill(board.Zones())
pcbnew.SaveBoard(PATH, board)
print(f"rev E: removed {len(old)} port 3 items, J1.4 = /P3-, J1.5 = /P3+")
