r"""
Patch 4 (2026-09-25), after the regeneration that moved the audio jack and the OLED socket:
  - J19 steps back 0.3 mm (5.4 -> 5.7): its front pad's copper was just inside 0.3 mm of the edge.
  - J19's two signal pads to R46 / R47, which that regeneration left unrouted: J19 had crossed the top of
    its routing window, so the window treated it as an obstacle instead of a target (fixed in gen_pcb.py).
    L (pad 2) runs on B.Cu and comes up beside R46; R (pad 3) runs on F.Cu round the outside and under R47.
  - OLED legends inside the outline instead of on its top edge, and short enough to fit its 27.3 mm width.
All three are also in gen_pcb.py.

  "C:\Program Files\KiCad\10.0\bin\python.exe" patch_board_4.py
"""
import os
import pcbnew
from pcbnew import VECTOR2I_MM as MM, FromMM

PCB = os.path.join(os.path.dirname(os.path.abspath(__file__)), "difftxlarge.kicad_pcb")
board = pcbnew.LoadBoard(PCB)
OX, OY = 266.0, 33.0

# every look-up first (see board_fixups.py: after a Remove() the SWIG layer stops typing its results)
j19 = next(f for f in board.GetFootprints() if f.GetReference() == "J19")
nL, nR = board.FindNet("/AUD_L_IN"), board.FindNet("/AUD_R_IN")
drop = [d for d in board.GetDrawings() if isinstance(d, pcbnew.PCB_TEXT) and (
        d.GetText() in ("0.96 IN OLED (SSD1306, I2C 0x3C)", "PLUGS IN FACE UP OVER THIS AREA", "PINS: GND VCC SCL SDA - 3.3 V")
        or (d.GetText() in ("G", "V", "C", "D") and abs(d.GetPosition().y / 1e6 - (OY - 2.3)) < 0.01
            and OX - 5 < d.GetPosition().x / 1e6 < OX + 5))]
assert len(drop) == 7, len(drop)
pin_x = {d.GetText(): d.GetPosition().x / 1e6 for d in drop if len(d.GetText()) == 1}

j19.SetPosition(MM(118.0, 5.7))
def P(ref, num):
    f = j19 if ref == "J19" else next(g for g in board.GetFootprints() if g.GetReference() == ref)
    p = f.FindPadByNumber(num).GetPosition()
    return (p.x / 1e6, p.y / 1e6)
L2, R3, r46, r47 = P("J19", "2"), P("J19", "3"), P("R46", "1"), P("R47", "1")
print("J19.2", L2, "J19.3", R3, "R46.1", r46, "R47.1", r47)
def seg(layer, net, pts, w=0.25):
    for a, b in zip(pts, pts[1:]):
        t = pcbnew.PCB_TRACK(board); t.SetStart(MM(*a)); t.SetEnd(MM(*b)); t.SetWidth(FromMM(w))
        t.SetLayer(layer); t.SetNet(net); board.Add(t)
def via(net, x, y):
    v = pcbnew.PCB_VIA(board); v.SetPosition(MM(x, y)); v.SetWidth(FromMM(0.7)); v.SetDrill(FromMM(0.3))
    v.SetNet(net); v.SetLayerPair(pcbnew.F_Cu, pcbnew.B_Cu); board.Add(v)
# L on B.Cu down and across, up through a via beside R46; R on F.Cu round the outside and under R47
seg(pcbnew.B_Cu, nL, [L2, (L2[0], 12.0), (124.5, 12.0), (124.5, r46[1]), (124.9, r46[1])])
via(nL, 124.9, r46[1])
seg(pcbnew.F_Cu, nL, [(124.9, r46[1]), r46])
seg(pcbnew.F_Cu, nR, [R3, (123.0, R3[1]), (123.0, 16.2), (r47[0], 16.2), r47])

for d in drop:
    board.Remove(d)
def text(s, x, y, size=1.0):
    tx = pcbnew.PCB_TEXT(board); tx.SetText(s); tx.SetPosition(MM(x, y)); tx.SetLayer(pcbnew.F_SilkS)
    tx.SetTextSize(MM(size, size)); tx.SetTextThickness(FromMM(0.15))
    tx.SetHorizJustify(pcbnew.GR_TEXT_H_ALIGN_CENTER); board.Add(tx)
for s, x in pin_x.items():
    text(s, x, OY + 2.3)
text("0.96 IN OLED", OX, OY + 11.0, 1.2)
text("SSD1306 I2C 0x3C", OX, OY + 13.6)
text("PLUGS IN FACE UP", OX, OY + 16.0)

pcbnew.ZONE_FILLER(board).Fill(board.Zones())
pcbnew.SaveBoard(PCB, board)
print("saved", PCB)
