import type { Prop, PropKind } from '$lib/api/types';

/** Normalized (0..1) pixel positions for a prop, derived from its kind when the layout has no points. */
export function derivePoints(kind: PropKind, n: number, matrix?: { width: number; height: number }): Float32Array {
	const pts = new Float32Array(n * 2);
	const set = (i: number, x: number, y: number) => {
		pts[i * 2] = x;
		pts[i * 2 + 1] = y;
	};
	if (n <= 0) return pts;
	switch (kind) {
		case 'arch': {
			for (let i = 0; i < n; i++) {
				const a = Math.PI - (i / Math.max(1, n - 1)) * Math.PI;
				set(i, 0.5 + 0.5 * Math.cos(a), 1 - Math.sin(a));
			}
			break;
		}
		case 'candycane': {
			// straight shaft then a hook
			const shaft = Math.round(n * 0.62);
			for (let i = 0; i < n; i++) {
				if (i < shaft) set(i, 0.72, 1 - (i / shaft) * 0.72);
				else {
					const t = (i - shaft) / Math.max(1, n - shaft - 1);
					const a = t * Math.PI;
					set(i, 0.46 + 0.26 * Math.cos(a), 0.28 - 0.26 * Math.sin(a));
				}
			}
			break;
		}
		case 'tree': {
			const strands = Math.max(4, Math.min(24, Math.round(Math.sqrt(n / 3))));
			const per = Math.ceil(n / strands);
			for (let i = 0; i < n; i++) {
				const s = Math.floor(i / per);
				const k = i % per;
				const t = k / Math.max(1, per - 1); // 0 bottom → 1 top
				const bx = s / Math.max(1, strands - 1);
				const up = s % 2 === 0 ? t : 1 - t; // zig-zag strands
				set(i, 0.5 + (bx - 0.5) * (1 - up), 1 - up);
			}
			break;
		}
		case 'matrix': {
			const w = matrix?.width ?? Math.ceil(Math.sqrt(n * 2));
			const h = matrix?.height ?? Math.ceil(n / w);
			for (let i = 0; i < n; i++) {
				const x = i % w;
				const y = Math.floor(i / w);
				set(i, (x + 0.5) / w, (y + 0.5) / h);
			}
			break;
		}
		case 'circle':
		case 'spinner': {
			if (kind === 'spinner') {
				const arms = 8;
				const per = Math.ceil(n / arms);
				for (let i = 0; i < n; i++) {
					const a = (Math.floor(i / per) / arms) * Math.PI * 2;
					const r = (((i % per) + 1) / per) * 0.5;
					set(i, 0.5 + r * Math.cos(a), 0.5 + r * Math.sin(a));
				}
			} else
				for (let i = 0; i < n; i++) {
					const a = (i / n) * Math.PI * 2 - Math.PI / 2;
					set(i, 0.5 + 0.5 * Math.cos(a), 0.5 + 0.5 * Math.sin(a));
				}
			break;
		}
		case 'star': {
			const verts: [number, number][] = [];
			for (let k = 0; k < 10; k++) {
				const a = (k / 10) * Math.PI * 2 - Math.PI / 2;
				const r = k % 2 === 0 ? 0.5 : 0.21;
				verts.push([0.5 + r * Math.cos(a), 0.52 + r * Math.sin(a)]);
			}
			polyline(verts, true, n, set);
			break;
		}
		case 'window': {
			polyline(
				[
					[0, 1],
					[0, 0],
					[1, 0],
					[1, 1]
				],
				true,
				n,
				set
			);
			break;
		}
		case 'icicles': {
			const drops = Math.max(4, Math.round(n / 10));
			const per = Math.ceil(n / drops);
			for (let i = 0; i < n; i++) {
				const d = Math.floor(i / per);
				const k = i % per;
				const len = 0.35 + 0.65 * ((Math.sin(d * 2.3) + 1) / 2);
				const down = d % 2 === 0 ? k / per : 1 - k / per;
				set(i, (d + 0.5) / drops, down * len);
			}
			break;
		}
		default: {
			for (let i = 0; i < n; i++) set(i, n === 1 ? 0.5 : i / (n - 1), 0.5);
		}
	}
	return pts;
}

function polyline(
	verts: [number, number][],
	closed: boolean,
	n: number,
	set: (i: number, x: number, y: number) => void
) {
	const segs: [number, number, number, number, number][] = [];
	let total = 0;
	const count = closed ? verts.length : verts.length - 1;
	for (let k = 0; k < count; k++) {
		const [x1, y1] = verts[k];
		const [x2, y2] = verts[(k + 1) % verts.length];
		const len = Math.hypot(x2 - x1, y2 - y1);
		segs.push([x1, y1, x2, y2, len]);
		total += len;
	}
	for (let i = 0; i < n; i++) {
		let d = (i / n) * total;
		for (const [x1, y1, x2, y2, len] of segs) {
			if (d <= len) {
				const t = len ? d / len : 0;
				set(i, x1 + (x2 - x1) * t, y1 + (y2 - y1) * t);
				break;
			}
			d -= len;
		}
	}
}

const cache = new Map<string, Float32Array>();

/** Normalized points for a prop (layout.points wins; otherwise derived from kind). Cached. */
export function propPoints(prop: Prop): Float32Array {
	const key = `${prop.id}:${prop.kind}:${prop.pixelCount}:${prop.layout?.points?.length ?? 0}:${prop.matrix?.width ?? 0}`;
	let p = cache.get(key);
	if (!p) {
		if (prop.layout?.points?.length) {
			p = new Float32Array(prop.pixelCount * 2);
			prop.layout.points.slice(0, prop.pixelCount).forEach(([x, y], i) => {
				p![i * 2] = x;
				p![i * 2 + 1] = y;
			});
		} else p = derivePoints(prop.kind, prop.pixelCount, prop.matrix);
		cache.set(key, p);
	}
	return p;
}

/** Axis-aligned bounds of all prop layouts in world units. */
export function worldBounds(props: Prop[]): { x: number; y: number; w: number; h: number } {
	let x0 = Infinity,
		y0 = Infinity,
		x1 = -Infinity,
		y1 = -Infinity;
	for (const p of props) {
		const l = p.layout;
		if (!l) continue;
		x0 = Math.min(x0, l.x);
		y0 = Math.min(y0, l.y);
		x1 = Math.max(x1, l.x + l.w);
		y1 = Math.max(y1, l.y + l.h);
	}
	if (!Number.isFinite(x0)) return { x: 0, y: 0, w: 1000, h: 500 };
	return { x: x0, y: y0, w: Math.max(1, x1 - x0), h: Math.max(1, y1 - y0) };
}

/** Default "home" layout box for props without one (lines them up along the bottom). */
export function fallbackLayout(index: number): { x: number; y: number; w: number; h: number; rotation: number } {
	return { x: 40 + (index % 10) * 110, y: 620 + Math.floor(index / 10) * 90, w: 90, h: 60, rotation: 0 };
}
