<script lang="ts">
	import { api } from '$lib/api/client';
	import type { EffectKind, EffectPreset, EffectSchema } from '$lib/api/types';
	import { EFFECT_KINDS } from '$lib/api/types';
	import { app } from '$lib/stores/app.svelte';
	import { toasts, confirm } from '$lib/stores/toasts.svelte';
	import { DEFAULT_EFFECT_SCHEMA, EFFECT_META, defaultParams } from '$lib/effects/render';
	import { newId } from '$lib/util/id';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import Drawer from '$lib/components/ui/Drawer.svelte';
	import EffectPreview from '$lib/components/viz/EffectPreview.svelte';
	import ParamEditor from '$lib/components/effects/ParamEditor.svelte';
	import BeatPulse from '$lib/components/effects/BeatPulse.svelte';
	import SaveState from '$lib/components/ui/SaveState.svelte';
	import { Radio, Square, Trash2, WandSparkles, Plus, Check } from '@lucide/svelte';

	const show = $derived(app.show);
	// Beat-reactive params (F2) get their own "Pulse to a beat" editor.
	const BEAT_KEYS = ['beatBpm', 'beatPhaseMs', 'beatDepth', 'beatDecayMs', 'beatFollowSong'];
	const editable = (kind: EffectKind) => (schema[kind] ?? []).filter((s) => !BEAT_KEYS.includes(s.key));
	let schema = $state<EffectSchema>(DEFAULT_EFFECT_SCHEMA);
	let draft = $state<EffectPreset | null>(null);
	let open = $state(false);
	let liveId = $state<string | null>(null);
	let liveTimer: ReturnType<typeof setTimeout>;

	$effect(() => {
		api.effects
			.schema()
			.then((s) => (schema = { ...DEFAULT_EFFECT_SCHEMA, ...s }))
			.catch(() => {});
	});

	const isSaved = $derived(!!draft && !!show?.effects.some((e) => e.id === draft!.id));
	const live = $derived(app.status?.state === 'effect');
	const isLive = $derived(live && !!draft && liveId === draft.id);

	function openPreset(p: EffectPreset) {
		draft = structuredClone($state.snapshot(p) as EffectPreset);
		draft.params = { ...defaultParams(schema[draft.effect] ?? []), ...draft.params };
		lastSaved = JSON.stringify(draft);
		fxSave = 'saved';
		open = true;
	}

	// Saved looks save themselves as you edit (a new look is saved with "Save as look").
	let fxSave = $state<'saved' | 'saving' | 'dirty'>('saved');
	let lastSaved = '';
	let fxTimer: ReturnType<typeof setTimeout> | undefined;
	$effect(() => {
		if (!draft || !open) return;
		const now = JSON.stringify(draft);
		if (!isSaved || now === lastSaved) return;
		fxSave = 'dirty';
		clearTimeout(fxTimer);
		fxTimer = setTimeout(async () => {
			if (!draft) return;
			fxSave = 'saving';
			const d = $state.snapshot(draft) as EffectPreset;
			try {
				await api.effects.update(d.id, d);
				lastSaved = JSON.stringify(d);
				await app.reloadShow();
				fxSave = 'saved';
			} catch (e) {
				fxSave = 'dirty';
				toasts.error('Couldn’t save the look', (e as Error).message);
			}
		}, 700);
	});
	function startFrom(kind: EffectKind) {
		draft = {
			id: newId(),
			name: `My ${EFFECT_META[kind].label.toLowerCase()}`,
			effect: kind,
			params: defaultParams(schema[kind] ?? []),
			target: { all: true }
		};
		open = true;
	}
	function changeKind(kind: EffectKind) {
		if (!draft) return;
		draft.effect = kind;
		draft.params = { ...defaultParams(schema[kind] ?? []) };
		paramsChanged();
	}

	function paramsChanged() {
		if (!isLive) return;
		clearTimeout(liveTimer);
		liveTimer = setTimeout(applyLive, 250);
	}

	async function applyLive() {
		if (!draft) return;
		try {
			await api.applyEffect($state.snapshot(draft) as EffectPreset);
			if (liveId !== draft.id)
				toasts.push({
					kind: 'info',
					message: `“${draft.name}” is on the display`,
					action: { label: 'Stop', run: stopLive }
				});
			liveId = draft.id;
		} catch (e) {
			toasts.error('Couldn’t apply the effect', (e as Error).message);
		}
	}
	async function stopLive() {
		await api.applyEffect(null).catch(() => {});
		liveId = null;
	}
	async function save() {
		if (!draft) return;
		const d = $state.snapshot(draft) as EffectPreset;
		lastSaved = JSON.stringify(d);
		await app.mutate(() => api.effects.create(d), { success: `Saved “${d.name}” to your looks` });
		fxSave = 'saved';
	}
	async function remove() {
		if (!draft) return;
		const d = structuredClone($state.snapshot(draft) as EffectPreset);
		if (!(await confirm({ title: `Delete “${d.name}”?`, confirmLabel: 'Delete', danger: true }))) return;
		await app.mutate(() => api.effects.remove(d.id));
		open = false;
		toasts.success(`Deleted ${d.name}`, {
			label: 'Undo',
			run: () => app.mutate(() => api.effects.create(d))
		});
	}
	async function quickApply(p: EffectPreset) {
		try {
			if (live && liveId === p.id) return stopLive();
			await api.applyEffect($state.snapshot(p) as EffectPreset);
			liveId = p.id;
		} catch (e) {
			toasts.error('Couldn’t apply the effect', (e as Error).message);
		}
	}

	function targetMode(): 'all' | 'groups' | 'props' {
		if (!draft || draft.target.all) return 'all';
		if (draft.target.propIds?.length) return 'props';
		return 'groups';
	}
	function toggleIn(list: 'groupIds' | 'propIds', id: string) {
		if (!draft) return;
		const cur = draft.target[list] ?? [];
		draft.target = {
			...draft.target,
			all: false,
			[list]: cur.includes(id) ? cur.filter((x) => x !== id) : [...cur, id]
		};
		paramsChanged();
	}
	function targetLabel(p: EffectPreset) {
		if (p.target.all || (!p.target.groupIds?.length && !p.target.propIds?.length)) return 'Whole display';
		const g =
			p.target.groupIds?.map((id) => show?.propGroups.find((x) => x.id === id)?.name).filter(Boolean) ?? [];
		const n = p.target.propIds?.length ?? 0;
		return [...g, n ? `${n} props` : ''].filter(Boolean).join(', ');
	}
</script>

<div class="page">
	<PageHeader
		title="Effects"
		subtitle="Ready-made looks you can put on the display right now, use as the idle look, or add to playlists."
	>
		{#snippet actions()}
			{#if live}<button class="btn" onclick={stopLive}><Square size={14} /> Stop live effect</button>{/if}
		{/snippet}
	</PageHeader>

	<div class="section-title" style="margin-top:0">
		<h2>Your looks</h2>
		<span class="faint small">{show?.effects.length ?? 0}</span>
	</div>
	{#if show && !show.effects.length}
		<p class="muted small empty-looks">
			No saved looks yet. Pick an effect below, make it yours, and save it — then use it as the idle look or
			in a playlist.
		</p>
	{/if}
	<div class="gallery">
		{#each show?.effects ?? [] as p (p.id)}
			{@const on = live && liveId === p.id}
			<article class="card fx interactive" class:on>
				<button class="pv" onclick={() => openPreset(p)} aria-label="Edit {p.name}"
					><EffectPreview kind={p.effect} params={p.params} height={130} /></button
				>
				<div class="meta">
					<button class="grow meta-open" onclick={() => openPreset(p)} tabindex="-1" aria-hidden="true">
						<div class="name ellipsis">{p.name}</div>
						<div class="faint tiny ellipsis">{EFFECT_META[p.effect].label} · {targetLabel(p)}</div>
					</button>
					<button class="btn sm {on ? 'primary' : ''}" onclick={() => quickApply(p)} aria-pressed={on}>
						{#if on}<Check size={14} /> Live{:else}<Radio size={14} /> Apply{/if}
					</button>
				</div>
			</article>
		{/each}
	</div>

	<div class="section-title"><h2>Start from an effect</h2></div>
	<div class="gallery small">
		{#each EFFECT_KINDS as k (k)}
			<button class="card fx base interactive" onclick={() => startFrom(k)}>
				<EffectPreview kind={k} params={defaultParams(schema[k] ?? [])} height={96} />
				<div class="meta col" style="align-items:flex-start;gap:2px">
					<div class="name row" style="gap:6px"><WandSparkles size={13} /> {EFFECT_META[k].label}</div>
					<div class="faint tiny">{EFFECT_META[k].blurb}</div>
				</div>
			</button>
		{/each}
	</div>
</div>

<Drawer bind:open width={600} title={draft?.name}>
	{#snippet header()}
		{#if draft}
			<input class="title-input" bind:value={draft.name} aria-label="Look name" />
			<div class="faint small drawer-sub">
				{#if isSaved}<SaveState state={fxSave} />{:else}New look — not saved yet{/if}{isLive
					? ' · live on the display'
					: ''}
			</div>
		{/if}
	{/snippet}
	{#if draft}
		<div class="bigpv card"><EffectPreview kind={draft.effect} params={draft.params} height={180} /></div>
		<p class="faint tiny" style="margin:-8px 0 16px">
			Preview is approximate — press <strong>Try it live</strong> to see it on your props.
		</p>

		<label class="field" style="margin-bottom:18px"
			><span class="label">Effect</span>
			<select
				class="select"
				value={draft.effect}
				onchange={(e) => changeKind((e.target as HTMLSelectElement).value as EffectKind)}
			>
				{#each EFFECT_KINDS as k (k)}<option value={k}>{EFFECT_META[k].label}</option>{/each}
			</select>
		</label>

		<ParamEditor schema={editable(draft.effect)} bind:params={draft.params} onchange={paramsChanged} />
		<BeatPulse bind:params={draft.params} onchange={paramsChanged} />

		<div class="field" style="margin-top:22px">
			<span class="label">Show it on</span>
			<div class="tmode">
				<button
					type="button"
					class="chip"
					aria-pressed={targetMode() === 'all'}
					onclick={() => {
						if (draft) {
							draft.target = { all: true };
							paramsChanged();
						}
					}}>Whole display</button
				>
				<button
					type="button"
					class="chip"
					aria-pressed={targetMode() === 'groups'}
					onclick={() => {
						if (draft) draft.target = { all: false, groupIds: draft.target.groupIds ?? [] };
					}}>Groups</button
				>
				<button
					type="button"
					class="chip"
					aria-pressed={targetMode() === 'props'}
					onclick={() => {
						if (draft)
							draft.target = {
								all: false,
								propIds: draft.target.propIds?.length ? draft.target.propIds : [show?.props[0]?.id ?? '']
							};
					}}>Pick props</button
				>
			</div>
			{#if targetMode() === 'groups'}
				<div class="row wrap" style="margin-top:8px">
					{#each show?.propGroups ?? [] as g (g.id)}<button
							type="button"
							class="chip"
							aria-pressed={draft.target.groupIds?.includes(g.id)}
							onclick={() => toggleIn('groupIds', g.id)}
							><span class="gd" style:background={g.color}></span>{g.name}</button
						>{/each}
				</div>
			{:else if targetMode() === 'props'}
				<div class="plist">
					{#each show?.props ?? [] as p (p.id)}
						<label class="pi"
							><input
								type="checkbox"
								class="check"
								checked={draft.target.propIds?.includes(p.id)}
								onchange={() => toggleIn('propIds', p.id)}
							/>
							{p.name}</label
						>
					{/each}
				</div>
			{/if}
		</div>
	{/if}
	{#snippet footer()}
		{#if isSaved}<button class="btn danger icon" onclick={remove} aria-label="Delete look"
				><Trash2 size={15} /></button
			>{/if}
		<span class="grow"></span>
		{#if isLive}
			<button class="btn" onclick={stopLive}><Square size={14} /> Stop</button>
		{:else}
			<button class="btn soft" onclick={applyLive}><Radio size={15} /> Try it live</button>
		{/if}
		{#if !isSaved}
			<button class="btn primary" onclick={save}><Plus size={15} /> Save as look</button>
		{/if}
	{/snippet}
</Drawer>

<style>
	.empty-looks {
		padding: 16px 18px;
		border-radius: var(--r-3);
		border: 1px dashed var(--border-3);
	}
	.meta-open {
		text-align: left;
		min-width: 0;
	}
	.drawer-sub {
		display: flex;
		align-items: center;
		gap: 6px;
		margin-top: 2px;
	}
	.gallery {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(240px, 1fr));
		gap: 14px;
	}
	.gallery.small {
		grid-template-columns: repeat(auto-fill, minmax(190px, 1fr));
	}
	.fx {
		overflow: hidden;
		display: flex;
		flex-direction: column;
		text-align: left;
	}
	.fx.on {
		border-color: var(--accent);
		box-shadow:
			0 0 0 1px var(--accent),
			0 8px 30px rgba(245, 165, 36, 0.15);
	}
	.pv {
		display: block;
		width: 100%;
	}
	.meta {
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 12px 14px;
		border-top: 1px solid var(--border);
	}
	.name {
		font-weight: 590;
		font-size: 13.5px;
	}
	.base .name {
		font-size: 13px;
	}
	.title-input {
		font-size: 19px;
		font-weight: 650;
		letter-spacing: -0.02em;
		background: transparent;
		border: 1px solid transparent;
		border-radius: 8px;
		padding: 2px 6px;
		margin-left: -6px;
		width: 100%;
	}
	.title-input:hover {
		border-color: var(--border-2);
	}
	.title-input:focus {
		border-color: var(--accent-line);
	}
	.bigpv {
		overflow: hidden;
		margin-bottom: 16px;
	}
	.tmode {
		display: flex;
		gap: 6px;
		flex-wrap: wrap;
	}
	.gd {
		width: 8px;
		height: 8px;
		border-radius: 50%;
	}
	.plist {
		margin-top: 8px;
		max-height: 220px;
		overflow: auto;
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: 4px;
		padding: 8px;
		border-radius: 10px;
		border: 1px solid var(--border);
	}
	.pi {
		display: flex;
		align-items: center;
		gap: 8px;
		font-size: 13px;
		padding: 6px;
		border-radius: 6px;
		cursor: pointer;
	}
	.pi:hover {
		background: var(--surface-2);
	}
	@media (max-width: 640px) {
		.gallery,
		.gallery.small {
			grid-template-columns: repeat(2, minmax(0, 1fr));
			gap: 10px;
		}
		.meta {
			flex-wrap: wrap;
		}
		.meta .btn {
			width: 100%;
		}
	}
</style>
