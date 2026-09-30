<script lang="ts">
	import type { EffectKind, EffectParams } from '$lib/api/types';
	import { renderEffect } from '$lib/effects/render';
	import { derivePoints } from '$lib/util/geometry';

	let { kind: effectKind, params, height = 120, animate = true }: { kind: EffectKind; params: EffectParams; height?: number; animate?: boolean } =
		$props();

	let canvas: HTMLCanvasElement | undefined = $state();
	let visible = $state(false);

	// A tiny scene: arch, tree, candy cane, and a roofline.
	const parts = [
		{ kind: 'arch' as const, n: 36, box: [0.04, 0.42, 0.26, 0.46] },
		{ kind: 'tree' as const, n: 120, box: [0.38, 0.1, 0.24, 0.78] },
		{ kind: 'candycane' as const, n: 18, box: [0.7, 0.5, 0.08, 0.38] },
		{ kind: 'candycane' as const, n: 18, box: [0.8, 0.5, 0.08, 0.38] },
		{ kind: 'line' as const, n: 60, box: [0.04, 0.1, 0.3, 0.02] },
		{ kind: 'line' as const, n: 40, box: [0.68, 0.18, 0.28, 0.02] }
	];
	const total = parts.reduce((n, p) => n + p.n, 0);
	const xs = new Float32Array(total);
	const ys = new Float32Array(total);
	const pts: Float32Array[] = [];
	{
		let k = 0;
		for (const p of parts) {
			const pp = derivePoints(p.kind, p.n);
			pts.push(pp);
			for (let i = 0; i < p.n; i++, k++) {
				xs[k] = p.box[0] + pp[i * 2] * p.box[2];
				ys[k] = p.box[1] + pp[i * 2 + 1] * p.box[3];
			}
		}
	}
	const buf = new Uint8ClampedArray(total * 3);

	$effect(() => {
		if (!canvas) return;
		const io = new IntersectionObserver(([e]) => (visible = e.isIntersecting));
		io.observe(canvas);
		return () => io.disconnect();
	});

	$effect(() => {
		if (!canvas || !visible) return;
		const c = canvas;
		const ctx = c.getContext('2d')!;
		const kind = effectKind;
		const prm = $state.snapshot(params) as EffectParams;
		const reduced = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
		let raf = 0;
		const t0 = performance.now();
		const frame = (now: number) => {
			const dpr = Math.min(2, window.devicePixelRatio || 1);
			const cw = c.clientWidth,
				ch = c.clientHeight;
			if (c.width !== cw * dpr) {
				c.width = cw * dpr;
				c.height = ch * dpr;
			}
			ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
			ctx.clearRect(0, 0, cw, ch);
			renderEffect(kind, prm, (now - t0) / 1000 + 2, total, buf, 0, { xs, ys, seed: 7 });
			ctx.globalCompositeOperation = 'lighter';
			for (let pass = 0; pass < 2; pass++) {
				ctx.globalAlpha = pass === 0 ? 0.25 : 1;
				const r = pass === 0 ? 4.5 : 1.6;
				for (let i = 0; i < total; i++) {
					const R = buf[i * 3],
						G = buf[i * 3 + 1],
						B = buf[i * 3 + 2];
					if (pass === 0 && R + G + B < 40) continue;
					ctx.fillStyle = R + G + B < 10 ? 'rgba(255,255,255,0.06)' : `rgb(${R},${G},${B})`;
					ctx.beginPath();
					ctx.arc(xs[i] * cw, ys[i] * ch, r, 0, 6.283);
					ctx.fill();
				}
			}
			ctx.globalAlpha = 1;
			ctx.globalCompositeOperation = 'source-over';
			if (animate && !reduced) raf = requestAnimationFrame(frame);
		};
		raf = requestAnimationFrame(frame);
		return () => cancelAnimationFrame(raf);
	});
</script>

<canvas bind:this={canvas} style:height="{height}px" aria-label="Animated preview of the {effectKind} effect"></canvas>

<style>
	canvas {
		width: 100%;
		display: block;
		background: radial-gradient(ellipse at 50% 130%, #16171e, #060608 70%);
	}
</style>
