<!--
	"Tonight's preview" of a smart playlist (F18): what would play on a given night,
	with start times and plain-language notes. Uses the rules as edited (unsaved).
-->
<script lang="ts">
	import type { PlaylistItem, Show, SmartRules } from '$lib/api/types';
	import { library, type SmartPreviewFull } from '$lib/library/api';
	import { fmtDuration } from '$lib/util/format';
	import { Dices, Info, Music, Mic, Clock } from '@lucide/svelte';
	import { untrack } from 'svelte';

	let {
		rules,
		playlistId,
		show,
		ontotal
	}: { rules: SmartRules; playlistId: string; show: Show; ontotal?: (ms: number) => void } = $props();

	const today = () => {
		const d = new Date();
		return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
	};
	let date = $state(today());
	let start = $state('');
	let seed = $state<number | undefined>(undefined);
	let data = $state<SmartPreviewFull | null>(null);
	let loading = $state(false);
	let error = $state<string | null>(null);
	let timer: ReturnType<typeof setTimeout> | undefined;
	let gen = 0;

	$effect(() => {
		// Re-run when the rules or the chosen night change (debounced).
		const snap = $state.snapshot(rules) as SmartRules;
		const q = { playlistId, date, start: start || undefined, seed };
		untrack(() => {
			clearTimeout(timer);
			timer = setTimeout(() => void run(snap, q), 350);
		});
		return () => clearTimeout(timer);
	});

	async function run(r: SmartRules, q: { playlistId: string; date: string; start?: string; seed?: number }) {
		const my = ++gen;
		loading = true;
		try {
			const d = await library.rulesPreview(r, q);
			if (my !== gen) return;
			data = d;
			ontotal?.(d.totalMs);
			error = null;
		} catch (e) {
			if (my === gen) error = (e as Error).message;
		} finally {
			if (my === gen) loading = false;
		}
	}

	function info(it: PlaylistItem): { name: string; icon: typeof Music; ms: number } {
		switch (it.type) {
			case 'sequence': {
				const s = show.sequences.find((x) => x.id === it.sequenceId);
				return { name: s?.name ?? 'Missing song', icon: Music, ms: s?.durationMs ?? 0 };
			}
			case 'dj':
				return { name: show.djClips.find((c) => c.id === it.djClipId)?.name ?? 'DJ clip', icon: Mic, ms: 0 };
			default:
				return { name: it.type === 'pause' ? 'Pause' : it.type, icon: Clock, ms: 0 };
		}
	}
	function at(iso?: string) {
		if (!iso) return '';
		// The daemon sends local show time with its offset: show the wall clock as is.
		const m = /T(\d{2}):(\d{2})/.exec(iso);
		if (!m) return '';
		const h = Number(m[1]);
		return `${h % 12 || 12}:${m[2]} ${h < 12 ? 'am' : 'pm'}`;
	}
</script>

<div class="tn" data-testid="smart-tonight">
	<div class="hd">
		<strong class="small">Tonight’s line-up</strong>
		<span class="grow"></span>
		<label class="sr-only" for="tn-date">Night</label>
		<input id="tn-date" class="input sm d" type="date" bind:value={date} />
		<label class="sr-only" for="tn-start">Start time</label>
		<input
			id="tn-start"
			class="input sm t"
			type="time"
			bind:value={start}
			title="Start time (default: your schedule)"
		/>
		<button
			class="btn sm ghost"
			onclick={() => (seed = Math.floor(Math.random() * 2 ** 31))}
			title="Shuffle the picks for this preview"><Dices size={14} /> Re-roll</button
		>
	</div>
	{#if error}
		<div class="small err" role="alert">{error}</div>
	{:else if data}
		{#each data.notes as n, i (i)}<div class="note small"><Info size={13} /> {n}</div>{/each}
		<ol class:dim={loading}>
			{#each data.items as it, i (it.id)}
				{@const x = info(it)}
				<li>
					<span class="tm num faint tiny">{at(data.startsAt?.[i])}</span>
					<x.icon size={14} />
					<span class="grow ellipsis small">{x.name}</span>
					{#if x.ms}<span class="faint tiny num">{fmtDuration(x.ms)}</span>{/if}
				</li>
			{:else}
				<li class="faint small">No songs yet — adjust the rules above.</li>
			{/each}
		</ol>
		<div class="faint tiny tot">
			{data.items.length} items · {fmtDuration(data.totalMs, { long: true })}{seed != null
				? ' · re-rolled (what plays uses the night’s own pick)'
				: ''}
		</div>
	{:else}
		<div class="faint small">Working it out…</div>
	{/if}
</div>

<style>
	.tn {
		display: flex;
		flex-direction: column;
		gap: 8px;
		padding: 12px;
		border-radius: 12px;
		background: var(--surface-2);
	}
	.hd {
		display: flex;
		align-items: center;
		gap: 8px;
		flex-wrap: wrap;
	}
	.d {
		width: 150px;
	}
	.t {
		width: 110px;
	}
	ol {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		transition: opacity 150ms;
	}
	ol.dim {
		opacity: 0.55;
	}
	li {
		display: flex;
		align-items: center;
		gap: 8px;
		min-height: 32px;
		border-bottom: 1px solid var(--border);
		color: var(--text-2);
	}
	li:last-child {
		border-bottom: 0;
	}
	.tm {
		width: 58px;
		flex: 0 0 auto;
	}
	.note {
		display: flex;
		gap: 6px;
		align-items: flex-start;
		color: var(--accent-text);
	}
	.err {
		color: var(--red);
	}
</style>
