// Offline decoder for a camera mapping run (F6/F7, WS4). Pure TypeScript, no
// DOM: runs in a Worker on the phone (decode.worker.ts) and in vitest against the
// synthetic yard simulator (simulate.ts).
//
// Pipeline (see docs/ARCHITECTURE.md §12.5):
//  1. activity map (temporal std) → candidate camera pixels
//  2. clock recovery: mean of the most active pixels correlated with the plan's
//     lit-fraction template → run start on the phone's clock
//  3. per pass / slot: mean of the frames fully inside each slot (edge frames are
//     skipped; timestamps, not frame counts, define the windows)
//  4. per-pixel references from the preamble (on / off level, off noise); weak
//     or flickering pixels are masked
//  5. phase A: top-w of the soft values → constant-weight codeword (one swap of
//     the least certain pair corrected); phase B: differential Gray bits
//  6. pass alignment (small shake compensated, big moves drop the pass),
//     majority vote across passes
//  7. connected components per (target, pixel) label; reflections and
//     ambiguous duplicates rejected; spatial outliers along a string rejected
import {
	codeBitsFor,
	codebook,
	frameFor,
	grayInverse,
	litFraction,
	PHASE_A,
	PHASE_B,
	schedule,
	type Plan
} from './mapcode';

export interface Frame {
	/** Capture time on the phone's clock (ms). */
	t: number;
	/** Luminance, row-major, width × height. */
	data: Uint8Array;
}
export interface Recording {
	width: number;
	height: number;
	frames: Frame[];
	/** Phone-clock time the pattern was expected to start (from the start request). */
	startHintMs?: number;
	/** Search ± this much around the hint (default 2500 ms). */
	hintWindowMs?: number;
}
export interface DecodeOptions {
	/** Minimum on−off contrast (0..255 levels). */
	minContrast?: number;
	/** Largest camera shake compensated between passes (px). */
	maxShiftPx?: number;
	/** Most candidate pixels examined (brightest-changing first). */
	maxCandidates?: number;
	/** Subtract a local (15×15) background mean from every frame, which removes
	 *  diffuse glow such as lit snow or a wall (default true). */
	background?: boolean;
}
export interface DecodedLight {
	k: number;
	idx: number;
	/** Image position, normalised 0..1. */
	x: number;
	y: number;
	conf: number;
	/** Camera pixels in the blob. */
	area: number;
}
/** A patch where a target was identified but its pixels could not be told apart. */
export interface DecodedRegion {
	k: number;
	x: number;
	y: number;
	area: number;
	conf: number;
}
export interface DecodeStats {
	frames: number;
	fps: number;
	offsetMs: number;
	clockScore: number;
	candidates: number;
	passesUsed: number;
	passShifts: [number, number][];
	/** Largest pass shift (px) against the first pass. */
	maxMotionPx: number;
	/** Per pass: share of bits agreeing with the combined decode. */
	passAgreement?: number[];
	/** Passes dropped for disagreeing (something moved through the scene). */
	droppedPasses?: number[];
	movedPasses: number[];
	/** Share of lit reference pixels at 255 (bloom). */
	saturatedPct: number;
	rejectedReflections: number;
	rejectedOutliers: number;
	ms: number;
}
export interface DecodeResult {
	ok: boolean;
	error?: string;
	lights: DecodedLight[];
	/** Targets seen where their pixels are too dense to separate. */
	regions?: DecodedRegion[];
	/** Per target: `[idx, x, y, conf]` (the MappingRun `detected` shape; idx −1 = a region). */
	detected: { k: number; pixels: [number, number, number, number][] }[];
	stats: DecodeStats;
	width: number;
	height: number;
	/** Contrast of decoded camera pixels (for the heat-map overlay), width × height. */
	heat?: Float32Array;
}

const LABEL_NONE = -1;
const IDX_BITS = 13; // label = k << 13 | idx (idx < 8191)
/** Target known (phase A) but pixel index not resolvable (dense strings). */
const IDX_UNKNOWN = (1 << IDX_BITS) - 1;

function median(a: number[]): number {
	if (!a.length) return 0;
	const s = [...a].sort((x, y) => x - y);
	return s[s.length >> 1];
}

function fail(error: string, stats: DecodeStats, rec: Recording): DecodeResult {
	return { ok: false, error, lights: [], detected: [], stats, width: rec.width, height: rec.height };
}

export function decode(rec: Recording, plan: Plan, opts: DecodeOptions = {}): DecodeResult {
	const t0 = typeof performance !== 'undefined' ? performance.now() : Date.now();
	const W = rec.width;
	const H = rec.height;
	const N = W * H;
	const frames = [...rec.frames].sort((a, b) => a.t - b.t);
	const F = frames.length;
	const sch = schedule(plan);
	const stats: DecodeStats = {
		frames: F,
		fps: 0,
		offsetMs: 0,
		clockScore: 0,
		candidates: 0,
		passesUsed: 0,
		passShifts: [],
		maxMotionPx: 0,
		movedPasses: [],
		saturatedPct: 0,
		rejectedReflections: 0,
		rejectedOutliers: 0,
		ms: 0
	};
	const done = (r: DecodeResult) => {
		r.stats.ms = Math.round((typeof performance !== 'undefined' ? performance.now() : Date.now()) - t0);
		return r;
	};
	if (F < 10) return done(fail('The recording is too short.', stats, rec));
	const intervals: number[] = [];
	for (let i = 1; i < F; i++) intervals.push(frames[i].t - frames[i - 1].t);
	const frameMs = Math.max(1, median(intervals));
	stats.fps = Math.round(10000 / frameMs) / 10;
	if (frameMs > plan.bitMs * 0.6)
		return done(
			fail(`The camera delivered only ${stats.fps} frames per second — too few for this pattern.`, stats, rec)
		);

	// 1. Activity (temporal std) per camera pixel.
	const sum = new Float64Array(N);
	const sq = new Float64Array(N);
	for (const f of frames) {
		const d = f.data;
		for (let i = 0; i < N; i++) {
			const v = d[i];
			sum[i] += v;
			sq[i] += v * v;
		}
	}
	const std = new Float32Array(N);
	for (let i = 0; i < N; i++) {
		const m = sum[i] / F;
		std[i] = Math.sqrt(Math.max(0, sq[i] / F - m * m));
	}
	// Sensor noise from consecutive-frame differences (blinking changes only a
	// few frame pairs, so the median pixel measures noise even when reflections
	// make the whole picture blink).
	const noiseList: number[] = [];
	for (let i = 0; i < N; i += 11) {
		let d2 = 0;
		for (let f = 1; f < F; f++) {
			const d = frames[f].data[i] - frames[f - 1].data[i];
			d2 += d * d;
		}
		noiseList.push(Math.sqrt(d2 / (2 * (F - 1))));
	}
	const noise = median(noiseList);
	const minContrast = opts.minContrast ?? 6;
	const floor = Math.max(minContrast * 0.4, 1.5 * noise + 1);
	let cand: number[] = [];
	for (let i = 0; i < N; i++) if (std[i] >= floor) cand.push(i);
	const maxC = opts.maxCandidates ?? 40000;
	if (cand.length > maxC)
		cand = cand
			.sort((a, b) => std[b] - std[a])
			.slice(0, maxC)
			.sort((a, b) => a - b);
	stats.candidates = cand.length;
	if (cand.length < 1)
		return done(fail('No blinking lights were seen. Is the display in the picture?', stats, rec));

	// 2. Clock recovery.
	const byStd = [...cand].sort((a, b) => std[b] - std[a]);
	const clockPx = byStd.slice(0, Math.max(12, Math.min(2000, Math.floor(cand.length * 0.25))));
	const g = new Float64Array(F);
	for (let i = 0; i < F; i++) {
		const d = frames[i].data;
		let s = 0;
		for (const p of clockPx) s += d[p];
		g[i] = s / clockPx.length;
	}
	const RES = 5;
	const tplLen = Math.ceil((sch.totalMs + 4 * plan.bitMs) / RES);
	const tpl = new Float32Array(tplLen);
	for (let j = 0; j < tplLen; j++) tpl[j] = litFraction(plan, frameFor(plan, plan.startPosMs + j * RES));
	const tplAt = (tau: number) => {
		if (tau < 0) return 0;
		const j = Math.floor(tau / RES);
		return j < tplLen ? tpl[j] : 0;
	};
	const score = (o: number) => {
		let sx = 0,
			sy = 0,
			sxx = 0,
			syy = 0,
			sxy = 0;
		for (let i = 0; i < F; i++) {
			const x = g[i];
			const y = tplAt(frames[i].t - o);
			sx += x;
			sy += y;
			sxx += x * x;
			syy += y * y;
			sxy += x * y;
		}
		const cov = sxy - (sx * sy) / F;
		const vx = sxx - (sx * sx) / F;
		const vy = syy - (sy * sy) / F;
		return vx > 1e-9 && vy > 1e-9 ? cov / Math.sqrt(vx * vy) : -1;
	};
	let lo: number, hi: number;
	if (rec.startHintMs != null) {
		const w = rec.hintWindowMs ?? 2500;
		lo = rec.startHintMs - w;
		hi = rec.startHintMs + w;
	} else {
		lo = frames[0].t - sch.leadInMs - sch.preambleMs;
		hi = frames[F - 1].t - sch.passMs;
	}
	let best = -2,
		bestO = lo;
	for (let o = lo; o <= hi; o += 20) {
		const s = score(o);
		if (s > best) {
			best = s;
			bestO = o;
		}
	}
	for (let o = bestO - 24; o <= bestO + 24; o += 2) {
		const s = score(o);
		if (s > best) {
			best = s;
			bestO = o;
		}
	}
	stats.clockScore = Math.round(best * 1000) / 1000;
	stats.offsetMs = bestO;
	if (best < 0.45)
		return done(
			fail(
				"Couldn't find the light pattern in the video. Keep the whole display in view and try again.",
				stats,
				rec
			)
		);

	// 3. Slot means per candidate, per pass (motion-compensated per pass).
	const codeBits = plan.phases & PHASE_A ? codeBitsFor(plan.targets.length) : 0;
	const pixelBits = plan.phases & PHASE_B ? plan.pixelBits : 0;
	const slotsPerPass = 12 + codeBits + 2 * pixelBits;
	const guard = Math.min(0.4 * plan.bitMs, Math.max(20, 0.3 * frameMs + 10));
	const C = cand.length;
	const passStartOf = (p: number) => sch.leadInMs + p * sch.passMs;

	// 3a. Motion: each pass's preamble "all on" image against the first pass's,
	// on the brightest reference points. All targets are lit then, so the match
	// is unambiguous even for regular grids of lights.
	/** Mean "on − off" image of preamble block `b` (0 or 1) of pass `p`. */
	const preambleDiff = (p: number, b: number): Float32Array | null => {
		const on = new Float32Array(N);
		const off = new Float32Array(N);
		let nOn = 0,
			nOff = 0;
		for (const f of frames) {
			const tau = f.t - bestO - passStartOf(p) - b * 6 * plan.bitMs;
			if (tau < 0 || tau >= 6 * plan.bitMs) continue;
			const s = Math.floor(tau / plan.bitMs);
			const within = tau - s * plan.bitMs;
			if (within < guard || within > plan.bitMs - guard) continue;
			const tgt = s < 3 ? on : off;
			const d = f.data;
			for (let i = 0; i < N; i++) tgt[i] += d[i];
			if (s < 3) nOn++;
			else nOff++;
		}
		if (!nOn || !nOff) return null;
		if (p === 0 && b === 0) {
			// Bloom: share of lit candidate pixels at full scale in the first "on" block.
			let lit = 0,
				sat = 0;
			for (const q of cand) {
				const o = on[q] / nOn;
				if (o - off[q] / nOff > minContrast) {
					lit++;
					if (o >= 250) sat++;
				}
			}
			stats.saturatedPct = lit ? Math.round((sat / lit) * 1000) / 10 : 0;
		}
		for (let i = 0; i < N; i++) on[i] = on[i] / nOn - off[i] / nOff;
		return on;
	};
	const maxTrack = opts.maxShiftPx ?? 12;
	const ref = preambleDiff(0, 0);
	let anchors: number[] = [];
	if (ref) {
		anchors = cand
			.filter((p) => ref[p] > Math.max(minContrast, 4 * noise))
			.sort((a, b) => ref[b] - ref[a])
			.slice(0, 400);
	}
	const shiftOf = (img: Float32Array | null): [number, number] | null => {
		if (!img || !ref || anchors.length < 4) return null;
		let bs = -Infinity,
			bx = 0,
			by = 0;
		for (let dy = -maxTrack; dy <= maxTrack; dy++)
			for (let dx = -maxTrack; dx <= maxTrack; dx++) {
				let sc = 0;
				for (const a of anchors) {
					const x = (a % W) + dx;
					const y = ((a / W) | 0) + dy;
					if (x >= 0 && y >= 0 && x < W && y < H) sc += Math.min(ref[a], img[y * W + x]);
				}
				sc -= (Math.abs(dx) + Math.abs(dy)) * 1e-3;
				if (sc > bs) {
					bs = sc;
					bx = dx;
					by = dy;
				}
			}
		return [bx, by];
	};
	// Shift of each preamble block: [pass][block].
	const blockShift: ([number, number] | null)[][] = [];
	for (let p = 0; p < plan.passes; p++)
		blockShift.push([p === 0 ? [0, 0] : shiftOf(preambleDiff(p, 0)), shiftOf(preambleDiff(p, 1))]);
	const far = (a: [number, number] | null, b: [number, number] | null) =>
		!!a && !!b && Math.hypot(a[0] - b[0], a[1] - b[1]) > 2.5;
	const passShift: [number, number][] = blockShift.map((b) => b[1] ?? b[0] ?? [0, 0]);
	stats.passShifts = passShift;
	stats.maxMotionPx = Math.round(Math.max(0, ...passShift.map(([x, y]) => Math.hypot(x, y))) * 10) / 10;
	for (let p = 0; p < plan.passes; p++) {
		// Moved inside this preamble, or between its end and the next preamble.
		const inPreamble = far(blockShift[p][0], blockShift[p][1]);
		const after = far(blockShift[p][1] ?? blockShift[p][0], blockShift[p + 1]?.[0] ?? null);
		if (inPreamble || after) stats.movedPasses.push(p);
	}

	// Local background: integral image of the current frame, 15×15 box mean.
	const bg = opts.background ?? true;
	const BR = 7;
	const ii = new Float64Array((W + 1) * (H + 1));
	const integral = (d: Uint8Array) => {
		for (let y = 0; y < H; y++) {
			let row = 0;
			const o = (y + 1) * (W + 1);
			const po = y * (W + 1);
			for (let x = 0; x < W; x++) {
				row += d[y * W + x];
				ii[o + x + 1] = ii[po + x + 1] + row;
			}
		}
	};
	const boxMean = (x: number, y: number) => {
		const x0 = Math.max(0, x - BR),
			y0 = Math.max(0, y - BR),
			x1 = Math.min(W, x + BR + 1),
			y1 = Math.min(H, y + BR + 1);
		const sum = ii[y1 * (W + 1) + x1] - ii[y0 * (W + 1) + x1] - ii[y1 * (W + 1) + x0] + ii[y0 * (W + 1) + x0];
		return sum / ((x1 - x0) * (y1 - y0));
	};

	const passes: number[] = [];
	// means[i][s * C + c] for passes[i]
	const means: Float32Array[] = [];
	for (let p = 0; p < plan.passes; p++) {
		if (stats.movedPasses.includes(p)) continue;
		const passStart = passStartOf(p);
		const [sx, sy] = passShift[p];
		const src = new Int32Array(C);
		const srcX = new Int16Array(C);
		const srcY = new Int16Array(C);
		for (let c = 0; c < C; c++) {
			const q = cand[c];
			const x = Math.min(W - 1, Math.max(0, (q % W) + sx));
			const y = Math.min(H - 1, Math.max(0, ((q / W) | 0) + sy));
			src[c] = y * W + x;
			srcX[c] = x;
			srcY[c] = y;
		}
		const acc = new Float32Array(slotsPerPass * C);
		const cnt = new Uint16Array(slotsPerPass);
		let lastFrameTau = -Infinity;
		for (const f of frames) {
			const tau = f.t - bestO - passStart;
			lastFrameTau = Math.max(lastFrameTau, tau);
			if (tau < 0 || tau >= slotsPerPass * plan.bitMs) continue;
			const s = Math.floor(tau / plan.bitMs);
			const within = tau - s * plan.bitMs;
			if (within < guard || within > plan.bitMs - guard) continue;
			cnt[s]++;
			const d = f.data;
			const base = s * C;
			if (bg) {
				integral(d);
				for (let c = 0; c < C; c++) acc[base + c] += d[src[c]] - boxMean(srcX[c], srcY[c]);
			} else for (let c = 0; c < C; c++) acc[base + c] += d[src[c]];
		}
		// A pass only counts when the recording covers it (preamble and codes).
		let missing = 0;
		for (let s = 0; s < slotsPerPass; s++) if (!cnt[s]) missing++;
		if (lastFrameTau < slotsPerPass * plan.bitMs - guard || missing > Math.max(1, slotsPerPass * 0.15))
			continue;
		for (let s = 0; s < slotsPerPass; s++) {
			const base = s * C;
			if (!cnt[s]) {
				acc.fill(NaN, base, base + C);
				continue;
			}
			const inv = 1 / cnt[s];
			for (let c = 0; c < C; c++) acc[base + c] *= inv;
		}
		passes.push(p);
		means.push(acc);
	}
	if (!passes.length)
		return done(
			fail(
				stats.movedPasses.length
					? 'The phone moved during the pattern. Lean it on something steady and try again.'
					: 'The recording stopped before one full pass of the pattern.',
				stats,
				rec
			)
		);

	// 4. Soft-combine passes and decode each candidate.
	const refOf = (m: Float32Array, c: number): [number, number, number] => {
		let on = 0,
			nOn = 0,
			off = 0,
			nOff = 0;
		for (let s = 0; s < 12; s++) {
			const x = m[s * C + c];
			if (Number.isNaN(x)) continue;
			if (s % 6 < 3) {
				on += x;
				nOn++;
			} else {
				off += x;
				nOff++;
			}
		}
		if (!nOn || !nOff) return [NaN, NaN, 0];
		off /= nOff;
		let so = 0;
		for (let s = 0; s < 12; s++) {
			const x = m[s * C + c];
			if (s % 6 >= 3 && !Number.isNaN(x)) so += (x - off) ** 2;
		}
		return [on / nOn, off, so];
	};
	const book = codeBits ? codebook(codeBits).slice(0, plan.targets.length) : [];
	const wordToK = new Map<number, number>();
	book.forEach((w, k) => wordToK.set(w, k));
	const weight = codeBits / 2;
	const v = new Float64Array(Math.max(1, codeBits));
	const order = new Array<number>(Math.max(1, codeBits));
	const x = new Float64Array(slotsPerPass);
	const xn = new Uint8Array(slotsPerPass);

	/** Hard decision on one slot vector: `[word, gray bits]` or null. */
	const hard = (vals: Float64Array, on: number, off: number): [number, number] | null => {
		const k0 = on - off;
		let word = 0;
		if (codeBits) {
			for (let i = 0; i < codeBits; i++) {
				v[i] = Number.isNaN(vals[12 + i]) ? 0.5 : (vals[12 + i] - off) / k0;
				order[i] = i;
			}
			order.sort((a, b) => v[b] - v[a]);
			for (let r = 0; r < weight; r++) word |= 1 << (codeBits - 1 - order[r]);
		}
		let bits = 0;
		for (let j = 0; j < pixelBits; j++) {
			const a = vals[12 + codeBits + 2 * j];
			const b = vals[12 + codeBits + 2 * j + 1];
			if (Number.isNaN(a) || Number.isNaN(b)) return null;
			bits = (bits << 1) | (a > b ? 1 : 0);
		}
		return [word, bits];
	};

	interface Pixel {
		q: number;
		c: number;
		label: number;
		conf: number;
		contrast: number;
		word: number;
		bits: number;
		unknown: boolean;
	}
	const decodeWith = (keep: number[]): Pixel[] => {
		const out: Pixel[] = [];
		for (let c = 0; c < C; c++) {
			x.fill(0);
			xn.fill(0);
			let so = 0,
				nso = 0,
				passesHere = 0;
			for (const pi of keep) {
				const m = means[pi];
				const [pon, , pso] = refOf(m, c);
				if (Number.isNaN(pon)) continue;
				passesHere++;
				so += pso;
				nso += 6;
				for (let s = 0; s < slotsPerPass; s++) {
					const val = m[s * C + c];
					if (Number.isNaN(val)) continue;
					x[s] += val;
					xn[s]++;
				}
			}
			if (!passesHere) continue;
			for (let s = 0; s < slotsPerPass; s++) x[s] = xn[s] ? x[s] / xn[s] : NaN;
			let on = 0,
				nOn = 0,
				off = 0,
				nOff = 0;
			for (let s = 0; s < 12; s++) {
				if (Number.isNaN(x[s])) continue;
				if (s % 6 < 3) {
					on += x[s];
					nOn++;
				} else {
					off += x[s];
					nOff++;
				}
			}
			if (!nOn || !nOff) continue;
			on /= nOn;
			off /= nOff;
			// Noise of one combined slot value.
			const sigma = Math.sqrt(so / Math.max(1, nso)) / Math.sqrt(passesHere);
			const k0 = on - off;
			if (k0 < Math.max(minContrast, 4 * sigma)) continue;
			let k = 0;
			let confA = 1;
			let word = 0;
			if (codeBits) {
				for (let i = 0; i < codeBits; i++) {
					const val = x[12 + i];
					v[i] = Number.isNaN(val) ? 0.5 : (val - off) / k0;
					order[i] = i;
				}
				order.sort((a, b) => v[b] - v[a]);
				let top = 0,
					bot = 0;
				for (let r = 0; r < codeBits; r++) {
					if (r < weight) {
						word |= 1 << (codeBits - 1 - order[r]);
						top += v[order[r]];
					} else bot += v[order[r]];
				}
				const margin = v[order[weight - 1]] - v[order[weight]];
				if (top / weight - bot / weight < 0.45 || margin < 0.1) continue;
				let kk = wordToK.get(word);
				confA = Math.min(1, margin * 1.5);
				if (kk === undefined) {
					// Swap the least certain pair (the most likely single error) —
					// only when that pair really is ambiguous.
					if (margin > 0.3) continue;
					const alt =
						word ^ (1 << (codeBits - 1 - order[weight - 1])) ^ (1 << (codeBits - 1 - order[weight]));
					kk = wordToK.get(alt);
					if (kk === undefined) continue;
					word = alt;
					confA = margin;
				}
				k = kk;
			} else if (plan.targets.length !== 1) continue;
			let idx = 0;
			let confB = 1;
			let bits = 0;
			if (pixelBits) {
				let ok = true;
				for (let j = 0; j < pixelBits; j++) {
					const a = x[12 + codeBits + 2 * j];
					const b = x[12 + codeBits + 2 * j + 1];
					if (Number.isNaN(a) || Number.isNaN(b)) {
						ok = false;
						break;
					}
					const d = (a - b) / k0;
					if (Math.abs(d) < 0.25 || Math.abs(a - b) < 2.5 * sigma) {
						ok = false;
						break;
					}
					confB = Math.min(confB, Math.abs(d));
					bits = (bits << 1) | (d > 0 ? 1 : 0);
				}
				if (!ok) {
					// The output is known but its lights are too close together here to
					// tell apart (a dense string far away): still a sighting of the target.
					if (codeBits && confA >= 0.3)
						out.push({
							q: cand[c],
							c,
							label: (k << IDX_BITS) | IDX_UNKNOWN,
							conf: confA * 0.5 * (passesHere / Math.max(1, keep.length)),
							contrast: k0,
							word,
							bits: 0,
							unknown: true
						});
					continue;
				}
				idx = grayInverse(bits);
				if (idx >= plan.targets[k].maxPixels || idx >= IDX_UNKNOWN) continue;
			}
			out.push({
				q: cand[c],
				c,
				label: (k << IDX_BITS) | idx,
				conf: Math.min(1, confA, confB) * (passesHere / Math.max(1, keep.length)),
				contrast: k0,
				word,
				bits,
				unknown: false
			});
		}
		return out;
	};

	// 5. Decode, then check each pass agrees with the combined answer; a pass
	// disturbed by something transient (headlights, a person walking by, a
	// nudge in the last pass) is dropped and the rest decoded again.
	let keep = passes.map((_, i) => i);
	let pixels = decodeWith(keep);
	if (keep.length >= 3 && pixels.length >= 8) {
		const agreement = keep.map((pi) => {
			let same = 0,
				all = 0;
			const m = means[pi];
			const vals = new Float64Array(slotsPerPass);
			for (const px of pixels) {
				if (px.unknown) continue;
				for (let s = 0; s < slotsPerPass; s++) vals[s] = m[s * C + px.c];
				const [on, off] = refOf(m, px.c);
				if (Number.isNaN(on) || on - off <= 0) continue;
				const h = hard(vals, on, off);
				if (!h) continue;
				const nb = codeBits + pixelBits;
				const diff = popcount32(h[0] ^ px.word) / 2 + popcount32(h[1] ^ px.bits);
				same += nb - Math.min(nb, diff);
				all += nb;
			}
			return all ? same / all : 1;
		});
		stats.passAgreement = agreement.map((a) => Math.round(a * 1000) / 1000);
		const bestA = Math.max(...agreement);
		const bad = keep.filter((_, i) => agreement[i] < 0.8 && agreement[i] < bestA - 0.12);
		if (bad.length && keep.length - bad.length >= 2) {
			stats.droppedPasses = bad.map((i) => passes[i]);
			keep = keep.filter((i) => !bad.includes(i));
			pixels = decodeWith(keep);
		}
	}
	stats.passesUsed = keep.length;

	const voted = new Int32Array(N).fill(LABEL_NONE);
	const votedConf = new Float32Array(N);
	const votedContrast = new Float32Array(N);
	for (const px of pixels) {
		voted[px.q] = px.label;
		votedConf[px.q] = px.conf;
		votedContrast[px.q] = px.contrast;
	}

	// 7. Blobs per label.
	interface Blob {
		label: number;
		w: number;
		sx: number;
		sy: number;
		area: number;
		peak: number;
		conf: number;
		sumContrast: number;
	}
	const seen = new Uint8Array(N);
	const blobs = new Map<number, Blob[]>();
	const stack: number[] = [];
	let contrastList: number[] = [];
	for (let q = 0; q < N; q++) {
		if (voted[q] === LABEL_NONE || seen[q]) continue;
		const label = voted[q];
		const b: Blob = { label, w: 0, sx: 0, sy: 0, area: 0, peak: 0, conf: 0, sumContrast: 0 };
		stack.length = 0;
		stack.push(q);
		seen[q] = 1;
		while (stack.length) {
			const p = stack.pop()!;
			const x = p % W;
			const y = (p / W) | 0;
			const wgt = votedContrast[p] * (0.25 + votedConf[p]);
			b.w += wgt;
			b.sx += (x + 0.5) * wgt;
			b.sy += (y + 0.5) * wgt;
			b.area++;
			b.peak = Math.max(b.peak, votedContrast[p]);
			b.conf += votedConf[p];
			b.sumContrast += votedContrast[p];
			for (let dy = -1; dy <= 1; dy++)
				for (let dx = -1; dx <= 1; dx++) {
					const nx = x + dx;
					const ny = y + dy;
					if (nx < 0 || ny < 0 || nx >= W || ny >= H) continue;
					const n = ny * W + nx;
					if (!seen[n] && voted[n] === label) {
						seen[n] = 1;
						stack.push(n);
					}
				}
		}
		const list = blobs.get(label);
		if (list) list.push(b);
		else blobs.set(label, [b]);
		contrastList.push(b.peak);
	}
	contrastList = contrastList.sort((a, b) => a - b);
	const p90 = contrastList.length ? contrastList[Math.floor(contrastList.length * 0.9)] : 0;
	const bigArea = Math.max(40, N * 0.004);
	const lights: DecodedLight[] = [];
	const regions: DecodedRegion[] = [];
	for (const [label, list] of blobs) {
		list.sort((a, b) => b.w - a.w);
		if ((label & IDX_UNKNOWN) === IDX_UNKNOWN) {
			// Target-only sightings: every solid patch counts (one prop can cover
			// several), reflections excepted.
			for (const b of list) {
				if (b.area < 2) continue;
				if (b.area > bigArea && b.sumContrast / b.area < 0.35 * p90) {
					stats.rejectedReflections++;
					continue;
				}
				regions.push({
					k: label >> IDX_BITS,
					x: b.sx / b.w / W,
					y: b.sy / b.w / H,
					area: b.area,
					conf: Math.round((b.conf / b.area) * 1000) / 1000
				});
			}
			continue;
		}
		const b = list[0];
		if (list.length > 1 && list[1].w > 0.8 * b.w) continue; // two equally strong places: ambiguous
		if (b.area > bigArea && b.sumContrast / b.area < 0.35 * p90) {
			stats.rejectedReflections++;
			continue;
		}
		lights.push({
			k: label >> IDX_BITS,
			idx: label & ((1 << IDX_BITS) - 1),
			x: b.sx / b.w / W,
			y: b.sy / b.w / H,
			conf: Math.round((b.conf / b.area) * 1000) / 1000,
			area: b.area
		});
	}

	// Spatial outliers along a string: neighbours in index should be neighbours in space.
	const byTarget = new Map<number, DecodedLight[]>();
	for (const l of lights) {
		const a = byTarget.get(l.k);
		if (a) a.push(l);
		else byTarget.set(l.k, [l]);
	}
	const out: DecodedLight[] = [];
	for (const [, arr] of byTarget) {
		arr.sort((a, b) => a.idx - b.idx);
		if (arr.length < 4) {
			out.push(...arr);
			continue;
		}
		const gaps: number[] = [];
		for (let i = 1; i < arr.length; i++) {
			const di = arr[i].idx - arr[i - 1].idx;
			if (di > 0 && di <= 3)
				gaps.push(Math.hypot((arr[i].x - arr[i - 1].x) * W, (arr[i].y - arr[i - 1].y) * H) / di);
		}
		const step = Math.max(1.5, median(gaps));
		// Ghosts: a bit error gives a second label on top of a real light of
		// the same string; keep the stronger one.
		const ghost = new Set<DecodedLight>();
		for (let i = 0; i < arr.length; i++)
			for (let j = i + 1; j < arr.length; j++) {
				const a = arr[i],
					b = arr[j];
				if (Math.hypot((a.x - b.x) * W, (a.y - b.y) * H) < 1.5)
					ghost.add(a.conf * a.area >= b.conf * b.area ? b : a);
			}
		for (let i = 0; i < arr.length; i++) {
			const l = arr[i];
			if (ghost.has(l)) {
				stats.rejectedOutliers++;
				continue;
			}
			let near = 0,
				far = 0;
			for (let j = Math.max(0, i - 4); j <= Math.min(arr.length - 1, i + 4); j++) {
				if (j === i) continue;
				const di = Math.abs(arr[j].idx - l.idx);
				if (di > 4) continue;
				const d = Math.hypot((arr[j].x - l.x) * W, (arr[j].y - l.y) * H);
				if (d > step * di * 1.6 + 2) far++;
				else if (di <= 2) near++;
			}
			// Far from its string neighbours, or alone and weak: likely a bit error.
			if ((far >= 2 && near === 0) || (near === 0 && l.conf < 0.35)) {
				stats.rejectedOutliers++;
				continue;
			}
			out.push(l);
		}
	}
	out.sort((a, b) => a.k - b.k || a.idx - b.idx);
	const detected: DecodeResult['detected'] = [];
	for (const l of out) {
		let d = detected[detected.length - 1];
		if (!d || d.k !== l.k) {
			d = { k: l.k, pixels: [] };
			detected.push(d);
		}
		d.pixels.push([l.idx, round4(l.x), round4(l.y), l.conf]);
	}
	// Regions go to the stored result as pixel index −1.
	regions.sort((a, b) => a.k - b.k || b.area - a.area);
	for (const r of regions) {
		let d = detected.find((x) => x.k === r.k);
		if (!d) {
			d = { k: r.k, pixels: [] };
			detected.push(d);
		}
		if (d.pixels.filter((p) => p[0] < 0).length < 24) d.pixels.push([-1, round4(r.x), round4(r.y), r.conf]);
	}
	detected.sort((a, b) => a.k - b.k);
	const heat = new Float32Array(N);
	for (let q = 0; q < N; q++) if (voted[q] !== LABEL_NONE) heat[q] = votedContrast[q];
	return done({ ok: true, lights: out, regions, detected, stats, width: W, height: H, heat });
}

const round4 = (x: number) => Math.round(x * 10000) / 10000;
function popcount32(x: number): number {
	let c = 0;
	for (let v = x >>> 0; v; v &= v - 1) c++;
	return c;
}
