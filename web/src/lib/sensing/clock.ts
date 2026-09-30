/**
 * Linear clock mapping (e.g. AudioContext seconds → performance.now() ms), fitted by least
 * squares over recent pairs with outlier rejection. Timestamps from the audio thread arrive
 * with scheduling jitter; the fit averages it out and follows slow drift.
 */
import { median } from './dsp';

export class LinearClock {
	private xs: number[] = [];
	private ys: number[] = [];
	private a = 0;
	private b = 1;
	private fitted = false;

	/** `nominalSlope`: y units per x unit when only one pair is known. */
	constructor(
		private nominalSlope = 1,
		private maxPairs = 240
	) {
		this.b = nominalSlope;
	}

	get count(): number {
		return this.xs.length;
	}

	add(x: number, y: number): void {
		if (!Number.isFinite(x) || !Number.isFinite(y)) return;
		this.xs.push(x);
		this.ys.push(y);
		if (this.xs.length > this.maxPairs) {
			this.xs.shift();
			this.ys.shift();
		}
		this.fitted = false;
	}

	private fit(): void {
		const n = this.xs.length;
		this.fitted = true;
		if (n === 0) return;
		if (n < 3) {
			this.b = this.nominalSlope;
			this.a = this.ys[n - 1] - this.b * this.xs[n - 1];
			return;
		}
		const solve = (idx: number[]) => {
			let sx = 0,
				sy = 0;
			for (const i of idx) {
				sx += this.xs[i];
				sy += this.ys[i];
			}
			const mx = sx / idx.length;
			const my = sy / idx.length;
			let sxx = 0,
				sxy = 0;
			for (const i of idx) {
				sxx += (this.xs[i] - mx) ** 2;
				sxy += (this.xs[i] - mx) * (this.ys[i] - my);
			}
			const b = sxx > 1e-12 ? sxy / sxx : this.nominalSlope;
			return [my - b * mx, b];
		};
		let idx = this.xs.map((_, i) => i);
		[this.a, this.b] = solve(idx);
		const res = idx.map((i) => this.ys[i] - (this.a + this.b * this.xs[i]));
		const m = median(res.map(Math.abs));
		const keep = idx.filter((_, k) => Math.abs(res[k]) <= Math.max(3 * m * 1.4826, 1e-9));
		if (keep.length >= 3 && keep.length < idx.length) {
			idx = keep;
			[this.a, this.b] = solve(idx);
		}
	}

	map(x: number): number {
		if (!this.fitted) this.fit();
		return this.a + this.b * x;
	}

	get slope(): number {
		if (!this.fitted) this.fit();
		return this.b;
	}
}
