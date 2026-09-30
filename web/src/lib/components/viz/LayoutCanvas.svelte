<script lang="ts">
	import type { Prop } from '$lib/api/types';
	import { propPoints, worldBounds } from '$lib/util/geometry';
	import { drawPixels } from '$lib/preview';
	import { liveSource, type FrameSource, type PropSlot } from '$lib/preview/source';

	let {
		props,
		edit = false,
		labels = false,
		editLabels = true,
		selected = $bindable<string | null>(null),
		highlight = [],
		onmove,
		height = '100%',
		source = liveSource
	}: {
		props: Prop[];
		edit?: boolean;
		labels?: boolean;
		/** Arrange mode labels every prop; off on small touch screens, where they'd overlap. */
		editLabels?: boolean;
		selected?: string | null;
		highlight?: string[];
		onmove?: (id: string, layout: { x: number; y: number }) => void;
		height?: string;
		/** Where frames come from: the real lights (default) or a local sequence preview (F3). */
		source?: FrameSource;
	} = $props();

	let canvas: HTMLCanvasElement | undefined = $state();
	let view = { x: 0, y: 0, s: 1 };
	let fitted = false;
	let lastRgb: Uint8Array | null = null;
	let lastSlots = new Map<string, PropSlot>();
	/** Point subsets for subsampled props (preview files keep ≤ 300 pixels a prop). */
	const subsets = new Map<string, { idx: Uint32Array; base: Float32Array; pts: Float32Array }>();

	function subsetPoints(id: string, base: Float32Array, idx: Uint32Array): Float32Array {
		const c = subsets.get(id);
		if (c && c.idx === idx && c.base === base) return c.pts;
		const pts = new Float32Array(idx.length * 2);
		for (let k = 0; k < idx.length; k++) {
			pts[k * 2] = base[idx[k] * 2] ?? 0;
			pts[k * 2 + 1] = base[idx[k] * 2 + 1] ?? 0;
		}
		subsets.set(id, { idx, base, pts });
		return pts;
	}
	let hover: string | null = $state(null);
	const moved = new Map<string, { x: number; y: number }>();

	export function fit() {
		if (!canvas) return;
		const b = worldBounds(props);
		const cw = canvas.clientWidth,
			ch = canvas.clientHeight;
		const pad = 40;
		const s = Math.min((cw - pad * 2) / b.w, (ch - pad * 2) / b.h);
		view = { s, x: (cw - b.w * s) / 2 - b.x * s, y: (ch - b.h * s) / 2 - b.y * s };
		fitted = true;
		redraw();
	}
	export function zoom(f: number) {
		if (!canvas) return;
		zoomAt(canvas.clientWidth / 2, canvas.clientHeight / 2, f);
	}

	function zoomAt(cx: number, cy: number, f: number) {
		const s2 = Math.max(0.05, Math.min(40, view.s * f));
		const k = s2 / view.s;
		view = { s: s2, x: cx - (cx - view.x) * k, y: cy - (cy - view.y) * k };
		redraw();
	}

	function layoutOf(p: Prop) {
		const l = p.layout ?? { x: 0, y: 0, w: 60, h: 40, rotation: 0 };
		const m = moved.get(p.id);
		return m ? { ...l, ...m } : l;
	}

	function redraw() {
		if (lastRgb) draw(lastRgb, lastSlots);
		else draw(new Uint8Array(0), new Map());
	}

	function draw(rgb: Uint8Array, slots: Map<string, PropSlot>) {
		lastRgb = rgb;
		lastSlots = slots;
		const c = canvas;
		if (!c) return;
		const ctx = c.getContext('2d')!;
		const dpr = Math.min(2, window.devicePixelRatio || 1);
		const cw = c.clientWidth,
			ch = c.clientHeight;
		if (c.width !== Math.round(cw * dpr) || c.height !== Math.round(ch * dpr)) {
			c.width = Math.round(cw * dpr);
			c.height = Math.round(ch * dpr);
		}
		if (!fitted && cw > 0) {
			fitted = true;
			fit();
			return;
		}
		ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
		ctx.fillStyle = '#060608';
		ctx.fillRect(0, 0, cw, ch);
		// subtle grid
		const g = 50 * view.s;
		if (g > 12) {
			ctx.strokeStyle = 'rgba(255,255,255,0.035)';
			ctx.lineWidth = 1;
			ctx.beginPath();
			for (let x = view.x % g; x < cw; x += g) {
				ctx.moveTo(Math.round(x) + 0.5, 0);
				ctx.lineTo(Math.round(x) + 0.5, ch);
			}
			for (let y = view.y % g; y < ch; y += g) {
				ctx.moveTo(0, Math.round(y) + 0.5);
				ctx.lineTo(cw, Math.round(y) + 0.5);
			}
			ctx.stroke();
		}
		const dotBase = Math.max(1.2, Math.min(5, 3.2 * Math.sqrt(view.s)));
		for (const p of props) {
			const l = layoutOf(p);
			const box = { x: view.x + l.x * view.s, y: view.y + l.y * view.s, w: l.w * view.s, h: l.h * view.s };
			if (box.x > cw || box.y > ch || box.x + box.w < 0 || box.y + box.h < 0) continue;
			const slot = slots.get(p.id);
			const pts = propPoints(p);
			const rot = l.rotation ? (l.rotation * Math.PI) / 180 : 0;
			if (rot) {
				ctx.save();
				ctx.translate(box.x + box.w / 2, box.y + box.h / 2);
				ctx.rotate(rot);
				ctx.translate(-(box.x + box.w / 2), -(box.y + box.h / 2));
			}
			let dot = p.matrix ? Math.max(1, (box.w / p.matrix.width) * 0.8) : dotBase;
			const n = slot ? (slot.idx ? Math.min(slot.n, slot.idx.length) : Math.min(slot.n, p.pixelCount)) : 0;
			if (slot && n > 0 && rgb.length >= slot.off + n * 3) {
				// A subsample draws bigger dots so the prop still reads as a whole.
				if (slot.idx && p.matrix) dot *= Math.max(1, Math.sqrt(p.pixelCount / n));
				const at = slot.idx ? subsetPoints(p.id, pts, slot.idx) : pts;
				drawPixels(ctx, at, rgb, slot.off, n, box, dot, !p.matrix);
			} else {
				ctx.fillStyle = 'rgba(255,255,255,0.12)';
				for (let i = 0; i < p.pixelCount; i += p.pixelCount > 1000 ? 3 : 1)
					ctx.fillRect(box.x + pts[i * 2] * box.w - 1, box.y + pts[i * 2 + 1] * box.h - 1, 2, 2);
			}
			if (rot) ctx.restore();
			const isSel = selected === p.id;
			const isHi = highlight.includes(p.id) || hover === p.id;
			if (edit || isSel || isHi) {
				ctx.strokeStyle = isSel ? '#F5A524' : isHi ? 'rgba(245,165,36,0.6)' : 'rgba(255,255,255,0.16)';
				ctx.setLineDash(isSel || isHi ? [] : [4, 4]);
				ctx.lineWidth = isSel ? 1.5 : 1;
				ctx.strokeRect(
					Math.round(box.x) - 4.5,
					Math.round(box.y) - 4.5,
					Math.round(box.w) + 9,
					Math.round(box.h) + 9
				);
				ctx.setLineDash([]);
			}
			if (labels || isSel || isHi || (edit && editLabels)) {
				ctx.font = '500 11px Inter Variable, Inter, system-ui, sans-serif';
				const tw = ctx.measureText(p.name).width;
				const lx = box.x - 4,
					ly = box.y + box.h + 8;
				ctx.fillStyle = isSel ? 'rgba(245,165,36,0.95)' : 'rgba(20,21,26,0.85)';
				ctx.beginPath();
				ctx.roundRect(lx, ly, tw + 12, 18, 5);
				ctx.fill();
				ctx.fillStyle = isSel ? '#1d1300' : 'rgba(236,236,239,0.9)';
				ctx.fillText(p.name, lx + 6, ly + 13);
			}
		}
	}

	$effect(() => {
		// redraw when props / selection / mode change
		void props;
		void selected;
		void edit;
		void labels;
		void editLabels;
		void highlight;
		redraw();
	});

	$effect(() => {
		if (!canvas) return;
		const ro = new ResizeObserver(() => {
			if (!fitted) fit();
			else redraw();
		});
		ro.observe(canvas);
		return () => ro.disconnect();
	});

	$effect(() => {
		// (Re)subscribe when the frame source changes: live lights or a local preview.
		const src = source;
		lastRgb = null;
		lastSlots = new Map();
		redraw();
		return src.subscribe(draw);
	});

	// ---------------------------------------------------------------- interaction
	const pointers = new Map<number, { x: number; y: number }>();
	let drag: {
		mode: 'pan' | 'move';
		id?: string;
		sx: number;
		sy: number;
		ox: number;
		oy: number;
		moved: boolean;
	} | null = null;
	let pinch: { d: number; s: number } | null = null;

	function hit(px: number, py: number): string | null {
		for (let i = props.length - 1; i >= 0; i--) {
			const p = props[i];
			const l = layoutOf(p);
			const x = view.x + l.x * view.s,
				y = view.y + l.y * view.s;
			const w = l.w * view.s,
				h = l.h * view.s;
			if (px >= x - 8 && px <= x + w + 8 && py >= y - 8 && py <= y + h + 8) return p.id;
		}
		return null;
	}

	function local(e: PointerEvent | WheelEvent) {
		const r = canvas!.getBoundingClientRect();
		return { x: e.clientX - r.left, y: e.clientY - r.top };
	}

	function down(e: PointerEvent) {
		canvas!.setPointerCapture(e.pointerId);
		const p = local(e);
		pointers.set(e.pointerId, p);
		if (pointers.size === 2) {
			const [a, b] = [...pointers.values()];
			pinch = { d: Math.hypot(a.x - b.x, a.y - b.y), s: view.s };
			drag = null;
			return;
		}
		const id = hit(p.x, p.y);
		if (edit && id) {
			const l = layoutOf(props.find((x) => x.id === id)!);
			drag = { mode: 'move', id, sx: p.x, sy: p.y, ox: l.x, oy: l.y, moved: false };
			selected = id;
		} else
			drag = { mode: 'pan', sx: p.x, sy: p.y, ox: view.x, oy: view.y, moved: false, id: id ?? undefined };
	}

	function move(e: PointerEvent) {
		const p = local(e);
		if (pointers.has(e.pointerId)) pointers.set(e.pointerId, p);
		if (pinch && pointers.size === 2) {
			const [a, b] = [...pointers.values()];
			const d = Math.hypot(a.x - b.x, a.y - b.y);
			const f = (pinch.s * (d / pinch.d)) / view.s;
			zoomAt((a.x + b.x) / 2, (a.y + b.y) / 2, f);
			return;
		}
		if (!drag) {
			const h = hit(p.x, p.y);
			if (h !== hover) {
				hover = h;
				redraw();
			}
			canvas!.style.cursor = edit && h ? 'move' : 'grab';
			return;
		}
		const dx = p.x - drag.sx,
			dy = p.y - drag.sy;
		if (Math.abs(dx) + Math.abs(dy) > 3) drag.moved = true;
		if (drag.mode === 'pan') {
			view = { ...view, x: drag.ox + dx, y: drag.oy + dy };
			canvas!.style.cursor = 'grabbing';
		} else if (drag.id) {
			const snap = e.shiftKey ? 1 : 5;
			moved.set(drag.id, {
				x: Math.round((drag.ox + dx / view.s) / snap) * snap,
				y: Math.round((drag.oy + dy / view.s) / snap) * snap
			});
		}
		redraw();
	}

	function up(e: PointerEvent) {
		pointers.delete(e.pointerId);
		if (pointers.size < 2) pinch = null;
		if (!drag) return;
		if (drag.mode === 'move' && drag.id && drag.moved) {
			const m = moved.get(drag.id);
			if (m) onmove?.(drag.id, m);
		} else if (!drag.moved) selected = drag.id ?? null;
		drag = null;
		canvas!.style.cursor = 'grab';
	}

	function wheel(e: WheelEvent) {
		e.preventDefault();
		const p = local(e);
		zoomAt(p.x, p.y, Math.exp(-e.deltaY * 0.0015));
	}

	/** Forget local drag overrides once the server copy has caught up. */
	$effect(() => {
		for (const p of props) {
			const m = moved.get(p.id);
			if (m && p.layout && Math.abs(p.layout.x - m.x) < 0.5 && Math.abs(p.layout.y - m.y) < 0.5)
				moved.delete(p.id);
		}
	});
</script>

<canvas
	bind:this={canvas}
	style:height
	onpointerdown={down}
	onpointermove={move}
	onpointerup={up}
	onpointercancel={up}
	onpointerleave={() => {
		if (hover) {
			hover = null;
			redraw();
		}
	}}
	onwheel={wheel}
	aria-label="{source.kind === 'preview'
		? 'Sequence preview (your lights are not affected)'
		: 'Live display preview'}. Drag to pan, scroll or pinch to zoom{edit ? ', drag a prop to move it' : ''}."
></canvas>

<style>
	canvas {
		width: 100%;
		display: block;
		touch-action: none;
		cursor: grab;
		background: #060608;
	}
</style>
