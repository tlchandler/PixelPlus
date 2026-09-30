/** Small signal-processing toolkit for the phone-side measurements (no dependencies). */

/** In-place radix-2 complex FFT (`re`, `im` length a power of two). `inverse` scales by 1/n. */
export function fft(re: Float64Array, im: Float64Array, inverse = false): void {
	const n = re.length;
	if (n !== im.length || (n & (n - 1)) !== 0) throw new Error('fft size must be a power of two');
	for (let i = 1, j = 0; i < n; i++) {
		let bit = n >> 1;
		for (; j & bit; bit >>= 1) j ^= bit;
		j ^= bit;
		if (i < j) {
			[re[i], re[j]] = [re[j], re[i]];
			[im[i], im[j]] = [im[j], im[i]];
		}
	}
	for (let len = 2; len <= n; len <<= 1) {
		const ang = ((inverse ? 2 : -2) * Math.PI) / len;
		const wr = Math.cos(ang);
		const wi = Math.sin(ang);
		const half = len >> 1;
		for (let i = 0; i < n; i += len) {
			let cr = 1;
			let ci = 0;
			for (let k = 0; k < half; k++) {
				const a = i + k;
				const b = a + half;
				const xr = re[b] * cr - im[b] * ci;
				const xi = re[b] * ci + im[b] * cr;
				re[b] = re[a] - xr;
				im[b] = im[a] - xi;
				re[a] += xr;
				im[a] += xi;
				const t = cr * wr - ci * wi;
				ci = cr * wi + ci * wr;
				cr = t;
			}
		}
	}
	if (inverse) {
		for (let i = 0; i < n; i++) {
			re[i] /= n;
			im[i] /= n;
		}
	}
}

export function nextPow2(n: number): number {
	let p = 1;
	while (p < n) p <<= 1;
	return p;
}

/** Second-order IIR section (RBJ cookbook), direct form I, streaming. */
export class Biquad {
	private x1 = 0;
	private x2 = 0;
	private y1 = 0;
	private y2 = 0;
	constructor(
		private b0: number,
		private b1: number,
		private b2: number,
		private a1: number,
		private a2: number
	) {}

	static lowpass(fs: number, f: number, q = Math.SQRT1_2): Biquad {
		const w = (2 * Math.PI * f) / fs;
		const alpha = Math.sin(w) / (2 * q);
		const c = Math.cos(w);
		const a0 = 1 + alpha;
		return new Biquad((1 - c) / 2 / a0, (1 - c) / a0, (1 - c) / 2 / a0, (-2 * c) / a0, (1 - alpha) / a0);
	}

	static highpass(fs: number, f: number, q = Math.SQRT1_2): Biquad {
		const w = (2 * Math.PI * f) / fs;
		const alpha = Math.sin(w) / (2 * q);
		const c = Math.cos(w);
		const a0 = 1 + alpha;
		return new Biquad((1 + c) / 2 / a0, -(1 + c) / a0, (1 + c) / 2 / a0, (-2 * c) / a0, (1 - alpha) / a0);
	}

	reset(): void {
		this.x1 = this.x2 = this.y1 = this.y2 = 0;
	}

	process(x: number): number {
		const y = this.b0 * x + this.b1 * this.x1 + this.b2 * this.x2 - this.a1 * this.y1 - this.a2 * this.y2;
		this.x2 = this.x1;
		this.x1 = x;
		this.y2 = this.y1;
		this.y1 = y;
		return y;
	}
}

/** A streaming band-pass: 2 high-pass + 2 low-pass sections (4th-order skirts). */
export class BandPass {
	private stages: Biquad[];
	constructor(fs: number, lo: number, hi: number) {
		const top = Math.min(hi, fs * 0.45);
		this.stages = [
			Biquad.highpass(fs, lo),
			Biquad.highpass(fs, lo),
			Biquad.lowpass(fs, top),
			Biquad.lowpass(fs, top)
		];
	}
	process(x: number): number {
		for (const s of this.stages) x = s.process(x);
		return x;
	}
	/** Filter a whole buffer (state carries over between calls). */
	run(input: ArrayLike<number>, out = new Float32Array(input.length)): Float32Array {
		for (let i = 0; i < input.length; i++) out[i] = this.process(input[i]);
		return out;
	}
}

export function median(values: ArrayLike<number>): number {
	const n = values.length;
	if (!n) return NaN;
	const a = Float64Array.from(values).sort();
	return n % 2 ? a[(n - 1) >> 1] : (a[n / 2 - 1] + a[n / 2]) / 2;
}

/** Median absolute deviation (unscaled). */
export function mad(values: ArrayLike<number>, center = median(values)): number {
	const d = new Float64Array(values.length);
	for (let i = 0; i < values.length; i++) d[i] = Math.abs(values[i] - center);
	return median(d);
}

/** Robust standard deviation (MAD × 1.4826). */
export function robustSigma(values: ArrayLike<number>): number {
	return values.length ? mad(values) * 1.4826 : NaN;
}

export function quantile(values: ArrayLike<number>, q: number): number {
	const n = values.length;
	if (!n) return NaN;
	const a = Float64Array.from(values).sort();
	const pos = Math.min(n - 1, Math.max(0, q * (n - 1)));
	const lo = Math.floor(pos);
	const hi = Math.ceil(pos);
	return a[lo] + (a[hi] - a[lo]) * (pos - lo);
}

/** Vertex offset (−0.5…0.5) of the parabola through three equally spaced samples. */
export function parabolicOffset(a: number, b: number, c: number): number {
	const den = a - 2 * b + c;
	if (den >= 0 || !Number.isFinite(den)) return 0;
	const d = (0.5 * (a - c)) / den;
	return Math.max(-0.5, Math.min(0.5, d));
}

/** Deterministic PRNG (mulberry32) for tests and simulations. */
export function rng(seed: number): () => number {
	let a = seed >>> 0;
	return () => {
		a = (a + 0x6d2b79f5) >>> 0;
		let t = a;
		t = Math.imul(t ^ (t >>> 15), t | 1);
		t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
		return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
	};
}

/** Standard normal from a uniform source (Box–Muller). */
export function gaussian(u: () => number): number {
	const a = Math.max(u(), 1e-12);
	return Math.sqrt(-2 * Math.log(a)) * Math.cos(2 * Math.PI * u());
}
