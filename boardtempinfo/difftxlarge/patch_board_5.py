r"""
Patch 5 (2026-09-25): J19's front pad (pad 1, GND) is 2.4 mm TALL - the footprint rotates it - so at y 5.7
its copper still came within 0.3 mm of the edge. J19 moves to y 6.1 (nose 0.85 mm behind the edge; the project enforces 0.5 mm copper-to-edge), and the two
audio traces patch 4 added are redrawn from the pads' new positions. Also in gen_pcb.py (J19 at 6.1).

  "C:\Program Files\KiCad\10.0\bin\python.exe" patch_board_5.py
"""
import os
import pcbnew
from pcbnew import VECTOR2I_MM as MM, FromMM

PCB = os.path.join(os.path.dirname(os.path.abspath(__file__)), "difftxlarge.kicad_pcb")
board = pcbnew.LoadBoard(PCB)
nL, nR = board.FindNet("/AUD_L_IN"), board.FindNet("/AUD_R_IN")
old = [t for t in board.GetTracks() if t.GetNetname() in ("/AUD_L_IN", "/AUD_R_IN")]
print("removing", len(old), "items")
j19 = next(f for f in board.GetFootprints() if f.GetReference() == "J19")
j19.SetPosition(MM(118.0, 6.1))
def P(ref, num):
    f = j19 if ref == "J19" else next(g for g in board.GetFootprints() if g.GetReference() == ref)
    p = f.FindPadByNumber(num).GetPosition()
    return (p.x / 1e6, p.y / 1e6)
L2, R3, r46, r47 = P("J19", "2"), P("J19", "3"), P("R46", "1"), P("R47", "1")
print("J19.2", L2, "J19.3", R3, "R46.1", r46, "R47.1", r47)
for t in old:
    board.Remove(t)
def seg(layer, net, pts, w=0.25):
    for a, b in zip(pts, pts[1:]):
        t = pcbnew.PCB_TRACK(board); t.SetStart(MM(*a)); t.SetEnd(MM(*b)); t.SetWidth(FromMM(w))
        t.SetLayer(layer); t.SetNet(net); board.Add(t)
v = pcbnew.PCB_VIA(board); v.SetPosition(MM(124.9, r46[1])); v.SetWidth(FromMM(0.7)); v.SetDrill(FromMM(0.3))
v.SetNet(nL); v.SetLayerPair(pcbnew.F_Cu, pcbnew.B_Cu); board.Add(v)
seg(pcbnew.B_Cu, nL, [L2, (L2[0], 12.0), (124.5, 12.0), (124.5, r46[1]), (124.9, r46[1])])
seg(pcbnew.F_Cu, nL, [(124.9, r46[1]), r46])
seg(pcbnew.F_Cu, nR, [R3, (123.0, R3[1]), (123.0, 16.2), (r47[0], 16.2), r47])
pcbnew.ZONE_FILLER(board).Fill(board.Zones())
pcbnew.SaveBoard(PCB, board)
print("saved", PCB)
