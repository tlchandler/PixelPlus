import { describe, expect, it } from 'vitest';
import { FlashMetric, detectFlashes, frameInterval, lumaFromRGBA, type FrameSample } from './video-onset';
import { simulateVideo, type VideoSim } from './testsignals';
import { eventsMs } from './schedule';

/** Small frames keep the tests fast; the algorithm doesn't depend on the resolution. */
function run(simIn: VideoSim, readoutFraction?: number) {
	const sim = { width: 80, height: 60, ...simIn };
	const frames = simulateVideo(sim);
	const m = new FlashMetric(sim.width ?? 160, sim.height ?? 120);
	const series: FrameSample[] = frames.map((f) => ({ t: f.t, ...m.push(f.luma) }));
	return detectFlashes(series, { flashMs: sim.flashMs, readoutFraction });
}

function errors(found: { t: number }[], expected: number[], tol: number): number[] {
	return expected.map((e) => {
		const near = found.filter((o) => Math.abs(o.t - e) < tol);
		expect(near.length, `flash at ${e.toFixed(1)}`).toBe(1);
		return near[0].t - e;
	});
}

const flashes = eventsMs(3)
	.slice(0, 20)
	.map((e) => e + 777.7);

describe('flash detection', () => {
	it('recovers onsets to a few ms at 30 fps (sub-frame interpolation)', () => {
		const found = run(
			{ fps: 30, durationMs: 18000, flashesMs: flashes, flashMs: 80, phaseMs: 3.3, noise: 2 },
			0
		);
		const err = errors(found, flashes, 40);
		expect(found.length).toBe(flashes.length);
		const worst = Math.max(...err.map(Math.abs));
		expect(worst).toBeLessThan(4);
	}, 30_000);

	it('works at 15 fps with a small lit area, drift and timestamp jitter', () => {
		const found = run(
			{
				fps: 15,
				durationMs: 18000,
				flashesMs: flashes,
				flashMs: 134,
				litFraction: 0.02,
				driftPerS: 1.5,
				jitterMs: 0.5,
				noise: 3,
				seed: 9
			},
			0
		);
		const err = errors(found, flashes, 70);
		const mean = err.reduce((a, b) => a + b, 0) / err.length;
		expect(Math.abs(mean)).toBeLessThan(4);
		expect(Math.max(...err.map(Math.abs))).toBeLessThan(10);
	}, 30_000);

	it('corrects for a rolling shutter using where the lights are', () => {
		const sim: VideoSim = {
			fps: 30,
			durationMs: 18000,
			flashesMs: flashes,
			flashMs: 80,
			litRows: [0.8, 0.95],
			readoutMs: 25,
			noise: 1.5
		};
		// Lights near the bottom are read ~22 ms after the top row.
		const naive = errors(run(sim, 0), flashes, 60);
		const corrected = errors(run(sim, 25 / (1000 / 30)), flashes, 60);
		const avg = (e: number[]) => e.reduce((a, b) => a + b, 0) / e.length;
		// Reference: the frame's centre row.
		expect(Math.abs(avg(corrected) - 12.5)).toBeLessThan(Math.abs(avg(naive) - 12.5));
	}, 30_000);

	it('ignores noise-only video and weak blips', () => {
		expect(run({ fps: 30, durationMs: 5000, flashesMs: [], flashMs: 80 }, 0)).toEqual([]);
		const m = new FlashMetric();
		const flat = new Uint8Array(160 * 120).fill(40);
		const r1 = m.push(flat);
		const r2 = m.push(flat);
		expect(r1.v).toBe(0);
		expect(r2.v).toBe(0);
		expect(r2.motion).toBe(0);
		expect(r2.mean).toBeGreaterThan(0);
	});

	it('luma and frame interval helpers', () => {
		const rgba = new Uint8ClampedArray([255, 255, 255, 255, 0, 0, 0, 255, 255, 0, 0, 255]);
		expect(Array.from(lumaFromRGBA(rgba))).toEqual([255, 0, 76]);
		expect(
			frameInterval([
				{ t: 0, v: 0 },
				{ t: 33, v: 0 },
				{ t: 67, v: 0 }
			])
		).toBeCloseTo(33.5, 5);
		expect(() => new FlashMetric().push(new Uint8Array(3))).toThrow();
	});
});
