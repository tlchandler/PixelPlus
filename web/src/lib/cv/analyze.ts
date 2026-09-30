// Camera mapping analysis (F6, WS4): decoded lights + the show → proposals.
// Pure and DOM-free (unit-tested in analyze.test.ts).
//
//  * which output lit where → props "not seen", props plugged into each other's
//    ports (swap), strings running backwards (reverse), strings shorter than
//    configured (pixel count), and new layout positions from the photo;
//  * image → layout mapping: a similarity transform (scale, rotation,
//    translation) fitted with RANSAC over props whose layout agrees with where
//    they were seen, so a swapped or misplaced prop cannot skew it.
import type { Prop, PropLayout, PropSegment, Show } from '$lib/api/types';
import type { CvProposal, RunTarget } from './types';

export type Detected = { k: number; pixels: [number, number, number, number][] }[];

/** 2D similarity as complex numbers: w = a·z + t. */
export interface Similarity {
	ar: number;
	ai: number;
	tr: number;
	ti: number;
}
export const applySim = (s: Similarity, x: number, y: number): [number, number] => [
	s.ar * x - s.ai * y + s.tr,
	s.ai * x + s.ar * y + s.ti
];

/** Least-squares similarity mapping `from` onto `to` (weights optional). */
export function fitSimilarity(
	from: [number, number][],
	to: [number, number][],
	w?: number[]
): Similarity | null {
	const n = from.length;
	if (n < 2) return null;
	let sw = 0,
		px = 0,
		py = 0,
		qx = 0,
		qy = 0;
	for (let i = 0; i < n; i++) {
		const wi = w?.[i] ?? 1;
		sw += wi;
		px += wi * from[i][0];
		py += wi * from[i][1];
		qx += wi * to[i][0];
		qy += wi * to[i][1];
	}
	px /= sw;
	py /= sw;
	qx /= sw;
	qy /= sw;
	let nr = 0,
		ni = 0,
		den = 0;
	for (let i = 0; i < n; i++) {
		const wi = w?.[i] ?? 1;
		const ax = from[i][0] - px,
			ay = from[i][1] - py;
		const bx = to[i][0] - qx,
			by = to[i][1] - qy;
		// (b) · conj(a)
		nr += wi * (bx * ax + by * ay);
		ni += wi * (by * ax - bx * ay);
		den += wi * (ax * ax + ay * ay);
	}
	if (den < 1e-9) return null;
	const ar = nr / den,
		ai = ni / den;
	return { ar, ai, tr: qx - (ar * px - ai * py), ti: qy - (ai * px + ar * py) };
}

/** Spearman rank correlation. */
export function spearman(a: number[], b: number[]): number {
	const n = a.length;
	if (n < 3) return 0;
	const rank = (v: number[]) => {
		const idx = v.map((x, i) => [x, i] as const).sort((p, q) => p[0] - q[0]);
		const r = new Array<number>(n);
		for (let i = 0; i < n;) {
			let j = i;
			while (j + 1 < n && idx[j + 1][0] === idx[i][0]) j++;
			for (let k = i; k <= j; k++) r[idx[k][1]] = (i + j) / 2;
			i = j + 1;
		}
		return r;
	};
	const ra = rank(a),
		rb = rank(b);
	const ma = (n - 1) / 2;
	let num = 0,
		da = 0,
		db = 0;
	for (let i = 0; i < n; i++) {
		num += (ra[i] - ma) * (rb[i] - ma);
		da += (ra[i] - ma) ** 2;
		db += (rb[i] - ma) ** 2;
	}
	return da && db ? num / Math.sqrt(da * db) : 0;
}

/** World (canvas) position of pixel `i` of a prop's layout, or the box centre. */
export function layoutPoint(l: PropLayout, i: number): [number, number] {
	const cx = l.x + l.w / 2,
		cy = l.y + l.h / 2;
	const p = l.points?.[i];
	if (!p) return [cx, cy];
	const dx = l.x + p[0] * l.w - cx,
		dy = l.y + p[1] * l.h - cy;
	const th = ((l.rotation || 0) * Math.PI) / 180;
	const c = Math.cos(th),
		s = Math.sin(th);
	return [cx + dx * c - dy * s, cy + dx * s + dy * c];
}

function layoutCentroid(p: Prop): [number, number] | null {
	const l = p.layout;
	if (!l) return null;
	if (!l.points?.length) return [l.x + l.w / 2, l.y + l.h / 2];
	let x = 0,
		y = 0;
	for (let i = 0; i < l.points.length; i++) {
		const [a, b] = layoutPoint(l, i);
		x += a;
		y += b;
	}
	return [x / l.points.length, y / l.points.length];
}
const layoutSize = (p: Prop) => Math.max(1, p.layout?.w ?? 0, p.layout?.h ?? 0);

/** One decoded light attributed to a prop pixel. */
export interface PropHit {
	propPixel: number;
	segment: number;
	/** Image position in pixels. */
	x: number;
	y: number;
	conf: number;
}

export interface PropFinding {
	propId: string;
	name: string;
	hits: PropHit[];
	/** Where the prop's output was identified without single pixels (dense strings), image px. */
	regions: [number, number][];
	/** Image centroid (px). */
	centroid?: [number, number];
	pixelCount: number;
}

export interface Analysis {
	proposals: CvProposal[];
	props: PropFinding[];
	transform: Similarity | null;
	/** Props seen / props in scope. */
	seen: number;
	total: number;
	/** Targets without a single decoded light. */
	notSeenTargets: number[];
}

/** Which prop pixel an output pixel drives. */
function propAt(
	show: Show,
	nodeId: string,
	output: number,
	idx: number
): { prop: Prop; segment: number; propPixel: number } | null {
	for (const prop of show.props) {
		for (let si = 0; si < prop.segments.length; si++) {
			const s = prop.segments[si];
			if (s.nodeId !== nodeId || s.output !== output) continue;
			if (idx < s.startPixel || idx >= s.startPixel + s.pixelCount) continue;
			const i = idx - s.startPixel;
			return { prop, segment: si, propPixel: s.propOffset + (s.reverse ? s.pixelCount - 1 - i : i) };
		}
	}
	return null;
}

const alone = (show: Show, seg: PropSegment) =>
	show.props.flatMap((p) => p.segments).filter((s) => s.nodeId === seg.nodeId && s.output === seg.output)
		.length === 1;

export interface AnalyzeOptions {
	/** Image size in pixels (the decoder's width/height). */
	width: number;
	height: number;
	/** Canvas box used when the show has no layout to align with. */
	fallbackBox?: { x: number; y: number; w: number; h: number };
}

export function analyze(show: Show, targets: RunTarget[], detected: Detected, o: AnalyzeOptions): Analysis {
	const W = o.width,
		H = o.height;
	const byProp = new Map<string, PropFinding>();
	const scopeProps = new Set<string>();
	for (const t of targets) for (const id of t.propIds ?? []) scopeProps.add(id);
	for (const id of scopeProps) {
		const p = show.props.find((x) => x.id === id);
		if (p) byProp.set(id, { propId: id, name: p.name, hits: [], regions: [], pixelCount: p.pixelCount });
	}
	const perTarget = new Map<number, [number, number, number, number][]>();
	for (const d of detected) perTarget.set(d.k, d.pixels);
	for (const t of targets) {
		for (const [idx, x, y, conf] of perTarget.get(t.k) ?? []) {
			if (idx < 0) {
				// A region: only attributable when one prop is on the output.
				const only = (t.propIds ?? []).length === 1 ? byProp.get(t.propIds![0]) : undefined;
				if (only) only.regions.push([x * W, y * H]);
				else
					for (const id of t.propIds ?? []) {
						// Seen, but not where exactly (several props share the output).
						const f = byProp.get(id);
						if (f && !f.regions.length) f.regions.push([NaN, NaN]);
					}
				continue;
			}
			const at = propAt(show, t.nodeId, t.output, idx);
			if (!at) continue;
			let f = byProp.get(at.prop.id);
			if (!f) {
				f = { propId: at.prop.id, name: at.prop.name, hits: [], regions: [], pixelCount: at.prop.pixelCount };
				byProp.set(at.prop.id, f);
			}
			f.hits.push({ propPixel: at.propPixel, segment: at.segment, x: x * W, y: y * H, conf });
		}
	}
	for (const f of byProp.values()) {
		const pts: [number, number][] =
			f.hits.length >= 3
				? f.hits.map((h) => [h.x, h.y])
				: [...f.hits.map((h) => [h.x, h.y] as [number, number]), ...f.regions];
		const ok = pts.filter((p) => Number.isFinite(p[0]));
		if (!ok.length) continue;
		f.centroid = [ok.reduce((a, p) => a + p[0], 0) / ok.length, ok.reduce((a, p) => a + p[1], 0) / ok.length];
	}
	const seenProp = (f: PropFinding) => f.hits.length > 0 || f.regions.length > 0;
	const props = [...byProp.values()];
	const proposals: CvProposal[] = [];
	let nid = 0;
	const push = (p: Omit<CvProposal, 'id'>) => proposals.push({ id: `p${++nid}`, ...p });
	const propOf = (id: string) => show.props.find((p) => p.id === id)!;

	// Not seen.
	const notSeenTargets = targets.filter((t) => !perTarget.get(t.k)?.length).map((t) => t.k);
	for (const f of props) {
		if (seenProp(f)) continue;
		const segs = propOf(f.propId).segments;
		const where = targets
			.filter((t) => segs.some((s) => s.nodeId === t.nodeId && s.output === t.output))
			.map((t) => t.label)
			.join(', ');
		push({
			kind: 'notSeen',
			propId: f.propId,
			message: `${f.name} wasn't seen`,
			detail: `Nothing on ${where || 'its output'} lit up in view. It may be hidden from the camera, unplugged, unpowered, or its fuse is blown.`,
			selected: false
		});
	}

	// Image → layout similarity (RANSAC over prop centroids).
	const withLayout = props.filter((f) => f.centroid && layoutCentroid(propOf(f.propId)));
	let transform: Similarity | null = null;
	if (withLayout.length >= 2) {
		const src = withLayout.map((f) => f.centroid!);
		const dst = withLayout.map((f) => layoutCentroid(propOf(f.propId))!);
		const size = withLayout.map((f) => layoutSize(propOf(f.propId)));
		const xs = dst.map((d) => d[0]),
			ys = dst.map((d) => d[1]);
		const diag = Math.hypot(Math.max(...xs) - Math.min(...xs), Math.max(...ys) - Math.min(...ys)) || 1;
		const tol = (i: number) => Math.max(0.6 * size[i], 0.04 * diag);
		// Spread (RMS radius) of each prop's lights in the image and of its layout:
		// a hypothesis must also get every inlier's size right.
		const rms = (pts: [number, number][]) => {
			const cx = pts.reduce((a, p) => a + p[0], 0) / pts.length;
			const cy = pts.reduce((a, p) => a + p[1], 0) / pts.length;
			return Math.sqrt(pts.reduce((a, p) => a + (p[0] - cx) ** 2 + (p[1] - cy) ** 2, 0) / pts.length);
		};
		const imgSpread = withLayout.map((f) => (f.hits.length >= 3 ? rms(f.hits.map((h) => [h.x, h.y])) : 0));
		const laySpread = withLayout.map((f) => {
			const l = propOf(f.propId).layout!;
			return l.points && l.points.length >= 3 ? rms(l.points.map((_, i) => layoutPoint(l, i))) : 0;
		});
		let bestIn: number[] = [];
		const n = withLayout.length;
		for (let i = 0; i < n; i++)
			for (let j = i + 1; j < n; j++) {
				const s = fitSimilarity([src[i], src[j]], [dst[i], dst[j]]);
				if (!s) continue;
				const scale = Math.hypot(s.ar, s.ai);
				if (!Number.isFinite(scale) || scale <= 0) continue;
				// A phone can't see the yard upside down or mirrored.
				if (Math.abs(Math.atan2(s.ai, s.ar)) > Math.PI / 4) continue;
				const inl: number[] = [];
				for (let k = 0; k < n; k++) {
					const [wx, wy] = applySim(s, src[k][0], src[k][1]);
					if (Math.hypot(wx - dst[k][0], wy - dst[k][1]) > tol(k)) continue;
					if (imgSpread[k] > 0 && laySpread[k] > 0) {
						const ratio = (imgSpread[k] * scale) / laySpread[k];
						if (ratio < 0.5 || ratio > 2) continue;
					}
					inl.push(k);
				}
				if (inl.length > bestIn.length) bestIn = inl;
			}
		// Need a majority (or both of two) to trust it.
		if (bestIn.length >= 2 && bestIn.length >= Math.ceil(n / 2)) {
			transform = fitSimilarity(
				bestIn.map((k) => src[k]),
				bestIn.map((k) => dst[k])
			);
		}
	}

	const flagged = new Set<string>();

	// Swaps / misplaced props.
	if (transform) {
		const nearest = (w: [number, number], except?: string) => {
			let best: Prop | null = null,
				bd = Infinity;
			for (const p of show.props) {
				if (p.id === except) continue;
				const c = layoutCentroid(p);
				if (!c) continue;
				const d = Math.hypot(w[0] - c[0], w[1] - c[1]) / layoutSize(p);
				if (d < bd) {
					bd = d;
					best = p;
				}
			}
			return best ? { prop: best, d: bd } : null;
		};
		const where = new Map<string, { prop: Prop; d: number } | null>();
		for (const f of withLayout) {
			const w = applySim(transform, f.centroid![0], f.centroid![1]);
			const own = layoutCentroid(propOf(f.propId))!;
			const ownD = Math.hypot(w[0] - own[0], w[1] - own[1]) / layoutSize(propOf(f.propId));
			const other = nearest(w, f.propId);
			where.set(f.propId, ownD > 1 && other && other.d < 0.6 ? other : null);
		}
		const done = new Set<string>();
		for (const f of withLayout) {
			const other = where.get(f.propId);
			if (!other || done.has(f.propId)) continue;
			const a = propOf(f.propId),
				b = other.prop;
			const back = where.get(b.id);
			const segA = f.hits[0]?.segment ?? 0;
			const fb = byProp.get(b.id);
			const segB = fb?.hits[0]?.segment ?? 0;
			const sa = a.segments[segA],
				sb = b.segments[segB];
			flagged.add(a.id);
			if (back && back.prop.id === a.id && sa && sb) {
				done.add(a.id);
				done.add(b.id);
				flagged.add(b.id);
				const safe = sa.pixelCount === sb.pixelCount || (alone(show, sa) && alone(show, sb));
				const la =
					targets.find((t) => t.nodeId === sa.nodeId && t.output === sa.output)?.label ??
					`output ${sa.output}`;
				const lb =
					targets.find((t) => t.nodeId === sb.nodeId && t.output === sb.output)?.label ??
					`output ${sb.output}`;
				if (safe)
					push({
						kind: 'swap',
						propId: a.id,
						message: `Swap ${a.name} and ${b.name}`,
						detail: `${a.name} lit up where ${b.name} stands and the other way round: their cables are on each other's ports (${la} ↔ ${lb}). Swapping fixes it in software.`,
						data: { a: { propId: a.id, segment: segA }, b: { propId: b.id, segment: segB } },
						selected: true
					});
				else
					push({
						kind: 'info',
						propId: a.id,
						message: `${a.name} and ${b.name} look swapped`,
						detail: `They share their outputs with other props, so swap the two cables (${la} ↔ ${lb}) or edit the wiring by hand.`,
						selected: false
					});
			} else {
				push({
					kind: 'info',
					propId: a.id,
					message: `${a.name} lit up where ${b.name} is in the layout`,
					detail: `Either the layout is off or ${a.name}'s cable is on another port. Check the wiring, or apply the new layout position.`,
					selected: false
				});
			}
		}
	}

	// Reversed strings (per segment), needs the transform and layout points.
	if (transform) {
		for (const f of withLayout) {
			const p = propOf(f.propId);
			const l = p.layout!;
			if (!l.points || l.points.length !== p.pixelCount || flagged.has(p.id)) continue;
			const pts = l.points.map((_, i) => layoutPoint(l, i));
			const bySeg = new Map<number, PropHit[]>();
			for (const h of f.hits) bySeg.set(h.segment, [...(bySeg.get(h.segment) ?? []), h]);
			for (const [si, hits] of bySeg) {
				if (hits.length < Math.max(4, 0.15 * p.segments[si].pixelCount)) continue;
				const seg = p.segments[si];
				const idx: number[] = [],
					near: number[] = [];
				for (const h of hits) {
					const [wx, wy] = applySim(transform, h.x, h.y);
					let bj = 0,
						bd = Infinity;
					for (let j = seg.propOffset; j < seg.propOffset + seg.pixelCount && j < pts.length; j++) {
						const d = (pts[j][0] - wx) ** 2 + (pts[j][1] - wy) ** 2;
						if (d < bd) {
							bd = d;
							bj = j;
						}
					}
					idx.push(h.propPixel);
					near.push(bj);
				}
				const rho = spearman(idx, near);
				if (rho < -0.6) {
					flagged.add(p.id);
					push({
						kind: 'reverse',
						propId: p.id,
						message: `${p.name} runs backwards`,
						detail: `Its lights count up from the other end than the layout says${p.segments.length > 1 ? ` (segment ${si + 1})` : ''}. Reversing the ${p.segments.length > 1 ? 'segment' : 'string'} fixes chases and text.`,
						data: { propId: p.id, segment: si },
						selected: true
					});
				}
			}
		}
	}

	// Pixel counts per output.
	for (const t of targets) {
		const px = perTarget.get(t.k) ?? [];
		const configured = t.configured ?? 0;
		if (px.length < 4 || !configured) continue;
		const maxIdx = Math.max(...px.map((p) => p[0]));
		const count = maxIdx + 1;
		const coverage = px.length / count;
		if (count < configured - 1 && coverage >= 0.5) {
			const last = (t.propIds ?? []).map(propOf).filter(Boolean).pop();
			push({
				kind: 'pixelCount',
				propId: last?.id,
				message: `${t.label}: only ${count} of ${configured} pixels lit`,
				detail: `The last ${configured - count} pixels never lit up. If the end of the string is just out of view, ignore this; otherwise the string is shorter than configured (or broken after pixel ${count}).`,
				data: { nodeId: t.nodeId, output: t.output, count, dead: [], updatePropCount: true },
				selected: false
			});
		}
	}

	// Layout from the photo.
	const allLayouts = show.props.filter((p) => p.layout);
	const box =
		o.fallbackBox ??
		(allLayouts.length
			? (() => {
					const x0 = Math.min(...allLayouts.map((p) => p.layout!.x)),
						y0 = Math.min(...allLayouts.map((p) => p.layout!.y));
					const x1 = Math.max(...allLayouts.map((p) => p.layout!.x + p.layout!.w)),
						y1 = Math.max(...allLayouts.map((p) => p.layout!.y + p.layout!.h));
					return { x: x0, y: y0, w: Math.max(1, x1 - x0), h: Math.max(1, y1 - y0) };
				})()
			: { x: 0, y: 0, w: 1200, h: (1200 * H) / W });
	const toWorld = (x: number, y: number): [number, number] => {
		if (transform) return applySim(transform, x, y);
		const s = Math.min(box.w / W, box.h / H);
		return [box.x + x * s, box.y + y * s];
	};
	for (const f of props) {
		const p = propOf(f.propId);
		if (flagged.has(p.id) || p.pixelCount < 1) continue;
		if (f.hits.length < Math.max(3, 0.2 * p.pixelCount)) continue;
		const known = new Map<number, [number, number]>();
		for (const h of f.hits) known.set(h.propPixel, toWorld(h.x, h.y));
		const ids = [...known.keys()].sort((a, b) => a - b);
		const world: [number, number][] = [];
		for (let i = 0; i < p.pixelCount; i++) {
			const k = known.get(i);
			if (k) {
				world.push(k);
				continue;
			}
			// Interpolate between the nearest decoded neighbours (clamp at the ends).
			let lo = -1,
				hi = -1;
			for (const j of ids) {
				if (j < i) lo = j;
				else if (j > i) {
					hi = j;
					break;
				}
			}
			if (lo < 0) world.push(known.get(hi)!);
			else if (hi < 0) world.push(known.get(lo)!);
			else {
				const a = known.get(lo)!,
					b = known.get(hi)!;
				const u = (i - lo) / (hi - lo);
				world.push([a[0] + (b[0] - a[0]) * u, a[1] + (b[1] - a[1]) * u]);
			}
		}
		const xs = world.map((w) => w[0]),
			ys = world.map((w) => w[1]);
		let x0 = Math.min(...xs),
			x1 = Math.max(...xs),
			y0 = Math.min(...ys),
			y1 = Math.max(...ys);
		if (x1 - x0 < 1) {
			x0 -= 0.5;
			x1 += 0.5;
		}
		if (y1 - y0 < 1) {
			y0 -= 0.5;
			y1 += 0.5;
		}
		const r4 = (v: number) => Math.round(v * 10000) / 10000;
		const r2 = (v: number) => Math.round(v * 100) / 100;
		const layout: PropLayout = {
			x: r2(x0),
			y: r2(y0),
			w: r2(x1 - x0),
			h: r2(y1 - y0),
			rotation: 0,
			points: world.map(([wx, wy]) => [r4((wx - x0) / (x1 - x0)), r4((wy - y0) / (y1 - y0))])
		};
		const seenPct = Math.round((f.hits.length / p.pixelCount) * 100);
		push({
			kind: 'layout',
			propId: p.id,
			message: `Update ${p.name}'s layout position`,
			detail: `${seenPct}% of its lights were seen${transform ? ', aligned to your existing layout' : ''}${seenPct < 100 ? '; the rest are filled in along the string' : ''}.`,
			data: { propId: p.id, layout },
			selected: !p.layout || p.layout.source === 'camera'
		});
	}

	const seen = props.filter(seenProp).length;
	return { proposals, props, transform, seen, total: props.length, notSeenTargets };
}
