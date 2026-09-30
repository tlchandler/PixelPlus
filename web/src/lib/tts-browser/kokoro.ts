// Lazy kokoro-js backend: loads the model on first use (dynamic import -> separate chunk),
// WebGPU (fp32) when available, else WASM (q8). Blends voices by averaging style vectors.

import type { SynthBackend } from './render';
import { phonemize } from './phonemize';
import { splitPhonemes, STYLE_DIM, STYLE_ROWS, styleOffset } from './chunk';
import { blendStyleVectors, dominantVoice, type ResolvedVoice } from './voices';

export const MODEL_ID = 'onnx-community/Kokoro-82M-v1.0-ONNX';
const HF_VOICES = `https://huggingface.co/${MODEL_ID}/resolve/main/voices`;

export interface BrowserTtsConfig {
	/** Force a device; default: webgpu if an adapter is available, else wasm. */
	device?: 'webgpu' | 'wasm';
	/** Default: fp32 on WebGPU, q8 on WASM. */
	dtype?: 'fp32' | 'fp16' | 'q8' | 'q4' | 'q4f16';
	/** Where voice .bin files are fetched from (default: Hugging Face). */
	voicesBaseUrl?: string;
	/** Self-hosted model mirror, e.g. '/api/v1/tts/browser-model/' laid out like huggingface.co. */
	modelHost?: string;
	/** onnxruntime-web .wasm location (default: jsDelivr CDN via transformers.js). */
	wasmPaths?: string;
}

const config: BrowserTtsConfig = {};

export function configureBrowserTts(c: BrowserTtsConfig): void {
	Object.assign(config, c);
	loaded = null; // reload with the new settings on next use
}

// Minimal structural types for what we use from kokoro-js / transformers.js.
interface TensorLike {
	dims: number[];
}
type TensorCtor = new (type: string, data: Float32Array | number[], dims: number[]) => unknown;
interface KokoroLike {
	model: (inputs: Record<string, unknown>) => Promise<{ waveform: { data: Float32Array } }>;
	tokenizer: (text: string, opts: { truncation: boolean }) => { input_ids: TensorLike };
	generate: (
		text: string,
		opts: { voice: string; speed: number }
	) => Promise<{ audio: Float32Array; sampling_rate: number }>;
}
interface ProgressInfo {
	status: string;
	file?: string;
	loaded?: number;
	total?: number;
}

let loaded: Promise<{ tts: KokoroLike; device: string; dtype: string }> | null = null;

export async function hasWebGpu(): Promise<boolean> {
	try {
		const gpu = (globalThis.navigator as Navigator & { gpu?: { requestAdapter(): Promise<unknown> } })?.gpu;
		return !!gpu && !!(await gpu.requestAdapter());
	} catch {
		return false;
	}
}

export function loadKokoro(onProgress?: (p: number) => void) {
	loaded ??= (async () => {
		const mod = await import('kokoro-js');
		if (config.wasmPaths) mod.env.wasmPaths = config.wasmPaths;
		if (config.modelHost) {
			const t = await import('@huggingface/transformers');
			t.env.remoteHost = config.modelHost;
			t.env.allowLocalModels = false;
		}
		const files = new Map<string, [number, number]>();
		const progress_callback = (info: ProgressInfo) => {
			if (info.status === 'progress' && info.file && info.total) {
				files.set(info.file, [info.loaded ?? 0, info.total]);
				let a = 0,
					b = 0;
				for (const [l, t] of files.values()) {
					a += l;
					b += t;
				}
				onProgress?.(b ? a / b : 0);
			}
		};
		const device = config.device ?? ((await hasWebGpu()) ? 'webgpu' : 'wasm');
		const dtype = config.dtype ?? (device === 'webgpu' ? 'fp32' : 'q8');
		try {
			const tts = await mod.KokoroTTS.from_pretrained(MODEL_ID, { dtype, device, progress_callback });
			return { tts: tts as unknown as KokoroLike, device, dtype };
		} catch (e) {
			if (device !== 'webgpu') throw e;
			console.warn('Kokoro on WebGPU failed, falling back to WASM', e);
			const tts = await mod.KokoroTTS.from_pretrained(MODEL_ID, {
				dtype: 'q8',
				device: 'wasm',
				progress_callback
			});
			return { tts: tts as unknown as KokoroLike, device: 'wasm', dtype: 'q8' };
		}
	})();
	loaded.catch(() => (loaded = null));
	return loaded;
}

// ------------------------------------------------------------ voice vectors ----

const styles = new Map<string, Promise<Float32Array>>();

async function fetchVoice(id: string): Promise<Float32Array> {
	const url = `${config.voicesBaseUrl ?? HF_VOICES}/${id}.bin`;
	let cache: Cache | undefined;
	try {
		cache = await caches.open('kokoro-voices'); // shared with kokoro-js; absent on plain http
		const hit = await cache.match(url);
		if (hit) return new Float32Array(await hit.arrayBuffer());
	} catch {
		cache = undefined;
	}
	const res = await fetch(url);
	if (!res.ok) throw new Error(`Could not load voice ${id} (${res.status})`);
	const buf = await res.arrayBuffer();
	if (cache) cache.put(url, new Response(buf.slice(0))).catch(() => undefined);
	const v = new Float32Array(buf);
	if (v.length !== STYLE_ROWS * STYLE_DIM) throw new Error(`voice ${id} has an unexpected size`);
	return v;
}

export function voiceVector(id: string): Promise<Float32Array> {
	let p = styles.get(id);
	if (!p) {
		p = fetchVoice(id);
		styles.set(id, p);
		p.catch(() => styles.delete(id));
	}
	return p;
}

export async function blendedStyle(blend: Record<string, number>): Promise<Float32Array> {
	const ids = Object.keys(blend);
	const vecs = await Promise.all(ids.map(voiceVector));
	return blendStyleVectors(Object.fromEntries(ids.map((id, i) => [id, vecs[i]])), blend);
}

function trimSilence(a: Float32Array, threshold = 0.01, padS = 0.05, sr = 24000): Float32Array {
	let s = 0;
	let e = a.length;
	while (s < e && Math.abs(a[s]) < threshold) s++;
	while (e > s && Math.abs(a[e - 1]) < threshold) e--;
	const pad = Math.round(padS * sr);
	return a.subarray(Math.max(0, s - pad), Math.min(a.length, e + pad));
}

let warnedFallback = false;

export const kokoroBackend: SynthBackend = {
	phonemize: (text, lang) => phonemize(text, lang),
	async synthesize(
		phonemes: string,
		voice: ResolvedVoice,
		speed: number,
		text: string
	): Promise<Float32Array> {
		const { tts } = await loadKokoro();
		try {
			const style = await blendedStyle(voice.blend);
			const chunks = splitPhonemes(phonemes);
			const parts: Float32Array[] = [];
			for (const chunk of chunks) {
				const { input_ids } = tts.tokenizer(chunk, { truncation: true });
				const Tensor = (input_ids as object).constructor as TensorCtor;
				const off = styleOffset(input_ids.dims[input_ids.dims.length - 1]);
				const { waveform } = await tts.model({
					input_ids,
					style: new Tensor('float32', style.slice(off, off + STYLE_DIM), [1, STYLE_DIM]),
					speed: new Tensor('float32', [speed], [1])
				});
				parts.push(chunks.length > 1 ? trimSilence(waveform.data) : waveform.data);
			}
			if (parts.length === 1) return parts[0];
			const gap = new Float32Array(Math.round(0.2 * 24000));
			const out = new Float32Array(parts.reduce((n, p) => n + p.length + gap.length, 0));
			let o = 0;
			for (const p of parts) {
				out.set(p, o);
				o += p.length + gap.length;
			}
			return out;
		} catch (e) {
			// Should the model internals change, fall back to the public API: dominant voice only,
			// phonemized by kokoro-js from the plain text (IPA pronunciation overrides are lost).
			if (!warnedFallback) console.warn('Blended synthesis failed; using the dominant voice', e);
			warnedFallback = true;
			const audio = await tts.generate(text, { voice: dominantVoice(voice.blend), speed });
			return audio.audio;
		}
	}
};
