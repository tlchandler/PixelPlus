<script lang="ts">
	import type { TagDef } from '$lib/api/types';
	import { tagColor } from '$lib/library/tags';
	import { X } from '@lucide/svelte';

	let {
		tags = [],
		defs = [],
		active = [],
		onpick,
		onremove,
		max = 0
	}: {
		tags?: string[];
		defs?: TagDef[];
		/** Highlighted tags (e.g. the current filter). */
		active?: string[];
		onpick?: (tag: string) => void;
		onremove?: (tag: string) => void;
		/** Show at most this many, then "+n" (0 = all). */
		max?: number;
	} = $props();

	const shown = $derived(max > 0 ? tags.slice(0, max) : tags);
</script>

{#if tags.length}
	<span class="chips">
		{#each shown as t (t)}
			<span class="chip" class:on={active.includes(t)} style:--c={tagColor(t, defs)}>
				{#if onpick}
					<button type="button" class="name" onclick={() => onpick(t)} aria-pressed={active.includes(t)}
						>{t}</button
					>
				{:else}<span class="name">{t}</span>{/if}
				{#if onremove}
					<button type="button" class="x" onclick={() => onremove(t)} aria-label="Remove tag {t}"
						><X size={11} /></button
					>
				{/if}
			</span>
		{/each}
		{#if max > 0 && tags.length > max}<span class="more faint">+{tags.length - max}</span>{/if}
	</span>
{/if}

<style>
	.chips {
		display: inline-flex;
		flex-wrap: wrap;
		gap: 4px;
		align-items: center;
		min-width: 0;
	}
	.chip {
		display: inline-flex;
		align-items: center;
		height: 20px;
		border-radius: 99px;
		font-size: 11.5px;
		font-weight: 560;
		color: var(--text-2);
		background: color-mix(in srgb, var(--c) 16%, transparent);
		box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--c) 35%, transparent);
		white-space: nowrap;
	}
	.chip.on {
		background: color-mix(in srgb, var(--c) 34%, transparent);
		color: var(--text);
	}
	.name {
		padding: 0 8px;
		line-height: 20px;
	}
	button.name {
		cursor: pointer;
	}
	.x {
		display: grid;
		place-items: center;
		width: 18px;
		height: 18px;
		margin-left: -4px;
		margin-right: 1px;
		border-radius: 50%;
		color: var(--text-3);
	}
	.x:hover {
		color: var(--text);
		background: color-mix(in srgb, var(--c) 30%, transparent);
	}
	.more {
		font-size: 11.5px;
	}
	@media (pointer: coarse) {
		.chip {
			height: 28px;
		}
		.name {
			line-height: 28px;
			padding: 0 10px;
		}
		.x {
			width: 26px;
			height: 26px;
		}
	}
</style>
