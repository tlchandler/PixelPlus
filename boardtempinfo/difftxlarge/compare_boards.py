r"""
Compare two boards' copper, footprints and silkscreen text by geometry (UUIDs ignored), to prove a full
regeneration reproduces the board that was patched in place.

  "C:\Program Files\KiCad\10.0\bin\python.exe" compare_boards.py A.kicad_pcb B.kicad_pcb
"""
import sys
import pcbnew


def snapshot(path):
    b = pcbnew.LoadBoard(path)
    r = lambda v: round(v / 1e6, 3)
    tracks, vias, fps, texts = set(), set(), set(), set()
    for t in b.GetTracks():
        if isinstance(t, pcbnew.PCB_VIA):
            vias.add((t.GetNetname(), r(t.GetPosition().x), r(t.GetPosition().y)))
        else:
            s, e = (r(t.GetStart().x), r(t.GetStart().y)), (r(t.GetEnd().x), r(t.GetEnd().y))
            tracks.add((t.GetNetname(), t.GetLayer(), min(s, e), max(s, e), r(t.GetWidth())))
    for f in b.GetFootprints():
        fps.add((f.GetReference(), r(f.GetPosition().x), r(f.GetPosition().y), round(f.GetOrientationDegrees(), 1),
                 f.GetFPIDAsString()))
    for d in b.GetDrawings():
        if isinstance(d, pcbnew.PCB_TEXT):
            texts.add((d.GetText(), r(d.GetPosition().x), r(d.GetPosition().y)))
    return {"tracks": tracks, "vias": vias, "footprints": fps, "silk text": texts}


a, bb = snapshot(sys.argv[1]), snapshot(sys.argv[2])
same = True
for k in a:
    only_a, only_b = a[k] - bb[k], bb[k] - a[k]
    print(f"{k:11s} A {len(a[k]):6d}  B {len(bb[k]):6d}  only in A {len(only_a):4d}  only in B {len(only_b):4d}")
    for x in sorted(only_a, key=str)[:8]:
        print("     A:", x)
    for x in sorted(only_b, key=str)[:30]:
        print("     B:", x)
    same &= not only_a and not only_b
print("IDENTICAL" if same else "DIFFERENT")
