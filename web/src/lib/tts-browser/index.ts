// In-browser DJ voice rendering (browser mode: Pi Zero 2 W / Pi 3, or any device without the
// TTS sidecar). kokoro-js + transformers.js are loaded lazily via dynamic import, so they live
// in their own chunk and cost nothing until the first render.
//
// Semantics match the device renderer (tts/pixelplus_tts): same voices, blends, pronunciation
// rules, script format, pauses and energy planning; the audio processing is a lighter
// approximation (see render.ts). Output: 16-bit mono WAV at 24 kHz, -16 LUFS by default.

import { kokoroBackend, loadKokoro } from './kokoro';
import { renderLines, type DialogLine, type RenderOptions } from './render';
import { BROWSER_VOICES, browserBlend, resolveVoice, type DjVoiceLike } from './voices';
import { wavBlob } from './wav';

export type { DjVoiceLike, DjVoiceObject, BrowserVoice, PresetVoice } from './voices';
export type { DialogLine, RenderOptions } from './render';
export type { Pronunciation } from './pronounce';
export { PRESET_VOICES, BROWSER_VOICES, normalizeBlend } from './voices';
export { parseScript, formatScript, ScriptError, type ScriptLine } from './script';
export { configureBrowserTts, type BrowserTtsConfig } from './kokoro';
export { encodeWav } from './wav';

// WebAssembly SIMD probe (onnxruntime-web needs SIMD); bytes from wasm-feature-detect.
const SIMD_PROBE = new Uint8Array([
	0, 97, 115, 109, 1, 0, 0, 0, 1, 5, 1, 96, 0, 1, 123, 3, 2, 1, 0, 10, 10, 1, 8, 0, 65, 0, 253, 15, 253, 98,
	11
]);

/** True when this browser can run Kokoro (WebAssembly with SIMD, fetch; WebGPU optional). */
export async function isBrowserTtsSupported(): Promise<boolean> {
	try {
		if (typeof window === 'undefined' || typeof WebAssembly !== 'object' || typeof fetch !== 'function')
			return false;
		return WebAssembly.validate(SIMD_PROBE);
	} catch {
		return false;
	}
}

/** The Kokoro voices available in the browser (English; kokoro-js has no other languages). */
export async function listBrowserVoices(): Promise<
	{ id: string; name: string; language: string; gender: 'male' | 'female' }[]
> {
	return BROWSER_VOICES.map(({ id, name, language, gender }) => ({ id, name, language, gender }));
}

/**
 * Render a multi-voice dialog to a WAV blob.
 *  - voice: Kokoro id, preset id/alias ("nick", "holly", "female"), or a DjVoice (blend of voices)
 *  - pauseMs: silence after the line (0 = default 350 ms gap between lines)
 *  - energy: 0 calm, 0.4 normal DJ (voice default), 1 hype, 1.5 extra hype; *asterisks* mark the punchline
 * onProgress goes 0..1 (model download/load counts as the first 30% when not yet loaded).
 */
export async function renderDialog(
	lines: { voice: DjVoiceLike; text: string; pauseMs: number; energy?: number }[],
	opts: {
		speed: number;
		pronunciations?: { word: string; say: string }[];
		onProgress?: (p: number) => void;
	} & Omit<RenderOptions, 'speed' | 'pronunciations' | 'onProgress'>
): Promise<Blob /* audio/wav */> {
	const report = opts.onProgress ?? (() => undefined);
	let loadDone = false;
	const loading = loadKokoro((p) => {
		if (!loadDone) report(0.3 * p);
	});
	const { device } = await loading;
	loadDone = true;
	report(0.3);
	// kokoro-js only has English voices: drop others from blends (with a warning).
	const prepared: DialogLine[] = lines.map((l, i) => {
		if (!l.text?.trim()) return l;
		const v = resolveVoice(l.voice, opts.voices);
		const { blend, dropped } = browserBlend(v.blend);
		if (dropped.length) console.warn(`line ${i + 1}: ${dropped.join(', ')} not available in the browser`);
		return { ...l, voice: { ...v, blend, energy: v.energy as Record<string, number> } };
	});
	const res = await renderLines(
		prepared,
		{ ...opts, onProgress: (p) => report(0.3 + 0.7 * p) },
		kokoroBackend
	);
	for (const w of res.warnings) console.warn(w);
	console.debug(`[tts-browser] rendered ${(res.samples.length / res.sampleRate).toFixed(1)} s on ${device}`);
	report(1);
	return wavBlob(res.samples, res.sampleRate);
}
