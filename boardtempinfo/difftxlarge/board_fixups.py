"""
Post-routing clean-up shared by gen_pcb.py and patch_board.py.

find_dangling_stubs(board) -> list of tracks to remove. The maze router can leave a short track where it
snapped a pad centre onto its grid and then reached the net another way. Any track under 1 mm with a free
end - nothing of its own net there: no pad, via, other track or filled zone - is dead, and removing one can
free the end of another, so it repeats until none is left.

It only QUERIES the board, and does so up front: after the first board.Remove() KiCad 10's SWIG layer hands
back untyped objects for later walks and even for VECTOR2I results, so the caller removes the returned
tracks afterwards, together with any other removals. Zones must already be filled.
"""
import pcbnew

SHORT = 1.0          # mm
VIA_R = 0.36         # a stub end this close to a via centre lands on the via


def _seg_dist(px, py, x1, y1, x2, y2):
    dx, dy = x2 - x1, y2 - y1
    L2 = dx * dx + dy * dy
    t = 0.0 if L2 == 0 else max(0.0, min(1.0, ((px - x1) * dx + (py - y1) * dy) / L2))
    cx, cy = x1 + t * dx, y1 + t * dy
    return ((px - cx) ** 2 + (py - cy) ** 2) ** 0.5


def find_dangling_stubs(board):
    pads, zones, vias, segs = {}, {}, {}, []
    for fp in board.GetFootprints():
        for p in fp.Pads():
            pads.setdefault(p.GetNetCode(), []).append(p)
    for z in board.Zones():
        if not z.GetIsRuleArea():
            zones.setdefault(z.GetNetCode(), []).append(z)
    for t in board.GetTracks():
        if isinstance(t, pcbnew.PCB_VIA):
            vias.setdefault(t.GetNetCode(), []).append((t.GetPosition().x / 1e6, t.GetPosition().y / 1e6))
        else:
            s, e = t.GetStart(), t.GetEnd()
            segs.append([t, t.GetNetCode(), t.GetLayer(), s.x / 1e6, s.y / 1e6, e.x / 1e6, e.y / 1e6,
                         t.GetWidth() / 2e6, t.GetLength() / 1e6, s, e])
    by_net = {}
    for sg in segs:
        by_net.setdefault((sg[1], sg[2]), []).append(sg)

    # static part: pads, vias and zones never change here, so ask KiCad once per stub end
    def static_ok(sg, x, y, pt):
        nc, L = sg[1], sg[2]
        # copper overlap, not centre-on-centre: a stub end whose own width reaches the pad counts
        if any(p.HitTest(pt, int(sg[7] * 1e6)) for p in pads.get(nc, ())):
            return True
        if any((x - vx) ** 2 + (y - vy) ** 2 <= VIA_R ** 2 for vx, vy in vias.get(nc, ())):
            return True
        return any(z.IsOnLayer(L) and z.HitTestFilledArea(L, pt) for z in zones.get(nc, ()))

    stubs = [sg for sg in segs if sg[8] < SHORT]
    fixed = {id(sg): (static_ok(sg, sg[3], sg[4], sg[9]), static_ok(sg, sg[5], sg[6], sg[10])) for sg in stubs}

    dead, alive = [], {id(sg) for sg in segs}
    while True:
        new = []
        for sg in stubs:
            if id(sg) not in alive:
                continue
            ok = []
            for k, (x, y) in enumerate(((sg[3], sg[4]), (sg[5], sg[6]))):
                if fixed[id(sg)][k]:
                    ok.append(True); continue
                ok.append(any(o is not sg and id(o) in alive and _seg_dist(x, y, o[3], o[4], o[5], o[6]) <= o[7] + sg[7] - 1e-3
                              for o in by_net[(sg[1], sg[2])]))
            if not all(ok):
                new.append(sg)
        if not new:
            return dead
        for sg in new:
            alive.discard(id(sg)); dead.append(sg[0])
