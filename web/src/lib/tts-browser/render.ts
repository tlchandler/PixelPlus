// Dialog rendering pipeline for browser mode. Backend-agnostic (the Kokoro backend lives in
// kokoro.ts) so the whole chain is testable with a fake synthesizer.
//
// Follows tts/pixelplus_tts/render.py. Where the device renderer uses Praat PSOLA + ffmpeg, this
// uses a lighter approximation:
//   * energy: pitch shift (WSOLA + resampling) of the whole line by pitch*energy semitones; hype
//     lines are split at a quiet point near the punchline start, the punchline gets the
//     fpp-voices lift (so it lands above the lead-in, measured with an autocorrelation pitch
//     tracker, soft-limited by the ceiling), its stretch, and a gain swell of boost*energy dB.
//     Not reproduced: widening of the pitch swings (needs PSOLA).
//   * radio chain: highpass 75 Hz + the voice's ffmpeg `equalizer` bands + presence boost;
//     no de-esser/compressor.
//   * loudness: BS.1770 (K-weighted, gated) to the target, then a -1.5 dBFS look-ahead limiter.

import { energyCurve, findEmphasis, planLine, punchLift, softCeiling } from './energy';
import {
	applyBiquads,
	applyGainCurve,
	biquad,
	concat,
	crossfadeConcat,
	limit,
	measureLufs,
	normalizeLoudness,
	parseFfmpegEq,
	percentile,
	pitchShift,
	quietCut,
	silence,
	voicedSemitones
} from './dsp';
import { compileRules, mergePronunciations, toPhonemes, type Pronunciation } from './pronounce';
import { hypeSetting, resolveVoice, type DjVoiceLike, type DjVoiceObject, type ResolvedVoice } from './voices';

export const SAMPLE_RATE = 24000;
export const DEFAULT_GAP_MS = 350;
export const LEAD_IN_S = 0.25;
export const TAIL_S = 0.6;

export interface DialogLine {
	voice: DjVoiceLike;
	text: string;
	pauseMs: number;
	energy?: number;
}

export interface RenderOptions {
	speed: number;
	pronunciations?: Pronunciation[];
	onProgress?: (p: number) => void;
	/** Apply the ~100 built-in pronunciation fixes too (default true). */
	builtinPronunciations?: boolean;
	/** Custom voices that lines may reference by id or name. */
	voices?: DjVoiceObject[];
	/** Placeholder values: {nextSong} -> placeholders.nextSong. */
	placeholders?: Record<string, string | number>;
	/** Target loudness (default -16 LUFS). */
	loudnessLufs?: number;
	/** Radio EQ + energy processing (default true). */
	fx?: boolean;
}

export interface SynthBackend {
	phonemize(text: string, lang: 'a' | 'b'): Promise<string>;
	/** Phonemes -> mono float audio at 24 kHz. `text` is the plain line (for fallbacks). */
	synthesize(phonemes: string, voice: ResolvedVoice, speed: number, text: string): Promise<Float32Array>;
}

export interface RenderResult {
	samples: Float32Array; // mono, 24 kHz, with lead-in and tail
	sampleRate: number;
	loudnessLufs: number;
	warnings: string[];
}

const PLACEHOLDER_RE = /\{([A-Za-z][A-Za-z0-9_]*)\}/g;

export function substitutePlaceholders(
	text: string,
	values: Record<string, string | number> = {}
): { text: string; missing: string[] } {
	const missing: string[] = [];
	const out = text.replace(PLACEHOLDER_RE, (_m, name: string) => {
		const v = values[name];
		if (v !== undefined && v !== null) return String(v);
		missing.push(name);
		return name.replace(/(?<=[a-z])(?=[A-Z])/g, ' ');
	});
	return { text: out, missing };
}

const median = (xs: number[]) => percentile(xs, 50);

/** Flat energy: raise the whole line by pitch*energy semitones. */
export function flatEnergy(audio: Float32Array, v: ResolvedVoice, energy: number, sr = SAMPLE_RATE): Float32Array {
	const semis = hypeSetting(v, 'pitch') * energy;
	return Math.abs(semis) > 0.05 ? pitchShift(audio, semis, 1, sr) : audio;
}

/** Hype: normal level through the lead-in, then lift/stretch/swell the punchline. */
export function hypeEnergy(
	audio: Float32Array,
	v: ResolvedVoice,
	phonemes: [string, string, string],
	base: number,
	energy: number,
	sr = SAMPLE_RATE
): Float32Array {
	const dur = audio.length / sr;
	const curve = energyCurve(phonemes, dur, base, energy);
	const total = phonemes.filter(Boolean).join(' ').length;
	const t0 = (dur * phonemes[0].length) / total;
	const t1 = phonemes[2] ? (dur * (phonemes[0].length + phonemes[1].length + 1)) / total : dur;
	const cut0 = phonemes[0] ? quietCut(audio, sr, t0) : 0;
	const cut1 = phonemes[2] ? Math.max(cut0, quietCut(audio, sr, t1)) : audio.length;
	const lead = audio.subarray(0, cut0);
	const punch = audio.subarray(cut0, cut1);
	const tail = audio.subarray(cut1);
	const peak = Math.max(...curve.map((c) => c[1]));
	const leadSemis = voicedSemitones(lead, sr);
	const punchSemis = voicedSemitones(punch, sr);
	const reset = punchLift(median(leadSemis), median(punchSemis), v, peak);
	const baseShift = hypeSetting(v, 'pitch') * base;
	let punchShift = baseShift + reset;
	// Excited, not shrieking: keep the punchline's top notes near the line's own top note.
	const allTop = percentile([...leadSemis, ...punchSemis], 95);
	const punchTop = percentile(punchSemis, 95);
	if (allTop !== null && punchTop !== null) {
		const ceilingHz = 2 ** (allTop / 12) * 2 ** (hypeSetting(v, 'ceiling') / 12);
		const topHz = 2 ** ((punchTop + punchShift) / 12);
		if (topHz > ceilingHz) punchShift = 12 * Math.log2(softCeiling(topHz, ceilingHz)) - punchTop;
	}
	const stretch = 1 + (hypeSetting(v, 'stretch') - 1) * peak;
	const leadOut = lead.length ? pitchShift(lead, baseShift, 1, sr) : lead;
	const punchOut = pitchShift(punch, punchShift, stretch, sr);
	const tailOut = tail.length ? pitchShift(tail, baseShift, 1, sr) : tail;
	const out = crossfadeConcat([leadOut, punchOut, tailOut], sr);
	const boost = hypeSetting(v, 'boost') * peak;
	if (!boost) return out;
	const tl = leadOut.length / sr;
	const tp = tl + punchOut.length / sr;
	const pts: [number, number][] = [
		[0, 0],
		[Math.max(tl - 0.25, 0), 0],
		[tl + 0.2, boost],
		[tp, boost]
	];
	if (tail.length) pts.push([tp + 0.25, 0]);
	return applyGainCurve(out, sr, pts.filter((p, n) => n === 0 || p[0] > pts[n - 1][0]));
}

export function radioEq(v: ResolvedVoice, energy: number, sr = SAMPLE_RATE) {
	const f = [biquad('highpass', 75, sr, 0.707), ...parseFfmpegEq(v.eq, sr)];
	if (energy > 0) f.push(biquad('peaking', 3500, sr, 1.5, 1.5 * energy));
	return f;
}

export async function renderLines(
	lines: DialogLine[],
	opts: RenderOptions,
	backend: SynthBackend
): Promise<RenderResult> {
	const sr = SAMPLE_RATE;
	const target = opts.loudnessLufs ?? -16;
	const fx = opts.fx !== false;
	const rules = compileRules(
		mergePronunciations(opts.pronunciations ?? [], opts.builtinPronunciations === false ? [] : undefined)
	);
	const speedMul = Math.min(Math.max(opts.speed || 1, 0.5), 2);
	const warnings: string[] = [];
	const pieces: Float32Array[] = [];
	const n = lines.length;
	if (!lines.some((l) => l.text?.trim())) throw new Error('Nothing to say');

	for (let i = 0; i < n; i++) {
		const line = lines[i];
		const { text, missing } = substitutePlaceholders((line.text ?? '').trim(), opts.placeholders);
		for (const m of missing) warnings.push(`line ${i + 1}: placeholder {${m}} has no value`);
		if (text) {
			const v = resolveVoice(line.voice, opts.voices);
			const energy = line.energy ?? v.defaultEnergy;
			const plan = planLine(v, energy, v.speed * speedMul);
			const speed = Math.min(Math.max(plan.speed, 0.5), 2);
			const parts: [string, string, string] = plan.hype ? findEmphasis(text) : [text.replace(/\*/g, ''), '', ''];
			const lang = v.lang === 'en-gb' ? 'b' : 'a';
			const ph = (await Promise.all(
				parts.map((p) => (p.trim() ? toPhonemes(p, rules, (s) => backend.phonemize(s, lang)) : Promise.resolve('')))
			)) as [string, string, string];
			const joined = ph.filter(Boolean).join(' ');
			if (!joined.trim()) throw new Error(`Line ${i + 1}: nothing to say`);
			let audio = await backend.synthesize(joined, v, speed, text.replace(/\*/g, ''));
			if (fx) {
				audio = plan.hype && ph[1] ? hypeEnergy(audio, v, ph, plan.base, energy, sr) : flatEnergy(audio, v, energy, sr);
				audio = applyBiquads(audio, radioEq(v, energy, sr));
				// hype lines sit a touch louder than the rest of the banter
				audio = normalizeLoudness(audio, sr, target + Math.min(energy, 1.5));
			}
			pieces.push(audio);
		}
		const gap = line.pauseMs > 0 ? line.pauseMs : i < n - 1 && text ? DEFAULT_GAP_MS : 0;
		if (gap) pieces.push(silence(gap / 1000, sr));
		opts.onProgress?.((i + 1) / n);
	}
	let mix = concat(pieces);
	if (fx) mix = limit(normalizeLoudness(mix, sr, target), sr, 0.84);
	const samples = concat([silence(LEAD_IN_S, sr), mix, silence(TAIL_S, sr)]);
	return { samples, sampleRate: sr, loudnessLufs: measureLufs(samples, sr), warnings };
}
