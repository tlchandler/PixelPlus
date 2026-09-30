// Pure-TypeScript audio DSP for the in-browser renderer (mono Float32Array).
// No WebAudio dependency so it runs (and is tested) in Node as well as in workers.

// ------------------------------------------------------------------ biquads ----

export interface Biquad {
	b0: number;
	b1: number;
	b2: number;
	a1: number;
	a2: number;
}

/** RBJ audio-EQ-cookbook filters. */
export function biquad(
	type: 'peaking' | 'highpass' | 'lowpass' | 'highshelf' | 'lowshelf',
	f: number,
	sr: number,
	q: number,
	gainDb = 0
): Biquad {
	const w0 = (2 * Math.PI * Math.min(f, sr * 0.49)) / sr;
	const cos = Math.cos(w0);
	const alpha = Math.sin(w0) / (2 * q);
	const A = 10 ** (gainDb / 40);
	let b0: number, b1: number, b2: number, a0: number, a1: number, a2: number;
	switch (type) {
		case 'peaking':
			[b0, b1, b2] = [1 + alpha * A, -2 * cos, 1 - alpha * A];
			[a0, a1, a2] = [1 + alpha / A, -2 * cos, 1 - alpha / A];
			break;
		case 'highpass':
			[b0, b1, b2] = [(1 + cos) / 2, -(1 + cos), (1 + cos) / 2];
			[a0, a1, a2] = [1 + alpha, -2 * cos, 1 - alpha];
			break;
		case 'lowpass':
			[b0, b1, b2] = [(1 - cos) / 2, 1 - cos, (1 - cos) / 2];
			[a0, a1, a2] = [1 + alpha, -2 * cos, 1 - alpha];
			break;
		case 'highshelf':
		case 'lowshelf': {
			const s = type === 'highshelf' ? 1 : -1;
			const sq = 2 * Math.sqrt(A) * alpha;
			b0 = A * (A + 1 + s * (A - 1) * cos + sq);
			b1 = -2 * s * A * (A - 1 + s * (A + 1) * cos);
			b2 = A * (A + 1 + s * (A - 1) * cos - sq);
			a0 = A + 1 - s * (A - 1) * cos + sq;
			a1 = 2 * s * (A - 1 - s * (A + 1) * cos);
			a2 = A + 1 - s * (A - 1) * cos - sq;
			break;
		}
	}
	return { b0: b0 / a0, b1: b1 / a0, b2: b2 / a0, a1: a1 / a0, a2: a2 / a0 };
}

export function applyBiquads(x: Float32Array, filters: Biquad[]): Float32Array {
	let y = x;
	for (const f of filters) {
		const out = new Float32Array(y.length);
		let x1 = 0,
			x2 = 0,
			y1 = 0,
			y2 = 0;
		for (let i = 0; i < y.length; i++) {
			const x0 = y[i];
			const y0 = f.b0 * x0 + f.b1 * x1 + f.b2 * x2 - f.a1 * y1 - f.a2 * y2;
			out[i] = y0;
			x2 = x1;
			x1 = x0;
			y2 = y1;
			y1 = y0;
		}
		y = out;
	}
	return y;
}

/**
 * The audible part of an ffmpeg filter string (a DjVoice `eq`): `equalizer` (t=q|o|h),
 * `highpass`, `lowpass`, `bass`/`lowshelf`, `treble`/`highshelf`. Other filters are ignored.
 */
export function parseFfmpegEq(eq: string | undefined, sr: number): Biquad[] {
	if (!eq) return [];
	const out: Biquad[] = [];
	for (const part of eq.split(',')) {
		const [name, args = ''] = part.trim().split(/=(.*)/s);
		const p: Record<string, string> = {};
		args.split(':').forEach((kv, i) => {
			const [k, v] = kv.includes('=') ? kv.split('=') : [String(i), kv];
			if (k) p[k] = v;
		});
		const num = (k: string, d: number) => (p[k] !== undefined && p[k] !== '' ? Number(p[k]) : d);
		const f = num('f', num('frequency', name === 'highpass' ? 3000 : 1000));
		const width = num('w', num('width', 0.707));
		const t = p.t ?? p.width_type ?? 'q';
		const q =
			t === 'o' ? 1 / (2 * Math.sinh((Math.LN2 / 2) * width)) : t === 'h' ? f / Math.max(width, 1e-3) : width;
		const g = num('g', num('gain', 0));
		if (!Number.isFinite(f) || !Number.isFinite(q) || q <= 0) continue;
		if (name === 'equalizer') out.push(biquad('peaking', f, sr, q, g));
		else if (name === 'highpass') out.push(biquad('highpass', f, sr, p.w || p.width ? q : 0.707));
		else if (name === 'lowpass') out.push(biquad('lowpass', f, sr, p.w || p.width ? q : 0.707));
		else if (name === 'bass' || name === 'lowshelf') out.push(biquad('lowshelf', num('f', 100), sr, 0.707, g));
		else if (name === 'treble' || name === 'highshelf') out.push(biquad('highshelf', num('f', 3000), sr, 0.707, g));
	}
	return out;
}

// ---------------------------------------------------------------- loudness ----

/** BS.1770 K-weighting pre-filter for any sample rate (libebur128's formulation). */
export function kWeighting(sr: number): Biquad[] {
	let K = Math.tan((Math.PI * 1681.974450955533) / sr);
	let Q = 0.7071752369554196;
	const Vh = 10 ** (3.999843853973347 / 20);
	const Vb = Vh ** 0.4996667741545416;
	let a0 = 1 + K / Q + K * K;
	const shelf: Biquad = {
		b0: (Vh + (Vb * K) / Q + K * K) / a0,
		b1: (2 * (K * K - Vh)) / a0,
		b2: (Vh - (Vb * K) / Q + K * K) / a0,
		a1: (2 * (K * K - 1)) / a0,
		a2: (1 - K / Q + K * K) / a0
	};
	K = Math.tan((Math.PI * 38.13547087602444) / sr);
	Q = 0.5003270373238773;
	a0 = 1 + K / Q + K * K;
	const hp: Biquad = { b0: 1, b1: -2, b2: 1, a1: (2 * (K * K - 1)) / a0, a2: (1 - K / Q + K * K) / a0 };
	return [shelf, hp];
}

/**
 * Integrated loudness (LUFS) of mono audio: ITU-R BS.1770 K-weighting (shelf + RLB highpass,
 * computed for any sample rate), 400 ms blocks with 75% overlap, -70 LUFS absolute and -10 LU
 * relative gates. Returns -Infinity for silence.
 */
export function measureLufs(x: Float32Array, sr: number): number {
	const k = applyBiquads(x, kWeighting(sr));
	const block = Math.round(0.4 * sr);
	const hop = Math.round(0.1 * sr);
	const powers: number[] = [];
	if (k.length < block) {
		let s = 0;
		for (let i = 0; i < k.length; i++) s += k[i] * k[i];
		if (k.length) powers.push(s / k.length);
	} else {
		for (let start = 0; start + block <= k.length; start += hop) {
			let s = 0;
			for (let i = start; i < start + block; i++) s += k[i] * k[i];
			powers.push(s / block);
		}
	}
	const lufs = (p: number) => -0.691 + 10 * Math.log10(p);
	const abs = powers.filter((p) => lufs(p) > -70);
	if (!abs.length) return -Infinity;
	const rel = lufs(abs.reduce((a, b) => a + b, 0) / abs.length) - 10;
	const gated = abs.filter((p) => lufs(p) > rel);
	return lufs(gated.reduce((a, b) => a + b, 0) / gated.length);
}

export function gainDb(x: Float32Array, db: number): Float32Array {
	const g = 10 ** (db / 20);
	const out = new Float32Array(x.length);
	for (let i = 0; i < x.length; i++) out[i] = x[i] * g;
	return out;
}

/** Linear gain to `targetLufs` (no-op for silence). */
export function normalizeLoudness(x: Float32Array, sr: number, targetLufs: number): Float32Array {
	const l = measureLufs(x, sr);
	return Number.isFinite(l) ? gainDb(x, targetLufs - l) : x;
}

/** Look-ahead peak limiter: never exceeds `ceiling` (linear), 2 ms attack, `releaseMs` release. */
export function limit(x: Float32Array, sr: number, ceiling = 0.84, releaseMs = 50): Float32Array {
	const n = x.length;
	const look = Math.max(1, Math.round(0.002 * sr));
	const need = new Float32Array(n);
	for (let i = 0; i < n; i++) {
		const a = Math.abs(x[i]);
		need[i] = a > ceiling ? ceiling / a : 1;
	}
	// forward-looking minimum so gain is already down when the peak arrives
	const minAhead = new Float32Array(n);
	const dq: number[] = [];
	for (let i = n - 1; i >= 0; i--) {
		while (dq.length && need[dq[dq.length - 1]] >= need[i]) dq.pop();
		dq.push(i);
		while (dq[0] > i + look) dq.shift();
		minAhead[i] = need[dq[0]];
	}
	const rel = Math.exp(-1 / ((releaseMs / 1000) * sr));
	const out = new Float32Array(n);
	let g = 1;
	for (let i = 0; i < n; i++) {
		const t = minAhead[i];
		g = t < g ? t : t + (g - t) * rel;
		out[i] = x[i] * Math.min(g, need[i]);
	}
	return out;
}

// ------------------------------------------------------------ time / pitch ----

/** Read x at fractional positions with cubic (Catmull-Rom) interpolation. */
export function resample(x: Float32Array, ratio: number, outLength?: number): Float32Array {
	// ratio = input samples advanced per output sample
	const n = outLength ?? Math.max(0, Math.floor(x.length / ratio));
	const out = new Float32Array(n);
	const at = (i: number) => (i < 0 ? 0 : i >= x.length ? 0 : x[i]);
	for (let j = 0; j < n; j++) {
		const p = j * ratio;
		const i = Math.floor(p);
		const f = p - i;
		const y0 = at(i - 1),
			y1 = at(i),
			y2 = at(i + 1),
			y3 = at(i + 2);
		out[j] =
			y1 + 0.5 * f * (y2 - y0 + f * (2 * y0 - 5 * y1 + 4 * y2 - y3 + f * (3 * (y1 - y2) + y3 - y0)));
	}
	return out;
}

function hann(n: number): Float32Array {
	const w = new Float32Array(n);
	for (let i = 0; i < n; i++) w[i] = 0.5 - 0.5 * Math.cos((2 * Math.PI * i) / n);
	return w;
}

/**
 * WSOLA time stretch: `factor` > 1 = longer/slower, pitch unchanged. Window ~32 ms at 24 kHz,
 * 50% overlap, ±8 ms similarity search.
 */
export function timeStretch(x: Float32Array, factor: number, sr = 24000): Float32Array {
	const outLen = Math.round(x.length * factor);
	if (Math.abs(factor - 1) < 1e-4 || x.length < 64) return resample(x, x.length / Math.max(outLen, 1), outLen);
	const N = 2 * Math.round((0.032 * sr) / 2);
	const Hs = N / 2;
	const tol = Math.round(0.008 * sr);
	const win = hann(N);
	const out = new Float32Array(outLen + N);
	const wsum = new Float32Array(outLen + N);
	const get = (i: number) => (i >= 0 && i < x.length ? x[i] : 0);
	let prev = 0;
	for (let k = 0; k * Hs < outLen; k++) {
		const ks = k * Hs;
		const nominal = Math.round(ks / factor);
		let pos = nominal;
		if (k > 0) {
			const natural = prev + Hs; // what would continue the last frame seamlessly
			let best = -Infinity;
			for (let d = -tol; d <= tol; d += 2) {
				const cand = nominal + d;
				if (cand < 0) continue;
				let c = 0;
				for (let i = 0; i < Hs; i += 2) c += get(cand + i) * get(natural + i);
				if (c > best) {
					best = c;
					pos = cand;
				}
			}
		}
		for (let i = 0; i < N; i++) {
			out[ks + i] += get(pos + i) * win[i];
			wsum[ks + i] += win[i];
		}
		prev = pos;
	}
	const y = new Float32Array(outLen);
	for (let i = 0; i < outLen; i++) y[i] = wsum[i] > 1e-3 ? out[i] / wsum[i] : 0;
	return y;
}

/** Shift pitch by `semitones` (duration × `stretch`) via WSOLA + resampling. */
export function pitchShift(x: Float32Array, semitones: number, stretch = 1, sr = 24000): Float32Array {
	const r = 2 ** (semitones / 12);
	const outLen = Math.round(x.length * stretch);
	if (Math.abs(semitones) < 0.01) return timeStretch(x, stretch, sr);
	let y = timeStretch(x, r * stretch, sr);
	if (r > 1) y = applyBiquads(y, [biquad('lowpass', (0.45 * sr) / r, sr, 0.707)]); // anti-alias
	return resample(y, y.length / outLen, outLen);
}

/** Per-frame f0 (Hz, 0 = unvoiced) by normalized autocorrelation on a 2x-decimated signal. */
export function estimatePitch(x: Float32Array, sr: number, hopS = 0.01, fmin = 60, fmax = 500): Float32Array {
	const d = new Float32Array(Math.floor(x.length / 2));
	for (let i = 0; i < d.length; i++) d[i] = 0.5 * (x[2 * i] + x[2 * i + 1]);
	const s = sr / 2;
	const W = Math.round(0.04 * s);
	const hop = Math.round(hopS * s);
	const minLag = Math.floor(s / fmax);
	const maxLag = Math.ceil(s / fmin);
	const frames = Math.max(0, Math.floor((d.length - W - maxLag) / hop));
	const f0 = new Float32Array(frames);
	for (let fr = 0; fr < frames; fr++) {
		const st = fr * hop;
		let e0 = 0;
		for (let i = 0; i < W; i++) e0 += d[st + i] * d[st + i];
		if (e0 / W < 1e-6) continue;
		const r = new Float32Array(maxLag + 2);
		let best = 0;
		for (let lag = minLag; lag <= maxLag + 1; lag++) {
			let c = 0,
				e1 = 0;
			for (let i = 0; i < W; i++) {
				c += d[st + i] * d[st + i + lag];
				e1 += d[st + i + lag] * d[st + i + lag];
			}
			r[lag] = c / Math.sqrt(e0 * e1 + 1e-12);
			if (lag <= maxLag && r[lag] > best) best = r[lag];
		}
		if (best <= 0.6) continue;
		// the smallest-lag peak close to the best one (avoids octave errors at 2T, 3T, ...)
		for (let lag = minLag + 1; lag <= maxLag; lag++) {
			if (r[lag] >= 0.9 * best && r[lag] >= r[lag - 1] && r[lag] >= r[lag + 1]) {
				f0[fr] = s / lag;
				break;
			}
		}
	}
	return f0;
}

export function percentile(values: number[], p: number): number | null {
	if (!values.length) return null;
	const s = [...values].sort((a, b) => a - b);
	const idx = (p / 100) * (s.length - 1);
	const lo = Math.floor(idx);
	const hi = Math.ceil(idx);
	return s[lo] + (s[hi] - s[lo]) * (idx - lo);
}

export function voicedSemitones(x: Float32Array, sr: number): number[] {
	return [...estimatePitch(x, sr)].filter((f) => f > 0).map((f) => 12 * Math.log2(f));
}

/** Sample index of the quietest 10 ms near `t` (±`searchS`): a clean place to split words. */
export function quietCut(x: Float32Array, sr: number, t: number, searchS = 0.15): number {
	const w = Math.round(0.01 * sr);
	const c = Math.round(t * sr);
	let best = Math.min(Math.max(c, 0), x.length);
	let bestE = Infinity;
	for (let s = Math.max(0, c - Math.round(searchS * sr)); s + w <= Math.min(x.length, c + Math.round(searchS * sr)); s += Math.round(w / 2)) {
		let e = 0;
		for (let i = s; i < s + w; i++) e += x[i] * x[i];
		// prefer cuts close to t on ties
		e *= 1 + Math.abs(s + w / 2 - c) / (sr * 10);
		if (e < bestE) {
			bestE = e;
			best = s + Math.round(w / 2);
		}
	}
	return best;
}

/** Concatenate with a short equal-power crossfade at each join. */
export function crossfadeConcat(parts: Float32Array[], sr: number, fadeS = 0.01): Float32Array {
	const nonEmpty = parts.filter((p) => p.length);
	if (!nonEmpty.length) return new Float32Array(0);
	let out = nonEmpty[0];
	for (const p of nonEmpty.slice(1)) {
		const f = Math.min(Math.round(fadeS * sr), out.length, p.length);
		const joined = new Float32Array(out.length + p.length - f);
		joined.set(out.subarray(0, out.length - f));
		for (let i = 0; i < f; i++) {
			const a = Math.cos(((i + 0.5) / f) * (Math.PI / 2));
			const b = Math.sin(((i + 0.5) / f) * (Math.PI / 2));
			joined[out.length - f + i] = out[out.length - f + i] * a + p[i] * b;
		}
		joined.set(p.subarray(f), out.length);
		out = joined;
	}
	return out;
}

/** Apply a gain envelope given as [(seconds, dB)] points with linear ramps. */
export function applyGainCurve(x: Float32Array, sr: number, points: [number, number][]): Float32Array {
	const out = new Float32Array(x.length);
	let k = 0;
	for (let i = 0; i < x.length; i++) {
		const t = i / sr;
		while (k < points.length - 1 && t > points[k + 1][0]) k++;
		let db: number;
		if (t <= points[0][0]) db = points[0][1];
		else if (k >= points.length - 1) db = points[points.length - 1][1];
		else {
			const [ta, va] = points[k];
			const [tb, vb] = points[k + 1];
			db = tb > ta ? va + ((vb - va) * (t - ta)) / (tb - ta) : vb;
		}
		out[i] = x[i] * 10 ** (db / 20);
	}
	return out;
}

export function silence(seconds: number, sr: number): Float32Array {
	return new Float32Array(Math.max(0, Math.round(seconds * sr)));
}

export function concat(parts: Float32Array[]): Float32Array {
	const out = new Float32Array(parts.reduce((n, p) => n + p.length, 0));
	let o = 0;
	for (const p of parts) {
		out.set(p, o);
		o += p.length;
	}
	return out;
}
