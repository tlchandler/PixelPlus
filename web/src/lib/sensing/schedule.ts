/**
 * Calibration pattern v2 — TypeScript mirror of `pixelplus_core::calpattern` (Rust).
 *
 * The leader answers `POST /player/calibration {on:true, pattern:'v2'}` with the plan; this
 * module rebuilds anything a (possibly older) daemon left out, from the seed alone. Keep the
 * two in step: `schedule.test.ts` and the Rust tests assert the same test vector.
 */

export const VERSION = 2;
/** One cycle of the pattern (and of its WAV); the engine may loop it. */
export const RUN_MS = 60_000;
/** Dark, silent lead-in before the first event of each cycle. */
export const LEAD_IN_MS = 2_000;
/** Quiet tail at the end of a cycle. */
export const TAIL_MS = 1_000;
export const GAP_MIN_MS = 450;
export const GAP_STEP_MS = 60;
export const GAP_STEPS = 8;
/** Events one measurement uses (about 21 s). */
export const MEASURE_EVENTS = 32;
export const FLASH_MIN_MS = 80;

export interface ChirpSpec {
	ms: number;
	f0Hz: number;
	f1Hz: number;
	rampMs: number;
}
export const CHIRP: ChirpSpec = { ms: 8, f0Hz: 2000, f1Hz: 4000, rampMs: 0.5 };
export const CHIRP_LEVEL = 0.85;

const LFSR_TAPS = 0x80200003;
const ZERO_SEED = 0x5eedca11;

/** What the phone needs to know about the running pattern. */
export interface CalPlan {
	/** 1 = classic click every second (ambiguous beyond ±500 ms), 2 = pseudo-random train. */
	v: number;
	seed: number;
	/** Event times from pattern position 0 (one cycle). */
	eventsMs: number[];
	flashMs: number;
	/** The pattern repeats after this (0 = never). */
	windowMs: number;
	leadInMs: number;
	chirp: ChirpSpec | null;
	/** Pattern position 0 is this long after the leader answered (if it said). */
	startsInMs?: number;
}

/** One Galois LFSR clock (unsigned 32-bit). */
export function lfsrStep(state: number): number {
	const lsb = state & 1;
	let s = state >>> 1;
	if (lsb) s = (s ^ LFSR_TAPS) >>> 0;
	return s >>> 0;
}

/** Event times (ms from position 0) of one cycle for `seed`. */
export function eventsMs(seed: number): number[] {
	let state = seed >>> 0 || ZERO_SEED;
	const end = RUN_MS - TAIL_MS;
	const out: number[] = [];
	let t = LEAD_IN_MS;
	while (t < end) {
		out.push(t);
		for (let i = 0; i < 8; i++) state = lfsrStep(state);
		t += GAP_MIN_MS + (state % GAP_STEPS) * GAP_STEP_MS;
	}
	return out;
}

export function flashLenMs(slotMs: number): number {
	const slot = Number.isFinite(slotMs) && slotMs > 0 ? slotMs : 0;
	return Math.max(FLASH_MIN_MS, 2 * slot);
}

/** The classic v1 pattern: flash + 4 ms 2 kHz click at every whole second. */
export function v1Plan(): CalPlan {
	const eventsMs: number[] = [];
	for (let t = 0; t < RUN_MS; t += 1000) eventsMs.push(t);
	return { v: 1, seed: 0, eventsMs, flashMs: 50, windowMs: 1000, leadInMs: 0, chirp: null };
}

/**
 * The plan from the leader's reply, filling gaps from the seed. A reply without a seed means
 * the leader only knows the classic pattern.
 */
export function planFromReply(reply: unknown): CalPlan {
	const r = (reply ?? {}) as Record<string, unknown>;
	const num = (v: unknown) => (typeof v === 'number' && Number.isFinite(v) ? v : undefined);
	const seed = num(r.seed);
	if (seed === undefined) {
		const p = v1Plan();
		const s = num(r.startsInMs);
		if (s !== undefined) p.startsInMs = s;
		return p;
	}
	const events =
		Array.isArray(r.eventsMs) && r.eventsMs.length > 0 && r.eventsMs.every((e) => typeof e === 'number')
			? (r.eventsMs as number[])
			: eventsMs(seed);
	const c = r.chirp as Partial<ChirpSpec> | undefined;
	const chirp =
		c && [c.ms, c.f0Hz, c.f1Hz].every((x) => typeof x === 'number')
			? { ms: c.ms!, f0Hz: c.f0Hz!, f1Hz: c.f1Hz!, rampMs: num(c.rampMs) ?? CHIRP.rampMs }
			: CHIRP;
	const plan: CalPlan = {
		v: num(r.v) ?? VERSION,
		seed,
		eventsMs: events,
		flashMs: num(r.flashMs) ?? FLASH_MIN_MS,
		windowMs: num(r.windowMs) ?? RUN_MS,
		leadInMs: num(r.leadInMs) ?? LEAD_IN_MS,
		chirp
	};
	const s = num(r.startsInMs);
	if (s !== undefined) plan.startsInMs = s;
	return plan;
}

/** The chirp (matched-filter template) at `sampleRate`; sample 0 is the event instant. */
export function chirp(sampleRate: number, spec: ChirpSpec = CHIRP, level = CHIRP_LEVEL): Float32Array {
	const len = Math.round((spec.ms / 1000) * sampleRate);
	const dur = spec.ms / 1000;
	const ramp = Math.max(1, (spec.rampMs / 1000) * sampleRate);
	const k = (spec.f1Hz - spec.f0Hz) / dur;
	const out = new Float32Array(len);
	for (let i = 0; i < len; i++) {
		const t = i / sampleRate;
		const phase = 2 * Math.PI * (spec.f0Hz * t + 0.5 * k * t * t);
		const fromEnd = len - 1 - i;
		let env = 1;
		if (i < ramp) env = 0.5 - 0.5 * Math.cos((Math.PI * i) / ramp);
		else if (fromEnd < ramp) env = 0.5 - 0.5 * Math.cos((Math.PI * fromEnd) / ramp);
		out[i] = Math.sin(phase) * env * level;
	}
	return out;
}

/** The v1 click: 4 ms 2 kHz tone with an exponential decay. */
export function v1Click(sampleRate: number): Float32Array {
	const len = Math.round(0.004 * sampleRate);
	const out = new Float32Array(len);
	for (let i = 0; i < len; i++) {
		const t = i / sampleRate;
		out[i] = Math.sin(2 * Math.PI * 2000 * t) * Math.exp(-i / (len / 4)) * 0.9;
	}
	return out;
}

/** The matched-filter template for a plan. */
export function template(plan: CalPlan, sampleRate: number): Float32Array {
	return plan.chirp ? chirp(sampleRate, plan.chirp) : v1Click(sampleRate);
}

/** The first `n` events after the lead-in span this long (ms), from position 0. */
export function measureSpanMs(plan: CalPlan, n = MEASURE_EVENTS): number {
	const last = plan.eventsMs[Math.min(n, plan.eventsMs.length) - 1] ?? 0;
	return last + plan.flashMs;
}
