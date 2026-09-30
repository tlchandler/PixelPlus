<!--
	Add / remove tags: current tags as chips plus a text box with suggestions.
	Type a tag and press Enter (or a comma). `onchange` gets the new list.
-->
<script lang="ts">
	import type { TagDef } from '$lib/api/types';
	import { parseTags, STARTER_TAGS } from '$lib/library/tags';
	import TagChips from './TagChips.svelte';
	import { Plus } from '@lucide/svelte';

	let {
		tags = [],
		suggestions = [],
		defs = [],
		label = 'Add a tag',
		onchange
	}: {
		tags?: string[];
		suggestions?: string[];
		defs?: TagDef[];
		label?: string;
		onchange: (tags: string[]) => void;
	} = $props();

	let text = $state('');
	const uid = $props.id();
	const options = $derived(
		[...new Set([...suggestions, ...(suggestions.length ? [] : STARTER_TAGS)])].filter(
			(t) => !tags.includes(t)
		)
	);

	function add() {
		const more = parseTags(text).filter((t) => !tags.includes(t));
		text = '';
		if (more.length) onchange([...tags, ...more]);
	}
	function key(e: KeyboardEvent) {
		if (e.key === 'Enter' || e.key === ',') {
			e.preventDefault();
			add();
		} else if (e.key === 'Backspace' && !text && tags.length) onchange(tags.slice(0, -1));
	}
</script>

<div class="ti">
	<TagChips {tags} {defs} onremove={(t) => onchange(tags.filter((x) => x !== t))} />
	<div class="add">
		<input
			class="input sm"
			bind:value={text}
			onkeydown={key}
			onblur={() => text.trim() && add()}
			list={uid}
			placeholder={tags.length ? 'Add another…' : 'e.g. kids, classic'}
			aria-label={label}
			maxlength="64"
		/>
		<button type="button" class="btn sm ghost icon" onclick={add} disabled={!text.trim()} aria-label="Add tag"
			><Plus size={14} /></button
		>
		<datalist id={uid}>
			{#each options.slice(0, 30) as o (o)}<option value={o}></option>{/each}
		</datalist>
	</div>
	{#if !tags.length && options.length}
		<div class="quick">
			{#each options.slice(0, 6) as o (o)}
				<button type="button" class="q" onclick={() => onchange([...tags, o])}>+ {o}</button>
			{/each}
		</div>
	{/if}
</div>

<style>
	.ti {
		display: flex;
		flex-direction: column;
		gap: 8px;
	}
	.add {
		display: flex;
		gap: 6px;
		align-items: center;
		max-width: 320px;
	}
	.quick {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
	}
	.q {
		height: 26px;
		padding: 0 10px;
		border-radius: 99px;
		font-size: 12px;
		color: var(--text-2);
		background: var(--surface-2);
		border: 1px dashed var(--border-2);
	}
	.q:hover {
		color: var(--text);
		border-color: var(--accent);
	}
	@media (pointer: coarse) {
		.q {
			height: 36px;
		}
	}
</style>
