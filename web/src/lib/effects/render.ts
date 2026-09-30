// Approximate client-side renderers for PixelPlus looks. Used for gallery thumbnails,
// live mini previews and the mock backend. The daemon has the authoritative renderer.
import type { EffectKind, EffectParams, EffectSchema, ParamSpec } from '$lib/api/types';
import { hexToRgb } from '$lib/util/format';

export interface RenderCtx {
	/** Global 0..1 horizontal position per pixel (for spatial effects across props). */
	xs?: Float32Array;
	/** Global 0..1 vertical position per pixel (0 = top). */
	ys?: Float32Array;
	seed?: number;
}

function hash(a: number, b: number): number {
	let h = (a * 374761393 + b * 668265263) | 0;
	h = Math.imul(h ^ (h >>> 13), 1274126177);
	h ^= h >>> 16;
	return (h >>> 0) / 4294967295;
}

function hsv(h: number, s: number, v: number): [number, number, number] {
	h = ((h % 1) + 1) % 1;
	const i = Math.floor(h * 6);
	const f = h * 6 - i;
	const p = v * (1 - s);
	const q = v * (1 - f * s);
	const t = v * (1 - (1 - f) * s);
	const m = [
		[v, t, p],
		[q, v, p],
		[p, v, t],
		[p, q, v],
		[t, p, v],
		[v, p, q]
	][i % 6];
	return [m[0] * 255, m[1] * 255, m[2] * 255];
}

const colorCache = new Map<string, [number, number, number]>();
function rgb(hex: string): [number, number, number] {
	let c = colorCache.get(hex);
	if (!c) {
		c = hexToRgb(hex);
		colorCache.set(hex, c);
	}
	return c;
}

function colorsOf(p: EffectParams, fallback: string[]): [number, number, number][] {
	const c = p.colors ?? (p.color ? [p.color as string] : fallback);
	const arr = Array.isArray(c) ? c : [String(c)];
	return (arr.length ? arr : fallback).map((h) => rgb(String(h)));
}

const num = (p: EffectParams, k: string, d: number) => (typeof p[k] === 'number' ? (p[k] as number) : d);

function mix(a: [number, number, number], b: [number, number, number], t: number): [number, number, number] {
	return [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t];
}

/**
 * Render `n` pixels of effect `kind` at time `t` (seconds) into `out` starting at byte `off`.
 */
export function renderEffect(
	kind: EffectKind,
	params: EffectParams,
	t: number,
	n: number,
	out: Uint8Array | Uint8ClampedArray,
	off = 0,
	ctx: RenderCtx = {}
): void {
	const speed = num(params, 'speed', 1);
	const bright = num(params, 'brightness', 100) / 100;
	const seed = ctx.seed ?? 0;
	const cols = colorsOf(params, ['#ff2a2a', '#ffffff']);
	const pos = (i: number) => (ctx.xs ? ctx.xs[i] : n > 1 ? i / (n - 1) : 0);
	const ypos = (i: number) => (ctx.ys ? ctx.ys[i] : 0.5);
	const put = (i: number, c: [number, number, number], k = 1) => {
		const j = off + i * 3;
		out[j] = Math.max(0, Math.min(255, c[0] * k * bright));
		out[j + 1] = Math.max(0, Math.min(255, c[1] * k * bright));
		out[j + 2] = Math.max(0, Math.min(255, c[2] * k * bright));
	};
	switch (kind) {
		case 'solid':
			for (let i = 0; i < n; i++) put(i, cols[0]);
			break;
		case 'chase': {
			const size = Math.max(1, num(params, 'size', 4));
			const dir = params.reverse ? -1 : 1;
			const shift = t * speed * 12 * dir;
			for (let i = 0; i < n; i++) {
				const band = Math.floor((i + shift + 100000 * size) / size);
				put(i, cols[((band % cols.length) + cols.length) % cols.length]);
			}
			break;
		}
		case 'twinkle': {
			const dens = num(params, 'density', 0.4);
			for (let i = 0; i < n; i++) {
				const ph = hash(i, seed) * 10;
				const rate = 0.6 + hash(i, seed + 7) * 1.2;
				const v = Math.max(0, Math.sin((t * speed * rate + ph) * Math.PI));
				const on = hash(i, seed + 3) < dens + 0.2;
				const c = cols[Math.floor(hash(i, seed + 11) * cols.length)];
				put(i, c, on ? 0.12 + 0.88 * v * v : 0.06);
			}
			break;
		}
		case 'rainbow': {
			const spread = num(params, 'spread', 1);
			for (let i = 0; i < n; i++) put(i, hsv(pos(i) * spread - t * speed * 0.25, 1, 1));
			break;
		}
		case 'colorwash': {
			const L = cols.length;
			const f = (t * speed * 0.25) % L;
			const a = Math.floor(f);
			const c = mix(cols[a % L], cols[(a + 1) % L], f - a);
			for (let i = 0; i < n; i++) put(i, c);
			break;
		}
		case 'candycane': {
			const w = Math.max(1, num(params, 'stripe', 5));
			const c2 = cols.length > 1 ? cols : [cols[0], [255, 255, 255] as [number, number, number]];
			for (let i = 0; i < n; i++) {
				const band = Math.floor((i + t * speed * 8) / w);
				put(i, c2[band % 2]);
			}
			break;
		}
		case 'fire': {
			const heat = num(params, 'intensity', 0.8);
			for (let i = 0; i < n; i++) {
				const y = ctx.ys ? 1 - ypos(i) : (i % 20) / 20;
				const flick = hash(i, Math.floor(t * 18 * speed) + seed);
				const h = Math.max(0, Math.min(1, heat * (1.15 - y) * (0.55 + 0.45 * flick)));
				put(i, [255 * Math.min(1, h * 1.6), 255 * Math.max(0, h - 0.35) * 1.2, 40 * Math.max(0, h - 0.8)]);
			}
			break;
		}
		case 'snow': {
			const dens = num(params, 'density', 0.3);
			const bg = cols.length > 1 ? cols[1] : ([0, 8, 30] as [number, number, number]);
			for (let i = 0; i < n; i++) {
				const lane = Math.floor(pos(i) * 40);
				const fall = (t * speed * 0.35 + hash(lane, seed)) % 1;
				const y = ctx.ys ? ypos(i) : (i % 25) / 25;
				const d = Math.abs(y - fall);
				const on = hash(lane, seed + 5) < dens + 0.35 && d < 0.06;
				put(i, on ? mix(bg, cols[0], 1 - d / 0.06) : bg);
			}
			break;
		}
		case 'sparkle': {
			const dens = num(params, 'density', 0.08);
			const base = cols[0];
			const spark = cols[1] ?? ([255, 255, 255] as [number, number, number]);
			const frame = Math.floor(t * 14 * speed);
			for (let i = 0; i < n; i++) put(i, hash(i, frame + seed) < dens ? spark : base, hash(i, frame + seed) < dens ? 1 : 0.55);
			break;
		}
		case 'wave': {
			const wl = Math.max(0.05, num(params, 'wavelength', 0.5));
			for (let i = 0; i < n; i++) {
				const v = (Math.sin(((pos(i) / wl) * 2 - t * speed) * Math.PI) + 1) / 2;
				put(i, mix(cols[0], cols[1] ?? [0, 0, 0], 1 - v), 0.25 + 0.75 * v);
			}
			break;
		}
		case 'meteor': {
			const tail = Math.max(0.02, num(params, 'tail', 0.2));
			const head = (t * speed * 0.45) % 1.4;
			for (let i = 0; i < n; i++) {
				const d = head - pos(i);
				const k = d >= 0 && d < tail ? Math.pow(1 - d / tail, 2) : 0;
				put(i, cols[0], k * (0.7 + 0.3 * hash(i, Math.floor(t * 20))));
			}
			break;
		}
		case 'strobe': {
			const rate = num(params, 'rate', 6);
			const on = (t * rate) % 1 < 0.18;
			for (let i = 0; i < n; i++) put(i, cols[0], on ? 1 : 0);
			break;
		}
		case 'breathe': {
			const v = (Math.sin(t * speed * Math.PI * 0.8) + 1) / 2;
			const L = cols.length;
			const which = Math.floor((t * speed * 0.4) / 1) % L;
			for (let i = 0; i < n; i++) put(i, cols[which], 0.08 + 0.92 * v * v);
			break;
		}
	}
}

const COLORS = (d: string[]): ParamSpec => ({ key: 'colors', label: 'Colors', kind: 'colors', default: d });
const SPEED: ParamSpec = { key: 'speed', label: 'Speed', kind: 'number', min: 0.1, max: 5, step: 0.1, default: 1 };
const BRIGHT: ParamSpec = {
	key: 'brightness',
	label: 'Brightness',
	kind: 'number',
	min: 5,
	max: 100,
	step: 5,
	default: 100
};

/** Default parameter schema (the daemon serves the authoritative one at GET /effects/schema). */
export const DEFAULT_EFFECT_SCHEMA: EffectSchema = {
	solid: [COLORS(['#ffb347']), BRIGHT],
	chase: [
		COLORS(['#ff2a2a', '#1fbf4f']),
		SPEED,
		{ key: 'size', label: 'Band size', kind: 'number', min: 1, max: 30, step: 1, default: 4 },
		{ key: 'reverse', label: 'Reverse direction', kind: 'bool', default: false },
		BRIGHT
	],
	twinkle: [
		COLORS(['#fff4d6', '#ffd27a']),
		SPEED,
		{ key: 'density', label: 'Density', kind: 'number', min: 0, max: 1, step: 0.05, default: 0.4 },
		BRIGHT
	],
	rainbow: [SPEED, { key: 'spread', label: 'Spread', kind: 'number', min: 0.2, max: 5, step: 0.1, default: 1 }, BRIGHT],
	colorwash: [COLORS(['#ff2a2a', '#1fbf4f', '#2a6bff']), SPEED, BRIGHT],
	candycane: [
		COLORS(['#ff1a1a', '#ffffff']),
		SPEED,
		{ key: 'stripe', label: 'Stripe width', kind: 'number', min: 1, max: 20, step: 1, default: 5 },
		BRIGHT
	],
	fire: [SPEED, { key: 'intensity', label: 'Intensity', kind: 'number', min: 0.2, max: 1.2, step: 0.05, default: 0.8 }, BRIGHT],
	snow: [
		COLORS(['#ffffff', '#001030']),
		SPEED,
		{ key: 'density', label: 'Density', kind: 'number', min: 0, max: 1, step: 0.05, default: 0.3 },
		BRIGHT
	],
	sparkle: [
		COLORS(['#1238ff', '#ffffff']),
		SPEED,
		{ key: 'density', label: 'Density', kind: 'number', min: 0.01, max: 0.5, step: 0.01, default: 0.08 },
		BRIGHT
	],
	wave: [
		COLORS(['#27d3ff', '#6a2bff']),
		SPEED,
		{ key: 'wavelength', label: 'Wavelength', kind: 'number', min: 0.05, max: 2, step: 0.05, default: 0.5 },
		BRIGHT
	],
	meteor: [
		COLORS(['#bfe6ff']),
		SPEED,
		{ key: 'tail', label: 'Tail length', kind: 'number', min: 0.02, max: 0.8, step: 0.02, default: 0.2 },
		BRIGHT
	],
	strobe: [
		COLORS(['#ffffff']),
		{ key: 'rate', label: 'Flashes per second', kind: 'number', min: 1, max: 20, step: 1, default: 6 },
		BRIGHT
	],
	breathe: [COLORS(['#ff2a2a', '#1fbf4f']), SPEED, BRIGHT]
};

export const EFFECT_META: Record<EffectKind, { label: string; blurb: string }> = {
	solid: { label: 'Solid', blurb: 'One steady color' },
	chase: { label: 'Chase', blurb: 'Bands of color running along each prop' },
	twinkle: { label: 'Twinkle', blurb: 'Gentle random twinkling' },
	rainbow: { label: 'Rainbow', blurb: 'Flowing rainbow across the display' },
	colorwash: { label: 'Color wash', blurb: 'Slow fade through colors' },
	candycane: { label: 'Candy cane', blurb: 'Moving stripes' },
	fire: { label: 'Fire', blurb: 'Flickering flames' },
	snow: { label: 'Snowfall', blurb: 'Falling flakes' },
	sparkle: { label: 'Sparkle', blurb: 'Glints over a base color' },
	wave: { label: 'Wave', blurb: 'Rolling waves between two colors' },
	meteor: { label: 'Meteor', blurb: 'Shooting stars with tails' },
	strobe: { label: 'Strobe', blurb: 'Fast flashes' },
	breathe: { label: 'Breathe', blurb: 'Slow pulsing glow' }
};

export function defaultParams(schema: ParamSpec[]): EffectParams {
	const p: EffectParams = {};
	for (const s of schema) p[s.key] = Array.isArray(s.default) ? [...s.default] : s.default;
	return p;
}
