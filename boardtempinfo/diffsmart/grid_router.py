"""
A small A* maze router, used by gen_pcb.py for everything that is not bus copper.

The two inner layers of this board are solid planes, so the router only ever works on F.Cu and B.Cu.
A via is a through via and therefore also reaches the planes, which is how every SMD ground and +12 V
pad gets connected - see stitch().

Call order: Grid(...) -> add_pads/add_zone_keepout/add_existing_tracks -> stitch() -> route().
"""
import heapq
import pcbnew

HARD = -1


class Grid:
    def __init__(self, board, fps, padnet, netid, trk, via, W, H, F, B, grid=0.3, clear=0.38, via_extra=0.25):
        self.board, self.fps, self.padnet, self.netid = board, fps, padnet, netid
        self.trk, self.via = trk, via
        self.W, self.H, self.F, self.B = W, H, F, B
        self.g, self.clear, self.via_extra = grid, clear, via_extra
        self.vclear = clear + via_extra      # a via centre needs more room than a track centreline
        self.NX, self.NY = int(W / grid) + 1, int(H / grid) + 1
        self.LI = {F: 0, B: 1}
        self.LAYERS = (F, B)
        self.blk = [[0] * (self.NX * self.NY) for _ in self.LAYERS]
        self.vblk = [[0] * (self.NX * self.NY) for _ in self.LAYERS]   # same map, via-sized clearance
        n2 = 2 * self.NX * self.NY
        self._dist = [0] * n2          # reused across searches, validated by _stamp
        self._prev = [0] * n2
        self._seen = [0] * n2
        self._gen = 0
        self.netcells = {}             # centreline cells of copper already on the board, per net
        self.vias = {}                 # stitching vias placed so far, per net
        for L in self.LAYERS:                      # board edge
            self.mark(L, 0, 0, W, 0.8, HARD); self.mark(L, 0, H - 0.8, W, H, HARD)
            self.mark(L, 0, 0, 0.8, H, HARD); self.mark(L, W - 0.8, 0, W, H, HARD)

    # -- grid bookkeeping ---------------------------------------------------------------------------
    def idx(self, cx, cy):
        return cy * self.NX + cx

    def cells(self, x1, y1, x2, y2):
        a = max(0, int(x1 / self.g)); b = min(self.NX - 1, int(x2 / self.g) + 1)
        c = max(0, int(y1 / self.g)); d = min(self.NY - 1, int(y2 / self.g) + 1)
        for cy in range(c, d + 1):
            base = cy * self.NX
            for cx in range(a, b + 1):
                yield base + cx

    def mark(self, layer, x1, y1, x2, y2, owner, pad=0.0):
        """Mark the track map at the given box and the via map at the box grown by via_extra."""
        g = self.blk[self.LI[layer]]
        for i in self.cells(x1, y1, x2, y2):
            g[i] = owner if g[i] in (0, owner) else HARD
        v = self.vblk[self.LI[layer]]
        e = self.via_extra + pad
        for i in self.cells(x1 - e, y1 - e, x2 + e, y2 + e):
            v[i] = owner if v[i] in (0, owner) else HARD

    def mark_seg(self, L, netname, x1, y1, x2, y2, w, extra=0.0):
        own = self.netid.get(netname, HARD)
        r = w / 2 + self.clear + extra
        n = max(1, int(((x2 - x1) ** 2 + (y2 - y1) ** 2) ** 0.5 / (self.g / 2)))
        for k in range(n + 1):
            x = x1 + (x2 - x1) * k / n
            y = y1 + (y2 - y1) * k / n
            self.mark(L, x - r, y - r, x + r, y + r, own)

    def pad_layers(self, p):
        if p.GetAttribute() in (pcbnew.PAD_ATTRIB_PTH, pcbnew.PAD_ATTRIB_NPTH):
            return [l for l in (self.F, self.B)]
        return [p.GetLayer()] if p.GetLayer() in self.LI else []

    def add_pads(self, escape=1.8):
        """Two passes. First every pad blocks its own area plus a clearance ring. Then each pad re-claims
        its own copper and a short corridor leading straight out of the package, because on a 0.95 mm
        SOT-23 or a 1.27 mm SOIC the clearance rings of neighbouring pins overlap and would otherwise
        wall the pin in completely - which is exactly how a pad ends up unroutable."""
        for fp in self.fps.values():
            for p in fp.Pads():
                bb = p.GetBoundingBox()
                x1, y1 = bb.GetLeft() / 1e6, bb.GetTop() / 1e6
                x2, y2 = bb.GetRight() / 1e6, bb.GetBottom() / 1e6
                nn = p.GetNetname()
                own = self.netid.get(nn, HARD) if nn else HARD
                for L in self.pad_layers(p):
                    self.mark(L, x1 - self.clear, y1 - self.clear, x2 + self.clear, y2 + self.clear, own)
        cores = [{}, {}]
        for fp in self.fps.values():
            for p in fp.Pads():
                nn = p.GetNetname()
                if not nn:
                    continue
                own = self.netid.get(nn, HARD)
                bb = p.GetBoundingBox()
                for L in self.pad_layers(p):
                    for i in self.cells(bb.GetLeft() / 1e6, bb.GetTop() / 1e6,
                                        bb.GetRight() / 1e6, bb.GetBottom() / 1e6):
                        cores[self.LI[L]][i] = own
        for fp in self.fps.values():
            fc = fp.GetPosition()
            for p in fp.Pads():
                nn = p.GetNetname()
                if not nn:
                    continue
                own = self.netid.get(nn, HARD)
                bb = p.GetBoundingBox()
                x1, y1 = bb.GetLeft() / 1e6, bb.GetTop() / 1e6
                x2, y2 = bb.GetRight() / 1e6, bb.GetBottom() / 1e6
                pc = p.GetPosition()
                dx, dy = (pc.x - fc.x) / 1e6, (pc.y - fc.y) / 1e6
                # a pad is escaped out of the package, not towards its middle: pick the axis the pad
                # actually sits off-centre on, and for a pad that is off-centre both ways (a SOIC corner
                # pin) follow the pad's own long side
                if abs(dy) < 0.3:
                    ax = "x"
                elif abs(dx) < 0.3:
                    ax = "y"
                else:
                    ax = "x" if (x2 - x1) >= (y2 - y1) else "y"
                pcx, pcy = pc.x / 1e6, pc.y / 1e6
                h = self.g / 2
                if ax == "x":
                    ey1, ey2 = pcy - h, pcy + h      # keep the corridor on the pad's own axis
                    ex1, ex2 = x1, x2
                    if dx >= -0.05:
                        ex2 += escape
                    if dx <= 0.05:
                        ex1 -= escape
                else:
                    ex1, ex2 = pcx - h, pcx + h
                    ey1, ey2 = y1, y2
                    if dy >= -0.05:
                        ey2 += escape
                    if dy <= 0.05:
                        ey1 -= escape
                for L in self.pad_layers(p):
                    li = self.LI[L]
                    g = self.blk[li]
                    for i in self.cells(ex1, ey1, ex2, ey2):
                        if cores[li].get(i, own) == own:      # never carve into another pad
                            g[i] = own
        for li in (0, 1):
            g = self.blk[li]
            for i, own in cores[li].items():
                g[i] = own

    def mark_force(self, layer, x1, y1, x2, y2, owner):
        g = self.blk[self.LI[layer]]
        for i in self.cells(x1, y1, x2, y2):
            g[i] = owner
        v = self.vblk[self.LI[layer]]
        for i in self.cells(x1, y1, x2, y2):
            v[i] = owner

    def add_zone_keepout(self, layer, x1, y1, x2, y2):
        self.mark(layer, x1 - 0.4, y1 - 0.4, x2 + 0.4, y2 + 0.4, HARD)

    def note_cells(self, netname, L, x1, y1, x2, y2):
        s = self.netcells.setdefault(netname, set())
        n = max(1, int(((x2 - x1) ** 2 + (y2 - y1) ** 2) ** 0.5 / (self.g / 2)))
        for k in range(n + 1):
            x = x1 + (x2 - x1) * k / n
            y = y1 + (y2 - y1) * k / n
            cx, cy = int(x / self.g), int(y / self.g)
            if 0 <= cx < self.NX and 0 <= cy < self.NY:
                s.add((self.LI[L], cy * self.NX + cx))

    def add_existing_tracks(self):
        for t in self.board.GetTracks():
            if isinstance(t, pcbnew.PCB_VIA):
                x, y = t.GetPosition().x / 1e6, t.GetPosition().y / 1e6
                # PCB_VIA::GetWidth() with no layer argument trips a wx assert in KiCad 10,
                # which pops a modal dialog and hangs a headless run - derive the radius from the drill
                r = t.GetDrillValue() / 2e6 + 0.15 + self.clear
                own = self.netid.get(t.GetNetname(), HARD)
                for L in self.LAYERS:
                    self.mark(L, x - r, y - r, x + r, y + r, own)
                    self.note_cells(t.GetNetname(), L, x, y, x, y)
            elif t.GetLayer() in self.LI:
                s, e = t.GetStart(), t.GetEnd()
                self.mark_seg(t.GetLayer(), t.GetNetname(),
                              s.x / 1e6, s.y / 1e6, e.x / 1e6, e.y / 1e6, t.GetWidth() / 1e6)
                self.note_cells(t.GetNetname(), t.GetLayer(), s.x / 1e6, s.y / 1e6, e.x / 1e6, e.y / 1e6)

    # -- search -------------------------------------------------------------------------------------
    def astar(self, sources, targets, own, via_cost=14):
        """Flat integer nodes and array-backed distances; the heuristic is the Manhattan distance to
        the target bounding box, which is admissible and O(1) instead of scanning every target cell."""
        NX, NY = self.NX, self.NY
        N = NX * NY
        tgt = set(targets)
        txs = [i % NX for _, i in tgt]; tys = [i // NX for _, i in tgt]
        tx0, tx1, ty0, ty1 = min(txs), max(txs), min(tys), max(tys)
        tnodes = {L * N + i for L, i in tgt}
        INF = 1 << 30
        self._gen += 1
        gen = self._gen
        dist, prev, seen = self._dist, self._prev, self._seen
        blk0, blk1 = self.blk[0], self.blk[1]
        vb0, vb1 = self.vblk[0], self.vblk[1]
        budget = 2000000
        pq = []
        for L, i in sources:
            n = L * N + i
            if seen[n] != gen:
                seen[n] = gen; dist[n] = 0; prev[n] = -2
                heapq.heappush(pq, (0, n, -1))

        def h(n):
            i = n % N
            cx, cy = i % NX, i // NX
            dx = tx0 - cx if cx < tx0 else (cx - tx1 if cx > tx1 else 0)
            dy = ty0 - cy if cy < ty0 else (cy - ty1 if cy > ty1 else 0)
            return dx + dy

        while pq:
            budget -= 1
            if budget < 0:
                return None
            _, n, par = heapq.heappop(pq)
            if seen[n] == gen and prev[n] != -2:
                continue
            prev[n] = par
            if n in tnodes:
                path = []
                while n != -1:
                    path.append((n // N, n % N)); n = prev[n]
                return path[::-1]
            L = n // N; i = n - L * N
            cx, cy = i % NX, i // NX
            g = dist[n]
            blk = blk0 if L == 0 else blk1
            for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                nx_, ny_ = cx + dx, cy + dy
                if nx_ < 0 or nx_ >= NX or ny_ < 0 or ny_ >= NY:
                    continue
                j = ny_ * NX + nx_
                v = blk[j]
                if v and v != own:
                    continue
                m = L * N + j
                ng = g + 1
                if seen[m] != gen:
                    seen[m] = gen; dist[m] = INF; prev[m] = -2
                if prev[m] == -2 and ng < dist[m]:
                    dist[m] = ng
                    heapq.heappush(pq, (ng + h(m), m, n))
            v0, v1 = vb0[i], vb1[i]
            if (not v0 or v0 == own) and (not v1 or v1 == own):
                m = (1 - L) * N + i
                ng = g + via_cost
                if seen[m] != gen:
                    seen[m] = gen; dist[m] = INF; prev[m] = -2
                if prev[m] == -2 and ng < dist[m]:
                    dist[m] = ng
                    heapq.heappush(pq, (ng + h(m), m, n))
        return None

    # -- emit ---------------------------------------------------------------------------------------
    def _flush(self, run, netname, w):
        if len(run) < 2:
            return
        L = self.LAYERS[run[0][0]]
        simp = [run[0]]
        for k in range(1, len(run) - 1):
            a, b, c = run[k - 1], run[k], run[k + 1]
            if (b[1] - a[1], b[2] - a[2]) != (c[1] - b[1], c[2] - b[2]):
                simp.append(b)
        simp.append(run[-1])
        self.trk(L, netname, [(p[1], p[2]) for p in simp], w)
        for a, b in zip(simp, simp[1:]):
            self.mark_seg(L, netname, a[1], a[2], b[1], b[2], w)

    def emit(self, path, netname, w, snap_start=None, snap_end=None):
        """Lay a cell path down as tracks and vias. Vias always go on the raw grid point; the snapped
        pad centre is only ever a track endpoint, otherwise a path that ends on a layer change drops a
        via straight onto the pad it was trying to reach."""
        own = self.netid.get(netname, HARD)
        pts = [(L, (i % self.NX) * self.g, (i // self.NX) * self.g) for L, i in path]
        disp = list(pts)
        if snap_start:
            disp[0] = (pts[0][0], snap_start[0], snap_start[1])
        if snap_end:
            disp[-1] = (pts[-1][0], snap_end[0], snap_end[1])
        run = [disp[0]]
        if disp[0] != pts[0]:
            run.append(pts[0])
        for k in range(1, len(pts)):
            if pts[k][0] != pts[k - 1][0]:
                vx, vy = pts[k][1], pts[k][2]
                self._flush(run, netname, w)
                self.via(netname, vx, vy)
                r = 0.3 + self.clear
                for L in self.LAYERS:
                    self.mark(L, vx - r, vy - r, vx + r, vy + r, own)
                run = [(pts[k][0], vx, vy)]
                if disp[k] != (pts[k][0], vx, vy):
                    run.append(disp[k])
            else:
                run.append(disp[k])
        self._flush(run, netname, w)

    # -- public -------------------------------------------------------------------------------------
    def cells_inside(self, x1, y1, x2, y2, cx, cy, pad=None):
        """Only cells that really sit on the pad. cells() rounds outwards, which is right for keepouts
        and wrong for targets: a track that ends one cell past the pad edge is a dangling end.

        The bounding box is not enough on its own. A round pad's bbox corners are outside its copper by
        (sqrt(2)-1) r, which on a 1.8 mm through-hole pad is 0.37 mm - more than a grid step - so a path
        allowed to finish there ends in open laminate. When the pad object is available, HitTest decides."""
        import math
        a, b = math.ceil(x1 / self.g), math.floor(x2 / self.g)
        c, d = math.ceil(y1 / self.g), math.floor(y2 / self.g)
        out = []
        for cy2 in range(max(0, c), min(self.NY - 1, d) + 1):
            for cx2 in range(max(0, a), min(self.NX - 1, b) + 1):
                if pad is not None:
                    from pcbnew import VECTOR2I_MM
                    if not pad.HitTest(VECTOR2I_MM(cx2 * self.g, cy2 * self.g), 0):
                        continue
                out.append(cy2 * self.NX + cx2)
        if not out:
            out = [int(round(cy / self.g)) * self.NX + int(round(cx / self.g))]
        return out

    def pad_cells(self, p):
        bb = p.GetBoundingBox()
        pc = p.GetPosition()
        return {(self.LI[L], i) for L in self.pad_layers(p)
                for i in self.cells_inside(bb.GetLeft() / 1e6, bb.GetTop() / 1e6,
                                           bb.GetRight() / 1e6, bb.GetBottom() / 1e6,
                                           pc.x / 1e6, pc.y / 1e6, p)}

    def components(self, cells):
        """Split a set of (layer, cell) into connected components, walking the grid and via-capable
        layer changes the same way the router itself would."""
        NX, NY = self.NX, self.NY
        todo, out = set(cells), []
        while todo:
            seed = todo.pop()
            comp, stack = {seed}, [seed]
            while stack:
                L, i = stack.pop()
                cx, cy = i % NX, i // NX
                for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                    nx_, ny_ = cx + dx, cy + dy
                    if 0 <= nx_ < NX and 0 <= ny_ < NY:
                        m = (L, ny_ * NX + nx_)
                        if m in todo:
                            todo.discard(m); comp.add(m); stack.append(m)
                m = (1 - L, i)
                if m in todo:
                    todo.discard(m); comp.add(m); stack.append(m)
            out.append(comp)
        return out

    def route(self, netname, w=0.25, split_pre=False):
        pads = [p for ref, fp in sorted(self.fps.items()) for p in fp.Pads() if p.GetNetname() == netname]
        names = [f"{ref}.{p.GetNumber()}" for ref, fp in sorted(self.fps.items())
                 for p in fp.Pads() if p.GetNetname() == netname]
        groups, centres = [], []
        for p in pads:
            g = self.pad_cells(p)
            if g:
                groups.append(g)
                centres.append((p.GetPosition().x / 1e6, p.GetPosition().y / 1e6))
        if len(groups) < 2:
            return True
        own = self.netid.get(netname, HARD)
        pre = self.netcells.get(netname)
        if pre:
            # This net already has copper. Normally all of it counts as the tree: a hand-routed trunk
            # is one piece, and even where it is not, the pieces are usually joined by a zone this
            # router does not model - so treating them as separate just produces redundant track.
            #
            # split_pre says otherwise. Hand-routed escape stubs on a fine-pitch part really are
            # disjoint, and seeding the tree with all of them would quietly assert they are joined:
            # the net "routes", and DRC then reports the ones that were never connected.
            pre_set = set(pre)
            if split_pre:
                comps = self.components(pre_set)
                comps.sort(key=len, reverse=True)
                tree = set(comps[0])
                rest = [(c, None) for c in comps[1:]] + list(zip(groups, centres))
            else:
                tree = set(pre_set)
                rest = list(zip(groups, centres))
            first = None
        else:
            pre_set = set()
            tree = set(groups[0])
            rest = list(zip(groups[1:], centres[1:]))
            first = centres[0]
        while rest:
            # take whichever pad is reachable now and retry the rest; connecting greedily in list order
            # can strand a pad that would have been reachable once the tree had grown
            hit = None
            for k, (g, _) in enumerate(rest):
                path = self.astar(tree, g, own)
                if path is not None and (hit is None or len(path) < len(hit[1])):
                    hit = (k, path)
            if hit is None:
                left = {id(g) for g, _ in rest}
                bad = [(names[i], groups[i]) for i, g in enumerate(groups) if id(g) in left]
                print("   ROUTE FAILED", netname, "unreachable:", [n for n, _ in bad][:6])
                # How boxed in is it? Flood from the pad's own cells and report how far it gets; a
                # handful of cells means the pad cannot escape its own package, thousands means the
                # pad is fine and the two halves of the net are separated by something larger.
                for nm, g in bad[:3]:
                    seen, stack = set(g), list(g)
                    NX = self.NX
                    while stack and len(seen) < 4000:
                        L, i = stack.pop()
                        cx, cy = i % NX, i // NX
                        for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                            nx_, ny_ = cx + dx, cy + dy
                            if not (0 <= nx_ < NX and 0 <= ny_ < self.NY):
                                continue
                            j = ny_ * NX + nx_
                            v = self.blk[L][j]
                            if v and v != own:
                                continue
                            if (L, j) not in seen:
                                seen.add((L, j)); stack.append((L, j))
                        v0, v1 = self.vblk[0][i], self.vblk[1][i]
                        if (not v0 or v0 == own) and (not v1 or v1 == own) and (1 - L, i) not in seen:
                            seen.add((1 - L, i)); stack.append((1 - L, i))
                    print(f"       {nm}: {len(seen)} cells reachable")
                return False
            k, path = hit
            self.emit(path, netname, w, snap_start=first, snap_end=rest[k][1])
            first = None
            tree |= set(path) | rest[k][0]
            rest.pop(k)
        return True

    def stitch(self, ref, num, netname):
        """Drop a via beside an SMD pad so the inner plane picks the net up."""
        p = self.fps[ref].FindPadByNumber(str(num))
        px, py = p.GetPosition().x / 1e6, p.GetPosition().y / 1e6
        own = self.netid[netname]
        import math
        for d in (1.0, 1.3, 1.6, 2.0, 2.5, 3.0, 3.6, 4.4):
            for a in range(16):
                ang = a * math.pi / 8
                vx, vy = px + d * math.cos(ang), py + d * math.sin(ang)
                if not (1 < vx < self.W - 1 and 1 < vy < self.H - 1):
                    continue
                if not all(self.vblk[L][i] in (0, own)
                           for L in (0, 1)
                           for i in self.cells(vx - 0.2, vy - 0.2, vx + 0.2, vy + 0.2)):
                    continue
                n = max(1, int(d / (self.g / 2)))
                r = 0.2 + 0.2
                clear_path = True
                for k in range(n + 1):
                    sx = px + (vx - px) * k / n
                    sy = py + (vy - py) * k / n
                    if not all(self.blk[0][i] in (0, own)
                               for i in self.cells(sx - r, sy - r, sx + r, sy + r)):
                        clear_path = False
                        break
                if clear_path:
                    self.trk(self.F, netname, [(px, py), (vx, vy)], 0.4)
                    self.via(netname, vx, vy, d=0.7, drill=0.3)
                    self.mark_seg(self.F, netname, px, py, vx, vy, 0.4)
                    r = 0.3 + self.clear
                    for L in self.LAYERS:
                        self.mark(L, vx - r, vy - r, vx + r, vy + r, own)
                    self.netcells.setdefault(netname, set()).add(
                        (0, int(vy / self.g) * self.NX + int(vx / self.g)))
                    self.vias.setdefault(netname, []).append((vx, vy))
                    return True
        # No room right beside the pad: walk outwards on the track map until a cell turns up where a
        # via will actually fit, and run a short track to it.
        from collections import deque
        NX = self.NX
        start = [(L, i) for L, i in self.pad_cells(p) if L == 0]
        seen = set(start)
        q = deque((n, [n]) for n in start)
        while q:
            (L, i), path = q.popleft()
            if len(path) > 2 and self.vblk[0][i] in (0, own) and self.vblk[1][i] in (0, own):
                self.emit([(0, j) for _, j in path], netname, 0.35, snap_start=(px, py))
                vx, vy = (i % NX) * self.g, (i // NX) * self.g
                self.via(netname, vx, vy, d=0.7, drill=0.3)
                r = 0.3 + self.clear
                for LL in self.LAYERS:
                    self.mark(LL, vx - r, vy - r, vx + r, vy + r, own)
                self.vias.setdefault(netname, []).append((vx, vy))
                return True
            cx, cy = i % NX, i // NX
            for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                nx_, ny_ = cx + dx, cy + dy
                if not (0 <= nx_ < NX and 0 <= ny_ < self.NY):
                    continue
                j = ny_ * NX + nx_
                if (0, j) in seen or self.blk[0][j] not in (0, own):
                    continue
                seen.add((0, j))
                q.append(((0, j), path + [(0, j)]))
        print("   STITCH FAILED", ref, num, netname, "bfs cells:", len(seen))
        return False
