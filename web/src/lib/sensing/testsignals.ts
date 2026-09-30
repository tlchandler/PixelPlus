/**
 * Synthetic test signals for the sensing tests (and the in-browser self-test): audio with
 * chirps at exact (fractional) times plus noise, hum, echoes and false clicks; camera frames
 * with flashes that start mid-exposure, noise, auto-exposure drift and rolling shutter.
 */
import { CHIRP, CHIRP_LEVEL, type ChirpSpec } from './schedule';
import { gaussian, rng } from './dsp';

/** The chirp waveform at an arbitrary time `t` seconds after its start (0 outside). */
export function chirpAt(t: number, spec: ChirpSpec = CHIRP, level = CHIRP_LEVEL): number {
	const dur = spec.ms / 1000;
	if (t < 0 || t >= dur) return 0;
	const ramp = spec.rampMs / 1000;
	const k = (spec.f1Hz - spec.f0Hz) / dur;
	const phase = 2 * Math.PI * (spec.f0Hz * t + 0.5 * k * t * t);
	let env = 1;
	if (t < ramp) env = 0.5 - 0.5 * Math.cos((Math.PI * t) / ramp);
	else if (dur - t < ramp) env = 0.5 - 0.5 * Math.cos((Math.PI * (dur - t)) / ramp);
	return Math.sin(phase) * env * level;
}

export interface AudioSim {
	sampleRate: number;
	durationMs: number;
	/** Chirp start times (ms). */
	eventsMs: number[];
	amplitude?: number;
	noise?: number;
	humAmp?: number;
	/** Echo: delay (ms) and relative gain. */
	echo?: { ms: number; gain: number };
	/** Extra false clicks (times in ms). */
	falseClicksMs?: number[];
	/** Flip polarity (some FM receivers / speakers). */
	invert?: boolean;
	seed?: number;
}

export function simulateAudio(o: AudioSim): Float32Array {
	const fs = o.sampleRate;
	const n = Math.round((o.durationMs / 1000) * fs);
	const out = new Float32Array(n);
	const u = rng(o.seed ?? 1);
	const amp = (o.amplitude ?? 0.1) * (o.invert ? -1 : 1);
	const noise = o.noise ?? 0.01;
	const hum = o.humAmp ?? 0;
	for (let i = 0; i < n; i++) out[i] = noise * gaussian(u) + hum * Math.sin((2 * Math.PI * 60 * i) / fs);
	const place = (ms: number, gain: number) => {
		const t0 = ms / 1000;
		const first = Math.max(0, Math.floor(t0 * fs));
		const last = Math.min(n, Math.ceil((t0 + CHIRP.ms / 1000) * fs) + 1);
		for (let i = first; i < last; i++) out[i] += gain * chirpAt(i / fs - t0);
	};
	for (const e of o.eventsMs) {
		place(e, amp);
		if (o.echo) place(e + o.echo.ms, amp * o.echo.gain);
	}
	for (const f of o.falseClicksMs ?? []) {
		// A broadband tick (not a chirp).
		const s = Math.round((f / 1000) * fs);
		for (let i = 0; i < 48 && s + i < n; i++) out[s + i] += (u() - 0.5) * amp * 2;
	}
	return out;
}

export interface VideoSim {
	fps: number;
	durationMs: number;
	/** Flash onsets (ms) and length. */
	flashesMs: number[];
	flashMs: number;
	/** Frame 0 starts exposing at this time (ms). */
	phaseMs?: number;
	width?: number;
	height?: number;
	/** Fraction of the picture that the lights cover. */
	litFraction?: number;
	/** Rows (0..1) where the lights are (top/bottom of the frame). */
	litRows?: [number, number];
	/** Readout time for a rolling shutter (ms, 0 = global shutter). */
	readoutMs?: number;
	noise?: number;
	/** Slow brightness drift (auto exposure), in luma units per second. */
	driftPerS?: number;
	/** Timestamp jitter (ms, std-dev). */
	jitterMs?: number;
	seed?: number;
}

/** Gamma encoding table: linear 0..1 (4096 steps) → 8-bit value. */
const ENC = Float32Array.from({ length: 4096 }, (_, i) => 255 * Math.pow(i / 4095, 1 / 2.2));

export interface SimFrame {
	/** Capture time (exposure start of the first row), with jitter. */
	t: number;
	luma: Uint8Array;
}

/** Frames of a scene with a lit region; exposure ≈ frame period (as at night). */
export function simulateVideo(o: VideoSim): SimFrame[] {
	const w = o.width ?? 160;
	const h = o.height ?? 120;
	const u = rng(o.seed ?? 2);
	const period = 1000 / o.fps;
	const frames: SimFrame[] = [];
	const lit = new Uint8Array(w * h);
	const [r0, r1] = o.litRows ?? [0.3, 0.7];
	const litFrac = o.litFraction ?? 0.1;
	for (let y = Math.floor(r0 * h); y < Math.floor(r1 * h); y++)
		for (let x = 0; x < w; x++) if (u() < litFrac / (r1 - r0)) lit[y * w + x] = 1;
	const base = new Float32Array(w * h);
	for (let i = 0; i < w * h; i++) base[i] = 20 + 25 * u();
	const readout = o.readoutMs ?? 0;
	const noiseAmp = o.noise ?? 2;
	const baseLin = base.map((b) => Math.pow(b / 255, 2.2));
	const flashes = [...o.flashesMs].sort((a, b) => a - b);
	const litFractionOf = (a: number, b: number) => {
		let s = 0;
		for (const f of flashes) {
			if (f > b) break;
			s += Math.max(0, Math.min(b, f + o.flashMs) - Math.max(a, f));
		}
		return s / (b - a);
	};
	for (let k = 0; ; k++) {
		const start = (o.phaseMs ?? 0) + k * period;
		if (start > o.durationMs) break;
		const luma = new Uint8Array(w * h);
		const drift = ((o.driftPerS ?? 0) * start) / 1000;
		// Per-row exposure window with a rolling shutter.
		const rowFrac = new Float32Array(h);
		for (let y = 0; y < h; y++) {
			const off = (y / h) * readout;
			rowFrac[y] = litFractionOf(start + off, start + off + period);
		}
		for (let y = 0; y < h; y++) {
			for (let x = 0; x < w; x++) {
				const i = y * w + x;
				// Light adds in linear space; the camera encodes with gamma 1/2.2.
				const linBase = drift ? Math.pow(Math.max(0, base[i] + drift) / 255, 2.2) : baseLin[i];
				const add = lit[i] ? 0.6 * rowFrac[y] : 0;
				const v = ENC[Math.min(4095, Math.round((linBase + add) * 4095))] + noiseAmp * gaussian(u);
				luma[i] = v <= 0 ? 0 : v >= 255 ? 255 : Math.round(v);
			}
		}
		frames.push({ t: start + (o.jitterMs ?? 0) * gaussian(u), luma });
	}
	return frames;
}
