<!--
	History of nights for the Reports list: one bar per night (songs played), oldest
	left, status shown as an icon row below (never colour alone). Hover / tap a bar for
	the night's numbers; click opens it. WS6 (F11).
-->
<script lang="ts">
	import type { ReportSummaryFull } from './api';

	let {
		nights,
		selected,
		onpick
	}: { nights: ReportSummaryFull[]; selected?: string; onpick?: (date: string) => void } = $props();

	const H = 110;
	const data = $derived([...nights].reverse());
	const max = $derived(Math.max(1, ...data.map((n) => n.itemsPlayed ?? 0)));
	let hover = $state<number | null>(null);
	const day = (d: string) =>
		new Date(d + 'T12:00:00').toLocaleDateString(undefined, {
			weekday: 'short',
			month: 'short',
			day: 'numeric'
		});
	const statusMark = { ok: '✓', warn: '!', fail: '✕' } as const;
</script>

<div class="nb" role="group" aria-label="Songs played per night">
	<div class="bars" style="height:{H}px">
		{#each data as n, i (n.date)}
			{@const v = n.itemsPlayed ?? 0}
			<button
				class="bar"
				class:sel={n.date === selected}
				style="--h:{Math.max(2, (v / max) * (H - 4))}px"
				aria-label="{day(n.date)}: {v} songs, {n.requests ?? 0} requests, {n.status}"
				onpointerenter={() => (hover = i)}
				onpointerleave={() => (hover = null)}
				onfocus={() => (hover = i)}
				onblur={() => (hover = null)}
				onclick={() => onpick?.(n.date)}
			>
				<span class="fill"></span>
			</button>
		{/each}
	</div>
	<div class="marks" aria-hidden="true">
		{#each data as n (n.date)}
			<span class="m {n.status}">{statusMark[n.status]}</span>
		{/each}
	</div>
	{#if hover != null && data[hover]}
		{@const n = data[hover]}
		<div
			class="tip"
			style="left:clamp(0px, calc({((hover + 0.5) / data.length) * 100}% - 90px), calc(100% - 180px))"
		>
			<strong>{day(n.date)}</strong>
			<span class="num">{n.itemsPlayed ?? 0} songs · {n.requests ?? 0} requests</span>
			<span class="faint">{n.problems ? `${n.problems} problems` : 'No problems'}</span>
		</div>
	{/if}
</div>

<style>
	.nb {
		position: relative;
	}
	.bars {
		display: flex;
		align-items: flex-end;
		gap: 2px;
		border-bottom: 1px solid var(--border-2);
	}
	.bar {
		flex: 1 1 0;
		min-width: 4px;
		height: 100%;
		display: flex;
		align-items: flex-end;
		padding: 0;
		background: transparent;
		border: 0;
		cursor: pointer;
	}
	.fill {
		display: block;
		width: 100%;
		height: var(--h);
		background: var(--accent);
		opacity: 0.7;
		border-radius: 4px 4px 0 0;
		transition: opacity 120ms var(--ease);
	}
	.bar:hover .fill,
	.bar:focus-visible .fill,
	.bar.sel .fill {
		opacity: 1;
	}
	.marks {
		display: flex;
		gap: 2px;
		margin-top: 4px;
	}
	.m {
		flex: 1 1 0;
		text-align: center;
		font-size: 10px;
		font-weight: 700;
		line-height: 14px;
		color: var(--text-3);
		overflow: hidden;
	}
	.m.warn {
		color: var(--accent-text);
	}
	.m.fail {
		color: var(--red);
	}
	.tip {
		position: absolute;
		top: 0;
		width: 180px;
		display: flex;
		flex-direction: column;
		gap: 1px;
		padding: 6px 10px;
		border-radius: var(--r-1);
		background: var(--surface-3);
		border: 1px solid var(--border-2);
		box-shadow: var(--shadow-2);
		font-size: 12px;
		pointer-events: none;
	}
</style>
