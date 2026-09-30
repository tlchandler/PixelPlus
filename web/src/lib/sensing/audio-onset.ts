/**
 * Microphone onset detection for the calibration chirp.
 *
 * Pipeline (streaming, any block size):
 * 1. Band-pass 1.5–5 kHz (two high-pass + two low-pass biquads) — removes hum, wind and hiss.
 * 2. Matched filter with the known chirp, run through the *same* band-pass (so the filter's
 *    phase cancels out and the peak is not delayed), as FFT overlap-save correlation. A
 *    quadrature (Hilbert) copy of the template gives an envelope that ignores polarity and the
 *    phase smearing of FM radios and small speakers.
 * 3. Onset = first envelope sample above `median + k·MAD` of the last 2 s, refined to the
 *    envelope peak within one template length and interpolated to a fraction of a sample. A refractory period
 *    skips room echoes.
 *
 * Output positions are absolute sample indices (fractional) of the chirp *start*, counting
 * from the first sample pushed (plus `startSample`).
 */
import { BandPass, fft, median, nextPow2, parabolicOffset } from './dsp';

export interface AudioOnset {
	/** Chirp start, in samples since the stream began (fractional). */
	sample: number;
	/** Envelope peak (template-normalised amplitude). */
	strength: number;
	/** Peak above the noise floor, in robust standard deviations. */
	snr: number;
}

export interface ChirpDetectorOptions {
	bandLoHz?: number;
	bandHiHz?: number;
	/** Threshold: median + k × MAD of the recent envelope. */
	k?: number;
	/** Ignore further onsets this soon after one (echoes). */
	refractoryMs?: number;
	/** Rolling window for the noise statistics. */
	windowMs?: number;
	/** Don't report onsets before this much signal was seen (statistics warm-up). */
	warmupMs?: number;
	/** Index of the first sample pushed. */
	startSample?: number;
}

const DECIMATE = 8;

export class ChirpDetector {
	readonly sampleRate: number;
	private bp: BandPass;
	private n: number;
	private hop: number;
	private tplLen: number;
	private hSpec: { re: Float64Array; im: Float64Array };
	private norm: number;
	/** Filtered samples not yet fully correlated (starts at absolute index `bufStart`). */
	private buf: Float32Array;
	private bufLen = 0;
	private bufStart: number;
	/** Envelope samples awaiting detection (starts at absolute index `envStart`). */
	private env: number[] = [];
	private envStart: number;
	/** Decimated envelope history for the noise statistics (block maxima). */
	private hist: number[] = [];
	private histMax: number;
	private decAcc = 0;
	private decCount = 0;
	private thr = Infinity;
	private floor = 0;
	private sigma = 0;
	private refractoryUntil = -Infinity;
	private warmupUntil: number;
	private lookahead: number;
	private peaks: number[] = [];
	private k: number;
	private refractory: number;
	private recentPeak = 0;
	private recentPeakAt = 0;

	constructor(sampleRate: number, template: Float32Array, opts: ChirpDetectorOptions = {}) {
		this.sampleRate = sampleRate;
		const lo = opts.bandLoHz ?? 1500;
		const hi = opts.bandHiHz ?? 5000;
		this.bp = new BandPass(sampleRate, lo, hi);
		this.k = opts.k ?? 8;
		this.refractory = ((opts.refractoryMs ?? 250) / 1000) * sampleRate;
		this.histMax = Math.round((((opts.windowMs ?? 2000) / 1000) * sampleRate) / DECIMATE);
		const start = opts.startSample ?? 0;
		this.bufStart = start;
		this.envStart = start;
		this.warmupUntil = start + ((opts.warmupMs ?? 300) / 1000) * sampleRate;

		// Template through the same band-pass (zero-phase overall), with a short tail.
		const tail = Math.ceil(0.002 * sampleRate);
		const tbp = new BandPass(sampleRate, lo, hi);
		const tpl = new Float64Array(template.length + tail);
		for (let i = 0; i < tpl.length; i++) tpl[i] = tbp.process(i < template.length ? template[i] : 0);
		this.tplLen = tpl.length;
		// The response starts up to one template length before its peak (sidelobes): search
		// that far past the first threshold crossing.
		this.lookahead = this.tplLen + Math.ceil(0.0005 * sampleRate) + 2;
		this.n = nextPow2(Math.max(4096, 4 * this.tplLen));
		this.hop = this.n - this.tplLen + 1;
		// Quadrature copy (Hilbert transform) of the template.
		const m = nextPow2(this.tplLen * 4);
		const qr = new Float64Array(m);
		const qi = new Float64Array(m);
		qr.set(tpl);
		fft(qr, qi);
		for (let i = 1; i < m / 2; i++) {
			// Multiply by −j for positive, +j for negative frequencies.
			const r = qr[i];
			qr[i] = qi[i];
			qi[i] = -r;
			const r2 = qr[m - i];
			qr[m - i] = -qi[m - i];
			qi[m - i] = r2;
		}
		qr[0] = qi[0] = 0;
		qr[m / 2] = qi[m / 2] = 0;
		fft(qr, qi, true);
		const quad = qr.subarray(0, this.tplLen);
		// Combined template spectrum: conj(H_I) + j·conj(H_Q).
		const hr = new Float64Array(this.n);
		const hiI = new Float64Array(this.n);
		hr.set(tpl);
		fft(hr, hiI);
		const gr = new Float64Array(this.n);
		const gi = new Float64Array(this.n);
		gr.set(quad);
		fft(gr, gi);
		const re = new Float64Array(this.n);
		const im = new Float64Array(this.n);
		for (let i = 0; i < this.n; i++) {
			// conj(HI) = hr − j·hiI ; j·conj(HQ) = j·(gr − j·gi) = gi + j·gr
			re[i] = hr[i] + gi[i];
			im[i] = -hiI[i] + gr[i];
		}
		this.hSpec = { re, im };
		let energy = 0;
		for (const v of tpl) energy += v * v;
		this.norm = energy > 0 ? 1 / energy : 1;
		this.buf = new Float32Array(this.n * 2);
	}

	/** Recent loudest chirp-like peak (for a "hearing clicks" meter), decays over 2 s. */
	get level(): { peak: number; snr: number } {
		return { peak: this.recentPeak, snr: this.sigma > 0 ? (this.recentPeak - this.floor) / this.sigma : 0 };
	}

	/** Feed consecutive samples; returns onsets that became certain. */
	push(samples: ArrayLike<number>): AudioOnset[] {
		const out: AudioOnset[] = [];
		for (let i = 0; i < samples.length; i++) {
			if (this.bufLen === this.buf.length) this.grow();
			this.buf[this.bufLen++] = this.bp.process(samples[i]);
			if (this.bufLen >= this.n) {
				this.correlateBlock();
				this.detect(out, false);
			}
		}
		return out;
	}

	/** Process what is buffered (zero-padded) and return the remaining onsets. */
	flush(): AudioOnset[] {
		const out: AudioOnset[] = [];
		if (this.bufLen > 0) {
			const pad = this.n - this.bufLen + this.tplLen;
			this.push(new Float32Array(Math.max(0, pad))).forEach((o) => out.push(o));
		}
		this.detect(out, true);
		return out;
	}

	private grow() {
		const b = new Float32Array(this.buf.length * 2);
		b.set(this.buf);
		this.buf = b;
	}

	private correlateBlock() {
		const n = this.n;
		const re = new Float64Array(n);
		const im = new Float64Array(n);
		for (let i = 0; i < n; i++) re[i] = this.buf[i];
		fft(re, im);
		const { re: hr, im: hi } = this.hSpec;
		for (let i = 0; i < n; i++) {
			const a = re[i];
			const b = im[i];
			re[i] = a * hr[i] - b * hi[i];
			im[i] = a * hi[i] + b * hr[i];
		}
		fft(re, im, true);
		// Valid outputs: lags 0 … n − tplLen (= hop − 1).
		for (let i = 0; i < this.hop; i++) {
			const e = Math.hypot(re[i], im[i]) * this.norm;
			this.env.push(e);
			this.decAcc = Math.max(this.decAcc, e);
			if (++this.decCount === DECIMATE) {
				this.hist.push(this.decAcc);
				this.decAcc = 0;
				this.decCount = 0;
			}
		}
		if (this.hist.length > this.histMax) this.hist.splice(0, this.hist.length - this.histMax);
		this.updateStats();
		// Slide the buffer by one hop.
		this.buf.copyWithin(0, this.hop, this.bufLen);
		this.bufLen -= this.hop;
		this.bufStart += this.hop;
	}

	private updateStats() {
		if (this.hist.length < 16) return;
		const med = median(this.hist);
		const dev = new Float64Array(this.hist.length);
		for (let i = 0; i < this.hist.length; i++) dev[i] = Math.abs(this.hist[i] - med);
		const m = median(dev);
		this.floor = med;
		this.sigma = Math.max(m * 1.4826, med * 0.05, 1e-7);
		this.thr = med + this.k * Math.max(m, med * 0.05, 1e-7);
	}

	private detect(out: AudioOnset[], final: boolean) {
		const env = this.env;
		const limit = final ? env.length : env.length - this.lookahead;
		let i = 0;
		// Recent-peak meter decays over ~2 s.
		const now = this.envStart + env.length;
		if (now - this.recentPeakAt > 2 * this.sampleRate) this.recentPeak *= 0.5;
		for (; i < limit; i++) {
			const abs = this.envStart + i;
			if (env[i] <= this.thr || abs < this.refractoryUntil || abs < this.warmupUntil) continue;
			// Peak within the look-ahead window.
			let p = i;
			const end = Math.min(env.length, i + this.lookahead);
			for (let j = i + 1; j < end; j++) if (env[j] > env[p]) p = j;
			const delta = p > 0 && p + 1 < env.length ? parabolicOffset(env[p - 1], env[p], env[p + 1]) : 0;
			const strength = env[p];
			// Relative gate once a few real chirps are known: ignore much weaker blips.
			const typical = this.peaks.length >= 3 ? median(this.peaks.slice(-9)) : 0;
			if (strength < 0.2 * typical) {
				i = p;
				continue;
			}
			const onset: AudioOnset = {
				sample: this.envStart + p + delta,
				strength,
				snr: this.sigma > 0 ? (strength - this.floor) / this.sigma : 0
			};
			out.push(onset);
			this.peaks.push(strength);
			if (this.peaks.length > 32) this.peaks.shift();
			if (strength >= this.recentPeak) {
				this.recentPeak = strength;
				this.recentPeakAt = onset.sample;
			}
			this.refractoryUntil = this.envStart + p + this.refractory;
			i = p;
		}
		// Keep one look-ahead window (plus interpolation neighbour) for the next call.
		const keep = final ? 0 : Math.min(env.length, this.lookahead + 1);
		const drop = env.length - keep;
		if (drop > 0) {
			env.splice(0, drop);
			this.envStart += drop;
		}
	}
}
