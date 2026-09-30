<!--
	Settings → Seasons (F8, ARCHITECTURE §12.7). WS6.
	Named copies of the season-specific settings (schedule, looks, song requests, games,
	dimming, props kept dark, DJ voice). Switch now (with a preview of what changes), or let
	PixelPlus switch by date. The live season's schedule is edited on the Schedule page;
	switching saves it back into its season first.
-->
<script lang="ts">
	import {
		ArrowLeft,
		CalendarRange,
		Plus,
		Pencil,
		Trash2,
		ArrowRightLeft,
		Check,
		Info,
		Copy,
		FilePlus
	} from '@lucide/svelte';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import Switch from '$lib/components/ui/Switch.svelte';
	import Modal from '$lib/components/ui/Modal.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import Segmented from '$lib/components/ui/Segmented.svelte';
	import type { ShowProfile } from '$lib/api/types';
	import { app } from '$lib/stores/app.svelte';
	import { confirm, toasts } from '$lib/stores/toasts.svelte';
	import { profilesApi } from '$lib/insight/api';

	const show = $derived(app.show);
	const profiles = $derived(show?.profiles ?? []);
	const activeId = $derived(show?.activeProfileId);
	const ICONS = ['🎄', '🎃', '🎆', '❄️', '🦃', '🐰', '❤️', '🍀', '🕎', '⭐'];
	const COLORS = ['#e5484d', '#f5a524', '#3fcf8e', '#5b9dff', '#a88bfa', '#ff8bd1', '#ececef'];
	const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

	let busy = $state(false);
	let creating = $state(false);
	let newName = $state('');
	let newFrom = $state<'copy' | 'empty'>('copy');
	let editing = $state<ShowProfile | null>(null);
	let switching = $state<{ p: ShowProfile; lines: string[] } | null>(null);

	function fmtMd(md?: string) {
		if (!md) return '';
		const [m, d] = md.split('-').map(Number);
		return `${MONTHS[(m || 1) - 1]} ${d}`;
	}
	const rangeText = (p: ShowProfile) =>
		p.dateRange ? `${fmtMd(p.dateRange.start)} – ${fmtMd(p.dateRange.end)}` : 'No dates';

	async function run<R>(fn: () => Promise<R>, ok?: string): Promise<R | undefined> {
		busy = true;
		try {
			const r = await fn();
			if (ok) toasts.success(ok);
			await app.reloadShow();
			return r;
		} catch (e) {
			toasts.error('Something went wrong', (e as Error).message);
			return undefined;
		} finally {
			busy = false;
		}
	}

	async function create() {
		const name = newName.trim() || 'New season';
		const p = await run(
			() => (newFrom === 'copy' ? profilesApi.capture(name) : profilesApi.create({ name })),
			`Added ${name}`
		);
		if (p) {
			creating = false;
			newName = '';
			editing = structuredClone(p);
		}
	}

	async function askSwitch(p: ShowProfile) {
		try {
			const d = await profilesApi.preview(p.id);
			switching = { p, lines: d.lines };
		} catch (e) {
			toasts.error("Couldn't prepare the switch", (e as Error).message);
		}
	}

	async function doSwitch() {
		if (!switching) return;
		const p = switching.p;
		switching = null;
		await run(() => profilesApi.activate(p.id, true), `Switched to ${p.name}`);
	}

	async function saveEdit() {
		if (!editing) return;
		const e = editing;
		const patch: Partial<ShowProfile> = {
			name: e.name,
			icon: e.icon || undefined,
			color: e.color || undefined,
			dateRange: e.dateRange,
			priority: Number(e.priority) || 0,
			requestsPlaylistId: e.requestsPlaylistId || undefined,
			requestsMessage: e.requestsMessage || undefined,
			defaultDjVoice: e.defaultDjVoice || undefined,
			gamesEnabled: e.gamesEnabled,
			disabledPropIds: e.disabledPropIds ?? [],
			tags: e.tags ?? []
		};
		// JSON merge patch: null removes a field.
		const body: Record<string, unknown> = { ...patch };
		for (const k of [
			'icon',
			'color',
			'dateRange',
			'requestsPlaylistId',
			'requestsMessage',
			'defaultDjVoice',
			'gamesEnabled'
		])
			if (body[k] === undefined) body[k] = null;
		const r = await run(() => profilesApi.update(e.id, body as Partial<ShowProfile>), 'Saved');
		if (r) editing = null;
	}

	async function remove(p: ShowProfile) {
		const ok = await confirm({
			title: `Delete ${p.name}?`,
			message:
				p.id === activeId
					? 'The show keeps its current settings; they just stop belonging to a season.'
					: 'Its schedule and settings are deleted. Sequences and playlists stay.',
			confirmLabel: 'Delete',
			danger: true
		});
		if (ok) await run(() => profilesApi.remove(p.id), `Deleted ${p.name}`);
	}

	async function setAuto(v: boolean) {
		await run(() => profilesApi.autoSwitch(v), v ? 'Seasons switch by date' : 'Automatic switching off');
	}

	// Date-range editor helpers (MM-DD).
	function setRange(which: 'start' | 'end', month: number, day: number) {
		if (!editing) return;
		const md = `${String(month).padStart(2, '0')}-${String(Math.min(day, daysIn(month))).padStart(2, '0')}`;
		const r = editing.dateRange ?? { start: '11-01', end: '01-06' };
		editing.dateRange = { ...r, [which]: md };
	}
	const daysIn = (m: number) => [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31][m - 1];
	const part = (md: string | undefined, i: 0 | 1) => Number((md ?? '01-01').split('-')[i]);
	function toggleDark(id: string) {
		if (!editing) return;
		const cur = new Set(editing.disabledPropIds ?? []);
		if (cur.has(id)) cur.delete(id);
		else cur.add(id);
		editing.disabledPropIds = [...cur];
	}
	const gamesChoice = (v: boolean | undefined) => (v === undefined ? 'keep' : v ? 'on' : 'off');
</script>

<svelte:head><title>Seasons · Settings · PixelPlus</title></svelte:head>

<div class="page seasons">
	<a class="btn ghost sm back" href="/settings"><ArrowLeft size={16} /> Settings</a>
	<PageHeader
		title="Seasons"
		subtitle="Keep a Halloween and a Christmas show, and switch everything at once — or let PixelPlus switch by date."
	>
		{#snippet actions()}
			<button class="btn primary" onclick={() => (creating = true)} disabled={busy}
				><Plus size={16} /> New season</button
			>
		{/snippet}
	</PageHeader>

	{#if !show}
		<div class="card skeleton" style="height:160px"></div>
	{:else if profiles.length === 0}
		<section class="card">
			<EmptyState
				icon={CalendarRange}
				title="No seasons yet"
				message="Save what's set up now as your first season (for example “Christmas”). Then make another for Halloween and switch between them in one tap."
			>
				<button class="btn primary" onclick={() => (creating = true)}
					>Save the current show as a season</button
				>
			</EmptyState>
		</section>
	{:else}
		<section class="card">
			<div class="card-head">
				<CalendarRange size={18} />
				<h2 class="grow">Switch by date</h2>
				<Switch
					label="Switch seasons by date"
					checked={!!show.profileAutoSwitch}
					disabled={busy}
					onchange={setAuto}
				/>
			</div>
			<div class="card-body">
				<p class="muted small">
					Every day at noon (and after a restart), outside show times, PixelPlus switches to the season whose
					dates include today. It makes a backup first and sends you a message.
				</p>
			</div>
		</section>

		<div class="cards">
			{#each profiles as p (p.id)}
				{@const active = p.id === activeId}
				<article class="card season" class:active style:--season={p.color ?? 'var(--accent)'}>
					<div class="top">
						<span class="icon" aria-hidden="true">{p.icon ?? '📅'}</span>
						<div class="grow" style="min-width:0">
							<h3 class="ellipsis">{p.name}</h3>
							<div class="faint small">{rangeText(p)}{p.priority ? ` · priority ${p.priority}` : ''}</div>
						</div>
						{#if active}<span class="badge green"><Check size={12} /> Active</span>{/if}
					</div>
					<ul class="facts small muted">
						<li>{p.schedule.entries.length} show time{p.schedule.entries.length === 1 ? '' : 's'}</li>
						<li>
							Idle look: {show.effects.find((e) => e.id === p.schedule.idleEffectId)?.name ?? 'none'}
						</li>
						<li>
							Requests: {show.playlists.find((x) => x.id === p.requestsPlaylistId)?.name ?? 'all songs'}
						</li>
						{#if p.disabledPropIds?.length}<li>{p.disabledPropIds.length} props kept dark</li>{/if}
					</ul>
					<div class="row acts">
						{#if !active}
							<button class="btn sm" onclick={() => askSwitch(p)} disabled={busy}
								><ArrowRightLeft size={14} /> Switch now</button
							>
						{:else}
							<a class="btn sm ghost" href="/schedule">Edit schedule</a>
						{/if}
						<span class="grow"></span>
						<button
							class="btn sm ghost icon"
							aria-label="Edit {p.name}"
							onclick={() => (editing = structuredClone($state.snapshot(p)) as ShowProfile)}
							><Pencil size={14} /></button
						>
						<button class="btn sm ghost icon" aria-label="Delete {p.name}" onclick={() => remove(p)}
							><Trash2 size={14} /></button
						>
					</div>
				</article>
			{/each}
		</div>
		<div class="notice info">
			<Info size={18} />
			<div class="small">
				Sequences, songs, props and playlists are shared by all seasons. What a season changes: its show
				times, idle and off looks, the song-request list and message, games, late-night dimming, props kept
				dark and the default DJ voice. Edits on the Schedule and other pages belong to the active season.
			</div>
		</div>
	{/if}
</div>

<Modal bind:open={creating} title="New season" size="sm">
	<div class="col form">
		<label class="field"
			><span>Name</span><input
				class="input"
				bind:value={newName}
				placeholder="Halloween"
				maxlength="60"
			/></label
		>
		<Segmented
			label="Start from"
			bind:value={newFrom}
			options={[
				{ value: 'copy', label: 'Copy of what’s live', icon: Copy },
				{ value: 'empty', label: 'Start empty', icon: FilePlus }
			]}
		/>
		<p class="faint small">
			{newFrom === 'copy'
				? 'Takes the current schedule, looks, request list and dimming.'
				: 'No show times yet: add them on the Schedule page after switching to it.'}
		</p>
	</div>
	{#snippet footer()}
		<button class="btn ghost" onclick={() => (creating = false)}>Cancel</button>
		<button class="btn primary" onclick={create} disabled={busy}>Add season</button>
	{/snippet}
</Modal>

<Modal
	open={switching !== null}
	title={switching ? `Switch to ${switching.p.name}?` : ''}
	onclose={() => (switching = null)}
	size="sm"
>
	{#if switching}
		<ul class="diff">
			{#each switching.lines as l (l)}<li>{l}</li>{/each}
		</ul>
		<p class="faint small">The current season keeps your latest edits. A backup is made first.</p>
	{/if}
	{#snippet footer()}
		<button class="btn ghost" onclick={() => (switching = null)}>Cancel</button>
		<button
			class="btn primary"
			onclick={doSwitch}
			disabled={busy || !!switching?.lines[0]?.startsWith("Can't")}>Switch</button
		>
	{/snippet}
</Modal>

<Modal open={editing !== null} title="Edit season" onclose={() => (editing = null)} size="lg">
	{#if editing && show}
		<div class="edit">
			<label class="field"
				><span>Name</span><input class="input" bind:value={editing.name} maxlength="60" /></label
			>
			<div class="field">
				<span>Icon</span>
				<div class="row wrap picks">
					{#each ICONS as i (i)}
						<button
							type="button"
							class="pick"
							class:on={editing.icon === i}
							aria-pressed={editing.icon === i}
							aria-label="Icon {i}"
							onclick={() => editing && (editing.icon = i)}>{i}</button
						>
					{/each}
				</div>
			</div>
			<div class="field">
				<span>Colour</span>
				<div class="row wrap picks">
					{#each COLORS as c (c)}
						<button
							type="button"
							class="swatch-btn"
							class:on={editing.color === c}
							style:background={c}
							aria-pressed={editing.color === c}
							aria-label="Colour {c}"
							onclick={() => editing && (editing.color = c)}
						></button>
					{/each}
				</div>
			</div>
			<div class="field">
				<span>Dates (for switching by date)</span>
				{#if editing.dateRange}
					<div class="row wrap">
						{#each ['start', 'end'] as const as w (w)}
							<select
								class="select sm"
								style="width:auto"
								aria-label="{w} month"
								value={part(editing.dateRange[w], 0)}
								onchange={(ev) =>
									setRange(w, Number(ev.currentTarget.value), part(editing?.dateRange?.[w], 1))}
							>
								{#each MONTHS as m, i (m)}<option value={i + 1}>{m}</option>{/each}
							</select>
							<input
								class="input sm num"
								style="width:64px"
								type="number"
								min="1"
								max="31"
								aria-label="{w} day"
								value={part(editing.dateRange[w], 1)}
								onchange={(ev) =>
									setRange(w, part(editing?.dateRange?.[w], 0), Number(ev.currentTarget.value))}
							/>
							{#if w === 'start'}<span class="muted">to</span>{/if}
						{/each}
						<button
							class="btn sm ghost"
							type="button"
							onclick={() => editing && (editing.dateRange = undefined)}>No dates</button
						>
					</div>
				{:else}
					<button
						class="btn sm"
						type="button"
						style="align-self:flex-start"
						onclick={() => editing && (editing.dateRange = { start: '11-01', end: '01-06' })}
						>Add dates</button
					>
				{/if}
			</div>
			<label class="field"
				><span>Priority when dates overlap (higher wins)</span><input
					class="input num"
					style="width:90px"
					type="number"
					bind:value={editing.priority}
				/></label
			>
			<label class="field"
				><span>Song requests from</span>
				<select class="select" bind:value={editing.requestsPlaylistId}>
					<option value={undefined}>All songs</option>
					{#each show.playlists as pl (pl.id)}<option value={pl.id}>{pl.name}</option>{/each}
				</select>
			</label>
			<label class="field"
				><span>Request page message</span><textarea
					class="textarea"
					rows="2"
					bind:value={editing.requestsMessage}></textarea></label
			>
			<div class="field">
				<span>Visitor games</span>
				<Segmented
					label="Visitor games"
					size="sm"
					value={gamesChoice(editing.gamesEnabled)}
					onchange={(v) => editing && (editing.gamesEnabled = v === 'keep' ? undefined : v === 'on')}
					options={[
						{ value: 'keep', label: 'Leave as is' },
						{ value: 'on', label: 'On' },
						{ value: 'off', label: 'Off' }
					]}
				/>
			</div>
			<label class="field"
				><span>Default DJ voice</span>
				<select class="select" bind:value={editing.defaultDjVoice}>
					<option value={undefined}>No preference</option>
					{#each show.djVoices as v (v.id)}<option value={v.id}>{v.name}</option>{/each}
				</select>
			</label>
			<div class="field wide">
				<span>Props kept dark this season</span>
				{#if show.props.length}
					<div class="props">
						{#each show.props as pr (pr.id)}
							<label class="prop small">
								<input
									type="checkbox"
									checked={editing.disabledPropIds?.includes(pr.id) ?? false}
									onchange={() => toggleDark(pr.id)}
								/>
								{pr.name}
							</label>
						{/each}
					</div>
				{:else}
					<span class="faint small">No props yet.</span>
				{/if}
			</div>
		</div>
	{/if}
	{#snippet footer()}
		<button class="btn ghost" onclick={() => (editing = null)}>Cancel</button>
		<button class="btn primary" onclick={saveEdit} disabled={busy || !editing?.name.trim()}>Save</button>
	{/snippet}
</Modal>

<style>
	.seasons {
		max-width: 980px;
		display: flex;
		flex-direction: column;
		gap: 16px;
	}
	.back {
		align-self: flex-start;
		margin-bottom: -8px;
	}
	.cards {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(260px, 1fr));
		gap: 16px;
	}
	.season {
		padding: 16px;
		display: flex;
		flex-direction: column;
		gap: 12px;
		box-shadow: inset 0 3px 0 var(--season);
	}
	.season.active {
		border-color: var(--accent-line);
	}
	.top {
		display: flex;
		align-items: center;
		gap: 10px;
	}
	.icon {
		font-size: 26px;
		line-height: 1;
	}
	h3 {
		margin: 0;
		font-size: 16px;
	}
	.facts {
		margin: 0;
		padding-left: 18px;
		display: flex;
		flex-direction: column;
		gap: 2px;
	}
	.acts {
		gap: 6px;
		align-items: center;
	}
	.form {
		gap: 12px;
	}
	.field {
		display: flex;
		flex-direction: column;
		gap: 6px;
		font-size: 13px;
	}
	.field > span {
		color: var(--text-2);
	}
	.edit {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: 14px 20px;
	}
	.edit .wide {
		grid-column: 1 / -1;
	}
	.picks {
		gap: 6px;
	}
	.pick {
		width: 36px;
		height: 36px;
		border-radius: var(--r-2);
		border: 1px solid var(--border-2);
		background: var(--surface-2);
		font-size: 18px;
	}
	.pick.on,
	.swatch-btn.on {
		box-shadow: var(--ring);
	}
	.swatch-btn {
		width: 28px;
		height: 28px;
		border-radius: 50%;
		border: 1px solid var(--border-3);
	}
	.props {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
		gap: 6px 12px;
		max-height: 220px;
		overflow: auto;
	}
	.prop {
		display: flex;
		gap: 8px;
		align-items: center;
	}
	.diff {
		margin: 0 0 12px;
		padding-left: 18px;
		display: flex;
		flex-direction: column;
		gap: 4px;
	}
	@media (max-width: 700px) {
		.edit {
			grid-template-columns: 1fr;
		}
	}
</style>
