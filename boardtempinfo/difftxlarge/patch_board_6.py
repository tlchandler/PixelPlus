r"""
Patch 6 (2026-09-25), after the regeneration with the independent review's fixes: D62 (SMDJ15A) loses its
library silk outline, which D61's new cathode bar overlapped, and gets the same cathode bar as the other
diodes. Also in gen_pcb.py. (The nine latch Description fields were corrected by plain text substitution in
the .kicad_pcb, the .kicad_sch and gen_sch.py together.)

  "C:\Program Files\KiCad\10.0\bin\python.exe" patch_board_6.py
"""
import os
import pcbnew
from pcbnew import VECTOR2I_MM as MM, FromMM

PCB = os.path.join(os.path.dirname(os.path.abspath(__file__)), "difftxlarge.kicad_pcb")
board = pcbnew.LoadBoard(PCB)
d62 = next(f for f in board.GetFootprints() if f.GetReference() == "D62")      # all look-ups before Remove()
silk = [gi for gi in d62.GraphicalItems() if gi.GetLayer() == pcbnew.F_SilkS]
c = d62.GetPosition(); p = d62.FindPadByNumber("1")
px, py = p.GetPosition().x / 1e6, p.GetPosition().y / 1e6
ux, uy = px - c.x / 1e6, py - c.y / 1e6
n = (ux * ux + uy * uy) ** 0.5; ux, uy = ux / n, uy / n
bb = p.GetBoundingBox(); bw, bh = bb.GetWidth() / 1e6, bb.GetHeight() / 1e6
along = (bw if abs(ux) > abs(uy) else bh) / 2.0
across = (bh if abs(ux) > abs(uy) else bw) / 2.0 + 0.2
print("removing", len(silk), "silk items from D62; cathode pad at", (px, py))
for gi in silk:
    d62.Remove(gi)
mx, my = px + ux * (along + 0.3), py + uy * (along + 0.3)
s = pcbnew.PCB_SHAPE(board); s.SetShape(pcbnew.SHAPE_T_SEGMENT)
s.SetStart(MM(mx - uy * across, my + ux * across)); s.SetEnd(MM(mx + uy * across, my - ux * across))
s.SetLayer(pcbnew.F_SilkS); s.SetWidth(FromMM(0.2)); board.Add(s)
pcbnew.ZONE_FILLER(board).Fill(board.Zones())
pcbnew.SaveBoard(PCB, board)
print("saved", PCB)
