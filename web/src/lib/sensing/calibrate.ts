/**
 * From detected clicks and flashes to "how much later the sound arrives than the lights".
 *
 * Both streams are timed on the phone's own clock and correlated separately against the
 * known schedule (`associate.ts`), so no phone ↔ controller clock sync is needed:
 *
 *   residual = (offset_audio − offset_video) − phoneBias
 *   newDelay = currentDelay + residual        (clamped to −500…2000 by the controller)
 *
 * Timestamps passed in must already include the phone's own latencies (microphone input
 * latency, camera capture time); `phoneBias` is what remains for this phone model (clap test).
 */
import { matchSchedule, type MatchResult } from './associate';
import type { CalPlan } from './schedule';

export const OUTPUT_DELAY_MIN = -500;
export const OUTPUT_DELAY_MAX = 2000;

export type Confidence = 'excellent' | 'good' | 'retry';
export type Problem =
	'noFlashes' | 'fewFlashes' | 'noClicks' | 'fewClicks' | 'noisyAudio' | 'shakyVideo' | 'ambiguous';

export interface MeasureInput {
	plan: CalPlan;
	/** Chirp starts heard (phone clock ms). */
	audioMs: number[];
	/** Flash onsets seen (phone clock ms). */
	videoMs: number[];
	windowStartMs: number;
	windowEndMs: number;
	/** Phone time of pattern position 0, if known (± ~1.5 s is fine). */
	pos0Ms?: number;
	/** The controller's current sound delay (lights are already delayed by this much). */
	currentDelayMs: number;
	/** This phone's own audio − video timing bias (clap test), ms. */
	biasMs?: number;
	/** The sound comes through a radio / stream (allows much larger delays). */
	radio?: boolean;
}

export interface MeasureOutcome {
	ok: boolean;
	/** Add this to the sound delay. */
	residualMs: number;
	/** Sound delay that matches (current + residual), before clamping. */
	suggestedDelayMs: number;
	/** The suggestion is outside what the controller allows. */
	clamped: boolean;
	/** ± for display (95 %), including the phone's own uncertainty. */
	uncertaintyMs: number;
	/** Event-to-event spread (robust σ) of both streams combined. */
	spreadMs: number;
	/** Matched events (the smaller of the two streams). */
	matches: number;
	confidence: Confidence;
	problem?: Problem;
	/** Friendly explanation of `problem`. */
	hint?: string;
	/** The classic pattern: only ±500 ms can be told apart. */
	limitedRange: boolean;
	audio: MatchResult;
	video: MatchResult;
}

export const HINTS: Record<Problem, string> = {
	noFlashes: "I couldn't see the lights flash. Point the camera at the lights and hold the phone still.",
	fewFlashes: 'I only saw some of the flashes. Get more lights in view and hold the phone steady.',
	noClicks: "I couldn't hear the clicks. Turn the sound up, or move closer to the speaker or radio.",
	fewClicks: 'I only heard some of the clicks. Turn the sound up, or move closer to the speaker or radio.',
	noisyAudio: 'Too noisy — move closer to the radio or speaker, or turn it up.',
	shakyVideo: 'The picture was too shaky — lean the phone on something and try again.',
	ambiguous: "The measurement wasn't clear enough. Please try again."
};

/** Largest acceptable event-to-event spread per stream. */
export const MAX_SPREAD_MS = 15;

function need(expected: number): number {
	return Math.max(8, Math.min(20, Math.ceil(0.6 * expected)));
}

export function confidenceOf(spreadMs: number): Confidence {
	return spreadMs < 5 ? 'excellent' : spreadMs < 12 ? 'good' : 'retry';
}

export function computeOutcome(inp: MeasureInput): MeasureOutcome {
	const { plan } = inp;
	const period = plan.windowMs || 0;
	const limitedRange = plan.v < 2 || (period > 0 && period < 6000);
	const bias = inp.biasMs ?? 0;
	const win = { windowStartMs: inp.windowStartMs, windowEndMs: inp.windowEndMs };

	// Video: the lights follow the pattern delayed by the current sound delay.
	let vMin: number;
	let vMax: number;
	if (inp.pos0Ms !== undefined && (!period || period > 4000)) {
		const c = inp.pos0Ms + inp.currentDelayMs;
		vMin = c - 1500;
		vMax = c + 1500;
	} else if (period) {
		const c = inp.pos0Ms ?? inp.windowStartMs;
		vMin = c - period / 2;
		vMax = c + period / 2;
	} else {
		vMin = inp.windowStartMs - (plan.eventsMs[plan.eventsMs.length - 1] ?? 0) - 3000;
		vMax = inp.windowEndMs;
	}
	const video = matchSchedule(inp.videoMs, plan.eventsMs, {
		minOffsetMs: vMin,
		maxOffsetMs: vMax,
		periodMs: period,
		...win
	});

	// Audio, relative to the video: residual range from where the sound can plausibly be.
	let rMin: number;
	let rMax: number;
	if (limitedRange) {
		rMin = -period / 2 + 1;
		rMax = period / 2;
	} else {
		const trueMin = -150;
		const trueMax = inp.radio ? 2300 : 900;
		rMin = trueMin - inp.currentDelayMs + bias;
		rMax = trueMax - inp.currentDelayMs + bias;
	}
	const base = Number.isFinite(video.offsetMs) ? video.offsetMs : (vMin + vMax) / 2;
	const audio = matchSchedule(inp.audioMs, plan.eventsMs, {
		minOffsetMs: base + rMin,
		maxOffsetMs: base + rMax,
		periodMs: period,
		...win
	});

	let problem: Problem | undefined;
	if (video.detections < 3 || video.matched < 3) problem = 'noFlashes';
	else if (audio.detections < 3 || audio.matched < 3) problem = 'noClicks';
	else if (video.matched < need(video.expected)) problem = 'fewFlashes';
	else if (audio.matched < need(audio.expected)) problem = 'fewClicks';
	else if (!(video.spreadMs <= MAX_SPREAD_MS)) problem = 'shakyVideo';
	else if (!(audio.spreadMs <= MAX_SPREAD_MS)) problem = 'noisyAudio';
	else if (video.peak < 1.5 * video.runnerUp || audio.peak < 1.5 * audio.runnerUp) problem = 'ambiguous';

	const residual = audio.offsetMs - video.offsetMs - bias;
	const sa = Number.isFinite(audio.spreadMs) ? audio.spreadMs : 99;
	const sv = Number.isFinite(video.spreadMs) ? video.spreadMs : 99;
	const spread = Math.hypot(sa, sv);
	const sem = Math.hypot(
		(2 * sa) / Math.sqrt(Math.max(1, audio.matched)),
		(2 * sv) / Math.sqrt(Math.max(1, video.matched))
	);
	const systematic = inp.biasMs !== undefined ? 2 : 5;
	const suggested = inp.currentDelayMs + residual;
	const clampedDelay = Math.min(OUTPUT_DELAY_MAX, Math.max(OUTPUT_DELAY_MIN, Math.round(suggested)));
	const ok = problem === undefined && Number.isFinite(residual);
	return {
		ok,
		residualMs: ok ? residual : NaN,
		suggestedDelayMs: ok ? suggested : NaN,
		clamped: ok && clampedDelay !== Math.round(suggested),
		uncertaintyMs: Math.max(1, Math.ceil(Math.hypot(sem, systematic))),
		spreadMs: spread,
		matches: Math.min(audio.matched, video.matched),
		confidence: ok ? confidenceOf(spread) : 'retry',
		problem,
		hint: problem ? HINTS[problem] : undefined,
		limitedRange,
		audio,
		video
	};
}

/** A verification run passes when the remaining error is this small. */
export const VERIFY_LIMIT_MS = 8;

export function verified(o: MeasureOutcome): boolean {
	return o.ok && Math.abs(o.residualMs) < VERIFY_LIMIT_MS;
}
