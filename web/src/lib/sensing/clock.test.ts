import { describe, expect, it } from 'vitest';
import { LinearClock } from './clock';
import { gaussian, rng } from './dsp';

describe('linear clock', () => {
	it('fits slope and offset through jitter and outliers', () => {
		const u = rng(3);
		const c = new LinearClock(1000);
		for (let i = 0; i < 200; i++) {
			const x = i * 0.05;
			let y = 12345 + x * 1000.2 + gaussian(u) * 0.3;
			if (i % 37 === 0) y += 40; // a late timestamp
			c.add(x, y);
		}
		expect(c.slope).toBeCloseTo(1000.2, 1);
		expect(Math.abs(c.map(5) - (12345 + 5001))).toBeLessThan(0.2);
	});

	it('uses the nominal slope with few pairs and ignores NaN', () => {
		const c = new LinearClock(1000);
		c.add(1, 5000);
		c.add(NaN, 1);
		expect(c.count).toBe(1);
		expect(c.map(2)).toBe(6000);
	});

	it('keeps only recent pairs', () => {
		const c = new LinearClock(1, 10);
		for (let i = 0; i < 30; i++) c.add(i, i < 20 ? i : i + 100);
		expect(c.count).toBe(10);
		expect(c.map(25)).toBeCloseTo(125, 6);
	});
});
