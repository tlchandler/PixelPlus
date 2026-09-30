<script lang="ts">
	import { Check, LoaderCircle, CircleAlert } from '@lucide/svelte';

	/** The one save indicator used everywhere changes save automatically. */
	let {
		state,
		invalidText = 'Fix the highlighted field to save'
	}: { state: 'saved' | 'saving' | 'dirty' | 'invalid'; invalidText?: string } = $props();
</script>

<span class="ss {state}" role="status" aria-live="polite">
	{#if state === 'saving' || state === 'dirty'}
		<LoaderCircle size={12} class="ss-spin" /> Saving…
	{:else if state === 'invalid'}
		<CircleAlert size={12} /> {invalidText}
	{:else}
		<Check size={12} /> Saved
	{/if}
</span>

<style>
	.ss {
		display: inline-flex;
		align-items: center;
		gap: 4px;
		font-size: 12px;
		font-weight: 520;
		color: var(--text-3);
		white-space: nowrap;
	}
	.ss.invalid {
		color: var(--red);
	}
	.ss :global(.ss-spin) {
		animation: ss-spin 0.9s linear infinite;
	}
	@keyframes ss-spin {
		to {
			transform: rotate(360deg);
		}
	}
</style>
