/**
 * Turns camera frames into metrics (`FrameMetric`), in a worker when the browser can
 * (OffscreenCanvas), else on the page. Results come back in frame order.
 */
import { FlashMetric, LUMA_H, LUMA_W, lumaFromRGBA, type FrameMetric } from './video-onset';

/** Downscale the current video frame to `w`×`h` luma on the page (also for mapping). */
export function grabLuma(
	video: HTMLVideoElement | HTMLCanvasElement | ImageBitmap,
	w = LUMA_W,
	h = LUMA_H,
	canvas?: HTMLCanvasElement
): Uint8Array | null {
	const c = canvas ?? document.createElement('canvas');
	c.width = w;
	c.height = h;
	const ctx = c.getContext('2d', { willReadFrequently: true });
	if (!ctx) return null;
	ctx.drawImage(video, 0, 0, w, h);
	return lumaFromRGBA(ctx.getImageData(0, 0, w, h).data);
}

export class FrameAnalyzer {
	private worker: Worker | null = null;
	private pending = new Map<number, (m: FrameMetric | null) => void>();
	private next = 0;
	private inline: FlashMetric | null = null;
	private canvas: HTMLCanvasElement | null = null;
	/** At most this many frames in flight; newer frames are dropped when the phone is slow. */
	private maxInFlight = 4;

	constructor(useWorker = true) {
		if (
			useWorker &&
			typeof Worker !== 'undefined' &&
			typeof OffscreenCanvas !== 'undefined' &&
			'createImageBitmap' in window
		) {
			try {
				this.worker = new Worker(new URL('./frames.worker.ts', import.meta.url), { type: 'module' });
				this.worker.onmessage = (e: MessageEvent<{ id: number; metric: FrameMetric }>) => {
					const cb = this.pending.get(e.data.id);
					this.pending.delete(e.data.id);
					cb?.(e.data.metric);
				};
				this.worker.onerror = () => this.fallBack();
			} catch {
				this.worker = null;
			}
		}
		if (!this.worker) this.fallBack();
	}

	private fallBack() {
		this.worker?.terminate();
		this.worker = null;
		for (const cb of this.pending.values()) cb(null);
		this.pending.clear();
		this.inline = new FlashMetric(LUMA_W, LUMA_H);
		this.canvas = document.createElement('canvas');
	}

	get usesWorker(): boolean {
		return this.worker !== null;
	}

	/** Metrics for the video's current frame (null if dropped). Call from the frame callback. */
	async analyze(video: HTMLVideoElement): Promise<FrameMetric | null> {
		if (this.worker) {
			if (this.pending.size >= this.maxInFlight) return null;
			let bitmap: ImageBitmap;
			try {
				bitmap = await createImageBitmap(video, {
					resizeWidth: LUMA_W,
					resizeHeight: LUMA_H,
					resizeQuality: 'low'
				});
			} catch {
				return null;
			}
			if (!this.worker) {
				bitmap.close();
				return null;
			}
			const id = this.next++;
			return new Promise((resolve) => {
				this.pending.set(id, resolve);
				this.worker!.postMessage({ id, bitmap }, [bitmap]);
			});
		}
		const luma = grabLuma(video, LUMA_W, LUMA_H, this.canvas ?? undefined);
		return luma && this.inline ? this.inline.push(luma) : null;
	}

	reset(): void {
		this.worker?.postMessage({ id: -1, reset: true });
		this.inline?.reset();
	}

	close(): void {
		this.worker?.terminate();
		this.worker = null;
		for (const cb of this.pending.values()) cb(null);
		this.pending.clear();
	}
}
