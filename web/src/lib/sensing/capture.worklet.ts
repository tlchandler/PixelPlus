/**
 * AudioWorklet that forwards raw microphone samples to the page in ~43 ms chunks, each tagged
 * with the audio-context frame index of its first sample. Loaded with
 * `import url from './capture.worklet.ts?worker&url'` and `audioWorklet.addModule(url)`.
 */

// The worklet global scope (not in lib.dom).
declare const currentFrame: number;
declare class AudioWorkletProcessor {
	readonly port: MessagePort;
}
declare function registerProcessor(name: string, ctor: unknown): void;

const CHUNK = 2048;

class CaptureProcessor extends AudioWorkletProcessor {
	private buf = new Float32Array(CHUNK);
	private len = 0;
	private start = 0;

	process(inputs: Float32Array[][]): boolean {
		const ch = inputs[0]?.[0];
		if (!ch) return true;
		if (this.len === 0) this.start = currentFrame;
		let i = 0;
		while (i < ch.length) {
			const n = Math.min(ch.length - i, CHUNK - this.len);
			this.buf.set(ch.subarray(i, i + n), this.len);
			this.len += n;
			i += n;
			if (this.len === CHUNK) {
				const out = this.buf;
				this.port.postMessage({ frame: this.start, samples: out }, [out.buffer]);
				this.buf = new Float32Array(CHUNK);
				this.len = 0;
				this.start = currentFrame + i;
			}
		}
		return true;
	}
}

registerProcessor('pp-capture', CaptureProcessor);

export {};
