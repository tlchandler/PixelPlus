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
 * Parameter keys match pixelplus-core `param_schema` (effects/params.rs).
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
	const bright = num(params, 'brightness', 100) / 100;
	const seed = ctx.seed ?? 0;
	const cols = colorsOf(params, ['#ff2a2a', '#ffffff']);
	const col = (k: string, d: string) => rgb(typeof params[k] === 'string' ? (params[k] as string) : d);
	const along = (i: number) => (n > 1 ? i / (n - 1) : 0);
	const pos = (i: number) => (ctx.xs ? ctx.xs[i] : along(i));
	const ypos = (i: number) => (ctx.ys ? ctx.ys[i] : 0.5);
	const rev = params.direction === 'reverse' ? -1 : 1;
	const put = (i: number, c: [number, number, number], k = 1) => {
		const j = off + i * 3;
		out[j] = Math.max(0, Math.min(255, c[0] * k * bright));
		out[j + 1] = Math.max(0, Math.min(255, c[1] * k * bright));
		out[j + 2] = Math.max(0, Math.min(255, c[2] * k * bright));
	};
	switch (kind) {
		case 'solid': {
			const c = col('color', '#ffb46b');
			for (let i = 0; i < n; i++) put(i, c);
			break;
		}
		case 'chase': {
			const size = Math.max(1, num(params, 'size', 3));
			const gap = Math.max(0, num(params, 'gap', 3));
			const bg = col('background', '#000000');
			const fade = params.fade !== false;
			const period = size + gap;
			const shift = t * num(params, 'speed', 8) * rev;
			for (let i = 0; i < n; i++) {
				const p = i - shift + 1e6 * period;
				const band = Math.floor(p / period);
				const inBand = p - band * period;
				if (inBand < size) {
					const k = fade ? 0.35 + 0.65 * (inBand / size) : 1;
					put(i, mix(bg, cols[band % cols.length], k));
				} else put(i, bg);
			}
			break;
		}
		case 'twinkle': {
			const dens = num(params, 'density', 0.4);
			const glow = num(params, 'glow', 0.1);
			const speed = num(params, 'speed', 1);
			for (let i = 0; i < n; i++) {
				const ph = hash(i, seed) * 10;
				const rate = 0.6 + hash(i, seed + 7) * 1.2;
				const v = Math.max(0, Math.sin((t * speed * rate + ph) * Math.PI));
				const on = hash(i, seed + 3) < dens;
				const c = cols[Math.floor(hash(i, seed + 11) * cols.length)];
				put(i, c, on ? glow + (1 - glow) * v * v : glow);
			}
			break;
		}
		case 'rainbow': {
			const spread = num(params, 'spread', 1);
			const sat = num(params, 'saturation', 1);
			const across = params.mode === 'across';
			for (let i = 0; i < n; i++)
				put(i, hsv((across ? pos(i) : along(i)) * spread - t * num(params, 'speed', 0.25) * rev, sat, 1));
			break;
		}
		case 'colorwash': {
			const L = cols.length;
			const spread = num(params, 'spread', 0);
			for (let i = 0; i < n; i++) {
				const f = (((t * num(params, 'speed', 0.05) * L + pos(i) * spread * L) % L) + L) % L;
				const a = Math.floor(f);
				put(i, mix(cols[a % L], cols[(a + 1) % L], f - a));
			}
			break;
		}
		case 'candycane': {
			const w = Math.max(1, num(params, 'stripeWidth', 4));
			const c2 = cols.length > 1 ? cols : [cols[0], [255, 255, 255] as [number, number, number]];
			for (let i = 0; i < n; i++) {
				const band = Math.floor((i - t * num(params, 'speed', 3) * rev + 1e6 * w) / w);
				put(i, c2[band % c2.length]);
			}
			break;
		}
		case 'fire': {
			const height = num(params, 'height', 0.8);
			const speed = num(params, 'speed', 1);
			const pal = String(params.palette ?? 'classic');
			const tint: Record<string, [number, number, number]> = {
				classic: [255, 150, 30],
				ember: [255, 70, 10],
				blue: [40, 120, 255],
				green: [60, 255, 80],
				purple: [190, 60, 255]
			};
			const base = tint[pal] ?? tint.classic;
			for (let i = 0; i < n; i++) {
				const y = ctx.ys ? 1 - ypos(i) : (i % 20) / 20;
				const flick = hash(i, Math.floor(t * 18 * speed) + seed);
				const h = Math.max(0, Math.min(1, (height * 1.2 - y) * (0.55 + 0.45 * flick)));
				const hot = Math.max(0, h - 0.55) * 2;
				put(i, [
					Math.min(255, base[0] * h + 255 * hot * 0.4),
					Math.min(255, base[1] * h * h + 200 * hot * 0.5),
					Math.min(255, base[2] * h + 120 * hot * 0.3)
				]);
			}
			break;
		}
		case 'snow': {
			const dens = num(params, 'density', 0.35);
			const flake = Math.max(0.01, num(params, 'flakeSize', 0.06));
			const wind = num(params, 'wind', 0);
			const c = col('color', '#ffffff');
			const bg = col('background', '#00061a');
			for (let i = 0; i < n; i++) {
				const x = pos(i) + wind * t * 0.1;
				const lane = Math.floor((((x % 1) + 1) % 1) * 40);
				const fall = (t * num(params, 'speed', 0.25) + hash(lane, seed)) % 1;
				const y = ctx.ys ? ypos(i) : (i % 25) / 25;
				const d = Math.abs(y - fall);
				const on = hash(lane, seed + 5) < dens + 0.2 && d < flake;
				put(i, on ? mix(bg, c, 1 - d / flake) : bg);
			}
			break;
		}
		case 'sparkle': {
			const dens = num(params, 'density', 0.08);
			const spark = col('sparkleColor', '#ffffff');
			const frame = Math.floor(t * 14 * num(params, 'speed', 1));
			for (let i = 0; i < n; i++) {
				const hit = hash(i, frame + seed) < dens;
				put(i, hit ? spark : cols[i % cols.length]);
			}
			break;
		}
		case 'wave': {
			const wl = Math.max(0.05, num(params, 'wavelength', 0.5));
			const dir = String(params.direction ?? 'right');
			const along_ = params.mode === 'along';
			const L = cols.length;
			for (let i = 0; i < n; i++) {
				const x = along_ ? along(i) : pos(i);
				const y = along_ ? 0.5 : ypos(i);
				let u = x;
				if (dir === 'left') u = 1 - x;
				else if (dir === 'up') u = 1 - y;
				else if (dir === 'down') u = y;
				else if (dir === 'out' || dir === 'in') {
					const r = Math.hypot(x - 0.5, y - 0.5);
					u = dir === 'out' ? r : 1 - r;
				}
				const f = ((((u / wl - t * num(params, 'speed', 0.3)) % 1) + 1) % 1) * L;
				const a = Math.floor(f);
				put(i, mix(cols[a % L], cols[(a + 1) % L], f - a));
			}
			break;
		}
		case 'meteor': {
			const tail = Math.max(1, num(params, 'tailLength', 15));
			const count = Math.max(1, Math.round(num(params, 'count', 1)));
			const speed = num(params, 'speed', 30);
			const sparkle = params.sparkleTail !== false;
			const span = n + tail;
			for (let i = 0; i < n; i++) {
				let k = 0;
				for (let m = 0; m < count; m++) {
					const headRaw = (t * speed + (m * span) / count + hash(m, seed) * 7) % span;
					const head = rev > 0 ? headRaw : n - headRaw;
					const d = rev > 0 ? head - i : i - head;
					if (d >= 0 && d < tail)
						k = Math.max(
							k,
							Math.pow(1 - d / tail, 2) * (sparkle ? 0.6 + 0.4 * hash(i, Math.floor(t * 20)) : 1)
						);
				}
				put(i, cols[0], k);
			}
			break;
		}
		case 'strobe': {
			const rate = num(params, 'rate', 4);
			const duty = num(params, 'duty', 0.15);
			const c = col('color', '#ffffff');
			const cyc = Math.floor(t * rate);
			const on = (t * rate) % 1 < duty;
			const pattern = String(params.pattern ?? 'all');
			for (let i = 0; i < n; i++) {
				let lit = on;
				if (pattern === 'random') lit = on && hash(i, cyc + seed) < 0.3;
				else if (pattern === 'alternate') lit = on && (i + cyc) % 2 === 0;
				put(i, c, lit ? 1 : 0);
			}
			break;
		}
		case 'breathe': {
			const period = Math.max(0.2, num(params, 'period', 4));
			const minB = num(params, 'minBrightness', 0.05);
			const ph = t / period;
			const v = (1 - Math.cos((ph % 1) * Math.PI * 2)) / 2;
			const c = cols[Math.floor(ph) % cols.length];
			for (let i = 0; i < n; i++) put(i, c, minB + (1 - minB) * v);
			break;
		}
	}
}

const P = {
	color: (key: string, label: string, d: string, help?: string): ParamSpec => ({
		key,
		label,
		kind: 'color',
		default: d,
		help
	}),
	colors: (key: string, label: string, d: string[], help?: string): ParamSpec => ({
		key,
		label,
		kind: 'colors',
		default: d,
		help
	}),
	num: (
		key: string,
		label: string,
		min: number,
		max: number,
		step: number,
		d: number,
		unit?: string,
		help?: string
	): ParamSpec => ({
		key,
		label,
		kind: 'number',
		min,
		max,
		step,
		default: d,
		unit,
		help
	}),
	bool: (key: string, label: string, d: boolean): ParamSpec => ({ key, label, kind: 'bool', default: d }),
	sel: (key: string, label: string, options: string[], d: string, help?: string): ParamSpec => ({
		key,
		label,
		kind: 'select',
		options,
		default: d,
		help
	})
};
const BRIGHT = P.num('brightness', 'Brightness', 0, 100, 1, 100, '%', 'Overall brightness of this look.');
const DIR = P.sel(
	'direction',
	'Direction',
	['forward', 'reverse'],
	'forward',
	"Which way along the prop's pixels the pattern moves."
);

/**
 * Fallback copy of the daemon's parameter schema (GET /effects/schema is authoritative),
 * mirroring crates/pixelplus-core/src/effects/params.rs.
 */
export const DEFAULT_EFFECT_SCHEMA: EffectSchema = {
	solid: [P.color('color', 'Color', '#ffb46b'), BRIGHT],
	chase: [
		P.colors('colors', 'Colors', ['#ff0000', '#00c000'], 'Each band of lit pixels takes the next color.'),
		P.color('background', 'Background', '#000000'),
		P.num('speed', 'Speed', 0, 60, 0.5, 8, 'px/s', '0 holds the pattern still.'),
		P.num('size', 'Band size', 1, 50, 1, 3, 'px'),
		P.num('gap', 'Gap', 0, 50, 1, 3, 'px', 'Unlit pixels between bands.'),
		DIR,
		P.bool('fade', 'Fading tail', true),
		BRIGHT
	],
	twinkle: [
		P.colors('colors', 'Colors', ['#ffb46b']),
		P.num('density', 'Density', 0, 1, 0.01, 0.4, undefined, 'Share of pixels twinkling at any moment.'),
		P.num('speed', 'Speed', 0.1, 5, 0.1, 1),
		P.num('glow', 'Base glow', 0, 0.8, 0.01, 0.1, undefined, 'How bright pixels stay between twinkles.'),
		BRIGHT
	],
	rainbow: [
		P.num('speed', 'Speed', 0, 5, 0.05, 0.25, 'cycles/s'),
		P.num('spread', 'Rainbows across', 0.1, 10, 0.1, 1),
		P.num('saturation', 'Saturation', 0, 1, 0.01, 1),
		P.sel(
			'mode',
			'Spread',
			['along', 'across'],
			'along',
			"Along each prop's pixels, or across the whole display."
		),
		DIR,
		BRIGHT
	],
	colorwash: [
		P.colors('colors', 'Colors', ['#ff0000', '#00c000', '#0040ff']),
		P.num('speed', 'Speed', 0, 2, 0.01, 0.05, 'cycles/s', 'Trips through the whole color list per second.'),
		P.num(
			'spread',
			'Spread',
			0,
			2,
			0.05,
			0,
			undefined,
			'0 = every prop the same color; higher staggers colors across the display.'
		),
		BRIGHT
	],
	candycane: [
		P.colors('colors', 'Stripe colors', ['#ff0000', '#ffffff']),
		P.num('stripeWidth', 'Stripe width', 1, 50, 1, 4, 'px'),
		P.num('speed', 'Speed', 0, 30, 0.5, 3, 'px/s'),
		DIR,
		BRIGHT
	],
	fire: [
		P.sel('palette', 'Flame color', ['classic', 'ember', 'blue', 'green', 'purple'], 'classic'),
		P.num('height', 'Flame height', 0.1, 1.5, 0.05, 0.8),
		P.num('speed', 'Speed', 0.1, 4, 0.1, 1),
		BRIGHT
	],
	snow: [
		P.color('color', 'Snow color', '#ffffff'),
		P.color('background', 'Sky color', '#00061a'),
		P.num('density', 'Amount of snow', 0, 1, 0.01, 0.35),
		P.num('speed', 'Fall speed', 0.05, 2, 0.05, 0.25, undefined, 'Prop heights per second.'),
		P.num('flakeSize', 'Flake size', 0.01, 0.3, 0.01, 0.06),
		P.num('wind', 'Wind', -1, 1, 0.05, 0),
		BRIGHT
	],
	sparkle: [
		P.colors('colors', 'Background colors', ['#0a1a4a']),
		P.color('sparkleColor', 'Sparkle color', '#ffffff'),
		P.num('density', 'Density', 0, 1, 0.01, 0.08),
		P.num('speed', 'Speed', 0.2, 5, 0.1, 1),
		BRIGHT
	],
	wave: [
		P.colors('colors', 'Colors', ['#0020ff', '#00c8ff', '#ffffff']),
		P.num('speed', 'Speed', 0, 5, 0.05, 0.3, 'waves/s'),
		P.num(
			'wavelength',
			'Wave length',
			0.05,
			4,
			0.05,
			0.5,
			undefined,
			'Length of one wave as a share of the display (or prop).'
		),
		P.sel('direction', 'Direction', ['right', 'left', 'up', 'down', 'out', 'in'], 'right'),
		P.sel(
			'mode',
			'Spread',
			['across', 'along'],
			'across',
			"Across the whole display, or along each prop's pixels."
		),
		BRIGHT
	],
	meteor: [
		P.colors('colors', 'Colors', ['#ffffff']),
		P.num('speed', 'Speed', 1, 200, 1, 30, 'px/s'),
		P.num('tailLength', 'Tail length', 1, 100, 1, 15, 'px'),
		P.num('count', 'Meteors per prop', 1, 20, 1, 1),
		DIR,
		P.bool('sparkleTail', 'Sparkling tail', true),
		BRIGHT
	],
	strobe: [
		P.color('color', 'Color', '#ffffff'),
		P.num('rate', 'Flashes per second', 0.5, 20, 0.5, 4, 'Hz'),
		P.num('duty', 'Flash length', 0.02, 0.9, 0.01, 0.15, undefined, 'Share of each cycle the lights are on.'),
		P.sel('pattern', 'Pattern', ['all', 'random', 'alternate'], 'all'),
		BRIGHT
	],
	breathe: [
		P.colors('colors', 'Colors', ['#ff0000', '#00c000'], 'Each breath uses the next color.'),
		P.num('period', 'Breath length', 0.5, 20, 0.1, 4, 's'),
		P.num('minBrightness', 'Lowest brightness', 0, 1, 0.01, 0.05),
		BRIGHT
	]
};

export const EFFECT_META: Record<EffectKind, { label: string; blurb: string }> = {
	solid: { label: 'Solid', blurb: 'Every pixel one steady color.' },
	chase: { label: 'Chase', blurb: 'Bands of color running along each prop.' },
	twinkle: { label: 'Twinkle', blurb: 'Pixels gently fade in and out at random.' },
	rainbow: { label: 'Rainbow', blurb: 'A flowing rainbow along each prop or across the display.' },
	colorwash: { label: 'Color Wash', blurb: 'The whole display slowly blends through a list of colors.' },
	candycane: { label: 'Candy Cane', blurb: 'Moving stripes, like a candy cane.' },
	fire: { label: 'Fire', blurb: 'Flickering flames rising from the bottom of each prop.' },
	snow: { label: 'Snow', blurb: 'Snowflakes drifting down.' },
	sparkle: { label: 'Sparkle', blurb: 'Quick glints over a background color.' },
	wave: { label: 'Wave', blurb: 'Smooth waves of color rolling across the display.' },
	meteor: { label: 'Meteor', blurb: 'Shooting stars with fading tails.' },
	strobe: { label: 'Strobe', blurb: 'Fast flashes.' },
	breathe: { label: 'Breathe', blurb: 'Slowly brightens and dims, like breathing.' }
};

export function defaultParams(schema: ParamSpec[]): EffectParams {
	const p: EffectParams = {};
	for (const s of schema) p[s.key] = Array.isArray(s.default) ? [...s.default] : s.default;
	return p;
}
