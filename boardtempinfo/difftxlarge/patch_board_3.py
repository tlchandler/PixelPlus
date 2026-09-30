r"""
Patch 3 (2026-09-25): the pin-1 triangle moves 1.2 mm further right, clear of J16's shroud outline (also in
gen_pcb.py). The one remaining track_dangling warning - a 0.3 mm 5V_PI stub beside J18's pads, its net fully
connected - is left: board_fixups counts its end as touching the pad, KiCad does not, and it is harmless.

  "C:\Program Files\KiCad\10.0\bin\python.exe" patch_board_3.py
"""
import os
import pcbnew
from pcbnew import VECTOR2I_MM as MM, FromMM

PCB = os.path.join(os.path.dirname(os.path.abspath(__file__)), "difftxlarge.kicad_pcb")
board = pcbnew.LoadBoard(PCB)
# all look-ups before any Remove() (see board_fixups.py)
p1 = next(f for f in board.GetFootprints() if f.GetReference() == "J16").FindPadByNumber("1").GetPosition()
p1x, p1y = p1.x / 1e6, p1.y / 1e6
tri = [d for d in board.GetDrawings() if isinstance(d, pcbnew.PCB_SHAPE) and d.GetShape() == pcbnew.SHAPE_T_POLY
       and d.GetLayer() == pcbnew.F_SilkS]
one = [d for d in board.GetDrawings() if isinstance(d, pcbnew.PCB_TEXT) and d.GetText() == "1"
       and abs(d.GetPosition().y / 1e6 - p1y) < 0.01]
assert len(tri) == 1 and len(one) == 1, (len(tri), len(one))
for x in tri:
    board.Remove(x)
t = pcbnew.PCB_SHAPE(board); t.SetShape(pcbnew.SHAPE_T_POLY)
t.SetPolyPoints([MM(p1x + 7.4, p1y - 1.3), MM(p1x + 7.4, p1y + 1.3), MM(p1x + 5.8, p1y)])
t.SetLayer(pcbnew.F_SilkS); t.SetFilled(True); t.SetWidth(FromMM(0.15)); board.Add(t)
one[0].SetPosition(MM(p1x + 8.8, p1y))
pcbnew.ZONE_FILLER(board).Fill(board.Zones())
pcbnew.SaveBoard(PCB, board)
print("saved", PCB)
