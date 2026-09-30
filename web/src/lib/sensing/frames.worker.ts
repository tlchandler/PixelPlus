/// <reference lib="webworker" />
/**
 * Frame analysis off the main thread: downscaled camera frames (ImageBitmap) in, flash /
 * motion metrics out (see `video-onset.ts`).
 */
import { FlashMetric, LUMA_H, LUMA_W, lumaFromRGBA } from './video-onset';

declare const self: DedicatedWorkerGlobalScope;

const metric = new FlashMetric(LUMA_W, LUMA_H);
const canvas = new OffscreenCanvas(LUMA_W, LUMA_H);
const ctx = canvas.getContext('2d', { willReadFrequently: true }) as OffscreenCanvasRenderingContext2D;
const luma = new Uint8Array(LUMA_W * LUMA_H);

self.onmessage = (e: MessageEvent<{ id: number; bitmap?: ImageBitmap; reset?: boolean }>) => {
	const { id, bitmap, reset } = e.data;
	if (reset) {
		metric.reset();
		return;
	}
	if (!bitmap) return;
	ctx.drawImage(bitmap, 0, 0, LUMA_W, LUMA_H);
	bitmap.close();
	const img = ctx.getImageData(0, 0, LUMA_W, LUMA_H);
	lumaFromRGBA(img.data, luma);
	self.postMessage({ id, metric: metric.push(luma) });
};
