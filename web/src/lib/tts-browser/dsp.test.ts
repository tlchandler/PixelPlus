import { describe, expect, it } from 'vitest';
import {
	applyBiquads,
	crossfadeConcat,
	estimatePitch,
	limit,
	measureLufs,
	normalizeLoudness,
	parseFfmpegEq,
	pitchShift,
	quietCut,
	timeStretch
} from './dsp';

const SR = 24000;
const sine = (f: number, s: number, a = 0.5, sr = SR) =>
	Float32Array.from({ length: Math.round(s * sr) }, (_, i) => a * Math.sin((2 * Math.PI * f * i) / sr));
const rms = (x: Float32Array) => Math.sqrt(x.reduce((s, v) => s + v * v, 0) / x.length);
const medianF0 = (x: Float32Array) => {
	const f = [...estimatePitch(x, SR)].filter((v) => v > 0).sort((a, b) => a - b);
	return f[Math.floor(f.length / 2)];
};

describe('loudness', () => {
	it('measures BS.1770 loudness (997 Hz full-scale sine = -3.01 LUFS)', () => {
		expect(measureLufs(sine(997, 3, 1, 48000), 48000)).toBeCloseTo(-3.01, 1);
		expect(measureLufs(sine(997, 3, 1), SR)).toBeCloseTo(-3.01, 0);
		expect(measureLufs(sine(997, 3, 0.1), SR)).toBeCloseTo(-23.01, 0);
		expect(measureLufs(new Float32Array(SR), SR)).toBe(-Infinity);
	});

	it('normalizes to the target', () => {
		const y = normalizeLoudness(sine(440, 2, 0.05), SR, -16);
		expect(measureLufs(y, SR)).toBeCloseTo(-16, 1);
	});

	it('limits peaks without touching quiet audio', () => {
		const x = sine(200, 0.5, 1.5);
		const y = limit(x, SR, 0.84);
		expect(Math.max(...y.map(Math.abs))).toBeLessThanOrEqual(0.8401);
		const q = sine(200, 0.5, 0.3);
		expect(limit(q, SR, 0.84)).toEqual(q);
	});
});

describe('eq', () => {
	it('parses ffmpeg equalizer chains and boosts the band', () => {
		const f = parseFfmpegEq('equalizer=f=3200:t=q:w=1.2:g=6,highpass=f=75,acompressor=threshold=-21dB', SR);
		expect(f).toHaveLength(2);
		const at3k =
			rms(applyBiquads(sine(3200, 1), [f[0]]).subarray(SR / 2)) / rms(sine(3200, 1).subarray(SR / 2));
		expect(20 * Math.log10(at3k)).toBeCloseTo(6, 0);
		const at200 =
			rms(applyBiquads(sine(200, 1), [f[0]]).subarray(SR / 2)) / rms(sine(200, 1).subarray(SR / 2));
		expect(Math.abs(20 * Math.log10(at200))).toBeLessThan(0.5);
		const hp = rms(applyBiquads(sine(30, 1), [f[1]]).subarray(SR / 2)) / rms(sine(30, 1).subarray(SR / 2));
		expect(hp).toBeLessThan(0.3);
		expect(parseFfmpegEq(undefined, SR)).toEqual([]);
	});
});

describe('time & pitch', () => {
	it('tracks pitch', () => {
		expect(medianF0(sine(150, 1))).toBeCloseTo(150, -1);
		expect(medianF0(sine(220, 1))).toBeCloseTo(220, -1);
	});

	it('time-stretches without changing pitch', () => {
		const x = sine(200, 1);
		const y = timeStretch(x, 1.25);
		expect(y.length).toBe(Math.round(x.length * 1.25));
		expect(medianF0(y)).toBeCloseTo(200, -1);
		expect(rms(y.subarray(2000, -2000))).toBeCloseTo(rms(x), 1);
	});

	it('shifts pitch without changing duration', () => {
		const x = sine(200, 1);
		const y = pitchShift(x, 3);
		expect(y.length).toBe(x.length);
		expect(medianF0(y) / 200).toBeCloseTo(2 ** (3 / 12), 1);
		const z = pitchShift(x, 2, 1.1);
		expect(z.length).toBe(Math.round(x.length * 1.1));
		expect(medianF0(z) / 200).toBeCloseTo(2 ** (2 / 12), 1);
	});

	it('finds a quiet cut point and crossfades joins', () => {
		const x = new Float32Array(SR);
		x.set(sine(200, 0.4), 0);
		x.set(sine(200, 0.4), Math.round(0.5 * SR));
		const cut = quietCut(x, SR, 0.47);
		expect(cut / SR).toBeGreaterThan(0.4);
		expect(cut / SR).toBeLessThan(0.5);
		const a = new Float32Array(1000).fill(1);
		const joined = crossfadeConcat([a, a], SR, 0.01);
		expect(joined.length).toBe(2000 - 240);
		expect(Math.min(...joined)).toBeGreaterThan(0.99);
	});
});
