/**
 * "Fine-tune for this phone": the clap test measures this phone's own sound-vs-picture timing
 * difference. The user claps 8–12 times about 1 m in front of the camera; each clap is heard
 * (a sharp broadband attack) and seen (the hands stop moving at contact).
 *
 *   bias = median(t_heard − 2.9 ms/m × distance − t_seen)
 *
 * It is stored per phone model and subtracted from later measurements.
 */
import { BandPass, median, robustSigma } from './dsp';

/** Speed of sound: ms per metre. */
export const MS_PER_METRE = 2.9;

/** Clap onsets (fractional sample indices) in a recording. */
export function detectClaps(samples: Float32Array, sampleRate: number, refractoryMs = 300): number[] {
	const n = samples.length;
	if (!n) return [];
	// Broadband attack: high-pass 1 kHz (band-pass up to 10 kHz), rectify, 1 ms smoothing.
	const bp = new BandPass(sampleRate, 1000, 10000);
	const env = new Float32Array(n);
	const alpha = 1 - Math.exp(-1 / (0.001 * sampleRate));
	let e = 0;
	for (let i = 0; i < n; i++) {
		const x = Math.abs(bp.process(samples[i]));
		e += alpha * (x - e);
		env[i] = e;
	}
	// Noise floor from a decimated copy.
	const dec: number[] = [];
	for (let i = 0; i < n; i += 64) dec.push(env[i]);
	const med = median(dec);
	const sig = Math.max(robustSigma(dec), med * 0.1, 1e-6);
	const thr = med + 12 * sig;
	const out: number[] = [];
	const refr = (refractoryMs / 1000) * sampleRate;
	const look = Math.round(0.01 * sampleRate);
	for (let i = 1; i < n; i++) {
		if (env[i] <= thr || env[i - 1] > thr) continue;
		// Local peak of the raw band-passed magnitude in the next 10 ms …
		let peak = env[i];
		for (let j = i; j < Math.min(n, i + look); j++) peak = Math.max(peak, env[j]);
		// … onset = where the envelope first reached 30 % of it (walking back from the crossing).
		const level = med + 0.3 * (peak - med);
		let k = i;
		while (k > 0 && env[k - 1] >= level) k--;
		while (k < n - 1 && env[k] < level) k++;
		const frac = k > 0 && env[k] !== env[k - 1] ? (level - env[k - 1]) / (env[k] - env[k - 1]) : 1;
		out.push(k - 1 + frac);
		i += refr;
	}
	return out;
}

export interface MotionSample {
	/** Frame capture time (start of exposure), ms. */
	t: number;
	/** Motion between the previous frame and this one. */
	motion: number;
}

/**
 * Contact instants of claps seen by the camera. Motion between frames k−1 and k covers the
 * interval between their exposure centres; the hands move until contact and then stop, so the
 * first interval with clearly less motion than the peak contains the contact, at the fraction
 * given by its motion relative to the peak.
 */
export function clapContacts(frames: MotionSample[], refractoryMs = 300): number[] {
	const n = frames.length;
	if (n < 5) return [];
	const d: number[] = [];
	for (let i = 1; i < n; i++) d.push(frames[i].t - frames[i - 1].t);
	const dt = median(d);
	const m = frames.map((f) => f.motion);
	const med = median(m);
	const thr = med + Math.max(6 * robustSigma(m), 0.01);
	const out: number[] = [];
	let until = -Infinity;
	for (let p = 1; p < n - 1; p++) {
		if (frames[p].t < until || m[p] < thr || m[p] < m[p - 1] || m[p] < m[p + 1]) continue;
		const peak = m[p];
		let c = p + 1;
		while (c < n && m[c] >= 0.9 * peak) c++;
		if (c >= n) break;
		const centre = (k: number) => frames[k].t + dt / 2;
		const f = Math.min(1, Math.max(0, (m[c] - med) / (peak - med)));
		out.push(centre(c - 1) + f * dt);
		until = frames[p].t + refractoryMs;
	}
	return out;
}

export interface BiasEstimate {
	biasMs: number;
	spreadMs: number;
	pairs: number;
}

/** Pair heard and seen claps (within ±150 ms) and estimate the phone's audio − video bias. */
export function estimateBias(heardMs: number[], seenMs: number[], distanceM = 1): BiasEstimate | null {
	const diffs: number[] = [];
	for (const a of heardMs) {
		const air = a - distanceM * MS_PER_METRE;
		let best: number | undefined;
		for (const v of seenMs)
			if (Math.abs(air - v) < 150 && (best === undefined || Math.abs(air - v) < Math.abs(air - best)))
				best = v;
		if (best !== undefined) diffs.push(air - best);
	}
	if (diffs.length < 5) return null;
	const biasMs = median(diffs);
	const spreadMs = robustSigma(diffs);
	if (!(spreadMs < 20) || Math.abs(biasMs) > 150) return null;
	return { biasMs, spreadMs, pairs: diffs.length };
}
