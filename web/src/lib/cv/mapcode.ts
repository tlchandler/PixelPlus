// TypeScript mirror of `pixelplus-core::mapcode` (WS4, F6/F7): the codebooks,
// Gray code, run schedule and deterministic frame function. The leader renders
// the pattern from the same plan; `mapcode.test.ts` checks this file against the
// Rust test vectors in crates/pixelplus-core/tests/fixtures/mapcode/vectors.json.
import type { MapPlan } from '$lib/api/types';

export const PHASE_A = 1;
export const PHASE_B = 2;
export const LEAD_IN_SLOTS = 5;
export const GAP_SLOTS = 3;
export const PREAMBLE = [true, true, true, false, false, false, true, true, true, false, false, false];

/** Plan as the daemon sends it (`countProbe` is a WS4 addition to the frozen type). */
export type Plan = MapPlan & { countProbe?: number | null };

export const gray = (n: number) => (n ^ (n >>> 1)) >>> 0;
export function grayInverse(g: number): number {
	let n = g;
	for (let s = g >>> 1; s > 0; s >>>= 1) n ^= s;
	return n >>> 0;
}
export function pixelBitsFor(maxPixels: number): number {
	if (maxPixels <= 2) return 1;
	return 32 - Math.clz32(maxPixels - 1);
}
export const popcount = (x: number) => {
	let c = 0;
	for (let v = x >>> 0; v; v &= v - 1) c++;
	return c;
};

const GOLAY_A = [
	[0, 1, 1, 1, 1, 1],
	[1, 0, 1, 2, 2, 1],
	[1, 1, 0, 1, 2, 2],
	[1, 2, 1, 0, 1, 2],
	[1, 2, 2, 1, 0, 1],
	[1, 1, 2, 2, 1, 0]
];

/** The 132 hexads of S(5,6,12) (supports of the weight-6 ternary Golay words). */
function hexads(): number[] {
	const out = new Set<number>();
	for (let m = 0; m < 729; m++) {
		const msg = [0, 1, 2, 3, 4, 5].map((i) => Math.floor(m / 3 ** i) % 3);
		const word = [...msg];
		for (let j = 0; j < 6; j++) {
			let s = 0;
			for (let i = 0; i < 6; i++) s += msg[i] * GOLAY_A[i][j];
			word.push(s % 3);
		}
		if (word.filter((x) => x !== 0).length === 6) {
			let bits = 0;
			word.forEach((x, i) => {
				if (x) bits |= 1 << (11 - i);
			});
			out.add(bits);
		}
	}
	return [...out].sort((a, b) => a - b);
}

function greedy(bits: number): number[] {
	const w = bits / 2;
	const book: number[] = [];
	for (let x = 0; x < 1 << bits; x++) {
		if (popcount(x) !== w) continue;
		let ok = true;
		for (const c of book)
			if (popcount(c ^ x) < 4) {
				ok = false;
				break;
			}
		if (ok) book.push(x);
	}
	return book;
}

const books = new Map<number, number[]>();
export function codebook(bits: number): number[] {
	let b = books.get(bits);
	if (!b) {
		b = bits === 12 ? hexads() : bits === 16 ? greedy(16) : [];
		books.set(bits, b);
	}
	return b;
}
export function codeBitsFor(nTargets: number): number {
	if (nTargets <= codebook(12).length) return 12;
	if (nTargets <= codebook(16).length) return 16;
	return 0;
}

export interface Schedule {
	bitMs: number;
	leadInMs: number;
	preambleMs: number;
	phaseAms: number;
	phaseBms: number;
	gapMs: number;
	passMs: number;
	totalMs: number;
	codeBits: number;
	pixelBits: number;
	passes: number;
}

function passSlots(plan: Plan): [number, number, number, number] {
	const a = plan.phases & PHASE_A ? codeBitsFor(plan.targets.length) : 0;
	const b = plan.phases & PHASE_B ? 2 * plan.pixelBits : 0;
	return [PREAMBLE.length, a, b, GAP_SLOTS];
}

export function schedule(plan: Plan): Schedule {
	const bit = Math.max(1, plan.bitMs);
	const [p, a, b, g] = passSlots(plan);
	const pass = (p + a + b + g) * bit;
	return {
		bitMs: plan.bitMs,
		leadInMs: LEAD_IN_SLOTS * bit,
		preambleMs: p * bit,
		phaseAms: a * bit,
		phaseBms: b * bit,
		gapMs: g * bit,
		passMs: pass,
		totalMs: LEAD_IN_SLOTS * bit + pass * plan.passes,
		codeBits: a,
		pixelBits: b ? plan.pixelBits : 0,
		passes: plan.passes
	};
}

export type MapSlot =
	| { kind: 'leadIn' }
	| { kind: 'preamble'; index: number; on: boolean }
	| { kind: 'phaseA'; index: number }
	| { kind: 'phaseB'; index: number; inverted: boolean }
	| { kind: 'gap' }
	| { kind: 'done' }
	| { kind: 'probe' };
export interface MapFrame {
	pass: number;
	slot: MapSlot;
}

/** The deterministic frame function (mirror of `mapcode::frame_for`). */
export function frameFor(plan: Plan, posMs: number): MapFrame {
	if (plan.countProbe != null) return { pass: 0, slot: { kind: 'probe' } };
	const bit = Math.max(1, plan.bitMs);
	const slot = Math.floor(Math.max(0, posMs - plan.startPosMs) / bit);
	if (slot < LEAD_IN_SLOTS) return { pass: 0, slot: { kind: 'leadIn' } };
	const [p, a, b, g] = passSlots(plan);
	const per = p + a + b + g;
	const s = slot - LEAD_IN_SLOTS;
	const pass = Math.floor(s / per);
	if (pass >= plan.passes) return { pass: 0, slot: { kind: 'done' } };
	let i = s % per;
	if (i < p) return { pass, slot: { kind: 'preamble', index: i, on: PREAMBLE[i] } };
	i -= p;
	if (i < a) return { pass, slot: { kind: 'phaseA', index: i } };
	i -= a;
	if (i < b) return { pass, slot: { kind: 'phaseB', index: Math.floor(i / 2), inverted: i % 2 === 1 } };
	return { pass, slot: { kind: 'gap' } };
}

export function codeword(plan: Plan, k: number): number {
	return codebook(codeBitsFor(plan.targets.length))[k] ?? 0;
}

/** Whether pixel `pixel` of target `k` is lit in `frame`. */
export function pixelOn(plan: Plan, frame: MapFrame, k: number, pixel: number): boolean {
	const t = plan.targets[k];
	if (!t || pixel >= t.maxPixels) return false;
	const s = frame.slot;
	switch (s.kind) {
		case 'preamble':
			return s.on;
		case 'phaseA': {
			const bits = codeBitsFor(plan.targets.length);
			return bits > s.index && ((codeword(plan, k) >> (bits - 1 - s.index)) & 1) === 1;
		}
		case 'phaseB': {
			if (s.index >= plan.pixelBits) return false;
			const bit = ((gray(pixel) >> (plan.pixelBits - 1 - s.index)) & 1) === 1;
			return bit !== s.inverted;
		}
		default:
			return false;
	}
}

/** Fraction of all target pixels lit at `posMs` (the decoder's clock template). */
export function litFraction(plan: Plan, frame: MapFrame): number {
	const s = frame.slot;
	if (s.kind === 'preamble') return s.on ? 1 : 0;
	if (s.kind === 'phaseA') {
		const n = plan.targets.length;
		if (!n) return 0;
		let on = 0;
		for (let k = 0; k < n; k++) if (pixelOn(plan, frame, k, 0)) on++;
		return on / n;
	}
	if (s.kind === 'phaseB') return 0.5;
	return 0;
}
