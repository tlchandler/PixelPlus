r"""
Patch 2 (2026-09-25): put back the three router links that patch 1's first stub finder removed by mistake
(it compared a stub end against the OTHER track's half-width only, so copper that overlapped at an angle
looked unconnected; fixed in board_fixups.py). Coordinates are the two free ends DRC reported for each net.
gen_pcb.py needs no change: it uses the fixed board_fixups.py.

  "C:\Program Files\KiCad\10.0\bin\python.exe" patch_board_2.py
"""
import os
import pcbnew
from pcbnew import VECTOR2I_MM as MM, FromMM

PCB = os.path.join(os.path.dirname(os.path.abspath(__file__)), "difftxlarge.kicad_pcb")
board = pcbnew.LoadBoard(PCB)
nets = {n: board.FindNet(n) for n in ("/EN_5V_DRV", "/EN_5V_PI", "/O7")}
for n, a, b in (("/EN_5V_DRV", (109.5, 55.1), (109.4, 54.8)),
                ("/EN_5V_PI", (109.5, 26.1), (109.4, 25.8)),
                ("/O7", (32.3, 118.5), (32.9, 119.2))):
    t = pcbnew.PCB_TRACK(board); t.SetStart(MM(*a)); t.SetEnd(MM(*b)); t.SetWidth(FromMM(0.2))
    t.SetLayer(pcbnew.F_Cu); t.SetNet(nets[n]); board.Add(t)
pcbnew.ZONE_FILLER(board).Fill(board.Zones())
pcbnew.SaveBoard(PCB, board)
print("saved", PCB)
