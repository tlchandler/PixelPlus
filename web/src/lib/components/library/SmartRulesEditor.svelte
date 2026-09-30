<!--
	Smart playlist rules in plain language (F18). Edits `rules` in place and calls
	`onchange` after every change.
-->
<script lang="ts">
	import type { PlaylistItem, Show, SmartOrder, SmartRules, TimeSpec } from '$lib/api/types';
	import { newId } from '$lib/util/id';
	import { allTags } from '$lib/library/tags';
	import TagChips from './TagChips.svelte';
	import TimeSpecPicker from '$lib/components/schedule/TimeSpecPicker.svelte';
	import Switch from '$lib/components/ui/Switch.svelte';
	import { Plus, X, Tag, Timer, History, Clock, ArrowDownUp, Pin, Mic } from '@lucide/svelte';

	let {
		rules = $bindable(),
		show,
		onchange
	}: { rules: SmartRules; show: Show; onchange?: () => void } = $props();

	const tags = $derived(allTags(show).map((t) => t.name));
	const defs = $derived(show.tagDefs ?? []);
	const songs = $derived(show.sequences);
	const clips = $derived(show.djClips);

	function set(patch: Partial<SmartRules>) {
		rules = { ...rules, ...patch };
		onchange?.();
	}
	function toggleIn(list: string[], t: string) {
		return list.includes(t) ? list.filter((x) => x !== t) : [...list, t];
	}
	const minutes = $derived(rules.targetDurationMs ? Math.round(rules.targetDurationMs / 60000) : 45);

	const ORDERS: { value: SmartOrder; label: string; help: string }[] = [
		{
			value: 'leastRecent',
			label: 'Least recently played first',
			help: 'Songs that haven’t played in a while go first.'
		},
		{
			value: 'rotation',
			label: 'Rotate through all songs',
			help: 'Picks up where last night left off, so every song gets its turn.'
		},
		{ value: 'shuffle', label: 'Shuffle', help: 'A new order every night (the same all night long).' },
		{ value: 'fixed', label: 'Library order', help: 'In the order of your Sequences page.' }
	];

	function addTimeRule() {
		const t: TimeSpec = { kind: 'clock', time: '19:00' };
		set({
			timeRules: [...rules.timeRules, { before: t, requireTags: tags.includes('kids') ? ['kids'] : [] }]
		});
	}
	function pinned(which: 'pinnedFirst' | 'pinnedLast', sequenceId: string) {
		if (!sequenceId) return;
		const item: PlaylistItem = { id: newId(), type: 'sequence', sequenceId };
		set({ [which]: [...rules[which], item] } as Partial<SmartRules>);
	}
	function unpin(which: 'pinnedFirst' | 'pinnedLast', id: string) {
		set({ [which]: rules[which].filter((i) => i.id !== id) } as Partial<SmartRules>);
	}
	function nameOf(it: PlaylistItem) {
		if (it.type === 'sequence') return songs.find((s) => s.id === it.sequenceId)?.name ?? 'Missing song';
		if (it.type === 'dj') return clips.find((c) => c.id === it.djClipId)?.name ?? 'Missing clip';
		return it.type;
	}
	const djId = $derived(rules.interleave[0]?.type === 'dj' ? rules.interleave[0].djClipId : '');
	function setDj(id: string, every = rules.interleaveEvery || 3) {
		set(
			id
				? { interleave: [{ id: newId(), type: 'dj', djClipId: id }], interleaveEvery: every }
				: { interleave: [], interleaveEvery: 0 }
		);
	}
</script>

<div class="rules" data-testid="smart-rules">
	<!-- include -->
	<div class="rule">
		<span class="ri"><Tag size={15} /></span>
		<div class="rb">
			<div class="rt">
				Include songs tagged
				{#if rules.includeTags.length > 1}
					<select
						class="select sm inline"
						value={rules.includeMode}
						onchange={(e) => set({ includeMode: (e.target as HTMLSelectElement).value as 'any' | 'all' })}
						aria-label="Match any or all tags"
					>
						<option value="any">any of</option>
						<option value="all">all of</option>
					</select>
				{/if}
				{#if !rules.includeTags.length}<span class="faint">(any tag — every song)</span>{/if}
			</div>
			<TagChips
				{tags}
				{defs}
				active={rules.includeTags}
				onpick={(t) => set({ includeTags: toggleIn(rules.includeTags, t) })}
			/>
			{#if !tags.length}<div class="faint tiny">
					Tag songs on the Sequences page (e.g. kids, classic) to pick them here.
				</div>{/if}
		</div>
	</div>

	<!-- exclude -->
	{#if tags.length}
		<div class="rule">
			<span class="ri"><X size={15} /></span>
			<div class="rb">
				<div class="rt">
					Leave out songs tagged {#if !rules.excludeTags.length}<span class="faint">(nothing left out)</span
						>{/if}
				</div>
				<TagChips
					{tags}
					{defs}
					active={rules.excludeTags}
					onpick={(t) => set({ excludeTags: toggleIn(rules.excludeTags, t) })}
				/>
			</div>
		</div>
	{/if}

	<!-- length -->
	<div class="rule">
		<span class="ri"><Timer size={15} /></span>
		<div class="rb row wrap">
			<label class="row lbl">
				<Switch
					checked={!!rules.targetDurationMs}
					label="Limit the total length"
					size="sm"
					onchange={(on: boolean) => set({ targetDurationMs: on ? minutes * 60000 : undefined })}
				/>
				<span>Total length about</span>
			</label>
			<input
				class="input sm num w"
				type="number"
				min="5"
				max="600"
				value={minutes}
				disabled={!rules.targetDurationMs}
				onchange={(e) =>
					set({ targetDurationMs: Math.max(1, Number((e.target as HTMLInputElement).value)) * 60000 })}
				aria-label="Total minutes"
			/>
			<span>min</span>
		</div>
	</div>

	<!-- no repeats -->
	<div class="rule">
		<span class="ri"><History size={15} /></span>
		<div class="rb row wrap">
			<span>Don’t repeat songs from the last</span>
			<select
				class="select sm inline"
				value={String(rules.noRepeatNights)}
				onchange={(e) => set({ noRepeatNights: Number((e.target as HTMLSelectElement).value) })}
				aria-label="Nights without repeats"
			>
				<option value="0">— (repeats are fine)</option>
				{#each [1, 2, 3, 4, 5, 6, 7] as n (n)}<option value={String(n)}
						>{n} {n === 1 ? 'night' : 'nights'}</option
					>{/each}
			</select>
		</div>
	</div>

	<!-- time rules -->
	{#each rules.timeRules as tr, i (i)}
		<div class="rule">
			<span class="ri"><Clock size={15} /></span>
			<div class="rb">
				<div class="rt row wrap">
					<span>Before</span>
					<div class="tsp">
						<TimeSpecPicker
							bind:value={
								() => tr.before,
								(v) => set({ timeRules: rules.timeRules.map((x, k) => (k === i ? { ...x, before: v } : x)) })
							}
							location={show.schedule.location}
							label="Rule time"
						/>
					</div>
					<span>only songs tagged</span>
					<button
						class="btn ghost icon sm"
						onclick={() => set({ timeRules: rules.timeRules.filter((_, k) => k !== i) })}
						aria-label="Remove this time rule"><X size={14} /></button
					>
				</div>
				<TagChips
					{tags}
					{defs}
					active={tr.requireTags}
					onpick={(t) =>
						set({
							timeRules: rules.timeRules.map((x, k) =>
								k === i ? { ...x, requireTags: toggleIn(x.requireTags, t) } : x
							)
						})}
				/>
			</div>
		</div>
	{/each}
	<button class="addrule" onclick={addTimeRule} disabled={!tags.length}
		><Plus size={14} /> Add a time rule (e.g. kids songs before 7 pm)</button
	>

	<!-- order -->
	<div class="rule">
		<span class="ri"><ArrowDownUp size={15} /></span>
		<div class="rb">
			<div class="rt row wrap">
				<span>Order:</span>
				<select
					class="select sm inline"
					value={rules.order}
					onchange={(e) => set({ order: (e.target as HTMLSelectElement).value as SmartOrder })}
					aria-label="Song order"
				>
					{#each ORDERS as o (o.value)}<option value={o.value}>{o.label}</option>{/each}
				</select>
			</div>
			<div class="faint tiny">{ORDERS.find((o) => o.value === rules.order)?.help}</div>
		</div>
	</div>

	<!-- pinned -->
	{#each [['pinnedFirst', 'Always start with'], ['pinnedLast', 'Always end with']] as [key, label] (key)}
		{@const k = key as 'pinnedFirst' | 'pinnedLast'}
		<div class="rule">
			<span class="ri"><Pin size={15} /></span>
			<div class="rb row wrap">
				<span>{label}</span>
				{#each rules[k] as it (it.id)}
					<span class="pin"
						>{nameOf(it)}<button onclick={() => unpin(k, it.id)} aria-label="Remove {nameOf(it)}"
							><X size={12} /></button
						></span
					>
				{/each}
				<select
					class="select sm inline"
					value=""
					onchange={(e) => {
						pinned(k, (e.target as HTMLSelectElement).value);
						(e.target as HTMLSelectElement).value = '';
					}}
					aria-label="{label} a song"
				>
					<option value="">{rules[k].length ? '+ another song' : 'pick a song…'}</option>
					{#each songs as s (s.id)}<option value={s.id}>{s.name}</option>{/each}
				</select>
			</div>
		</div>
	{/each}

	<!-- interleave -->
	{#if clips.length}
		<div class="rule">
			<span class="ri"><Mic size={15} /></span>
			<div class="rb row wrap">
				<span>Every</span>
				<select
					class="select sm inline"
					value={String(rules.interleaveEvery || 3)}
					onchange={(e) => setDj(djId, Number((e.target as HTMLSelectElement).value))}
					disabled={!djId}
					aria-label="How many songs between DJ clips"
				>
					{#each [1, 2, 3, 4, 5, 6] as n (n)}<option value={String(n)}>{n}</option>{/each}
				</select>
				<span>songs, play</span>
				<select
					class="select sm inline"
					value={djId}
					onchange={(e) => setDj((e.target as HTMLSelectElement).value)}
					aria-label="DJ clip between songs"
				>
					<option value="">no DJ clip</option>
					{#each clips as c (c.id)}<option value={c.id}>{c.name}</option>{/each}
				</select>
			</div>
		</div>
	{/if}
</div>

<style>
	.rules {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}
	.rule {
		display: flex;
		gap: 12px;
		padding: 10px 0;
		border-bottom: 1px solid var(--border);
	}
	.ri {
		width: 28px;
		height: 28px;
		border-radius: 8px;
		display: grid;
		place-items: center;
		background: var(--surface-2);
		color: var(--text-2);
		flex: 0 0 auto;
	}
	.rb {
		flex: 1;
		min-width: 0;
		display: flex;
		flex-direction: column;
		gap: 8px;
		font-size: 13.5px;
	}
	.rb.row {
		flex-direction: row;
		align-items: center;
		gap: 8px;
	}
	.rt {
		display: flex;
		align-items: center;
		gap: 8px;
		flex-wrap: wrap;
	}
	.lbl {
		display: inline-flex;
		align-items: center;
		gap: 8px;
		cursor: pointer;
	}
	.inline {
		width: auto;
		min-width: 0;
		max-width: 100%;
	}
	.w {
		width: 76px;
	}
	.tsp {
		min-width: 0;
	}
	.pin {
		display: inline-flex;
		align-items: center;
		gap: 4px;
		height: 28px;
		padding: 0 4px 0 10px;
		border-radius: 99px;
		background: var(--surface-2);
		border: 1px solid var(--border);
		font-size: 12.5px;
	}
	.pin button {
		display: grid;
		place-items: center;
		width: 22px;
		height: 22px;
		border-radius: 50%;
		color: var(--text-3);
	}
	.addrule {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		align-self: flex-start;
		margin: 6px 0 4px 40px;
		min-height: 36px;
		font-size: 13px;
		font-weight: 560;
		color: var(--accent-text);
	}
	.addrule:disabled {
		color: var(--text-3);
	}
</style>
