// Voice catalog, Nick & Holly presets and blending math for in-browser Kokoro.
// Mirrors tts/pixelplus_tts/voices.py (keep the two in sync).

export interface DjVoiceObject {
	id: string;
	name?: string;
	description?: string;
	blend: Record<string, number>;
	speed?: number;
	lang?: string;
	eq?: string;
	defaultEnergy?: number;
	energy?: Record<string, number>;
}

/** A Kokoro base voice id ("af_heart"), a preset id/alias ("nick", "female"), or a DjVoice. */
export type DjVoiceLike = string | DjVoiceObject;

export interface BrowserVoice {
	id: string;
	name: string;
	language: string;
	gender: 'male' | 'female';
	grade?: string;
}

/** Energy tuning, snake-free internal names (maxLift). */
export interface EnergyTuning {
	pitch: number;
	range: number;
	speed: number;
	stretch: number;
	boost: number;
	lift: number;
	ceiling: number;
	maxLift: number;
}

export interface ResolvedVoice {
	id: string;
	name: string;
	blend: Record<string, number>; // normalized, sums to 1
	speed: number;
	lang: 'en-us' | 'en-gb';
	eq?: string;
	defaultEnergy: number;
	energy: Partial<EnergyTuning>;
}

// kokoro-js ships the English Kokoro v1.0 voices (grades from hexgrad's VOICES.md).
const EN_VOICES: [string, string, string][] = [
	['af_heart', 'Heart', 'A'],
	['af_alloy', 'Alloy', 'C'],
	['af_aoede', 'Aoede', 'C+'],
	['af_bella', 'Bella', 'A-'],
	['af_jessica', 'Jessica', 'D'],
	['af_kore', 'Kore', 'C+'],
	['af_nicole', 'Nicole', 'B-'],
	['af_nova', 'Nova', 'C'],
	['af_river', 'River', 'D'],
	['af_sarah', 'Sarah', 'C+'],
	['af_sky', 'Sky', 'C-'],
	['am_adam', 'Adam', 'F+'],
	['am_echo', 'Echo', 'D'],
	['am_eric', 'Eric', 'D'],
	['am_fenrir', 'Fenrir', 'C+'],
	['am_liam', 'Liam', 'D'],
	['am_michael', 'Michael', 'C+'],
	['am_onyx', 'Onyx', 'D'],
	['am_puck', 'Puck', 'C+'],
	['am_santa', 'Santa', 'D-'],
	['bf_alice', 'Alice', 'D'],
	['bf_emma', 'Emma', 'B-'],
	['bf_isabella', 'Isabella', 'C'],
	['bf_lily', 'Lily', 'D'],
	['bm_daniel', 'Daniel', 'D'],
	['bm_fable', 'Fable', 'C'],
	['bm_george', 'George', 'C'],
	['bm_lewis', 'Lewis', 'D+']
];

export const BROWSER_VOICES: readonly BrowserVoice[] = EN_VOICES.map(([id, name, grade]) => ({
	id,
	name,
	language: id[0] === 'b' ? 'en-gb' : 'en-us',
	gender: id[1] === 'f' ? 'female' : 'male',
	grade
}));
const BROWSER_IDS = new Set(BROWSER_VOICES.map((v) => v.id));

/** What fpp-voices used when a key is missing from a voice's energy block. */
export const FALLBACK_ENERGY: EnergyTuning = {
	pitch: 0,
	range: 1,
	speed: 1,
	stretch: 1,
	boost: 0,
	lift: 0,
	ceiling: 3,
	maxLift: 12
};
/** Neutral DJ tuning for base voices / blends without their own energy block. */
export const DEFAULT_ENERGY: EnergyTuning = {
	pitch: 1.5,
	range: 1.5,
	speed: 1,
	stretch: 1,
	boost: 3,
	lift: 2.5,
	ceiling: 3,
	maxLift: 8
};

/** Nick & Holly, as DjVoice JSON (from tlchandler/fpp-voices voices.json). */
export interface PresetVoice extends DjVoiceObject {
	name: string;
	description: string;
	speed: number;
	lang: string;
	defaultEnergy: number;
	energy: Record<string, number>;
}

export const PRESET_VOICES: readonly PresetVoice[] = [
	{
		id: 'nick',
		name: 'Nick',
		description: 'Male DJ - warm, upbeat, classic radio baritone',
		blend: { am_echo: 0.3, am_fenrir: 0.3, am_puck: 0.4 },
		speed: 1.05,
		lang: 'en-us',
		eq: 'equalizer=f=150:t=q:w=1.0:g=2.5,equalizer=f=3200:t=q:w=1.2:g=2',
		defaultEnergy: 0.4,
		energy: { pitch: 1.5, range: 1.5, speed: 1.0, stretch: 1.0, boost: 3, lift: 3, ceiling: 3, maxLift: 9 }
	},
	{
		id: 'holly',
		name: 'Holly',
		description: 'Female DJ - bright, friendly, energetic',
		blend: { af_heart: 0.5, af_kore: 0.5 },
		speed: 1.05,
		lang: 'en-us',
		eq: 'equalizer=f=220:t=q:w=1.0:g=1.5,equalizer=f=4000:t=q:w=1.2:g=2',
		defaultEnergy: 0.4,
		energy: { pitch: 2, range: 1.5, speed: 1.1, stretch: 1.05, boost: 3, lift: 2.5, ceiling: 3, maxLift: 8 }
	}
];
const ALIASES: Record<string, string> = { male: 'nick', m: 'nick', he: 'nick', female: 'holly', f: 'holly', she: 'holly' };

/** Drop non-positive weights and scale the rest to sum to 1. */
export function normalizeBlend(blend: Record<string, number>): Record<string, number> {
	const out: Record<string, number> = {};
	let total = 0;
	for (const [k, w] of Object.entries(blend)) {
		if (typeof w === 'number' && Number.isFinite(w) && w > 0) {
			out[k] = (out[k] ?? 0) + w;
			total += w;
		}
	}
	if (total <= 0) throw new Error('A voice blend needs at least one voice with a positive weight');
	for (const k of Object.keys(out)) out[k] /= total;
	return out;
}

/**
 * Keep only voices kokoro-js can load in the browser (English). Non-English parts are dropped
 * and the rest renormalized; if nothing is left the blend falls back to af_heart.
 */
export function browserBlend(blend: Record<string, number>): { blend: Record<string, number>; dropped: string[] } {
	const norm = normalizeBlend(blend);
	const dropped = Object.keys(norm).filter((k) => !BROWSER_IDS.has(k));
	const kept = Object.fromEntries(Object.entries(norm).filter(([k]) => BROWSER_IDS.has(k)));
	return { blend: Object.keys(kept).length ? normalizeBlend(kept) : { af_heart: 1 }, dropped };
}

/** The heaviest voice in a blend (used when style blending is unavailable). */
export function dominantVoice(blend: Record<string, number>): string {
	return Object.entries(blend).sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))[0][0];
}

/** Weighted average of Kokoro style tensors (each 510×256 float32, flattened). */
export function blendStyleVectors(
	vectors: Record<string, Float32Array>,
	weights: Record<string, number>
): Float32Array {
	const w = normalizeBlend(weights);
	let out: Float32Array | null = null;
	for (const [id, wt] of Object.entries(w)) {
		const v = vectors[id];
		if (!v) throw new Error(`missing style vector for ${id}`);
		if (!out) out = new Float32Array(v.length);
		else if (out.length !== v.length) throw new Error('style vectors differ in size');
		for (let i = 0; i < v.length; i++) out[i] += v[i] * wt;
	}
	return out as Float32Array;
}

function energyOf(e: Record<string, number> | undefined): Partial<EnergyTuning> {
	if (!e || !Object.keys(e).length) return { ...DEFAULT_ENERGY };
	const out: Partial<EnergyTuning> = {};
	for (const [k, v] of Object.entries(e)) {
		const key = (k === 'max_lift' ? 'maxLift' : k) as keyof EnergyTuning;
		if (key in FALLBACK_ENERGY && typeof v === 'number') out[key] = v;
	}
	return out;
}

/** Voice reference -> resolved voice (presets, aliases, custom voices, base ids, objects). */
export function resolveVoice(ref: DjVoiceLike, custom: DjVoiceObject[] = []): ResolvedVoice {
	let v: DjVoiceObject | undefined;
	if (typeof ref === 'string') {
		const key = ref.trim().toLowerCase();
		v =
			custom.find((c) => c.id.toLowerCase() === key || (c.name ?? '').toLowerCase() === key) ??
			PRESET_VOICES.find((p) => p.id === (ALIASES[key] ?? key) || p.name.toLowerCase() === key);
		if (!v && /^[a-z]{2}_[a-z]+$/.test(key)) {
			const info = BROWSER_VOICES.find((b) => b.id === key);
			v = { id: key, name: info?.name ?? key, blend: { [key]: 1 }, speed: 1 };
		}
		if (!v) throw new Error(`Unknown voice "${ref}"`);
	} else {
		v = ref;
	}
	const blend = normalizeBlend(v.blend);
	const first = dominantVoice(blend);
	const lang = (v.lang ?? (first[0] === 'b' ? 'en-gb' : 'en-us')) === 'en-gb' ? 'en-gb' : 'en-us';
	return {
		id: v.id,
		name: v.name ?? v.id,
		blend,
		speed: v.speed ?? 1,
		lang,
		eq: v.eq,
		defaultEnergy: v.defaultEnergy ?? 0.4,
		energy: energyOf(v.energy)
	};
}

export function hypeSetting(v: Pick<ResolvedVoice, 'energy'>, key: keyof EnergyTuning): number {
	return v.energy[key] ?? FALLBACK_ENERGY[key];
}
