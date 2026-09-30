"""Port 3 polarity fix (2026-09-29): J14 pin 5 = P3P (+), pin 4 = P3N (-), the Falcon standard.

The old board had 4(+)/5(-), copied from difftx rev D, which a PixelController SRx1 showed to be reversed.
P3P now drops through a via at (13.0, 50.51) and runs on B.Cu between pads 4 and 6 to pad 5 (0.2 mm track,
0.22 mm to each pad). P3N takes the F.Cu hop from (12.0, 49.49) to pad 4. Same geometry on diffrx and diffsmart.
Mirrored in gen_pcb.py. Run with KiCad's Python from the design folder:  python patch_port3.py <board>.kicad_pcb
"""
import sys
import pcbnew
from pcbnew import VECTOR2I_MM as MM, FromMM, ToMM

path = sys.argv[1]
board = pcbnew.LoadBoard(path)
F, B = pcbnew.F_Cu, pcbnew.B_Cu
net = lambda n: board.FindNet(n)

j14 = board.FindFootprintByReference("J14")
pads = {p.GetNumber(): p for p in j14.Pads()}
p4, p5 = pads["4"], pads["5"]
assert (p4.GetNetname(), p5.GetNetname()) == ("/P3P", "/P3N"), "already patched?"
assert (round(ToMM(p4.GetPosition().x), 2), round(ToMM(p4.GetPosition().y), 2)) == (9.25, 50.51)
assert (round(ToMM(p5.GetPosition().x), 2), round(ToMM(p5.GetPosition().y), 2)) == (7.47, 49.49)

# every F.Cu track of the old net that touches pad 4 (P3P) or pad 5 (P3N): the two jack runs and the escape stubs
doomed = []
for t in board.GetTracks():
    if isinstance(t, pcbnew.PCB_VIA) or t.GetLayer() != F:
        continue
    for pad in (p4, p5):
        if t.GetNetname() == pad.GetNetname() and (pad.HitTest(t.GetStart()) or pad.HitTest(t.GetEnd())):
            doomed.append(t)
ends = sorted((round(ToMM(max(t.GetStart().x, t.GetEnd().x)), 2), t.GetNetname()) for t in doomed)
assert (13.0, "/P3P") in ends and (12.0, "/P3N") in ends, ends
for t in doomed:
    board.Remove(t)

p4.SetNet(net("/P3N"))
p5.SetNet(net("/P3P"))

def trk(layer, netname, pts, w=0.25):
    for (x1, y1), (x2, y2) in zip(pts, pts[1:]):
        t = pcbnew.PCB_TRACK(board); t.SetStart(MM(x1, y1)); t.SetEnd(MM(x2, y2))
        t.SetWidth(FromMM(w)); t.SetLayer(layer); t.SetNet(net(netname)); board.Add(t)

v = pcbnew.PCB_VIA(board); v.SetPosition(MM(13.0, 50.51)); v.SetWidth(FromMM(0.7)); v.SetDrill(FromMM(0.3))
v.SetNet(net("/P3P")); v.SetLayerPair(F, B); board.Add(v)
trk(B, "/P3P", [(13.0, 50.51), (11.98, 49.49), (7.47, 49.49)], 0.2)
trk(F, "/P3N", [(9.25, 50.51), (10.98, 50.51), (12.0, 49.49)])

pcbnew.ZONE_FILLER(board).Fill(board.Zones())
pcbnew.SaveBoard(path, board)
print(f"patched {path}: removed {len(doomed)} tracks, J14.4 = /P3N, J14.5 = /P3P")
