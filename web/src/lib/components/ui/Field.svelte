<script lang="ts">
	import type { Snippet } from 'svelte';
	let {
		label,
		hint,
		span = false,
		children,
		id
	}: { label: string; hint?: string; span?: boolean; children: Snippet; id?: string } = $props();
</script>

<label class="field" class:span-2={span} for={id}>
	<span class="label">{label}</span>
	{@render children()}
	{#if hint}<span class="hint">{hint}</span>{/if}
</label>
