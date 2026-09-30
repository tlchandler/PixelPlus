<script lang="ts">
	import type { Prop } from '$lib/api/types';
	import { propPoints } from '$lib/util/geometry';
	import { drawPixels, fitBox, onPreview } from '$lib/preview';

	let { prop, height = 96, live = true }: { prop: Prop; height?: number; live?: boolean } = $props();

	let canvas: HTMLCanvasElement | undefined = $state();
	let visible = $state(false);

	$effect(() => {
		if (!canvas) return;
		const io = new IntersectionObserver(([e]) => (visible = e.isIntersecting), { rootMargin: '100px' });
		io.observe(canvas);
		return () => io.disconnect();
	});

	$effect(() => {
		if (!canvas || !visible || !live) return;
		const c = canvas;
		const p = prop;
		const ctx = c.getContext('2d')!;
		const pts = propPoints(p);
		const aw = p.layout?.w ?? 1,
			ah = p.layout?.h ?? 1;
		const draw = (rgb: Uint8Array, offsets: Map<string, number>) => {
			const dpr = Math.min(2, window.devicePixelRatio || 1);
			const cw = c.clientWidth,
				ch = c.clientHeight;
			if (c.width !== cw * dpr) {
				c.width = cw * dpr;
				c.height = ch * dpr;
			}
			ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
			ctx.clearRect(0, 0, cw, ch);
			const off = offsets.get(p.id);
			if (off == null) return;
			const box = fitBox(cw, ch, Math.max(aw, ah * 0.2), Math.max(ah, aw * 0.08), 12);
			const dot =
				p.pixelCount > 1500
					? Math.max(1, box.w / (p.matrix?.width ?? 80)) * 0.8
					: p.pixelCount > 400
						? 2.2
						: 3.2;
			drawPixels(ctx, pts, rgb, off, p.pixelCount, box, dot, p.pixelCount < 1500);
		};
		return onPreview(draw);
	});
</script>

<canvas bind:this={canvas} style:height="{height}px" aria-label="Live preview of {prop.name}"></canvas>

<style>
	canvas {
		width: 100%;
		display: block;
		background: radial-gradient(ellipse at 50% 120%, #15161c, var(--canvas-bg) 70%);
		border-radius: inherit;
	}
</style>
