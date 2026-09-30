/** Kokoro-82M base voices available for DJ voice blends. */
export const KOKORO_VOICES: { id: string; name: string; gender: 'male' | 'female'; accent: 'US' | 'UK' }[] = [
	{ id: 'af_heart', name: 'Heart', gender: 'female', accent: 'US' },
	{ id: 'af_bella', name: 'Bella', gender: 'female', accent: 'US' },
	{ id: 'af_nicole', name: 'Nicole', gender: 'female', accent: 'US' },
	{ id: 'af_sarah', name: 'Sarah', gender: 'female', accent: 'US' },
	{ id: 'af_sky', name: 'Sky', gender: 'female', accent: 'US' },
	{ id: 'af_nova', name: 'Nova', gender: 'female', accent: 'US' },
	{ id: 'af_river', name: 'River', gender: 'female', accent: 'US' },
	{ id: 'af_jessica', name: 'Jessica', gender: 'female', accent: 'US' },
	{ id: 'af_kore', name: 'Kore', gender: 'female', accent: 'US' },
	{ id: 'af_aoede', name: 'Aoede', gender: 'female', accent: 'US' },
	{ id: 'af_alloy', name: 'Alloy', gender: 'female', accent: 'US' },
	{ id: 'am_adam', name: 'Adam', gender: 'male', accent: 'US' },
	{ id: 'am_michael', name: 'Michael', gender: 'male', accent: 'US' },
	{ id: 'am_echo', name: 'Echo', gender: 'male', accent: 'US' },
	{ id: 'am_eric', name: 'Eric', gender: 'male', accent: 'US' },
	{ id: 'am_fenrir', name: 'Fenrir', gender: 'male', accent: 'US' },
	{ id: 'am_liam', name: 'Liam', gender: 'male', accent: 'US' },
	{ id: 'am_onyx', name: 'Onyx', gender: 'male', accent: 'US' },
	{ id: 'am_puck', name: 'Puck', gender: 'male', accent: 'US' },
	{ id: 'am_santa', name: 'Santa', gender: 'male', accent: 'US' },
	{ id: 'bf_emma', name: 'Emma', gender: 'female', accent: 'UK' },
	{ id: 'bf_isabella', name: 'Isabella', gender: 'female', accent: 'UK' },
	{ id: 'bf_alice', name: 'Alice', gender: 'female', accent: 'UK' },
	{ id: 'bf_lily', name: 'Lily', gender: 'female', accent: 'UK' },
	{ id: 'bm_george', name: 'George', gender: 'male', accent: 'UK' },
	{ id: 'bm_lewis', name: 'Lewis', gender: 'male', accent: 'UK' },
	{ id: 'bm_daniel', name: 'Daniel', gender: 'male', accent: 'UK' },
	{ id: 'bm_fable', name: 'Fable', gender: 'male', accent: 'UK' }
];

export const ENERGY_LEVELS = [
	{ value: 0, label: 'Calm' },
	{ value: 0.4, label: 'Normal' },
	{ value: 1, label: 'Hype' },
	{ value: 1.5, label: 'Extra hype' }
];

export const ENERGY_KEYS: { key: string; label: string; min: number; max: number; step: number; def: number; hint: string }[] = [
	{ key: 'pitch', label: 'Pitch lift', min: 0, max: 4, step: 0.1, def: 1.5, hint: 'Semitones the voice rises when hyped' },
	{ key: 'range', label: 'Intonation range', min: 0.5, max: 3, step: 0.05, def: 1.5, hint: 'How much the melody of the voice swings' },
	{ key: 'speed', label: 'Pace', min: 0.5, max: 1.5, step: 0.01, def: 1, hint: 'Speed multiplier at full energy' },
	{ key: 'stretch', label: 'Word stretch', min: 0.5, max: 1.5, step: 0.01, def: 1, hint: 'Drawn-out stressed words' },
	{ key: 'boost', label: 'Loudness boost', min: 0, max: 8, step: 0.5, def: 3, hint: 'dB added at full energy' },
	{ key: 'lift', label: 'Presence lift', min: 0, max: 6, step: 0.25, def: 2.5, hint: 'Brighter, more forward tone (dB)' },
	{ key: 'ceiling', label: 'Headroom', min: 0, max: 6, step: 0.25, def: 3, hint: 'Peak ceiling below full scale (dB)' },
	{ key: 'maxLift', label: 'Max lift', min: 0, max: 16, step: 0.5, def: 8, hint: 'Upper bound on presence lift (dB)' }
];

export function energyLabel(e: number | undefined): string {
	const v = e ?? 0.4;
	let best = ENERGY_LEVELS[0];
	for (const l of ENERGY_LEVELS) if (Math.abs(l.value - v) < Math.abs(best.value - v)) best = l;
	return best.label;
}
