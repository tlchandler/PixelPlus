/**
 * Camera flash detection with sub-frame timing.
 *
 * Per frame (160×120 luma, see `FlashMetric`):
 * - Pixels are linearised (gamma 2.2) so a half-exposed flash reads as half as bright.
 * - Baseline = per-pixel minimum of the previous 8 frames ("dark" even right after a flash,
 *   since flashes are ≤ 3 frames and ≥ 450 ms apart); follows auto-exposure drift.
 * - Signal = mean of the top 5 % positive differences from the baseline, so a few small lit
 *   windows count as much as a whole lit house. Also: the lit pixels' vertical centroid (for the
 *   rolling-shutter correction) and a motion measure (for the clap test).
 *
 * Over the recorded series (`detectFlashes`):
 * - Threshold = rolling median + k × MAD (min. absolute level).
 * - A flash that starts during frame j's exposure lights a fraction f of it:
 *   onset = t_j + Δt·(1 − f), f = (v_j − floor)/(full − floor), taking frame j−1 instead when
 *   it was already partly lit. Assumes exposure ≈ frame time (true at night, and what the
 *   exposure lock asks for).
 * - Rolling shutter: rows are read top to bottom over ≈ `readoutFraction`·Δt; the onset is
 *   corrected to the frame's centre row using the lit pixels' centroid.
 */
import { median, quantile } from './dsp';

export const LUMA_W = 160;
export const LUMA_H = 120;

/** Linear light (0..1023) for each 8-bit gamma-encoded value. */
const LIN = (() => {
	const t = new Uint16Array(256);
	for (let i = 0; i < 256; i++) t[i] = Math.round(Math.pow(i / 255, 2.2) * 1023);
	return t;
})();

/** Rec. 601 luma from RGBA pixels. */
export function lumaFromRGBA(rgba: Uint8ClampedArray | Uint8Array, out?: Uint8Array): Uint8Array {
	const n = rgba.length >> 2;
	const o = out && out.length === n ? out : new Uint8Array(n);
	for (let i = 0, j = 0; i < n; i++, j += 4)
		o[i] = (rgba[j] * 77 + rgba[j + 1] * 150 + rgba[j + 2] * 29) >> 8;
	return o;
}

export interface FrameMetric {
	/** Flash signal (0..1, linear light). */
	v: number;
	/** Vertical centroid (0 = top, 1 = bottom) of the brightest changes. */
	cy: number;
	/** Motion: mean of the top 20 % absolute changes from the previous frame (0..1). */
	motion: number;
	/** Mean brightness (0..1, linear), for the aiming meter. */
	mean: number;
}

export class FlashMetric {
	private hist: Uint16Array[] = [];
	private prev: Uint16Array | null = null;
	private lin: Uint16Array;
	private minBuf: Uint16Array;
	private counts = new Uint32Array(1024);
	private mcounts = new Uint32Array(1024);

	constructor(
		readonly width = LUMA_W,
		readonly height = LUMA_H,
		private historyLen = 8,
		private topFraction = 0.05
	) {
		this.lin = new Uint16Array(width * height);
		this.minBuf = new Uint16Array(width * height);
	}

	reset(): void {
		this.hist = [];
		this.prev = null;
	}

	push(luma: Uint8Array | Uint8ClampedArray): FrameMetric {
		const n = this.width * this.height;
		if (luma.length !== n) throw new Error(`expected ${n} luma values, got ${luma.length}`);
		const lin = this.lin;
		let sum = 0;
		for (let i = 0; i < n; i++) {
			lin[i] = LIN[luma[i]];
			sum += lin[i];
		}
		let v = 0;
		let cy = 0.5;
		if (this.hist.length > 0) {
			// Baseline: per-pixel minimum of the history.
			const min = this.minBuf;
			min.set(this.hist[0]);
			for (let h = 1; h < this.hist.length; h++) {
				const f = this.hist[h];
				for (let i = 0; i < n; i++) if (f[i] < min[i]) min[i] = f[i];
			}
			const counts = this.counts;
			counts.fill(0);
			for (let i = 0; i < n; i++) {
				const d = lin[i] - min[i];
				if (d > 0) counts[d]++;
			}
			const [thr, mean] = topMean(counts, Math.max(1, Math.round(n * this.topFraction)));
			v = mean / 1023;
			if (thr > 0) {
				let wy = 0;
				let w = 0;
				for (let y = 0; y < this.height; y++) {
					const row = y * this.width;
					for (let x = 0; x < this.width; x++) {
						const d = lin[row + x] - min[row + x];
						if (d >= thr) {
							wy += d * y;
							w += d;
						}
					}
				}
				if (w > 0) cy = wy / w / Math.max(1, this.height - 1);
			}
		}
		let motion = 0;
		if (this.prev) {
			const mc = this.mcounts;
			mc.fill(0);
			const p = this.prev;
			for (let i = 0; i < n; i++) mc[Math.abs(lin[i] - p[i])]++;
			motion = topMean(mc, Math.max(1, Math.round(n * 0.2)))[1] / 1023;
		}
		// History (copy, oldest dropped).
		const keep = this.hist.length >= this.historyLen ? this.hist.shift()! : new Uint16Array(n);
		keep.set(lin);
		this.hist.push(keep);
		this.prev = keep;
		return { v, cy, motion, mean: sum / n / 1023 };
	}
}

/** From a histogram of values 0..1023: [lowest value included, mean] of the top `count`. */
function topMean(counts: Uint32Array, count: number): [number, number] {
	let left = count;
	let sum = 0;
	let low = 0;
	for (let d = counts.length - 1; d > 0 && left > 0; d--) {
		const c = counts[d];
		if (!c) continue;
		const take = Math.min(c, left);
		sum += take * d;
		left -= take;
		low = d;
	}
	const got = count - left;
	return [low, got > 0 ? sum / count : 0];
}

export interface FrameSample {
	/** Frame capture time (start of exposure of the first row), phone clock ms. */
	t: number;
	v: number;
	cy?: number;
}

export interface VideoOnset {
	/** Estimated flash onset (phone clock ms). */
	t: number;
	/** Plateau level of this flash. */
	level: number;
	/** Index of the first frame above the threshold. */
	frame: number;
}

export interface DetectFlashOptions {
	/** Threshold: floor + k × robust sigma. */
	k?: number;
	/** Minimum absolute signal for a flash (linear light, 0..1). */
	minLevel?: number;
	/** Flash length (ms), to find the plateau. */
	flashMs?: number;
	/** Skip this long after an onset. */
	refractoryMs?: number;
	/** Rolling-shutter readout as a fraction of the frame time (0 = global shutter). */
	readoutFraction?: number;
}

/** Median frame interval (ms). */
export function frameInterval(frames: FrameSample[]): number {
	const d: number[] = [];
	for (let i = 1; i < frames.length; i++) {
		const x = frames[i].t - frames[i - 1].t;
		if (x > 0 && x < 250) d.push(x);
	}
	return d.length ? median(d) : 1000 / 30;
}

/** Find flash onsets in a series of frame metrics (see module docs). */
export function detectFlashes(frames: FrameSample[], opts: DetectFlashOptions = {}): VideoOnset[] {
	const n = frames.length;
	if (n < 10) return [];
	const k = opts.k ?? 8;
	const minLevel = opts.minLevel ?? 0.004;
	const flashMs = opts.flashMs ?? 80;
	const dt = frameInterval(frames);
	const refractory = opts.refractoryMs ?? Math.max(300, flashMs + 2 * dt);
	const readout = (opts.readoutFraction ?? 0.75) * dt;
	const win = Math.max(20, Math.round(2000 / dt));
	const vals = frames.map((f) => f.v);

	// Rolling noise floor from the preceding ~2 s (flash frames are a small minority).
	const floor = new Float64Array(n);
	const sigma = new Float64Array(n);
	for (let i = 0; i < n; i++) {
		const a = Math.max(0, i - win);
		const b = Math.max(a + Math.min(win, n), i);
		const w = vals.slice(a, Math.min(n, Math.max(b, a + 10)));
		const med = median(w);
		const dev = w.map((x) => Math.abs(x - med));
		floor[i] = med;
		sigma[i] = Math.max(median(dev) * 1.4826, 1e-4);
	}

	// Candidates: first frame above threshold after a quiet one.
	const plateauFrames = Math.max(2, Math.ceil(flashMs / dt) + 1);
	const cands: { j: number; peak: number }[] = [];
	let until = -Infinity;
	for (let j = 1; j < n; j++) {
		if (frames[j].t < until) continue;
		const thr = floor[j] + Math.max(k * sigma[j], minLevel);
		if (vals[j] <= thr) continue;
		let peak = vals[j];
		for (let q = j + 1; q < Math.min(n, j + plateauFrames); q++) peak = Math.max(peak, vals[q]);
		cands.push({ j, peak });
		until = frames[j].t + refractory;
	}
	if (!cands.length) return [];
	// Typical flash level; ignore much weaker blips (a car's headlights, a bird).
	const typical = quantile(
		cands.map((c) => c.peak),
		0.75
	);
	const fullFrameLikely = flashMs >= 1.8 * dt;
	const out: VideoOnset[] = [];
	for (const { j, peak } of cands) {
		if (peak < 0.25 * typical) continue;
		const fl = floor[j];
		const full = Math.max(fullFrameLikely ? peak : Math.max(peak, typical), fl + 1e-6);
		const frac = (i: number) => Math.min(1, Math.max(0, (vals[i] - fl) / (full - fl)));
		const tiny = Math.max((2.5 * sigma[j]) / (full - fl), 0.02);
		let i = j;
		if (j > 0 && frac(j - 1) > tiny) i = j - 1;
		const f = frac(i);
		let t = frames[i].t + dt * (1 - f);
		// Rolling shutter: lit rows below the top were exposed later.
		const cy = frames[j].cy ?? 0.5;
		t += readout * (cy - 0.5);
		out.push({ t, level: peak - fl, frame: j });
	}
	return out;
}

/** Live "seeing flashes" meter: flashes in the recent series and their contrast. */
export function flashMeter(
	frames: FrameSample[],
	opts: DetectFlashOptions = {}
): { count: number; contrast: number } {
	const found = detectFlashes(frames, opts);
	if (!found.length) return { count: 0, contrast: 0 };
	const floor = median(frames.map((f) => f.v));
	return { count: found.length, contrast: median(found.map((f) => f.level)) / Math.max(floor, 1e-3) };
}
