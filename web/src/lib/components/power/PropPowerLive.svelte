<!-- The power budgets a prop draws from (F12, WS3): its receiver port fuse, the
     receiver's main fuse, its power supply and the display cap, with live load and
     whether the limiter is dimming them. Used in the props drawer's Power tab. -->
<script lang="ts">
	import { onDestroy, onMount } from 'svelte';
	import type { Prop } from '$lib/api/types';
	import { request } from '$lib/api/client';
	import { app } from '$lib/stores/app.svelte';
	import { receiverFor, portOf } from '$lib/util/boards';
	import { groupLabel, load, type LiveNode } from '$lib/power/power';
	import { Gauge } from '@lucide/svelte';

	let { prop }: { prop: Prop } = $props();

	const show = $derived(app.show);
	const mode = $derived(show?.settings.power?.mode ?? 'warn');
	let live = $state<LiveNode[]>([]);
	let off: (() => void) | undefined;
	let poll: ReturnType<typeof setInterval> | undefined;

	async function refresh() {
		try {
			live = (await request<{ nodes: LiveNode[] }>('GET', '/power/live')).nodes;
		} catch {
			/* the page still shows the static budget */
		}
	}
	onMount(() => {
		void refresh();
		poll = setInterval(refresh, 4000);
		off = app.onMessage('power', (d) => (live = d.nodes as LiveNode[]));
	});
	onDestroy(() => {
		clearInterval(poll);
		off?.();
	});

	/** The group ids this prop's pixels count against, per node. */
	const mine = $derived.by(() => {
		const out: { nodeId: string; id: string }[] = [];
		if (!show) return out;
		const add = (nodeId: string, id: string) => {
			if (!out.some((o) => o.nodeId === nodeId && o.id === id)) out.push({ nodeId, id });
		};
		for (const seg of prop.segments) {
			const rx = receiverFor(show, seg.nodeId, seg.output);
			if (rx) {
				add(seg.nodeId, `port:${rx.id}:${portOf(seg.output)}`);
				add(seg.nodeId, `bus:${rx.id}`);
			}
			for (const s of show.powerSupplies ?? []) {
				const fed =
					(rx && s.receiverIds.includes(rx.id)) ||
					s.directOutputs.some((d) => d.nodeId === seg.nodeId && d.output === seg.output);
				if (fed) add(seg.nodeId, `supply:${s.id}`);
			}
			add(seg.nodeId, 'global');
		}
		return out;
	});
	const rows = $derived(
		mine
			.map(({ nodeId, id }) => ({
				id,
				g: live.find((n) => n.nodeId === nodeId)?.groups.find((g) => g.id === id)
			}))
			.filter((r) => !!r.g)
	);
	const supplyName = $derived(
		(show?.powerSupplies ?? []).find((s) => mine.some((m) => m.id === `supply:${s.id}`))?.name
	);
</script>

<div class="plive">
	<div class="row between head">
		<span class="row small" style="gap:6px"><Gauge size={15} /> Power limiter</span>
		<a class="small" href="/settings/power">{mode === 'off' ? 'Turn on' : 'Settings'}</a>
	</div>
	{#if mode === 'off'}
		<p class="faint small">
			The limiter is off. Turn it on to keep this prop’s fuses and power supply within their ratings
			automatically.
		</p>
	{:else if !rows.length}
		<p class="faint small">
			{supplyName ? `Fed by ${supplyName}.` : 'No power supply is set for this prop’s receiver yet.'}
			Budgets show up here once the display is lit.
		</p>
	{:else}
		{#each rows as r (r.id)}
			{@const g = r.g!}
			{@const l = load(g.amps, g.budget)}
			<div class="grp">
				<div class="row between small">
					<span class="ellipsis">{groupLabel(show, r.id)}</span>
					<span class="num"
						>{g.amps == null ? '—' : `${g.amps.toFixed(1)} A`} of {g.budget.toFixed(1)} A{g.scale < 0.995
							? ` · ${mode === 'limit' ? 'dimmed' : 'would dim'} to ${Math.round(g.scale * 100)} %`
							: ''}</span
					>
				</div>
				<div class="bar"><span class:hot={l > 0.9} style:width="{Math.min(100, l * 100)}%"></span></div>
			</div>
		{/each}
	{/if}
</div>

<style>
	.plive {
		display: flex;
		flex-direction: column;
		gap: 8px;
		padding: 12px;
		border-radius: 12px;
		border: 1px solid var(--border);
	}
	.head a {
		color: var(--accent);
	}
	.grp {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}
	.bar {
		height: 6px;
		border-radius: 3px;
		background: var(--surface-2);
		overflow: hidden;
	}
	.bar span {
		display: block;
		height: 100%;
		background: var(--green, #3ccf8e);
		transition: width 400ms ease;
	}
	.bar span.hot {
		background: var(--accent);
	}
</style>
