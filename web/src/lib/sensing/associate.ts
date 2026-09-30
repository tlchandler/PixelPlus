/**
 * Association of detected events with the known calibration schedule.
 *
 * All pairwise differences `x_i − s_j` (detection − scheduled time, restricted to a search
 * range) go into a 2 ms histogram; a ±tolerance box filter turns it into "how many detections
 * would match at this offset". The best offset absorbs the unknown phone ↔ leader clock
 * difference and every constant latency. Matches within ±tolerance are then refined: the offset
 * becomes their median residual (twice) and finally the mean of the inliers, each scheduled
 * event is used at most once, and the spread is the robust standard deviation of the residuals.
 */
import { median, robustSigma } from './dsp';

export interface MatchOptions {
	/** Search range for the offset (detection time − schedule time), ms. */
	minOffsetMs: number;
	maxOffsetMs: number;
	/** A detection matches an event within this distance. */
	tolMs?: number;
	binMs?: number;
	/** The schedule repeats with this period (0 = no repetition). */
	periodMs?: number;
	/** Recording window (phone clock), to count the events that should have been seen. */
	windowStartMs?: number;
	windowEndMs?: number;
}

export interface MatchResult {
	offsetMs: number;
	spreadMs: number;
	matched: number;
	/** Scheduled events inside the recording window at this offset. */
	expected: number;
	detections: number;
	/** Per matched event: [detection time, schedule time, residual]. */
	pairs: [number, number, number][];
	/** Best and runner-up box-filtered counts (away from the best offset). */
	peak: number;
	runnerUp: number;
}

/** Schedule times (possibly repeated) that can pair with detections in [lo, hi]. */
function expand(events: number[], period: number, lo: number, hi: number): number[] {
	if (!period) return events.filter((e) => e >= lo && e <= hi);
	const out: number[] = [];
	const k0 = Math.floor((lo - events[events.length - 1]) / period) - 1;
	const k1 = Math.ceil((hi - events[0]) / period) + 1;
	for (let k = k0; k <= k1; k++)
		for (const e of events) {
			const s = e + k * period;
			if (s >= lo && s <= hi) out.push(s);
		}
	return out.sort((a, b) => a - b);
}

function nearest(sorted: number[], x: number): number {
	let lo = 0;
	let hi = sorted.length - 1;
	while (hi - lo > 1) {
		const mid = (lo + hi) >> 1;
		if (sorted[mid] <= x) lo = mid;
		else hi = mid;
	}
	return Math.abs(sorted[lo] - x) <= Math.abs(sorted[hi] - x) ? sorted[lo] : sorted[hi];
}

export function matchSchedule(detections: number[], events: number[], o: MatchOptions): MatchResult {
	const tol = o.tolMs ?? 25;
	const bin = o.binMs ?? 2;
	const empty: MatchResult = {
		offsetMs: NaN,
		spreadMs: NaN,
		matched: 0,
		expected: 0,
		detections: detections.length,
		pairs: [],
		peak: 0,
		runnerUp: 0
	};
	if (!detections.length || !events.length || !(o.maxOffsetMs > o.minOffsetMs)) return empty;
	const det = [...detections].sort((a, b) => a - b);
	const period = o.periodMs ?? 0;
	const sched = expand(
		events,
		period,
		det[0] - o.maxOffsetMs - tol,
		det[det.length - 1] - o.minOffsetMs + tol
	);
	if (!sched.length) return empty;

	const nb = Math.ceil((o.maxOffsetMs - o.minOffsetMs) / bin) + 1;
	const hist = new Float64Array(nb);
	// For each detection, each schedule entry in range: vote (once per detection per bin).
	let j0 = 0;
	for (const x of det) {
		while (j0 < sched.length && x - sched[j0] > o.maxOffsetMs) j0++;
		for (let j = j0; j < sched.length; j++) {
			const d = x - sched[j];
			if (d < o.minOffsetMs) break;
			hist[Math.floor((d - o.minOffsetMs) / bin)] += 1;
		}
	}
	// Box filter ±tol.
	const half = Math.round(tol / bin);
	const box = new Float64Array(nb);
	let acc = 0;
	for (let i = 0; i < Math.min(nb, half); i++) acc += hist[i];
	for (let i = 0; i < nb; i++) {
		if (i + half < nb) acc += hist[i + half];
		if (i - half - 1 >= 0) acc -= hist[i - half - 1];
		box[i] = acc;
	}
	// Best offset: highest box count; ties → the histogram's own peak nearby.
	let best = 0;
	for (let i = 1; i < nb; i++) if (box[i] > box[best]) best = i;
	// Centre of the plateau of equal maxima.
	let end = best;
	while (end + 1 < nb && box[end + 1] === box[best]) end++;
	let offset = o.minOffsetMs + ((best + end) / 2) * bin + bin / 2;
	const peak = box[best];
	let runnerUp = 0;
	const excl = Math.round((2 * tol) / bin);
	for (let i = 0; i < nb; i++)
		if (Math.abs(i - (best + end) / 2) > excl) runnerUp = Math.max(runnerUp, box[i]);

	// Refine: median of residuals of unique matches, twice.
	let pairs: [number, number, number][] = [];
	for (let iter = 0; iter < 3; iter++) {
		const bySched = new Map<number, [number, number, number]>();
		for (const x of det) {
			const s = nearest(sched, x - offset);
			const r = x - (s + offset);
			if (Math.abs(r) >= tol) continue;
			const prev = bySched.get(s);
			if (!prev || Math.abs(r) < Math.abs(prev[2])) bySched.set(s, [x, s, r]);
		}
		pairs = [...bySched.values()].sort((a, b) => a[1] - b[1]);
		if (!pairs.length) break;
		if (iter < 2) offset += median(pairs.map((p) => p[2]));
	}
	let residuals = pairs.map((p) => p[2]);
	// Final step: mean of the inliers (more efficient than the median for Gaussian jitter).
	if (residuals.length >= 5) {
		const sig = Math.max(robustSigma(residuals), 0.5);
		const inl = residuals.filter((r) => Math.abs(r) <= 2.5 * sig);
		if (inl.length >= 3) {
			const shift = inl.reduce((a, b) => a + b, 0) / inl.length;
			offset += shift;
			pairs = pairs.map(([x, s, r]) => [x, s, r - shift]);
			residuals = pairs.map((p) => p[2]);
		}
	}
	let expected = pairs.length;
	if (o.windowStartMs !== undefined && o.windowEndMs !== undefined) {
		const all = expand(events, period, o.windowStartMs - offset, o.windowEndMs - offset);
		expected = Math.max(pairs.length, all.length);
	}
	return {
		offsetMs: offset,
		spreadMs: residuals.length >= 3 ? robustSigma(residuals) : NaN,
		matched: pairs.length,
		expected,
		detections: det.length,
		pairs,
		peak,
		runnerUp
	};
}
