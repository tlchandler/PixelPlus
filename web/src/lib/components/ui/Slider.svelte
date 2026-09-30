<script lang="ts">
	let {
		value = $bindable(0),
		min = 0,
		max = 100,
		step = 1,
		label,
		disabled = false,
		oninput,
		onchange,
		format
	}: {
		value?: number;
		min?: number;
		max?: number;
		step?: number;
		label: string;
		disabled?: boolean;
		oninput?: (v: number) => void;
		onchange?: (v: number) => void;
		format?: (v: number) => string;
	} = $props();
	const pct = $derived(((value - min) / (max - min || 1)) * 100);
</script>

<div class="slider">
	<input
		type="range"
		class="range"
		{min}
		{max}
		{step}
		{disabled}
		aria-label={label}
		aria-valuetext={format ? format(value) : undefined}
		bind:value
		style:--pct="{pct}%"
		oninput={() => oninput?.(value)}
		onchange={() => onchange?.(value)}
	/>
	{#if format}<span class="val num">{format(value)}</span>{/if}
</div>

<style>
	.slider {
		display: flex;
		align-items: center;
		gap: 12px;
		width: 100%;
	}
	.val {
		min-width: 48px;
		text-align: right;
		font-size: 12.5px;
		color: var(--text-2);
	}
</style>
