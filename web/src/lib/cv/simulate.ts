// Synthetic yard camera for testing the mapping decoder (F6/F7, WS4).
//
// Renders what a phone would record while the leader plays a mapping plan:
// LEDs with a Gaussian point spread, ambient light, a flickering streetlight,
// wall/window reflections, occluders, auto-exposure drift, read + shot noise,
// 8-bit clipping, rolling shutter, exposure integration across bit edges,
// timestamp jitter, dropped frames and camera shake.
import type { Frame, Recording } from './decode';
import { frameFor, pixelOn, schedule, type Plan } from './mapcode';

export interface SimLed {
	k: number;
	idx: number;
	x: number;
	y: number;
	/** Peak brightness above ambient when on (0..255 before gain). */
	amp: number;
	visible: boolean;
}

export interface SimOptions {
	width?: number;
	height?: number;
	fps?: number;
	/** Timestamp jitter (± ms). */
	jitterMs?: number;
	/** Drop every n-th frame (0 = none). */
	dropEvery?: number;
	/** Exposure time (ms); default = frame interval (night). */
	exposureMs?: number;
	/** Top-to-bottom readout time (ms). */
	rollingShutterMs?: number;
	/** Gaussian read noise σ (levels). */
	noise?: number;
	/** Shot noise factor (σ = k·√signal). */
	shot?: number;
	/** Ambient level. */
	ambient?: number;
	/** Auto-exposure gain drift amplitude (0.2 = ±20 %). */
	aeDrift?: number;
	psfSigma?: number;
	/** Model the capture's max-pool downsampling from full-resolution video: a
	 *  pixel sees the PSF at the nearest point of its square (default true). */
	maxPool?: boolean;
	/** Streetlight flicker: position, radius, amplitude, frequency. */
	flicker?: { x: number; y: number; r: number; amp: number; hz: number };
	/** Diffuse reflections of targets `ks` onto a rectangle (window/wall/snow). */
	reflections?: { x: number; y: number; w: number; h: number; gain: number; ks: number[] }[];
	/** Hand shake: per-frame σ (px) and a permanent jump at `jumpAtMs`. */
	shake?: { sigmaPx?: number; jumpAtMs?: number; jumpPx?: [number, number] };
	/** Recording starts this long before the pattern (ms). */
	leadMs?: number;
	/** Recording continues this long after the pattern (ms). */
	tailMs?: number;
	/** Pattern starts at this phone-clock time (ms). */
	patternStartMs?: number;
	seed?: number;
}

/** Small deterministic PRNG (mulberry32). */
export function rng(seed: number) {
	let a = seed >>> 0;
	const next = () => {
		a = (a + 0x6d2b79f5) >>> 0;
		let t = a;
		t = Math.imul(t ^ (t >>> 15), t | 1);
		t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
		return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
	};
	const gauss = () => {
		const u = Math.max(1e-12, next());
		return Math.sqrt(-2 * Math.log(u)) * Math.cos(2 * Math.PI * next());
	};
	return { next, gauss };
}

export interface SceneOptions {
	width: number;
	height: number;
	/** LEDs per target (≤ maxPixels). */
	counts: number[];
	/** Spacing between LEDs along a string (px). */
	spacing?: number;
	amp?: number;
	/** Random amplitude spread (0.3 = ±30 %). */
	ampSpread?: number;
	/** Fraction of LEDs hidden behind occluders (a contiguous run per affected target). */
	occlude?: number;
	/** Reverse the index order of these targets (a string wired backwards). */
	reversed?: number[];
	seed?: number;
}

export type Shape = 'line' | 'arch' | 'circle' | 'cane';

/** Lay the targets out as simple prop shapes on a grid. */
export function makeScene(o: SceneOptions): {
	leds: SimLed[];
	shapes: { k: number; shape: Shape; cx: number; cy: number }[];
} {
	const r = rng(o.seed ?? 1);
	const n = o.counts.length;
	const cols = Math.ceil(Math.sqrt(n * (o.width / o.height)));
	const rows = Math.ceil(n / cols);
	const cw = o.width / cols;
	const ch = o.height / rows;
	const spacing = o.spacing ?? 4;
	const leds: SimLed[] = [];
	const shapes: { k: number; shape: Shape; cx: number; cy: number }[] = [];
	const kinds: Shape[] = ['line', 'arch', 'circle', 'cane'];
	for (let k = 0; k < n; k++) {
		const cx = (k % cols) * cw + cw / 2;
		const cy = Math.floor(k / cols) * ch + ch / 2;
		const shape = kinds[k % kinds.length];
		const count = o.counts[k];
		const len = spacing * Math.max(1, count - 1);
		const pts: [number, number][] = [];
		for (let i = 0; i < count; i++) {
			const u = count > 1 ? i / (count - 1) : 0.5;
			if (shape === 'line') {
				const ang = (k * 0.7) % Math.PI;
				pts.push([cx + Math.cos(ang) * (u - 0.5) * len, cy + Math.sin(ang) * (u - 0.5) * len]);
			} else if (shape === 'arch') {
				const rad = len / Math.PI;
				const a = Math.PI * (1 - u);
				pts.push([cx + Math.cos(a) * rad, cy + rad / 2 - Math.sin(a) * rad]);
			} else if (shape === 'circle') {
				const rad = len / (2 * Math.PI);
				const a = 2 * Math.PI * u * (count / (count + 1));
				pts.push([cx + Math.cos(a) * rad, cy + Math.sin(a) * rad]);
			} else {
				// Candy cane: straight stick then a hook.
				const stick = 0.7;
				const rad = (len * (1 - stick)) / Math.PI;
				if (u <= stick) pts.push([cx, cy + len * stick * 0.5 - u * len]);
				else {
					const a = Math.PI * ((u - stick) / (1 - stick));
					pts.push([cx + rad - Math.cos(a) * rad, cy - len * stick * 0.5 - Math.sin(a) * rad]);
				}
			}
		}
		if (o.reversed?.includes(k)) pts.reverse();
		shapes.push({ k, shape, cx, cy });
		const hideFrom = o.occlude && r.next() < o.occlude * 3 ? Math.floor(r.next() * count) : -1;
		const hideLen = hideFrom >= 0 ? Math.ceil(count / 3) : 0;
		pts.forEach(([x, y], idx) => {
			const amp = (o.amp ?? 160) * (1 + (o.ampSpread ?? 0.25) * (r.next() * 2 - 1));
			const inFrame = x >= 1 && y >= 1 && x < o.width - 1 && y < o.height - 1;
			const hidden = hideFrom >= 0 && idx >= hideFrom && idx < hideFrom + hideLen;
			leds.push({ k, idx, x, y, amp, visible: inFrame && !hidden });
		});
	}
	return { leds, shapes };
}

/** Render the recording of `plan` for `leds`. */
export function simulate(plan: Plan, leds: SimLed[], o: SimOptions = {}): Recording {
	const W = o.width ?? 240;
	const H = o.height ?? 135;
	const fps = o.fps ?? 30;
	const dt = 1000 / fps;
	const r = rng(o.seed ?? 7);
	const exposure = o.exposureMs ?? dt * 0.9;
	const rs = o.rollingShutterMs ?? 0;
	const sigma = o.psfSigma ?? 0.8;
	const rad = Math.ceil(sigma * 3);
	const maxPool = o.maxPool ?? true;
	const noise = o.noise ?? 2;
	const shot = o.shot ?? 0.6;
	const ambient = o.ambient ?? 8;
	const sch = schedule(plan);
	const start = o.patternStartMs ?? 1000;
	const lead = o.leadMs ?? 800;
	const tail = o.tailMs ?? 500;
	const frames: Frame[] = [];
	const img = new Float32Array(W * H);
	// Fraction of the exposure window [t−exp, t] during which pixel idx of target k is on.
	const onFraction = (k: number, idx: number, tEnd: number) => {
		const S = 4;
		let on = 0;
		for (let s = 0; s < S; s++) {
			const t = tEnd - exposure * ((s + 0.5) / S) - start;
			if (t < 0) continue;
			if (pixelOn(plan, frameFor(plan, t), k, idx)) on++;
		}
		return on / S;
	};
	let n = 0;
	for (let t = start - lead; t < start + sch.totalMs + tail; t += dt, n++) {
		if (o.dropEvery && n % o.dropEvery === o.dropEvery - 1) continue;
		const ts = t + (o.jitterMs ? (r.next() * 2 - 1) * o.jitterMs : 0);
		img.fill(0);
		// Ambient: gentle gradient (sky glow at the top).
		for (let y = 0; y < H; y++) {
			const a = ambient * (1.3 - (0.6 * y) / H);
			img.fill(a, y * W, (y + 1) * W);
		}
		let ox = 0,
			oy = 0;
		if (o.shake) {
			const s = o.shake.sigmaPx ?? 0;
			ox = s ? r.gauss() * s : 0;
			oy = s ? r.gauss() * s : 0;
			if (o.shake.jumpAtMs != null && ts - start >= o.shake.jumpAtMs && o.shake.jumpPx) {
				ox += o.shake.jumpPx[0];
				oy += o.shake.jumpPx[1];
			}
		}
		const lit = new Map<number, number>();
		for (const led of leds) {
			if (!led.visible && !o.reflections?.some((rf) => rf.ks.includes(led.k))) continue;
			const rowT = rs * (led.y / H);
			const f = onFraction(led.k, led.idx, ts + rowT);
			if (!f) continue;
			lit.set(led.k, (lit.get(led.k) ?? 0) + f * led.amp);
			if (!led.visible) continue;
			const lx = led.x + ox;
			const ly = led.y + oy;
			const cx = Math.floor(lx);
			const cy = Math.floor(ly);
			for (let dy = -rad; dy <= rad; dy++)
				for (let dx = -rad; dx <= rad; dx++) {
					const x = cx + dx;
					const y = cy + dy;
					if (x < 0 || y < 0 || x >= W || y >= H) continue;
					let ddx = x + 0.5 - lx;
					let ddy = y + 0.5 - ly;
					if (maxPool) {
						ddx = Math.max(0, Math.abs(ddx) - 0.5);
						ddy = Math.max(0, Math.abs(ddy) - 0.5);
					}
					img[y * W + x] += f * led.amp * Math.exp(-(ddx * ddx + ddy * ddy) / (2 * sigma * sigma));
				}
		}
		for (const rf of o.reflections ?? []) {
			let s = 0;
			for (const k of rf.ks) s += lit.get(k) ?? 0;
			const v = (s * rf.gain) / Math.max(1, rf.ks.length * 20);
			for (let y = Math.max(0, Math.round(rf.y + oy)); y < Math.min(H, Math.round(rf.y + rf.h + oy)); y++)
				for (let x = Math.max(0, Math.round(rf.x + ox)); x < Math.min(W, Math.round(rf.x + rf.w + ox)); x++)
					img[y * W + x] += v;
		}
		if (o.flicker) {
			const fl = o.flicker;
			const a = fl.amp * (0.5 + 0.5 * Math.sin(2 * Math.PI * fl.hz * (ts / 1000)));
			for (let y = Math.max(0, fl.y - fl.r); y < Math.min(H, fl.y + fl.r); y++)
				for (let x = Math.max(0, fl.x - fl.r); x < Math.min(W, fl.x + fl.r); x++) {
					const d = Math.hypot(x - fl.x, y - fl.y) / fl.r;
					if (d < 1) img[y * W + x] += a * (1 - d);
				}
		}
		const gain = 1 + (o.aeDrift ?? 0) * Math.sin((2 * Math.PI * (ts - start)) / 9000);
		const data = new Uint8Array(W * H);
		for (let i = 0; i < W * H; i++) {
			const s = img[i] * gain;
			const val = s + r.gauss() * (noise + shot * Math.sqrt(Math.max(0, s)));
			data[i] = val <= 0 ? 0 : val >= 255 ? 255 : Math.round(val);
		}
		frames.push({ t: ts, data });
	}
	return { width: W, height: H, frames, startHintMs: start, hintWindowMs: 2000 };
}

/** Score a decode against the scene's ground truth. */
export function score(
	result: { lights: { k: number; idx: number; x: number; y: number }[] },
	leds: SimLed[],
	W: number,
	H: number,
	tolPx = 2.5
) {
	const truth = new Map<string, SimLed>();
	for (const l of leds) truth.set(`${l.k}:${l.idx}`, l);
	let right = 0,
		wrong = 0;
	for (const d of result.lights) {
		const t = truth.get(`${d.k}:${d.idx}`);
		if (t && t.visible && Math.hypot(t.x - d.x * W, t.y - d.y * H) <= tolPx) right++;
		else wrong++;
	}
	const visible = leds.filter((l) => l.visible).length;
	return {
		visible,
		right,
		wrong,
		found: visible ? right / visible : 1,
		wrongRate: result.lights.length ? wrong / result.lights.length : 0
	};
}
