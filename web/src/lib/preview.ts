// Shared live-preview hub: one WebSocket subscription, one rAF loop, many canvases.
import { app } from '$lib/stores/app.svelte';

type Drawer = (frame: Uint8Array, offsets: Map<string, number>) => void;

const drawers = new Set<Drawer>();
let latest: Uint8Array | null = null;
let dirty = false;
let raf = 0;
let unsub: (() => void) | null = null;
let offsets = new Map<string, number>();
let offsetsFor: unknown = null;

function loop() {
	raf = requestAnimationFrame(loop);
	if (!dirty || !latest) return;
	dirty = false;
	if (offsetsFor !== app.show) {
		offsets = app.previewOffsets();
		offsetsFor = app.show;
	}
	for (const d of drawers) d(latest, offsets);
}

export function onPreview(d: Drawer, fps = 20): () => void {
	drawers.add(d);
	if (!unsub) {
		unsub = app.subscribePreview((rgb) => {
			latest = rgb;
			dirty = true;
		}, fps);
		raf = requestAnimationFrame(loop);
	}
	if (latest) queueMicrotask(() => latest && d(latest, offsets));
	return () => {
		drawers.delete(d);
		if (!drawers.size && unsub) {
			unsub();
			unsub = null;
			cancelAnimationFrame(raf);
		}
	};
}

/** Draw pixels as glowing dots. `pts` are normalized box coords; box in canvas px. */
export function drawPixels(
	ctx: CanvasRenderingContext2D,
	pts: Float32Array,
	rgb: Uint8Array | Uint8ClampedArray,
	off: number,
	n: number,
	box: { x: number; y: number; w: number; h: number },
	dot: number,
	glow = true
) {
	const r = Math.max(0.6, dot / 2);
	if (glow) {
		ctx.globalCompositeOperation = 'lighter';
		ctx.globalAlpha = 0.22;
		const gr = r * 3;
		for (let i = 0; i < n; i++) {
			const j = off + i * 3;
			const R = rgb[j],
				G = rgb[j + 1],
				B = rgb[j + 2];
			if (R + G + B < 30) continue;
			ctx.fillStyle = `rgb(${R},${G},${B})`;
			const x = box.x + pts[i * 2] * box.w;
			const y = box.y + pts[i * 2 + 1] * box.h;
			ctx.beginPath();
			ctx.arc(x, y, gr, 0, 6.283);
			ctx.fill();
		}
		ctx.globalAlpha = 1;
		ctx.globalCompositeOperation = 'source-over';
	}
	for (let i = 0; i < n; i++) {
		const j = off + i * 3;
		const R = rgb[j],
			G = rgb[j + 1],
			B = rgb[j + 2];
		const x = box.x + pts[i * 2] * box.w;
		const y = box.y + pts[i * 2 + 1] * box.h;
		ctx.fillStyle = R + G + B < 12 ? 'rgba(255,255,255,0.07)' : `rgb(${R},${G},${B})`;
		if (r < 1.5) ctx.fillRect(x - r, y - r, r * 2, r * 2);
		else {
			ctx.beginPath();
			ctx.arc(x, y, r, 0, 6.283);
			ctx.fill();
		}
	}
}

/** Fit a w×h aspect box into a canvas with padding. */
export function fitBox(cw: number, ch: number, aw: number, ah: number, pad = 8) {
	const s = Math.min((cw - pad * 2) / Math.max(1, aw), (ch - pad * 2) / Math.max(1, ah));
	const w = aw * s,
		h = ah * s;
	return { x: (cw - w) / 2, y: (ch - h) / 2, w, h };
}
