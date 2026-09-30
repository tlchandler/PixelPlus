<script lang="ts">
	import type { NodeStatus } from '$lib/api/types';
	import { fmtMs, syncGrade } from '$lib/util/sync';
	import { Timer } from '@lucide/svelte';

	/** Timing quality of a follower: a badge ("In sync ±0.3 ms") that opens the details. */
	let { node, frameMs = 25 }: { node: NodeStatus; frameMs?: number } = $props();
	const g = $derived(syncGrade(node, frameMs));
	const q = $derived(node.sync);
	let open = $state(false);
	let root = $state<HTMLElement>();

	function onDocClick(e: MouseEvent) {
		if (open && root && !root.contains(e.target as Node)) open = false;
	}
	function onKey(e: KeyboardEvent) {
		if (e.key === 'Escape') open = false;
	}
	const pct = (v: number) => `${Math.round(v)} %`;
	const ms = (v: number) => `${v < 10 ? v.toFixed(2) : Math.round(v)} ms`;
</script>

<svelte:document onclick={onDocClick} onkeydown={onKey} />

<span class="sync" bind:this={root}>
	<button
		type="button"
		class="badge {g.tone}"
		aria-expanded={open}
		aria-haspopup="dialog"
		title="Timing with the show leader"
		onclick={() => (open = !open)}
	>
		<Timer size={12} />
		{g.label}
	</button>
	{#if open}
		<div class="pop card" role="dialog" aria-label="Timing details">
			<div class="ttl">Timing with the show leader</div>
			{#if q}
				<dl>
					<dt>Clock accuracy</dt>
					<dd>{fmtMs(q.offsetErrorMs)}</dd>
					{#if q.timelineErrorMs != null}<dt>Following the show</dt>
						<dd>{fmtMs(q.timelineErrorMs)}</dd>{/if}
					<dt>Network round trip</dt>
					<dd>{ms(q.rttMs)} best · {ms(q.rttP50Ms)} typical · {ms(q.rttP95Ms)} worst</dd>
					<dt>Jitter</dt>
					<dd>{ms(q.jitterMs)}</dd>
					<dt>Clock drift</dt>
					<dd>{q.driftPpm.toFixed(1)} ppm</dd>
					<dt>Lost packets</dt>
					<dd>{pct(q.lossPct)}</dd>
					{#if q.refreshHz}<dt>Pixel refresh</dt>
						<dd>{Math.round(q.refreshHz)} Hz · frame changes within {fmtMs(500 / q.refreshHz)}</dd>{/if}
					<dt>Wi-Fi power saving</dt>
					<dd>{node.wifiPowerSave == null ? 'n/a' : node.wifiPowerSave ? 'On — turn it off' : 'Off'}</dd>
					<dt>Timestamps</dt>
					<dd>{q.kernelTimestamps ? 'Kernel (precise)' : 'Software'}</dd>
				</dl>
			{:else}
				<p class="faint small">No timing report yet. It appears a few seconds after the controller joins.</p>
			{/if}
			{#if g.tips.length}
				<ul class="tips">
					{#each g.tips as t (t)}<li>{t}</li>{/each}
				</ul>
			{/if}
		</div>
	{/if}
</span>

<style>
	.sync {
		position: relative;
		display: inline-flex;
	}
	button.badge {
		border: 0;
		cursor: pointer;
		font: inherit;
		font-size: 11.5px;
		font-weight: 560;
	}
	button.badge:focus-visible {
		box-shadow: var(--ring);
		outline: none;
	}
	.pop {
		position: absolute;
		z-index: 30;
		top: calc(100% + 6px);
		left: 0;
		width: min(340px, 86vw);
		padding: 12px 14px;
		box-shadow: var(--shadow-2);
		font-size: 12.5px;
	}
	.ttl {
		font-weight: 620;
		margin-bottom: 8px;
	}
	dl {
		display: grid;
		grid-template-columns: auto 1fr;
		gap: 4px 12px;
		margin: 0;
	}
	dt {
		color: var(--text-2);
	}
	dd {
		margin: 0;
		font-variant-numeric: tabular-nums;
	}
	.tips {
		margin: 10px 0 0;
		padding-left: 18px;
		color: var(--text-2);
	}
	.tips li + li {
		margin-top: 4px;
	}
</style>
