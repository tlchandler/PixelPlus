r"""
Apply small, local changes to the saved board in place, without re-running gen_pcb.py (which re-places and
re-routes everything and takes 30+ minutes). Every change made here is ALSO made in gen_pcb.py, so a full
regeneration reproduces the same board. Run with KiCad 10's Python:

  "C:\Program Files\KiCad\10.0\bin\python.exe" patch_board.py

Patch 1 (2026-09-25): silkscreen tidy after the full-severity DRC, and removal of router stubs.
"""
import os
import pcbnew
from pcbnew import VECTOR2I_MM as MM, FromMM
from board_fixups import find_dangling_stubs

HERE = os.path.dirname(os.path.abspath(__file__))
PCB = os.path.join(HERE, "difftxlarge.kicad_pcb")
board = pcbnew.LoadBoard(PCB)
PI_X0, PI_Y0 = 8.0, 3.0
def footprint(ref):
    return next(f for f in board.GetFootprints() if f.GetReference() == ref)

def text(s, x, y, size=1.0, bold=False):
    tx = pcbnew.PCB_TEXT(board); tx.SetText(s); tx.SetPosition(MM(x, y)); tx.SetLayer(pcbnew.F_SilkS)
    tx.SetTextSize(MM(size, size)); tx.SetTextThickness(FromMM(max(size * (0.2 if bold else 0.15), 0.15)))
    tx.SetHorizJustify(pcbnew.GR_TEXT_H_ALIGN_CENTER); board.Add(tx)

def find_text(prefix):
    hits = [d for d in board.GetDrawings() if isinstance(d, pcbnew.PCB_TEXT) and d.GetText().startswith(prefix)]
    assert len(hits) == 1, (prefix, len(hits))
    return hits[0]

# Look everything up FIRST: after the first Remove() KiCad's SWIG layer hands back untyped objects for any
# further GetFootprints() / GetDrawings() walk, so all removals and additions come after the lookups.
fp = {ref: footprint(ref) for ref in ("J16", "J17", "J18")}
silk_to_strip = [(fp[r], gi) for r in ("J17", "J18") for gi in list(fp[r].GraphicalItems()) if gi.GetLayer() == pcbnew.F_SilkS]
old_texts = [find_text(p) for p in ("RASPBERRY PI 3B+ / 4 / 5 - FACE UP", "AUDIO: PATCH CABLE", "PI RIBBON - PIN 1")]
p1 = fp["J16"].FindPadByNumber("1").GetPosition()
p1x, p1y = p1.x / 1e6, p1.y / 1e6
old_tri = [d for d in board.GetDrawings() if isinstance(d, pcbnew.PCB_SHAPE) and d.GetShape() == pcbnew.SHAPE_T_POLY
           and d.GetLayer() == pcbnew.F_SilkS]
one = [d for d in board.GetDrawings() if isinstance(d, pcbnew.PCB_TEXT) and d.GetText() == "1"
       and abs(d.GetPosition().y / 1e6 - p1y) < 0.01]
assert len(old_tri) == 1 and len(one) == 1, (len(old_tri), len(one))
dead = find_dangling_stubs(board)

# connector silk outlines that ran into their own pads or labels, and references that sat on other copper
for f, gi in silk_to_strip:
    f.Remove(gi)
for r in ("J16", "J17"):
    fp[r].Reference().SetVisible(False)
for t in old_texts + old_tri:
    board.Remove(t)
# the Pi legend fits inside the Pi outline; the ribbon legend moves off the pull-down row into it
text("RASPBERRY PI 3B+ / 4 / 5", PI_X0 + 42.5, PI_Y0 + 20.5, 2.0, bold=True)
text("FACE UP ON M2.5 STANDOFFS, 10 MM OR TALLER", PI_X0 + 42.5, PI_Y0 + 24.0, 1.2)
text("AUDIO: PI JACK (3B+/4) OR USB SOUND CARD (5) -> J19", PI_X0 + 42.5, PI_Y0 + 33.5, 1.0)
text("J16 PI RIBBON - PIN 1 = PI PIN 1 (3V3)", 60.5, PI_Y0 + 44.0, 1.0)
# pin-1 triangle and its "1" 1.2 mm further right, clear of the header's own outline
tri = pcbnew.PCB_SHAPE(board); tri.SetShape(pcbnew.SHAPE_T_POLY)
tri.SetPolyPoints([MM(p1x + 6.2, p1y - 1.3), MM(p1x + 6.2, p1y + 1.3), MM(p1x + 4.6, p1y)])
tri.SetLayer(pcbnew.F_SilkS); tri.SetFilled(True); tri.SetWidth(FromMM(0.15)); board.Add(tri)
one[0].SetPosition(MM(p1x + 7.6, p1y))

for t in dead:
    board.Remove(t)
print("removed", len(dead), "dangling stubs")
filler = pcbnew.ZONE_FILLER(board)
filler.Fill(board.Zones())
pcbnew.SaveBoard(PCB, board)
print("saved", PCB)
