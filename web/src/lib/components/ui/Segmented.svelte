<script lang="ts" generics="T extends string | number">
	import type { Component } from 'svelte';
	let {
		value = $bindable(),
		options,
		label,
		size = 'md',
		onchange
	}: {
		value: T;
		options: { value: T; label: string; icon?: Component<any> }[];
		label: string;
		size?: 'sm' | 'md';
		onchange?: (v: T) => void;
	} = $props();
</script>

<div class="seg {size}" role="radiogroup" aria-label={label}>
	{#each options as o (o.value)}
		<button
			type="button"
			role="radio"
			aria-checked={value === o.value}
			class:active={value === o.value}
			onclick={() => {
				value = o.value;
				onchange?.(o.value);
			}}
		>
			{#if o.icon}<o.icon size={15} />{/if}
			<span>{o.label}</span>
		</button>
	{/each}
</div>

<style>
	.seg {
		display: inline-flex;
		padding: 3px;
		gap: 2px;
		border-radius: 10px;
		background: var(--surface-2);
		border: 1px solid var(--border);
		max-width: 100%;
		overflow-x: auto;
		scrollbar-width: none;
	}
	button {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		height: 32px;
		padding: 0 12px;
		border-radius: 7px;
		font-size: 13px;
		font-weight: 520;
		color: var(--text-2);
		white-space: nowrap;
		transition: all 160ms var(--ease);
	}
	.sm button {
		height: 26px;
		padding: 0 10px;
		font-size: 12.5px;
	}
	button:hover {
		color: var(--text);
	}
	button.active {
		background: var(--surface-3);
		color: var(--text);
		box-shadow:
			var(--shadow-1),
			0 0 0 1px var(--border-2);
	}
	@media (pointer: coarse) {
		button,
		.sm button {
			height: 44px;
			padding: 0 14px;
			font-size: 13.5px;
		}
	}
</style>
