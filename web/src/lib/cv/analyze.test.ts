import { describe, expect, it } from 'vitest';
import type { Prop, Show } from '$lib/api/types';
import { analyze, applySim, fitSimilarity, layoutPoint, spearman, type Detected } from './analyze';
import type { RunTarget } from './types';

function arch(id: string, x: number, n: number, output: number, extra: Partial<Prop> = {}): Prop {
	const points: [number, number][] = Array.from({ length: n }, (_, i) => {
		const a = Math.PI * (1 - i / (n - 1));
		return [0.5 + 0.5 * Math.cos(a), 1 - Math.sin(a)];
	});
	return {
		id,
		name: `Arch ${id}`,
		kind: 'arch',
		pixelCount: n,
		channelStart: 0,
		channelsPerPixel: 3,
		segments: [
			{ nodeId: 'n1', output, startPixel: 0, pixelCount: n, propOffset: 0, reverse: false, nullPixels: 0 }
		],
		groupIds: [],
		layout: { x, y: 100, w: 80, h: 40, rotation: 0, points, source: 'xlights' },
		...extra
	};
}

function yard(): Show {
	const props = [
		arch('a', 0, 20, 1),
		arch('b', 120, 20, 2),
		arch('c', 240, 20, 3),
		arch('d', 360, 20, 4),
		arch('e', 480, 20, 5)
	];
	return { props, nodes: [{ id: 'n1', name: 'Garage', outputs: [] }], receivers: [] } as unknown as Show;
}

// The camera sees the canvas scaled by 0.4 and shifted (image px), 320×180.
const cam = { ar: 0.4, ai: 0, tr: 20, ti: 40 };
const toImage = (x: number, y: number): [number, number] => [(x - cam.tr) / cam.ar, (y - cam.ti) / cam.ar];
void toImage;
const W = 320,
	H = 180;

/** Detections of target k when output k+1 drives the prop `shownAt` (default: its own). */
function detect(
	show: Show,
	targets: RunTarget[],
	opts: { swap?: [string, string]; reverse?: string; short?: [string, number]; hide?: string } = {}
): Detected {
	const out: Detected = [];
	for (const t of targets) {
		let pid = t.propIds![0];
		if (opts.hide === pid) continue;
		if (opts.swap?.[0] === pid) pid = opts.swap[1];
		else if (opts.swap?.[1] === pid) pid = opts.swap[0];
		const p = show.props.find((x) => x.id === pid)!;
		const n = opts.short?.[0] === t.propIds![0] ? opts.short[1] : p.pixelCount;
		const pixels: [number, number, number, number][] = [];
		for (let idx = 0; idx < n; idx++) {
			const i = opts.reverse === t.propIds![0] ? p.pixelCount - 1 - idx : idx;
			const [wx, wy] = layoutPoint(p.layout!, i);
			pixels.push([idx, (wx * 0.4 + 20) / W, (wy * 0.4 + 40) / H, 0.9]);
		}
		out.push({ k: t.k, pixels });
	}
	return out;
}

const targetsOf = (show: Show): RunTarget[] =>
	show.props.map((p, k) => ({
		k,
		nodeId: 'n1',
		output: p.segments[0].output,
		label: `Garage J1-${k + 1}`,
		propIds: [p.id],
		configured: p.pixelCount
	}));

describe('mapping analysis', () => {
	it('fits similarities and ranks', () => {
		const s = fitSimilarity(
			[
				[0, 0],
				[10, 0],
				[0, 5]
			],
			[
				[5, 5],
				[5, 25],
				[-5, 5]
			]
		)!;
		const [x, y] = applySim(s, 10, 0);
		expect(x).toBeCloseTo(5);
		expect(y).toBeCloseTo(25);
		expect(spearman([1, 2, 3, 4], [4, 3, 2, 1])).toBeCloseTo(-1);
		expect(spearman([1, 2, 3, 4], [10, 20, 30, 40])).toBeCloseTo(1);
	});

	it('a correct yard proposes nothing but camera layouts for non-xLights props', () => {
		const show = yard();
		const t = targetsOf(show);
		const a = analyze(show, t, detect(show, t), { width: W, height: H });
		expect(a.seen).toBe(5);
		expect(a.transform).not.toBeNull();
		expect(a.proposals.filter((p) => p.kind !== 'layout')).toEqual([]);
		// Layouts reproduce the xLights positions (the fit maps back to the canvas).
		const l = a.proposals.find((p) => p.propId === 'c')!.data!.layout as { x: number; w: number };
		expect(l.x).toBeCloseTo(240, 0);
		expect(l.w).toBeCloseTo(80, 0);
		expect(a.proposals.every((p) => !p.selected)).toBe(true);
	});

	it('finds a swap, a reversed string, a short string and a hidden prop', () => {
		const show = yard();
		const t = targetsOf(show);
		const det = detect(show, t, { swap: ['b', 'd'], reverse: 'a', short: ['e', 15], hide: 'c' });
		const a = analyze(show, t, det, { width: W, height: H });
		const kinds = (k: string) => a.proposals.filter((p) => p.kind === k);
		const swap = kinds('swap');
		expect(swap).toHaveLength(1);
		expect(swap[0].data).toEqual({ a: { propId: 'b', segment: 0 }, b: { propId: 'd', segment: 0 } });
		expect(swap[0].selected).toBe(true);
		expect(kinds('reverse').map((p) => p.propId)).toEqual(['a']);
		expect(kinds('notSeen').map((p) => p.propId)).toEqual(['c']);
		const pc = kinds('pixelCount');
		expect(pc).toHaveLength(1);
		expect(pc[0].data).toMatchObject({ nodeId: 'n1', output: 5, count: 15 });
		// No layout proposals for props whose wiring is in question.
		expect(
			kinds('layout')
				.map((p) => p.propId)
				.sort()
		).toEqual(['e']);
		expect(a.seen).toBe(4);
		expect(a.notSeenTargets).toEqual([2]);
	});

	it('props seen only as regions (dense strings) count as seen and can still be swapped', () => {
		const show = yard();
		const t = targetsOf(show);
		const det = detect(show, t, { swap: ['b', 'd'] }).map((d) => ({
			k: d.k,
			pixels: [
				[-1, d.pixels[5][1], d.pixels[5][2], 0.5],
				[-1, d.pixels[15][1], d.pixels[15][2], 0.5]
			] as [number, number, number, number][]
		}));
		const a = analyze(show, t, det, { width: W, height: H });
		expect(a.seen).toBe(5);
		expect(a.proposals.filter((p) => p.kind === 'notSeen')).toEqual([]);
		expect(a.proposals.filter((p) => p.kind === 'swap')).toHaveLength(1);
		expect(a.proposals.filter((p) => p.kind === 'layout')).toEqual([]);
	});

	it('without any layout, positions come from the photo', () => {
		const show = yard();
		for (const p of show.props) delete p.layout;
		const t = targetsOf(show);
		const det = detect(yard(), t);
		const a = analyze(show, t, det, { width: W, height: H });
		expect(a.transform).toBeNull();
		const lay = a.proposals.filter((p) => p.kind === 'layout');
		expect(lay).toHaveLength(5);
		expect(lay.every((p) => p.selected)).toBe(true);
		const pts = (lay[0].data!.layout as { points: [number, number][] }).points;
		expect(pts).toHaveLength(20);
		expect(pts.every(([x, y]) => x >= 0 && x <= 1 && y >= 0 && y <= 1)).toBe(true);
	});
});
