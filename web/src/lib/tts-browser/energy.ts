// Energy / hype planning, ported 1:1 from tts/pixelplus_tts/energy.py (fpp-voices).
// Energy: 0 calm, 0.4 normal DJ (default), 1 hype, 1.5 extra hype.

import { hypeSetting, type ResolvedVoice } from './voices';

export type Curve = [number, number][];

/**
 * Split a hype line into [lead-in, punchline, tail]. Words in *asterisks* are the punchline,
 * otherwise the last phrase: "Sit back, relax, *and enjoy the show!*".
 */
export function findEmphasis(text: string): [string, string, string] {
	const m = /\*([^*]+)\*/.exec(text);
	if (m) {
		return [
			text.slice(0, m.index).replace(/\*/g, ''),
			m[1],
			text.slice(m.index + m[0].length).replace(/\*/g, '')
		];
	}
	const sentences = text.trim().split(/(?<=[.!?])\s+/);
	let lead = sentences.slice(0, -1).join(' ');
	const last = sentences[sentences.length - 1];
	const clauses = last.split(/(?<=[,;:—])\s+/);
	let punch = clauses[clauses.length - 1];
	lead = [lead, ...clauses.slice(0, -1)].filter(Boolean).join(' ');
	const words = punch.split(/\s+/).filter(Boolean);
	if (!lead && words.length > 2) {
		const half = Math.floor(words.length / 2);
		lead = words.slice(0, half).join(' ');
		punch = words.slice(half).join(' ');
	}
	return [lead, punch, ''];
}

/** -> base energy, synthesis speed and whether the line is a hype line. */
export function planLine(
	voice: Pick<ResolvedVoice, 'defaultEnergy' | 'speed' | 'energy'>,
	energy: number,
	speed?: number
): { base: number; speed: number; hype: boolean } {
	const base = Math.min(energy, voice.defaultEnergy ?? 0);
	const spd = (speed || voice.speed || 1) * (1 + (hypeSetting(voice, 'speed') - 1) * base);
	return { base, speed: spd, hype: energy > base };
}

/** Energy over time for a hype line (lead-in at base, build into punchline, ease back for a tail). */
export function energyCurve(
	phonemes: [string, string, string],
	duration: number,
	base: number,
	energy: number
): Curve {
	const total = phonemes.filter(Boolean).join(' ').length;
	const t0 = (duration * phonemes[0].length) / total;
	const t1 = phonemes[2] ? (duration * (phonemes[0].length + phonemes[1].length + 1)) / total : duration;
	const curve: Curve = [
		[0, base],
		[Math.max(t0 - 0.25, 0), base],
		[t0 + 0.2, energy],
		[t1, energy]
	];
	if (phonemes[2]) curve.push([Math.min(t1 + 0.25, duration), base]);
	return curve.filter((p, n) => n === 0 || p[0] > curve[n - 1][0]);
}

/** Semitones to raise the punchline so it lands lift*peak above the lead-in (>= 0, <= maxLift). */
export function punchLift(
	leadSemis: number | null,
	punchSemis: number | null,
	voice: Pick<ResolvedVoice, 'energy'>,
	peak: number
): number {
	if (leadSemis === null || punchSemis === null) return 0;
	const target = hypeSetting(voice, 'lift') * peak;
	return Math.min(Math.max(0, leadSemis - punchSemis + target), hypeSetting(voice, 'maxLift'));
}

/** Ease pitches above the ceiling back towards it (4th-root compression). */
export function softCeiling(f: number, ceiling: number): number {
	return f > ceiling ? ceiling * (f / ceiling) ** 0.25 : f;
}

/** Linear interpolation of a curve at time t (np.interp semantics). */
export function curveAt(curve: Curve, t: number): number {
	if (t <= curve[0][0]) return curve[0][1];
	for (let i = 1; i < curve.length; i++) {
		if (t <= curve[i][0]) {
			const [ta, va] = curve[i - 1];
			const [tb, vb] = curve[i];
			return va + ((vb - va) * (t - ta)) / (tb - ta);
		}
	}
	return curve[curve.length - 1][1];
}
