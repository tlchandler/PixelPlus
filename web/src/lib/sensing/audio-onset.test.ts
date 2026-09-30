import { describe, expect, it } from 'vitest';
import { ChirpDetector, type AudioOnset } from './audio-onset';
import { chirp, eventsMs, v1Click } from './schedule';
import { simulateAudio } from './testsignals';

const FS = 48000;

function detect(samples: Float32Array, tpl = chirp(FS), block = 128): AudioOnset[] {
	const d = new ChirpDetector(FS, tpl);
	const out: AudioOnset[] = [];
	for (let i = 0; i < samples.length; i += block) out.push(...d.push(samples.subarray(i, i + block)));
	out.push(...d.flush());
	return out;
}

/** Each expected time has exactly one detection within `tol` ms; returns the errors. */
function errors(found: AudioOnset[], expected: number[], tol = 1): number[] {
	const ms = found.map((o) => (o.sample / FS) * 1000);
	return expected.map((e) => {
		const near = ms.filter((m) => Math.abs(m - e) < tol);
		expect(near.length, `event at ${e.toFixed(3)} ms`).toBe(1);
		return near[0] - e;
	});
}

describe('chirp detector (matched filter)', () => {
	const offset = 1234.567; // not a whole sample
	const events = eventsMs(99)
		.slice(0, 24)
		.map((e) => e + offset);

	it('finds every chirp to within 0.1 ms in moderate noise with hum', () => {
		const s = simulateAudio({
			sampleRate: FS,
			durationMs: 20000,
			eventsMs: events,
			amplitude: 0.05,
			noise: 0.02,
			humAmp: 0.3
		});
		const found = detect(s);
		const err = errors(found, events);
		expect(Math.max(...err.map(Math.abs))).toBeLessThan(0.1);
		expect(found.length).toBe(events.length);
	});

	it('works with odd block sizes, inverted polarity, echoes and false clicks', () => {
		const s = simulateAudio({
			sampleRate: FS,
			durationMs: 20000,
			eventsMs: events,
			amplitude: 0.08,
			noise: 0.01,
			invert: true,
			echo: { ms: 35, gain: 0.6 },
			falseClicksMs: [3100, 7777, 12001]
		});
		const found = detect(s, chirp(FS), 333);
		const err = errors(found, events);
		expect(Math.max(...err.map(Math.abs))).toBeLessThan(0.1);
		// Echoes are skipped (refractory); false broadband ticks may appear but are few.
		expect(found.length - events.length).toBeLessThanOrEqual(3);
	});

	it('still finds most chirps at low SNR (buried in noise)', () => {
		// Chirp peak 0.02 vs noise σ 0.03 per sample: hard to see by eye; the matched filter
		// gains ~20 dB.
		const s = simulateAudio({
			sampleRate: FS,
			durationMs: 20000,
			eventsMs: events,
			amplitude: 0.02,
			noise: 0.03,
			seed: 5
		});
		const found = detect(s);
		const ms = found.map((o) => (o.sample / FS) * 1000);
		const hits = events.filter((e) => ms.some((m) => Math.abs(m - e) < 0.5));
		expect(hits.length).toBeGreaterThanOrEqual(20);
	});

	it('detects the classic v1 click with its own template', () => {
		const ev = [1500, 2500, 3500, 4500].map((e) => e + 0.37);
		const s = new Float32Array(FS * 5);
		const click = v1Click(FS);
		for (const e of ev) {
			const at = Math.round((e / 1000) * FS);
			s.set(click, at);
		}
		const found = detect(s, v1Click(FS));
		const err = errors(
			found,
			ev.map((e) => Math.round((e / 1000) * FS) / (FS / 1000))
		);
		expect(Math.max(...err.map(Math.abs))).toBeLessThan(0.1);
	});

	it('reports nothing in silence or plain noise', () => {
		expect(detect(new Float32Array(FS * 3))).toEqual([]);
		const s = simulateAudio({ sampleRate: FS, durationMs: 5000, eventsMs: [], noise: 0.05 });
		expect(detect(s).length).toBeLessThanOrEqual(1);
	});
});
