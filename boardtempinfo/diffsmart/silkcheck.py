r"""Silkscreen sweep for diffsmart. Run with KiCad 10's Python:

  "C:\Program Files\KiCad\10.0\bin\python.exe" silkcheck.py

KiCad's DRC does check silk against pads and the board edge, but not silk against silk, and a full
`kicad-cli pcb drc` run takes minutes. This does all three in a couple of seconds so the labelling can
be iterated on: every F.SilkS text and graphic is tested against every other, against every exposed pad
and against the board outline.
"""
import os
import wx
import pcbnew

wx.DisableAsserts()
HERE = os.path.dirname(os.path.abspath(__file__))
board = pcbnew.LoadBoard(os.path.join(HERE, "diffsmart.kicad_pcb"))
MM = 1e6

def mm(v):
    return round(v / MM, 2)

# ---- collect every silkscreen item -------------------------------------------------------------
items = []
for d in board.GetDrawings():
    if d.GetLayer() == pcbnew.F_SilkS:
        label = f"text {d.GetText()!r}" if d.GetClass() == "PCB_TEXT" else "graphic"
        items.append((label, d.GetBoundingBox()))
for f in board.GetFootprints():
    for t in (f.Reference(), f.Value()):
        if t.IsVisible() and t.GetLayer() == pcbnew.F_SilkS:
            items.append((f"<{f.GetReference()} {t.GetText()}>", t.GetBoundingBox()))
    for g in f.GraphicalItems():
        if g.GetLayer() == pcbnew.F_SilkS:
            items.append((f"silk of {f.GetReference()}", g.GetBoundingBox()))

print(f"{len(items)} silkscreen items on F.SilkS")

# ---- silk against silk --------------------------------------------------------------------------
# Footprint outlines are allowed to graze each other; only flag a text against anything else.
print("\nsilk over silk:")
n = 0
for i in range(len(items)):
    for j in range(i + 1, len(items)):
        a, b = items[i], items[j]
        if not a[1].Intersects(b[1]):
            continue
        if not (a[0].startswith("text") or b[0].startswith("text") or
                a[0].startswith("<") or b[0].startswith("<")):
            continue
        n += 1
        print(f"  {a[0][:58]:60s} x {b[0][:46]}")
print(f"  {n} pairs")

# ---- silk against exposed copper ------------------------------------------------------------------
print("\nsilk over pads:")
pads = [(f"{f.GetReference()}.{p.GetNumber()} [{p.GetNetname()}]", p.GetBoundingBox())
        for f in board.GetFootprints() for p in f.Pads()
        if p.GetAttribute() != pcbnew.PAD_ATTRIB_NPTH]
n = 0
for label, bb in items:
    for pl, pb in pads:
        if bb.Intersects(pb):
            n += 1
            print(f"  {label[:58]:60s} x {pl}")
print(f"  {n} pairs")

# ---- silk against the board outline -----------------------------------------------------------------
print("\nsilk clipped by the board edge:")
bb = board.GetBoardEdgesBoundingBox()
n = 0
for label, b in items:
    if (b.GetLeft() < bb.GetLeft() or b.GetRight() > bb.GetRight()
            or b.GetTop() < bb.GetTop() or b.GetBottom() > bb.GetBottom()):
        n += 1
        print(f"  {label[:58]:60s} x {mm(b.GetLeft())}..{mm(b.GetRight())}"
              f"  y {mm(b.GetTop())}..{mm(b.GetBottom())}")
print(f"  {n} items")
