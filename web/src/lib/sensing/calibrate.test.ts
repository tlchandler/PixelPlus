import { describe, expect, it } from 'vitest';
import { computeOutcome, confidenceOf, verified } from './calibrate';
import { ChirpDetector } from './audio-onset';
import { FlashMetric, detectFlashes } from './video-onset';
import { chirp, planFromReply, v1Plan, MEASURE_EVENTS } from './schedule';
import { simulateAudio, simulateVideo } from './testsignals';
import { gaussian, rng } from './dsp';

const FS = 48000;

/**
 * End to end on synthetic signals: the leader starts the pattern at phone time P0; lights are
 * delayed by the current setting, sound arrives `trueDelay` after leaving the controller.
 */
function session(o: {
	seed: number;
	trueDelay: number;
	current: number;
	fps?: number;
	noise?: number;
	radio?: boolean;
}) {
	const plan = planFromReply({ seed: o.seed, startsInMs: 500 });
	const P0 = 3000.25; // phone ms when pattern position 0 happened (unknown to the phone)
	const recStart = 2000;
	const events = plan.eventsMs.slice(0, MEASURE_EVENTS + 2);
	const recEnd = P0 + events[events.length - 1] + o.trueDelay + 800;
	const heard = events.map((e) => P0 + e + o.trueDelay - recStart);
	const seen = events.map((e) => P0 + e + o.current - recStart);
	const audio = simulateAudio({
		sampleRate: FS,
		durationMs: recEnd - recStart,
		eventsMs: heard,
		amplitude: 0.05,
		noise: o.noise ?? 0.02,
		humAmp: 0.1,
		seed: o.seed
	});
	const det = new ChirpDetector(FS, chirp(FS));
	const onsets = [...det.push(audio), ...det.flush()];
	const audioMs = onsets.map((x) => recStart + (x.sample / FS) * 1000);
	const frames = simulateVideo({
		fps: o.fps ?? 30,
		durationMs: recEnd - recStart,
		flashesMs: seen,
		flashMs: plan.flashMs,
		phaseMs: 7.1,
		noise: 2,
		seed: o.seed + 1
	});
	const m = new FlashMetric();
	const series = frames.map((f) => ({ t: recStart + f.t, ...m.push(f.luma) }));
	const videoMs = detectFlashes(series, { flashMs: plan.flashMs, readoutFraction: 0 }).map((v) => v.t);
	// The phone's coarse estimate of P0 (response time + startsInMs) is off by ~200 ms.
	return computeOutcome({
		plan,
		audioMs,
		videoMs,
		windowStartMs: recStart,
		windowEndMs: recEnd,
		pos0Ms: P0 + 180,
		currentDelayMs: o.current,
		radio: o.radio
	});
}

// Signal simulation is CPU-heavy; allow for a busy machine running the whole suite.
describe('calibration outcome (synthetic end-to-end)', { timeout: 60_000 }, () => {
	it('measures an FM-like delay within a couple of ms', () => {
		const out = session({ seed: 11, trueDelay: 312.4, current: 100, radio: true });
		expect(out.ok).toBe(true);
		expect(out.residualMs).toBeGreaterThan(212.4 - 2);
		expect(out.residualMs).toBeLessThan(212.4 + 2);
		expect(out.suggestedDelayMs).toBeCloseTo(100 + out.residualMs, 6);
		expect(out.matches).toBeGreaterThanOrEqual(30);
		expect(out.confidence).toBe('excellent');
		expect(out.uncertaintyMs).toBeGreaterThanOrEqual(5);
		expect(out.limitedRange).toBe(false);
	});

	it('finds a negative residual (lights too late) and works at 15 fps', () => {
		const out = session({ seed: 12, trueDelay: 40, current: 250, fps: 15 });
		expect(out.ok).toBe(true);
		expect(Math.abs(out.residualMs - -210)).toBeLessThan(3);
	});

	it('verifies a correct setting', () => {
		const out = session({ seed: 13, trueDelay: 180, current: 180 });
		expect(verified(out)).toBe(true);
		expect(Math.abs(out.residualMs)).toBeLessThan(2);
	});

	it('large radio delays need the radio option', () => {
		expect(session({ seed: 14, trueDelay: 1500, current: 0, radio: true }).residualMs).toBeCloseTo(1500, -1);
		// Without it the search range stops at 900 ms: the result is not trusted.
		expect(session({ seed: 14, trueDelay: 1500, current: 0 }).ok).toBe(false);
	});

	it('explains what went wrong', () => {
		const plan = planFromReply({ seed: 3 });
		const u = rng(1);
		const P0 = 1000;
		const ev = plan.eventsMs.slice(0, 32);
		const clean = ev.map((e) => P0 + e);
		const base = { plan, windowStartMs: 0, windowEndMs: P0 + ev[31] + 1000, pos0Ms: P0, currentDelayMs: 0 };
		expect(computeOutcome({ ...base, audioMs: clean, videoMs: [] }).problem).toBe('noFlashes');
		expect(computeOutcome({ ...base, audioMs: [], videoMs: clean }).problem).toBe('noClicks');
		expect(computeOutcome({ ...base, audioMs: clean, videoMs: clean.slice(0, 12) }).problem).toBe(
			'fewFlashes'
		);
		expect(
			computeOutcome({ ...base, audioMs: clean.filter((_, i) => i % 3 === 0), videoMs: clean }).problem
		).toBe('fewClicks');
		const noisy = clean.map((t) => t + 45 * gaussian(u));
		const r = computeOutcome({ ...base, audioMs: noisy, videoMs: clean });
		expect(['noisyAudio', 'fewClicks']).toContain(r.problem);
		expect(r.ok).toBe(false);
		expect(r.hint).toMatch(/radio|speaker/);
		const shaky = computeOutcome({
			...base,
			audioMs: clean,
			videoMs: clean.map((t) => t + 45 * gaussian(u))
		});
		expect(['shakyVideo', 'fewFlashes']).toContain(shaky.problem);
		// Perfect data: exact.
		const good = computeOutcome({ ...base, audioMs: clean.map((t) => t + 57), videoMs: clean });
		expect(good.ok).toBe(true);
		expect(good.residualMs).toBeCloseTo(57, 6);
		// A known phone bias is removed.
		const biased = computeOutcome({ ...base, audioMs: clean.map((t) => t + 57), videoMs: clean, biasMs: 12 });
		expect(biased.residualMs).toBeCloseTo(45, 6);
		expect(biased.uncertaintyMs).toBeLessThanOrEqual(good.uncertaintyMs);
	});

	it('clamps to the controller range and flags it', () => {
		const plan = planFromReply({ seed: 4 });
		const ev = plan.eventsMs.slice(0, 32).map((e) => 1000 + e);
		const out = computeOutcome({
			plan,
			audioMs: ev.map((t) => t + 800),
			videoMs: ev,
			windowStartMs: 0,
			windowEndMs: 30000,
			pos0Ms: 1000,
			currentDelayMs: 1500,
			radio: true
		});
		expect(out.ok).toBe(true);
		expect(out.clamped).toBe(true);
	});

	it('classic v1 pattern: limited to ±500 ms', () => {
		const plan = v1Plan();
		const ev = plan.eventsMs.slice(2, 30).map((e) => 400 + e);
		const out = computeOutcome({
			plan,
			audioMs: ev.map((t) => t + 130),
			videoMs: ev,
			windowStartMs: 0,
			windowEndMs: 31000,
			currentDelayMs: 0
		});
		expect(out.limitedRange).toBe(true);
		expect(out.ok).toBe(true);
		expect(out.residualMs).toBeCloseTo(130, 6);
		const wrapped = computeOutcome({
			plan,
			audioMs: ev.map((t) => t + 700),
			videoMs: ev,
			windowStartMs: 0,
			windowEndMs: 31000,
			currentDelayMs: 0
		});
		expect(wrapped.residualMs).toBeCloseTo(-300, 6);
	});

	it('confidence badges', () => {
		expect(confidenceOf(3)).toBe('excellent');
		expect(confidenceOf(8)).toBe('good');
		expect(confidenceOf(13)).toBe('retry');
	});
});
