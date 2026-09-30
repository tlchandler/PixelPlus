import { describe, expect, it } from 'vitest';
import { BandPass, fft, gaussian, mad, median, parabolicOffset, quantile, rng, robustSigma } from './dsp';

describe('dsp', () => {
	it('fft matches a direct DFT and inverts', () => {
		const n = 64;
		const u = rng(1);
		const re = Float64Array.from({ length: n }, () => u() - 0.5);
		const im = new Float64Array(n);
		const r2 = Float64Array.from(re);
		const i2 = new Float64Array(n);
		fft(r2, i2);
		for (const k of [0, 1, 5, 31, 63]) {
			let sr = 0;
			let si = 0;
			for (let t = 0; t < n; t++) {
				sr += re[t] * Math.cos((-2 * Math.PI * k * t) / n);
				si += re[t] * Math.sin((-2 * Math.PI * k * t) / n);
			}
			expect(r2[k]).toBeCloseTo(sr, 9);
			expect(i2[k]).toBeCloseTo(si, 9);
		}
		fft(r2, i2, true);
		for (let t = 0; t < n; t++) expect(r2[t]).toBeCloseTo(re[t], 9);
		expect(im.length).toBe(n);
		expect(() => fft(new Float64Array(3), new Float64Array(3))).toThrow();
	});

	it('band-pass keeps 3 kHz and rejects 200 Hz and 12 kHz', () => {
		const fs = 48000;
		const level = (f: number) => {
			const bp = new BandPass(fs, 1500, 5000);
			let e = 0;
			for (let i = 0; i < fs / 4; i++) {
				const y = bp.process(Math.sin((2 * Math.PI * f * i) / fs));
				if (i > fs / 8) e = Math.max(e, Math.abs(y));
			}
			return e;
		};
		expect(level(3000)).toBeGreaterThan(0.8);
		expect(level(200)).toBeLessThan(0.02);
		expect(level(12000)).toBeLessThan(0.1);
	});

	it('robust statistics', () => {
		expect(median([3, 1, 2])).toBe(2);
		expect(median([4, 1, 2, 3])).toBe(2.5);
		expect(Number.isNaN(median([]))).toBe(true);
		expect(mad([1, 2, 3, 4, 100])).toBe(1);
		expect(quantile([0, 10], 0.5)).toBe(5);
		const u = rng(7);
		const xs = Array.from({ length: 4000 }, () => gaussian(u) * 3);
		expect(robustSigma(xs)).toBeGreaterThan(2.8);
		expect(robustSigma(xs)).toBeLessThan(3.2);
	});

	it('parabolic interpolation finds the vertex', () => {
		const f = (x: number) => -((x - 0.3) ** 2);
		expect(parabolicOffset(f(-1), f(0), f(1))).toBeCloseTo(0.3, 9);
		expect(parabolicOffset(1, 1, 1)).toBe(0);
	});
});
