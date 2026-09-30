// Records a mapping run from the phone camera (F6/F7, WS4) on top of WS1's
// CameraCapture: max-pooled luma frames with capture timestamps, a live
// "what blinks" heat map, and the camera locks that keep levels stable.
import type { CameraCapture } from '$lib/sensing/camera';
import { decode, type DecodeOptions, type DecodeResult, type Frame, type Recording } from './decode';
import type { Plan } from './mapcode';
import { maxPoolLuma, poolSize } from './pool';

/** Most frames kept (≈ 45 s at 30 fps, ~2.6 MB per second at 320×180). */
const MAX_FRAMES = 1400;

export class MapRecorder {
	readonly w: number;
	readonly h: number;
	private frames: Frame[] = [];
	private unsub: (() => void) | null = null;
	private canvas: HTMLCanvasElement;
	private ctx: CanvasRenderingContext2D | null;
	private sw: number;
	private sh: number;
	private min: Uint8Array;
	private max: Uint8Array;
	private lastT = -Infinity;
	dropped = 0;

	constructor(
		private cam: CameraCapture,
		width = 320
	) {
		const v = cam.video;
		const { w, h } = poolSize(v.videoWidth, v.videoHeight, width);
		this.w = w;
		this.h = h;
		// Read the video at 3× the decoder size, then max-pool 3×3.
		this.sw = w * 3;
		this.sh = h * 3;
		this.canvas = document.createElement('canvas');
		this.canvas.width = this.sw;
		this.canvas.height = this.sh;
		this.ctx = this.canvas.getContext('2d', { willReadFrequently: true });
		this.min = new Uint8Array(w * h).fill(255);
		this.max = new Uint8Array(w * h);
	}

	start(): void {
		this.frames = [];
		this.min.fill(255);
		this.max.fill(0);
		this.unsub = this.cam.onFrame((f) => {
			if (!this.ctx) return;
			// Faster than 40 fps adds nothing but memory.
			if (f.t - this.lastT < 24) return;
			if (this.frames.length >= MAX_FRAMES) {
				this.dropped++;
				return;
			}
			this.lastT = f.t;
			this.ctx.drawImage(this.cam.video, 0, 0, this.sw, this.sh);
			const data = maxPoolLuma(
				this.ctx.getImageData(0, 0, this.sw, this.sh).data,
				this.sw,
				this.sh,
				this.w,
				this.h
			);
			for (let i = 0; i < data.length; i++) {
				const v = data[i];
				if (v < this.min[i]) this.min[i] = v;
				if (v > this.max[i]) this.max[i] = v;
			}
			this.frames.push({ t: f.t, data });
		});
	}

	get count(): number {
		return this.frames.length;
	}

	/** Brightness swing per camera pixel so far (0..255), for the live overlay. */
	swing(): Uint8Array {
		const o = new Uint8Array(this.w * this.h);
		for (let i = 0; i < o.length; i++) o[i] = this.max[i] > this.min[i] ? this.max[i] - this.min[i] : 0;
		return o;
	}

	stop(startHintMs?: number): Recording {
		this.unsub?.();
		this.unsub = null;
		return { width: this.w, height: this.h, frames: this.frames, startHintMs, hintWindowMs: 2500 };
	}
}

export interface LockResult {
	exposure: boolean;
	focus: boolean;
	whiteBalance: boolean;
	note?: string;
}

type Caps = MediaTrackCapabilities & { focusMode?: string[]; whiteBalanceMode?: string[] };

/** Lock exposure (WS1), focus and white balance where the phone allows it. */
export async function lockCamera(cam: CameraCapture): Promise<LockResult> {
	const exp = await cam.lockExposure();
	const caps = (cam.track.getCapabilities?.() ?? {}) as Caps;
	const tryLock = async (name: 'focusMode' | 'whiteBalanceMode') => {
		if (!caps[name]?.includes('manual')) return false;
		try {
			await cam.track.applyConstraints({ advanced: [{ [name]: 'manual' } as MediaTrackConstraintSet] });
			return true;
		} catch {
			return false;
		}
	};
	const focus = await tryLock('focusMode');
	const whiteBalance = await tryLock('whiteBalanceMode');
	return { exposure: exp.locked, focus, whiteBalance, note: exp.reason };
}

/** A JPEG of the current camera picture (for the review screen and the layout background). */
export async function snapshot(video: HTMLVideoElement, maxW = 1600): Promise<Blob | null> {
	const vw = video.videoWidth,
		vh = video.videoHeight;
	if (!vw || !vh) return null;
	const s = Math.min(1, maxW / vw);
	const c = document.createElement('canvas');
	c.width = Math.round(vw * s);
	c.height = Math.round(vh * s);
	c.getContext('2d')?.drawImage(video, 0, 0, c.width, c.height);
	for (const q of [0.82, 0.7, 0.55]) {
		const b = await new Promise<Blob | null>((r) => c.toBlob(r, 'image/jpeg', q));
		if (!b || b.size <= 1_900_000) return b;
	}
	return null;
}

/** Decode in a worker (falls back to this thread). */
export function decodeAsync(rec: Recording, plan: Plan, opts?: DecodeOptions): Promise<DecodeResult> {
	return new Promise((resolve, reject) => {
		let worker: Worker | null = null;
		try {
			worker = new Worker(new URL('./decode.worker.ts', import.meta.url), { type: 'module' });
		} catch {
			worker = null;
		}
		const inline = () => {
			try {
				resolve(decode(rec, plan, opts));
			} catch (e) {
				reject(e);
			}
		};
		if (!worker) return inline();
		worker.onmessage = (e: MessageEvent<{ res?: DecodeResult; error?: string }>) => {
			worker?.terminate();
			if (e.data.res) resolve(e.data.res);
			else reject(new Error(e.data.error ?? 'decode failed'));
		};
		worker.onerror = () => {
			worker?.terminate();
			inline();
		};
		// Frames are copied (not transferred) so a fallback can still use them.
		worker.postMessage({ rec, plan, opts });
	});
}

/** Pick the bit length for the camera's frame rate (≥ 3 clean frames per bit). */
export function bitMsFor(fps: number): number {
	if (fps >= 20) return 200;
	if (fps >= 13) return 250;
	return 320;
}
