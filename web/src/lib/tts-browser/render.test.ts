import { describe, expect, it } from 'vitest';
import { measureLufs } from './dsp';
import { DEFAULT_GAP_MS, LEAD_IN_S, renderLines, substitutePlaceholders, TAIL_S, type SynthBackend } from './render';
import { blendStyleVectors, browserBlend, normalizeBlend, PRESET_VOICES, resolveVoice } from './voices';
import { splitPhonemes, styleOffset } from './chunk';

const SR = 24000;

/** 60 ms of a 150 Hz tone per phoneme character. */
function fakeBackend() {
	const calls: { phonemes: string; voice: string; speed: number; blend: Record<string, number> }[] = [];
	const backend: SynthBackend = {
		phonemize: async (t) => t.trim().toLowerCase(),
		synthesize: async (phonemes, voice, speed) => {
			calls.push({ phonemes, voice: voice.id, speed, blend: voice.blend });
			const n = Math.round((0.06 * phonemes.length * SR) / speed);
			return Float32Array.from({ length: n }, (_, i) => 0.3 * Math.sin((2 * Math.PI * 150 * i) / SR));
		}
	};
	return { backend, calls };
}

describe('voices', () => {
	it('normalizes and blends style vectors', () => {
		expect(normalizeBlend({ am_echo: 3, am_puck: 1, am_onyx: 0 })).toEqual({ am_echo: 0.75, am_puck: 0.25 });
		expect(() => normalizeBlend({ am_echo: 0 })).toThrow();
		const a = new Float32Array(510 * 256).fill(1);
		const b = new Float32Array(510 * 256).fill(3);
		const out = blendStyleVectors({ af_a: a, af_b: b }, { af_a: 1, af_b: 3 });
		expect(out[0]).toBeCloseTo(2.5);
		expect(out[out.length - 1]).toBeCloseTo(2.5);
	});

	it('resolves presets, aliases and base voices', () => {
		expect(resolveVoice('female').id).toBe('holly');
		expect(resolveVoice('nick').energy.maxLift).toBe(9);
		expect(resolveVoice('bf_emma').lang).toBe('en-gb');
		expect(resolveVoice({ id: 'x', blend: { af_sky: 1 } }).energy.lift).toBe(2.5); // default tuning
		expect(() => resolveVoice('rudolph')).toThrow();
		expect(PRESET_VOICES.map((v) => v.id)).toEqual(['nick', 'holly']);
	});

	it('drops non-English voices from browser blends', () => {
		expect(browserBlend({ af_heart: 1, jf_alpha: 1 })).toEqual({ blend: { af_heart: 1 }, dropped: ['jf_alpha'] });
		expect(browserBlend({ zf_xiaobei: 1 }).blend).toEqual({ af_heart: 1 });
	});

	it('picks the kokoro-js style row and splits long phonemes', () => {
		expect(styleOffset(2)).toBe(0);
		expect(styleOffset(12)).toBe(10 * 256);
		expect(styleOffset(9999)).toBe(509 * 256);
		const ph = Array.from({ length: 60 }, (_, i) => `wˈɜːd${i}${i % 10 === 9 ? '.' : ''}`).join(' ');
		const chunks = splitPhonemes(ph, 120);
		expect(chunks.every((c) => c.length <= 120)).toBe(true);
		expect(chunks.join(' ')).toBe(ph);
	});
});

describe('renderLines', () => {
	it('substitutes placeholders', () => {
		expect(substitutePlaceholders('Up next, {nextSong}! {showName}', { nextSong: 'Feliz Navidad' })).toEqual({
			text: 'Up next, Feliz Navidad! show Name',
			missing: ['showName']
		});
	});

	it('applies pauses, default gaps, speeds and energy defaults', async () => {
		const { backend, calls } = fakeBackend();
		const res = await renderLines(
			[
				{ voice: '', text: '', pauseMs: 400 },
				{ voice: 'nick', text: 'abcde', pauseMs: 0 },
				{ voice: 'holly', text: 'abcde', pauseMs: 1000, energy: 1.5 },
				{ voice: 'af_heart', text: 'abcde', pauseMs: 0 }
			],
			{ speed: 1, fx: false, builtinPronunciations: false },
			backend
		);
		expect(calls.map((c) => c.voice)).toEqual(['nick', 'holly', 'af_heart']);
		expect(calls[0].speed).toBeCloseTo(1.05); // Nick's hype speed setting is 1.0
		expect(calls[1].speed).toBeCloseTo(1.05 * 1.04); // Holly speeds up with her base energy
		const speech = calls.reduce((s, c) => s + Math.round((0.06 * c.phonemes.length * SR) / c.speed), 0);
		const expected = LEAD_IN_S + 0.4 + DEFAULT_GAP_MS / 1000 + 1.0 + speech / SR + TAIL_S;
		expect(res.samples.length / SR).toBeCloseTo(expected, 2);
		expect(res.sampleRate).toBe(SR);
	});

	it('splices pronunciations and reaches the loudness target with fx', async () => {
		const { backend, calls } = fakeBackend();
		const res = await renderLines(
			[
				{ voice: 'holly', text: 'Merry Christmas from TSO, *everybody!*', pauseMs: 0, energy: 1.5 },
				{ voice: { id: 'elf', blend: { af_sky: 2, af_bella: 2 } }, text: 'Hi {name}', pauseMs: 0 }
			],
			{
				speed: 1,
				pronunciations: [{ word: 'everybody', say: '/ˈɛvɹibˌɑdi/' }],
				placeholders: { name: 'Noel' },
				loudnessLufs: -18
			},
			backend
		);
		expect(calls[0].phonemes).toBe('merry christmas from t s o, ˈɛvɹibˌɑdi !');
		expect(calls[1].phonemes).toBe('hi noʊˈɛl'); // built-in Noel IPA
		expect(calls[1].blend).toEqual({ af_sky: 0.5, af_bella: 0.5 });
		expect(res.warnings).toEqual([]);
		expect(res.loudnessLufs).toBeCloseTo(-18, 0);
		expect(measureLufs(res.samples, SR)).toBeCloseTo(-18, 0);
		expect(Math.max(...res.samples.map(Math.abs))).toBeLessThanOrEqual(0.85);
	});

	it('refuses an empty dialog', async () => {
		const { backend } = fakeBackend();
		await expect(renderLines([{ voice: 'nick', text: '  ', pauseMs: 0 }], { speed: 1 }, backend)).rejects.toThrow();
	});
});
