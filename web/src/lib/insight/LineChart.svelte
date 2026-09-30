<!--
	Single-series line chart for the Reports page (one per controller: small multiples
	instead of a multi-colour legend). Thin 2px line, recessive grid, crosshair + tooltip
	on hover / touch, min–max labels in text ink. WS6 (F11).
-->
<script lang="ts">
	let {
		points,
		unit,
		label,
		digits = 1,
		tz,
		height = 96
	}: {
		points: [number, number][];
		unit: string;
		label: string;
		digits?: number;
		tz?: string;
		height?: number;
	} = $props();

	const W = 300;
	let hover = $state<number | null>(null);
	let svgEl = $state<SVGSVGElement | null>(null);

	const ext = $derived.by(() => {
		if (!points.length) return { t0: 0, t1: 1, lo: 0, hi: 1 };
		const ts = points.map((p) => p[0]);
		const vs = points.map((p) => p[1]);
		let lo = Math.min(...vs);
		let hi = Math.max(...vs);
		if (hi - lo < 1e-9) {
			lo -= 1;
			hi += 1;
		}
		const pad = (hi - lo) * 0.12;
		return { t0: Math.min(...ts), t1: Math.max(...ts, Math.min(...ts) + 1), lo: lo - pad, hi: hi + pad };
	});
	const x = (t: number) => ((t - ext.t0) / (ext.t1 - ext.t0)) * W;
	const y = (v: number) => height - 6 - ((v - ext.lo) / (ext.hi - ext.lo)) * (height - 12);
	const path = $derived(
		points.map((p, i) => `${i ? 'L' : 'M'}${x(p[0]).toFixed(1)},${y(p[1]).toFixed(1)}`).join(' ')
	);
	const minV = $derived(points.length ? Math.min(...points.map((p) => p[1])) : 0);
	const maxV = $derived(points.length ? Math.max(...points.map((p) => p[1])) : 0);

	const fmtT = (t: number) =>
		new Date(t).toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit', timeZone: tz });

	function move(e: PointerEvent) {
		if (!svgEl || !points.length) return;
		const r = svgEl.getBoundingClientRect();
		const fx = ((e.clientX - r.left) / r.width) * W;
		let best = 0;
		let bd = Infinity;
		points.forEach((p, i) => {
			const d = Math.abs(x(p[0]) - fx);
			if (d < bd) {
				bd = d;
				best = i;
			}
		});
		hover = best;
	}
	const hp = $derived(hover != null ? points[hover] : null);
</script>

<figure class="lc">
	<figcaption class="row between">
		<span class="small">{label}</span>
		{#if points.length}
			<span class="faint tiny num">{minV.toFixed(digits)}–{maxV.toFixed(digits)} {unit}</span>
		{/if}
	</figcaption>
	{#if points.length < 2}
		<div class="empty faint small" style="height:{height}px">Not enough data</div>
	{:else}
		<div class="plot">
			<svg
				bind:this={svgEl}
				viewBox="0 0 {W} {height}"
				preserveAspectRatio="none"
				role="img"
				aria-label="{label}: {minV.toFixed(digits)} to {maxV.toFixed(digits)} {unit}"
				onpointermove={move}
				onpointerdown={move}
				onpointerleave={() => (hover = null)}
			>
				<line class="grid" x1="0" x2={W} y1={y(maxV)} y2={y(maxV)} vector-effect="non-scaling-stroke" />
				<line class="grid" x1="0" x2={W} y1={y(minV)} y2={y(minV)} vector-effect="non-scaling-stroke" />
				<path d={path} class="line" vector-effect="non-scaling-stroke" />
				{#if hp}
					<line
						class="cross"
						x1={x(hp[0])}
						x2={x(hp[0])}
						y1="0"
						y2={height}
						vector-effect="non-scaling-stroke"
					/>
				{/if}
				<rect x="0" y="0" width={W} {height} fill="transparent" />
			</svg>
			{#if hp}
				<div
					class="dotm"
					style="left:{(x(hp[0]) / W) * 100}%;top:{(y(hp[1]) / height) * 100}%"
					aria-hidden="true"
				></div>
				<div class="tip" style="left:clamp(0px, calc({(x(hp[0]) / W) * 100}% - 50px), calc(100% - 100px))">
					<strong class="num">{hp[1].toFixed(digits)} {unit}</strong>
					<span class="faint">{fmtT(hp[0])}</span>
				</div>
			{/if}
		</div>
		<div class="row between faint tiny num axis">
			<span>{fmtT(points[0][0])}</span><span>{fmtT(points[points.length - 1][0])}</span>
		</div>
	{/if}
</figure>

<style>
	.lc {
		margin: 0;
		display: flex;
		flex-direction: column;
		gap: 6px;
		min-width: 0;
	}
	.plot {
		position: relative;
	}
	svg {
		display: block;
		width: 100%;
		height: auto;
		touch-action: pan-y;
		cursor: crosshair;
	}
	.line {
		fill: none;
		stroke: var(--accent);
		stroke-width: 2;
		stroke-linejoin: round;
		stroke-linecap: round;
	}
	.grid {
		stroke: var(--border-2);
		stroke-width: 1;
		stroke-dasharray: 2 3;
	}
	.cross {
		stroke: var(--border-3);
		stroke-width: 1;
	}
	.dotm {
		position: absolute;
		width: 9px;
		height: 9px;
		border-radius: 50%;
		background: var(--accent);
		box-shadow: 0 0 0 2px var(--surface);
		transform: translate(-50%, -50%);
		pointer-events: none;
	}
	.tip {
		position: absolute;
		top: -6px;
		width: 100px;
		transform: translateY(-100%);
		display: flex;
		flex-direction: column;
		align-items: center;
		padding: 4px 8px;
		border-radius: var(--r-1);
		background: var(--surface-3);
		border: 1px solid var(--border-2);
		box-shadow: var(--shadow-2);
		font-size: 12px;
		pointer-events: none;
		white-space: nowrap;
	}
	.empty {
		display: grid;
		place-items: center;
		border: 1px dashed var(--border-2);
		border-radius: var(--r-2);
	}
	.axis {
		margin-top: -2px;
	}
</style>
