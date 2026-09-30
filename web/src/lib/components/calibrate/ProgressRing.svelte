<script lang="ts">
	import type { Snippet } from 'svelte';
	let {
		value,
		size = 132,
		stroke = 10,
		label,
		children
	}: { value: number; size?: number; stroke?: number; label: string; children?: Snippet } = $props();
	const r = $derived((size - stroke) / 2);
	const c = $derived(2 * Math.PI * r);
	const v = $derived(Math.max(0, Math.min(1, value)));
</script>

<div
	class="ring"
	style="width:{size}px;height:{size}px"
	role="progressbar"
	aria-label={label}
	aria-valuemin="0"
	aria-valuemax="100"
	aria-valuenow={Math.round(v * 100)}
>
	<svg width={size} height={size} viewBox="0 0 {size} {size}" aria-hidden="true">
		<circle cx={size / 2} cy={size / 2} {r} fill="none" stroke="var(--surface-3)" stroke-width={stroke} />
		<circle
			cx={size / 2}
			cy={size / 2}
			{r}
			fill="none"
			stroke="var(--accent)"
			stroke-width={stroke}
			stroke-linecap="round"
			stroke-dasharray={c}
			stroke-dashoffset={c * (1 - v)}
			transform="rotate(-90 {size / 2} {size / 2})"
		/>
	</svg>
	<div class="inner">{@render children?.()}</div>
</div>

<style>
	.ring {
		position: relative;
		flex: 0 0 auto;
	}
	circle {
		transition: stroke-dashoffset 240ms var(--ease);
	}
	.inner {
		position: absolute;
		inset: 0;
		display: grid;
		place-items: center;
		text-align: center;
	}
</style>
