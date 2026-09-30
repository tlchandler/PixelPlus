import { describe, expect, it } from 'vitest';
import {
	CHIRP,
	GAP_MIN_MS,
	GAP_STEP_MS,
	GAP_STEPS,
	LEAD_IN_MS,
	MEASURE_EVENTS,
	RUN_MS,
	TAIL_MS,
	chirp,
	eventsMs,
	flashLenMs,
	lfsrStep,
	measureSpanMs,
	planFromReply,
	template,
	v1Plan
} from './schedule';

describe('calibration schedule (mirror of pixelplus_core::calpattern)', () => {
	it('matches the Rust test vector', () => {
		// Same numbers as calpattern.rs `test_vector_seed_1`.
		expect(eventsMs(1).slice(0, 8)).toEqual([2000, 2570, 3200, 4010, 4760, 5630, 6500, 6950]);
	});

	it('is deterministic, seed-dependent, and handles seed 0 and high bits', () => {
		expect(eventsMs(42)).toEqual(eventsMs(42));
		expect(eventsMs(42)).not.toEqual(eventsMs(43));
		expect(eventsMs(0)).toEqual(eventsMs(0x5eedca11));
		expect(eventsMs(0xdeadbeef).length).toBeGreaterThan(64);
		expect(lfsrStep(0x80000000)).toBe(0x40000000);
		expect(lfsrStep(1)).toBe(0x80200003);
	});

	it('has the documented shape', () => {
		for (const seed of [1, 2, 3, 0xdeadbeef, 12345]) {
			const ev = eventsMs(seed);
			expect(ev[0]).toBe(LEAD_IN_MS);
			expect(ev[ev.length - 1]).toBeLessThan(RUN_MS - TAIL_MS);
			for (let i = 1; i < ev.length; i++) {
				const gap = ev[i] - ev[i - 1];
				expect(gap).toBeGreaterThanOrEqual(GAP_MIN_MS);
				expect(gap).toBeLessThan(GAP_MIN_MS + GAP_STEPS * GAP_STEP_MS);
				expect((gap - GAP_MIN_MS) % GAP_STEP_MS).toBe(0);
			}
			const span = ev[MEASURE_EVENTS - 1] - ev[0];
			expect(span).toBeGreaterThan(14_000);
			expect(span).toBeLessThan(27_000);
		}
	});

	it('flash length and chirp', () => {
		expect(flashLenMs(0)).toBe(80);
		expect(flashLenMs(50)).toBe(100);
		expect(flashLenMs(NaN)).toBe(80);
		const c = chirp(48000);
		expect(c.length).toBe(384);
		expect(Math.abs(c[0])).toBeLessThan(1e-6);
		expect(Math.max(...Array.from(c, Math.abs))).toBeLessThanOrEqual(0.85 + 1e-6);
	});

	it('builds plans from replies, old and new', () => {
		const v2 = planFromReply({ seed: 7, startsInMs: 400 });
		expect(v2.v).toBe(2);
		expect(v2.eventsMs).toEqual(eventsMs(7));
		expect(v2.windowMs).toBe(RUN_MS);
		expect(v2.chirp).toEqual(CHIRP);
		expect(v2.startsInMs).toBe(400);
		const full = planFromReply({
			seed: 7,
			v: 2,
			eventsMs: [2000, 2600],
			flashMs: 100,
			windowMs: 5000,
			chirp: CHIRP
		});
		expect(full.eventsMs).toEqual([2000, 2600]);
		expect(full.flashMs).toBe(100);
		const v1 = planFromReply({ ok: true, on: true });
		expect(v1.v).toBe(1);
		expect(v1.windowMs).toBe(1000);
		expect(v1).toMatchObject(v1Plan());
		expect(template(v1, 48000).length).toBe(192);
		expect(measureSpanMs(v2)).toBe(eventsMs(7)[31] + 80);
	});
});
