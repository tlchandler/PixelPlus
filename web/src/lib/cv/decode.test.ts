// Decode accuracy on synthetic yard recordings (F6/F7). The spec's targets:
// under 1 % wrong IDs and over 95 % of visible (separable) pixels decoded.
import { describe, expect, it } from 'vitest';
import { decode } from './decode';
import { pixelBitsFor, PHASE_B, type Plan } from './mapcode';
import { makeScene, score, simulate, type SceneOptions, type SimOptions } from './simulate';

function plan(counts: number[], extra: Partial<Plan> = {}): Plan {
	return {
		seed: 1,
		bitMs: 200,
		level: 77,
		passes: 3,
		phases: 3,
		targets: counts.map((c, k) => ({ nodeId: 'n', output: k + 1, maxPixels: c })),
		pixelBits: pixelBitsFor(Math.max(...counts)),
		startPosMs: 0,
		...extra
	};
}

const COUNTS = [12, 16, 10, 14, 12, 9, 15, 11, 13, 10, 12, 14];

function run(
	sim: SimOptions,
	scene: Partial<SceneOptions> = {},
	counts = COUNTS,
	planExtra: Partial<Plan> = {}
) {
	const W = sim.width ?? 240;
	const H = sim.height ?? 135;
	const p = plan(counts, planExtra);
	const { leds } = makeScene({ width: W, height: H, counts, seed: 3, ...scene });
	const rec = simulate(p, leds, { width: W, height: H, ...sim });
	const res = decode(rec, p);
	return { p, leds, rec, res, s: score(res, leds, W, H) };
}

function expectGood(r: ReturnType<typeof run>, found = 0.95) {
	expect(r.res.ok, r.res.error).toBe(true);
	expect(r.s.found, JSON.stringify({ s: r.s, stats: r.res.stats })).toBeGreaterThan(found);
	expect(r.s.wrongRate, JSON.stringify(r.s)).toBeLessThan(0.01);
}

describe('mapping decoder on synthetic yards', { timeout: 60_000 }, () => {
	it('clean night, 30 fps', () => {
		const r = run({ fps: 30, noise: 1.5 });
		expectGood(r);
		expect(r.s.found).toBe(1);
		expect(r.res.stats.clockScore).toBeGreaterThan(0.9);
		expect(Math.abs(r.res.stats.offsetMs - 1000)).toBeLessThan(80);
		// The MappingRun "detected" shape: per target, [idx, x, y, conf] sorted by index.
		const d = r.res.detected[0];
		expect(d.k).toBe(0);
		expect(d.pixels[0][0]).toBe(0);
		expect(d.pixels[0][1]).toBeGreaterThan(0);
		expect(d.pixels[0][1]).toBeLessThan(1);
	});

	it('low light at 15 fps: noise, dropped frames, AE drift, rolling shutter, jitter', () => {
		expectGood(
			run(
				{ fps: 15, noise: 4, shot: 1, dropEvery: 7, aeDrift: 0.2, rollingShutterMs: 25, jitterMs: 6 },
				{ amp: 45, occlude: 0.1 }
			)
		);
	});

	it('distant, dim single-pixel lights over a glowing lawn', () => {
		expectGood(
			run(
				{
					fps: 24,
					noise: 3,
					psfSigma: 0.6,
					reflections: [{ x: 0, y: 70, w: 240, h: 65, gain: 0.5, ks: [0, 1, 2, 3] }]
				},
				{ amp: 22, spacing: 3 }
			)
		);
	});

	it('window reflections, a flickering streetlight and occluded lights', () => {
		const r = run(
			{
				fps: 30,
				noise: 2,
				flicker: { x: 30, y: 20, r: 12, amp: 60, hz: 99 },
				reflections: [{ x: 100, y: 90, w: 60, h: 30, gain: 1.5, ks: [0, 1, 2, 3, 4, 5] }]
			},
			{ occlude: 0.25 }
		);
		expectGood(r);
		// Hidden lights are "not seen", never wrongly placed.
		const hidden = new Set(r.leds.filter((l) => !l.visible).map((l) => `${l.k}:${l.idx}`));
		expect(hidden.size).toBeGreaterThan(0);
		expect(r.res.lights.filter((l) => hidden.has(`${l.k}:${l.idx}`))).toEqual([]);
	});

	it('snow: the whole picture glows with two props', () => {
		expectGood(
			run({
				fps: 30,
				noise: 2,
				reflections: [
					{ x: 0, y: 0, w: 240, h: 135, gain: 0.4, ks: [0, 1] },
					{ x: 60, y: 60, w: 40, h: 40, gain: 2, ks: [2] }
				]
			})
		);
	});

	it('very bright props (bloom) are reported for a lower level', () => {
		const r = run({ fps: 30, noise: 2 }, { amp: 600 });
		expectGood(r);
		expect(r.res.stats.saturatedPct).toBeGreaterThan(5);
	});

	it('hand shake and a nudge mid-run: the disturbed pass is dropped, the rest compensated', () => {
		const r = run({ fps: 30, noise: 2, shake: { sigmaPx: 0.3, jumpAtMs: 12000, jumpPx: [3, -2] } });
		expectGood(r);
		expect(r.res.stats.movedPasses).toEqual([1]);
		expect(r.res.stats.passShifts[2]).toEqual([3, -2]);
		const big = run({ fps: 30, noise: 2, shake: { jumpAtMs: 16000, jumpPx: [10, 0] } });
		expectGood(big);
		expect(big.res.stats.maxMotionPx).toBe(10);
	});

	it('a big display: 60 outputs of 20 lights at 320×180', () => {
		expectGood(run({ fps: 30, noise: 2, width: 320, height: 180 }, { spacing: 3 }, Array(60).fill(20)));
	});

	it('more than 132 outputs switches to 16-bit codes', () => {
		const r = run({ fps: 30, noise: 2, width: 320, height: 180 }, { spacing: 4 }, Array(140).fill(3));
		expectGood(r);
		expect(new Set(r.res.lights.map((l) => l.k)).size).toBe(140);
	});

	it('worst case (all of the above at once) stays free of wrong IDs', () => {
		const r = run(
			{
				fps: 15,
				noise: 4,
				shot: 1,
				dropEvery: 7,
				aeDrift: 0.25,
				rollingShutterMs: 30,
				jitterMs: 8,
				flicker: { x: 30, y: 20, r: 12, amp: 80, hz: 99 },
				reflections: [{ x: 100, y: 90, w: 60, h: 30, gain: 3, ks: [0, 1, 2, 3, 4, 5] }],
				shake: { sigmaPx: 0.4, jumpAtMs: 12000, jumpPx: [2, 1] }
			},
			{ amp: 30, occlude: 0.2 }
		);
		expectGood(r, 0.7);
	});

	it('dense strings far away: outputs are still found as regions, never mislabelled', () => {
		const counts = [160, 220, 120, 180];
		const r = run({ fps: 30, noise: 2 }, { spacing: 0.7, amp: 90 }, counts);
		expect(r.res.ok, r.res.error).toBe(true);
		const regions = r.res.regions ?? [];
		expect(new Set(regions.map((g) => g.k)).size).toBe(counts.length);
		for (const g of regions) {
			const nearest = r.leds
				.map((l) => [l, Math.hypot(l.x - g.x * 240, l.y - g.y * 135)] as const)
				.sort((a, b) => a[1] - b[1])[0];
			expect(nearest[0].k, JSON.stringify(g)).toBe(g.k);
		}
		expect(r.s.wrongRate).toBeLessThan(0.01);
		// Stored as pixel index −1.
		expect(r.res.detected.some((d) => d.pixels.some((p) => p[0] === -1))).toBe(true);
	});

	it('finds the pattern without a start hint', () => {
		const p = plan(COUNTS);
		const { leds } = makeScene({ width: 240, height: 135, counts: COUNTS, seed: 3 });
		const rec = simulate(p, leds, { fps: 30, noise: 2, leadMs: 3000 });
		delete rec.startHintMs;
		const res = decode(rec, p);
		expect(score(res, leds, 240, 135).found).toBeGreaterThan(0.95);
	});

	it('F7: phase B on one output probed past its end finds the last responding pixel', () => {
		const configured = 50;
		const probe = Math.max(Math.floor(configured * 1.25), configured + 64);
		const real = 48; // two pixels short of the configuration
		const p: Plan = {
			...plan([probe]),
			phases: PHASE_B,
			targets: [{ nodeId: 'n', output: 1, maxPixels: probe }],
			pixelBits: pixelBitsFor(probe)
		};
		const { leds } = makeScene({ width: 240, height: 135, counts: [real], seed: 4, spacing: 3 });
		const rec = simulate(p, leds, { fps: 30, noise: 2 });
		const res = decode(rec, p);
		expect(res.ok, res.error).toBe(true);
		const idx = res.lights.map((l) => l.idx);
		expect(Math.max(...idx)).toBe(real - 1);
		expect(score(res, leds, 240, 135).found).toBeGreaterThan(0.95);
	});

	it('explains failures', () => {
		const p = plan(COUNTS);
		const dark = simulate(p, [], { fps: 30, noise: 2 });
		const r1 = decode(dark, p);
		expect(r1.ok).toBe(false);
		expect(r1.error).toMatch(/blinking|pattern/);
		const short = { ...dark, frames: dark.frames.slice(0, 5) };
		expect(decode(short, p).error).toMatch(/too short/);
		const { leds } = makeScene({ width: 240, height: 135, counts: COUNTS, seed: 3 });
		const rec = simulate(p, leds, { fps: 30, noise: 2 });
		const cut = { ...rec, frames: rec.frames.filter((f) => f.t < 5000) };
		expect(decode(cut, p).error).toMatch(/stopped before|pattern/);
		const slow = simulate(p, leds, { fps: 4 });
		expect(decode(slow, p).error).toMatch(/frames per second/);
	});
});
