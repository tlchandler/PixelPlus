import { describe, expect, it } from 'vitest';
import { clapContacts, detectClaps, estimateBias, MS_PER_METRE, type MotionSample } from './clap';
import { gaussian, rng } from './dsp';

const FS = 48000;

function clapAudio(timesMs: number[], durationMs: number, seed = 1): Float32Array {
	const u = rng(seed);
	const n = Math.round((durationMs / 1000) * FS);
	const out = new Float32Array(n);
	for (let i = 0; i < n; i++) out[i] = 0.003 * gaussian(u);
	for (const t of timesMs) {
		const s = Math.round((t / 1000) * FS);
		for (let i = 0; i < 0.03 * FS && s + i < n; i++)
			out[s + i] += 0.5 * gaussian(u) * Math.exp(-i / (0.004 * FS));
	}
	return out;
}

/** Hands approach at constant speed for 150 ms, stop at contact; 30 fps, exposure = frame. */
function clapMotion(contactsMs: number[], durationMs: number, fps = 30, phase = 5): MotionSample[] {
	const dt = 1000 / fps;
	const u = rng(9);
	const moving = (a: number, b: number) => {
		let s = 0;
		for (const c of contactsMs) s += Math.max(0, Math.min(b, c) - Math.max(a, c - 150));
		return s / (b - a);
	};
	const out: MotionSample[] = [];
	for (let t = phase; t < durationMs; t += dt) {
		// Motion between the exposure centres of the previous frame and this one.
		out.push({ t, motion: 0.002 + 0.2 * moving(t - dt / 2, t + dt / 2) + 0.001 * Math.abs(gaussian(u)) });
	}
	return out;
}

// Signal simulation is CPU-heavy; allow for a busy machine running the whole suite.
describe('clap test', { timeout: 60_000 }, () => {
	const claps = [1000, 2150, 3320, 4400, 5610, 6700, 7890, 9010, 10100, 11300].map((t) => t + 0.4);

	it('detects clap onsets in the sound to within a millisecond', () => {
		const found = detectClaps(clapAudio(claps, 12000), FS).map((s) => (s / FS) * 1000);
		expect(found.length).toBe(claps.length);
		for (let i = 0; i < claps.length; i++) expect(Math.abs(found[i] - claps[i])).toBeLessThan(1);
	});

	it('finds contacts in the picture and estimates the bias', () => {
		const bias = 23; // audio pipeline 23 ms later than video (plus 1 m of air)
		const heard = claps.map((c) => c + bias + MS_PER_METRE);
		const seen = clapContacts(clapMotion(claps, 12000));
		expect(seen.length).toBe(claps.length);
		for (let i = 0; i < claps.length; i++) expect(Math.abs(seen[i] - claps[i])).toBeLessThan(4);
		const est = estimateBias(heard, seen)!;
		expect(est.pairs).toBe(claps.length);
		expect(Math.abs(est.biasMs - bias)).toBeLessThan(3);
	});

	it('refuses to guess with too few or inconsistent claps', () => {
		expect(estimateBias([1000, 2000], [1000, 2000])).toBeNull();
		const u = rng(2);
		const heard = claps;
		const seen = claps.map((c) => c + 60 * gaussian(u));
		expect(estimateBias(heard, seen)).toBeNull();
		expect(detectClaps(new Float32Array(0), FS)).toEqual([]);
		expect(clapContacts([])).toEqual([]);
	});
});
