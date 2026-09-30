import { describe, expect, it } from 'vitest';
import { matchSchedule } from './associate';
import { eventsMs } from './schedule';
import { gaussian, rng } from './dsp';

/** Spec test: random offset, jitter, 30 % drop-outs and false positives → error < 2 ms. */
// Signal simulation is CPU-heavy; allow for a busy machine running the whole suite.
describe('schedule association', { timeout: 60_000 }, () => {
	it('finds the offset through drop-outs, false positives and jitter', () => {
		for (let trial = 0; trial < 40; trial++) {
			const u = rng(100 + trial);
			const events = eventsMs(1000 + trial);
			const offset = 5000 + u() * 20000; // unknown phone↔leader offset
			const jitter = 2;
			const det: number[] = [];
			for (const e of events.slice(0, 32)) if (u() > 0.3) det.push(e + offset + jitter * gaussian(u));
			const real = det.length;
			const span = [det[0], det[det.length - 1]];
			for (let i = 0; i < 10; i++) det.push(span[0] + u() * (span[1] - span[0]));
			const m = matchSchedule(det, events, {
				minOffsetMs: 0,
				maxOffsetMs: 30000,
				periodMs: 60000
			});
			expect(Math.abs(m.offsetMs - offset), `trial ${trial}`).toBeLessThan(2);
			expect(m.matched).toBeGreaterThanOrEqual(real);
			expect(m.matched).toBeLessThanOrEqual(real + 2);
			expect(m.spreadMs).toBeGreaterThan(1);
			expect(m.spreadMs).toBeLessThan(3.5);
			expect(m.peak).toBeGreaterThan(1.3 * m.runnerUp);
		}
	});

	it('handles a pattern that wrapped around (period) and counts expected events', () => {
		const events = eventsMs(5);
		const period = 60000;
		const offset = 1234.5;
		// Record the end of one cycle and the start of the next.
		const det = events
			.flatMap((e) => [e, e + period])
			.map((e) => e + offset)
			.filter((t) => t > 50000 && t < 75000);
		const m = matchSchedule(det, events, {
			minOffsetMs: offset - 1500,
			maxOffsetMs: offset + 1500,
			periodMs: period,
			windowStartMs: 50000,
			windowEndMs: 75000
		});
		expect(m.offsetMs).toBeCloseTo(offset, 6);
		expect(m.matched).toBe(det.length);
		expect(m.expected).toBe(det.length);
		expect(m.spreadMs).toBe(0);
	});

	it('uses each scheduled event once and reports missing events', () => {
		const events = [1000, 1600, 2300, 2800, 3700];
		const det = [1010, 1011, 1612, 2309, 3710];
		const m = matchSchedule(det, events, {
			minOffsetMs: -100,
			maxOffsetMs: 100,
			windowStartMs: 900,
			windowEndMs: 4000
		});
		expect(m.matched).toBe(4);
		expect(m.expected).toBe(5);
		expect(m.offsetMs).toBeGreaterThan(9);
		expect(m.offsetMs).toBeLessThan(12);
	});

	it('returns nothing useful for empty input', () => {
		expect(matchSchedule([], [1, 2], { minOffsetMs: 0, maxOffsetMs: 10 }).matched).toBe(0);
		expect(Number.isNaN(matchSchedule([5], [], { minOffsetMs: 0, maxOffsetMs: 10 }).offsetMs)).toBe(true);
	});
});
