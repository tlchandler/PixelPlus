<script lang="ts">
	import { untrack } from 'svelte';
	import { api } from '$lib/api/client';
	import type { CountdownItem, Playlist, PlaylistItem, Show, SmartRules } from '$lib/api/types';
	import { app } from '$lib/stores/app.svelte';
	import { toasts, confirm } from '$lib/stores/toasts.svelte';
	import { fmtDuration, plural } from '$lib/util/format';
	import { newId } from '$lib/util/id';
	import { playerAct } from '$lib/player';
	import { sortable, moveItem } from '$lib/actions/sortable';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import Switch from '$lib/components/ui/Switch.svelte';
	import Modal from '$lib/components/ui/Modal.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import Skeleton from '$lib/components/ui/Skeleton.svelte';
	import Segmented from '$lib/components/ui/Segmented.svelte';
	import SaveState from '$lib/components/ui/SaveState.svelte';
	import SmartRulesEditor from '$lib/components/library/SmartRulesEditor.svelte';
	import SmartTonight from '$lib/components/library/SmartTonight.svelte';
	import CountdownItemEditor from '$lib/components/playlist/CountdownItemEditor.svelte';
	import { newCountdownItem } from '$lib/playlist/countdown';
	import {
		Plus,
		Play,
		Trash2,
		Copy,
		Shuffle,
		Repeat,
		GripVertical,
		Music,
		Mic,
		WandSparkles,
		Clock,
		Terminal,
		AudioLines,
		X,
		ListMusic,
		Gamepad2,
		Search,
		Check,
		Sparkles,
		Timer,
		Pencil
	} from '@lucide/svelte';

	type Section = 'intro' | 'items' | 'outro';
	const show = $derived(app.show);
	let selectedId = $state<string | null>(null);
	let draft = $state<Playlist | null>(null);
	let saveState = $state<'saved' | 'saving' | 'dirty'>('saved');
	let saveTimer: ReturnType<typeof setTimeout>;
	let target = $state<Section>('items');
	let libTab = $state<'sequence' | 'dj' | 'effect' | 'media' | 'more'>('sequence');
	let libQ = $state('');
	let addOpen = $state(false);
	let dragOver = $state<Section | null>(null);
	/** Length of tonight's smart line-up (from the preview). */
	let smartTotal = $state(0);
	/** Rules kept while "Smart" is switched off, so switching back restores them. */
	let stashedRules: SmartRules | null = null;
	let editing = $state<{ section: Section; id: string; item: CountdownItem } | null>(null);

	function defaultRules(): SmartRules {
		return {
			includeTags: [],
			includeMode: 'any',
			excludeTags: [],
			noRepeatNights: 1,
			timeRules: [],
			order: 'leastRecent',
			pinnedFirst: [],
			pinnedLast: [],
			interleave: [],
			interleaveEvery: 0
		};
	}
	function setSmart(on: boolean) {
		if (!draft) return;
		if (on) {
			draft.smart = stashedRules ?? defaultRules();
			if (target === 'items') target = 'intro';
		} else {
			stashedRules = $state.snapshot(draft.smart) as SmartRules;
			delete draft.smart;
		}
		queueSave();
	}
	function editCountdown(section: Section, it: PlaylistItem) {
		if (it.type !== 'countdown') return;
		editing = { section, id: it.id, item: structuredClone($state.snapshot(it)) as CountdownItem };
	}
	function saveCountdown() {
		if (!draft || !editing) return;
		const e = editing;
		draft[e.section] = draft[e.section].map((x) => (x.id === e.id ? { ...e.item, id: e.id } : x));
		queueSave();
	}

	$effect(() => {
		if (!show) return;
		if (!selectedId || !show.playlists.some((p) => p.id === selectedId))
			selectedId = show.playlists[0]?.id ?? null;
	});
	$effect(() => {
		const p = show?.playlists.find((x) => x.id === selectedId);
		untrack(() => {
			if (p && (draft?.id !== p.id || saveState === 'saved'))
				draft = structuredClone($state.snapshot(p) as Playlist);
			if (!p) draft = null;
		});
	});

	function queueSave() {
		saveState = 'dirty';
		clearTimeout(saveTimer);
		saveTimer = setTimeout(save, 600);
	}
	async function save() {
		if (!draft) return;
		saveState = 'saving';
		const d = $state.snapshot(draft) as Playlist;
		try {
			await api.playlists.update(d.id, d);
			await app.reloadShow();
			saveState = 'saved';
		} catch (e) {
			toasts.error('Could not save playlist', (e as Error).message);
			saveState = 'dirty';
		}
	}

	function itemInfo(
		it: PlaylistItem,
		s: Show
	): { name: string; sub: string; ms: number; icon: typeof Music; tone: string } {
		switch (it.type) {
			case 'sequence': {
				const q = s.sequences.find((x) => x.id === it.sequenceId);
				const m = s.media.find((x) => x.id === q?.mediaId);
				return {
					name: q?.name ?? 'Missing sequence',
					sub: m ? 'Song' : 'Light-only sequence',
					ms: q?.durationMs ?? 0,
					icon: Music,
					tone: 'accent'
				};
			}
			case 'dj': {
				const c = s.djClips.find((x) => x.id === it.djClipId);
				const m = s.media.find((x) => x.id === c?.mediaId);
				return {
					name: c?.name ?? 'Missing clip',
					sub: c?.dynamic ? 'DJ · live text' : 'DJ clip',
					ms: m?.durationMs ?? 10000,
					icon: Mic,
					tone: 'purple'
				};
			}
			case 'effect': {
				const e = s.effects.find((x) => x.id === it.effectId);
				return {
					name: e?.name ?? 'Missing effect',
					sub: 'Effect',
					ms: it.durationMs,
					icon: WandSparkles,
					tone: 'blue'
				};
			}
			case 'media': {
				const m = s.media.find((x) => x.id === it.mediaId);
				return {
					name: m?.name ?? 'Missing audio',
					sub: 'Audio only',
					ms: m?.durationMs ?? 0,
					icon: AudioLines,
					tone: 'green'
				};
			}
			case 'pause':
				return { name: 'Pause', sub: 'Dark and quiet', ms: it.durationMs, icon: Clock, tone: '' };
			case 'command':
				return {
					name: commandLabel(it.command),
					sub: commandKind(it.command),
					ms: 0,
					icon: it.command.startsWith('games') ? Gamepad2 : Terminal,
					tone: ''
				};
			// F4 (WS3 editor, WS2 page): a minimal row until the countdown editor lands.
			case 'countdown':
				return {
					name: 'Countdown',
					sub: `${Math.round(it.durationMs / 1000)} s to show start`,
					ms: it.durationMs,
					icon: Timer,
					tone: 'accent'
				};
		}
	}
	function commandKind(c: string) {
		return c.startsWith('games.') ? 'Game' : c.startsWith('overlay.') ? 'Message' : 'Action';
	}
	function commandLabel(c: string) {
		return (
			(
				{
					'games.invite': 'Show game invite',
					'games.stop': 'Stop game',
					'overlay.text': 'Scroll a message'
				} as Record<string, string>
			)[c] ?? c
		);
	}

	function total(p: Playlist | null) {
		if (!p || !show) return 0;
		const s = show;
		const sum = (l: PlaylistItem[]) => l.reduce((n, it) => n + itemInfo(it, s).ms, 0);
		if (p.smart) {
			const main = p.id === draft?.id && smartTotal ? smartTotal : (p.smart.targetDurationMs ?? 0);
			return sum(p.intro) + main + sum(p.outro);
		}
		return sum([...p.intro, ...p.items, ...p.outro]);
	}

	/** Items added while the add sheet is open (it stays open so you can add several). */
	let addedNow = $state<string[]>([]);
	function add(it: Omit<PlaylistItem, 'id'>, section: Section = target, key?: string) {
		if (!draft) return;
		// A smart playlist picks its own songs: extra items go into the intro.
		if (draft.smart && section === 'items') section = 'intro';
		const item = { ...it, id: newId() } as PlaylistItem;
		draft[section] = [...draft[section], item];
		queueSave();
		if (addOpen) {
			if (key) addedNow = [...addedNow, key];
			return;
		}
		toasts.push({
			kind: 'success',
			message: `Added to ${section === 'items' ? 'the playlist' : section}`,
			timeout: 1800
		});
	}
	$effect(() => {
		if (!addOpen) addedNow = [];
	});
	let titleInput: HTMLInputElement | undefined = $state();
	/** New playlist: put the cursor in its name, selected, ready to type over (like Linear). */
	function focusTitle() {
		setTimeout(() => {
			titleInput?.focus();
			titleInput?.select();
		}, 60);
	}
	function removeItem(section: Section, i: number) {
		if (!draft) return;
		const removed = draft[section][i];
		draft[section] = draft[section].filter((_, k) => k !== i);
		queueSave();
		toasts.push({
			kind: 'info',
			message: 'Item removed',
			action: {
				label: 'Undo',
				run: () => {
					if (!draft) return;
					const arr = [...draft[section]];
					arr.splice(i, 0, removed);
					draft[section] = arr;
					queueSave();
				}
			}
		});
	}
	function reorder(section: Section, from: number, to: number) {
		if (!draft) return;
		draft[section] = moveItem(draft[section], from, to);
		queueSave();
	}

	async function createPlaylist() {
		const p = await app.mutate(() =>
			api.playlists.create({
				name: 'New playlist',
				items: [],
				intro: [],
				outro: [],
				shuffle: false,
				repeat: true,
				crossfadeMs: 0
			})
		);
		if (p) {
			selectedId = p.id;
			focusTitle();
		}
	}
	async function duplicate() {
		if (!draft) return;
		const d = $state.snapshot(draft) as Playlist;
		const p = await app.mutate(
			() => api.playlists.create({ ...d, id: undefined as unknown as string, name: `${d.name} copy` }),
			{ success: 'Playlist duplicated' }
		);
		if (p) selectedId = p.id;
	}
	async function remove() {
		if (!draft) return;
		const d = structuredClone($state.snapshot(draft) as Playlist);
		const used = show?.schedule.entries.filter((e) => e.playlistId === d.id) ?? [];
		if (
			!(await confirm({
				title: `Delete “${d.name}”?`,
				message: used.length
					? `It’s used by ${used.length} schedule ${used.length === 1 ? 'entry' : 'entries'}, which will stop working.`
					: undefined,
				confirmLabel: 'Delete playlist',
				danger: true
			}))
		)
			return;
		await app.mutate(() => api.playlists.remove(d.id));
		selectedId = null;
		toasts.success(`Deleted ${d.name}`, {
			label: 'Undo',
			run: () => app.mutate(() => api.playlists.create(d))
		});
	}

	function libDrag(e: DragEvent, it: Omit<PlaylistItem, 'id'>) {
		e.dataTransfer?.setData('application/x-pixelplus-item', JSON.stringify(it));
		if (e.dataTransfer) e.dataTransfer.effectAllowed = 'copy';
	}
	function sectionDrop(e: DragEvent, section: Section) {
		const raw = e.dataTransfer?.getData('application/x-pixelplus-item');
		dragOver = null;
		if (!raw) return;
		e.preventDefault();
		add(JSON.parse(raw), section);
	}

	const library = $derived.by(() => {
		if (!show) return [];
		const f = (n: string) => !libQ || n.toLowerCase().includes(libQ.toLowerCase());
		switch (libTab) {
			case 'sequence':
				return show.sequences
					.filter((s) => f(s.name))
					.map((s) => ({
						key: s.id,
						name: s.name,
						sub: fmtDuration(s.durationMs),
						item: { type: 'sequence', sequenceId: s.id } as Omit<PlaylistItem, 'id'>
					}));
			case 'dj':
				return show.djClips
					.filter((s) => f(s.name))
					.map((s) => ({
						key: s.id,
						name: s.name,
						sub: s.dynamic ? 'Live text' : `${s.lines.length} lines`,
						item: { type: 'dj', djClipId: s.id } as Omit<PlaylistItem, 'id'>
					}));
			case 'effect':
				return show.effects
					.filter((s) => f(s.name))
					.map((s) => ({
						key: s.id,
						name: s.name,
						sub: '30 s',
						item: { type: 'effect', effectId: s.id, durationMs: 30000 } as Omit<PlaylistItem, 'id'>
					}));
			case 'media':
				return show.media
					.filter((s) => f(s.name))
					.map((s) => ({
						key: s.id,
						name: s.name,
						sub: fmtDuration(s.durationMs),
						item: { type: 'media', mediaId: s.id } as Omit<PlaylistItem, 'id'>
					}));
			default:
				return [
					{
						key: 'pause',
						name: 'Pause',
						sub: '10 s of darkness',
						item: { type: 'pause', durationMs: 10000 } as Omit<PlaylistItem, 'id'>
					},
					{
						key: 'inv',
						name: 'Show game invite',
						sub: 'Flash the game URL / QR on the matrix',
						item: { type: 'command', command: 'games.invite', args: {} } as Omit<PlaylistItem, 'id'>
					},
					{
						key: 'stop',
						name: 'Stop game',
						sub: 'End any game in progress',
						item: { type: 'command', command: 'games.stop', args: {} } as Omit<PlaylistItem, 'id'>
					},
					{
						key: 'countdown',
						name: 'Countdown to showtime',
						sub: 'Big numbers on the matrix, then the show starts',
						item: newCountdownItem() as Omit<PlaylistItem, 'id'>
					},
					{
						key: 'text',
						name: 'Scroll a message',
						sub: 'Text on the matrix',
						item: {
							type: 'command',
							command: 'overlay.text',
							args: { text: 'Merry Christmas!', color: '#ff2a2a' }
						} as Omit<PlaylistItem, 'id'>
					}
				];
		}
	});

	const sectionMeta: { id: Section; title: string; hint: string }[] = [
		{ id: 'intro', title: 'Intro', hint: 'Plays once when the playlist starts' },
		{ id: 'items', title: 'Playlist', hint: 'The main line-up' },
		{ id: 'outro', title: 'Outro', hint: 'Plays once at the end of the night' }
	];
</script>

{#snippet libraryPanel()}
	<div class="lib">
		<div class="lib-tabs">
			<Segmented
				bind:value={libTab}
				size="sm"
				label="Library"
				options={[
					{ value: 'sequence', label: 'Songs' },
					{ value: 'dj', label: 'DJ' },
					{ value: 'effect', label: 'Effects' },
					{ value: 'media', label: 'Audio' },
					{ value: 'more', label: 'More' }
				]}
			/>
		</div>
		<div class="input-group">
			<span class="prefix"><Search size={15} /></span><input
				class="input sm"
				placeholder="Search library"
				bind:value={libQ}
				aria-label="Search library"
			/>
		</div>
		<div class="lib-target faint tiny">
			Adding to <strong>{sectionMeta.find((s) => s.id === target)?.title}</strong> · pick a section in the playlist
			to change it
		</div>
		<div class="lib-list">
			{#each library as l (l.key)}
				{@const times = addedNow.filter((k) => k === l.key).length}
				<button
					class="lib-item"
					class:added={times > 0}
					draggable="true"
					ondragstart={(e) => libDrag(e, l.item)}
					onclick={() => add(l.item, target, l.key)}
				>
					<div class="grow">
						<div class="ellipsis small"><strong>{l.name}</strong></div>
						<div class="faint tiny ellipsis">{l.sub}</div>
					</div>
					{#if times}<span class="addedmark"><Check size={15} />{times > 1 ? ` ×${times}` : ''}</span
						>{:else}<Plus size={16} />{/if}
				</button>
			{:else}
				<div class="faint small" style="padding:12px">Nothing here yet.</div>
			{/each}
		</div>
	</div>
{/snippet}

<div class="page">
	<PageHeader
		title="Playlists"
		subtitle="Mix songs, DJ breaks, effects and pauses. Changes save automatically."
	>
		{#snippet actions()}
			<button class="btn primary" onclick={createPlaylist}><Plus size={16} /> New playlist</button>
		{/snippet}
	</PageHeader>

	{#if !show}
		<div class="card card-pad"><Skeleton count={8} h={36} /></div>
	{:else if !show.playlists.length}
		<div class="card">
			<EmptyState
				icon={ListMusic}
				title="No playlists yet"
				message="A playlist is your show’s running order. Start one and add your sequences."
			>
				<button class="btn primary" onclick={createPlaylist}><Plus size={16} /> Create a playlist</button>
			</EmptyState>
		</div>
	{:else}
		<div class="layout">
			<nav class="pls" aria-label="Playlists">
				{#each show.playlists as p (p.id)}
					{@const n = p.items.length + p.intro.length + p.outro.length}
					<button class="pl" class:on={p.id === selectedId} onclick={() => (selectedId = p.id)}>
						<span class="plicon"><ListMusic size={18} /></span>
						<span class="grow"
							><span class="ellipsis plname">{p.name}</span><span class="faint tiny"
								>{plural(n, 'item')} · {fmtDuration(total(p), { long: true })}</span
							></span
						>
						{#if p.smart}<span title="Smart playlist"><Sparkles size={13} class="faint" /></span>{/if}
						{#if p.shuffle}<Shuffle size={13} class="faint" />{/if}
						{#if p.repeat}<Repeat size={13} class="faint" />{/if}
					</button>
				{/each}
			</nav>

			{#if draft}
				<section class="builder card">
					<header class="bhead">
						<input
							class="title-input"
							bind:this={titleInput}
							bind:value={draft.name}
							oninput={queueSave}
							aria-label="Playlist name"
						/>
						<span class="save"><SaveState state={saveState} /></span>
						<span class="grow"></span>
						<button
							class="btn primary sm"
							onclick={() => playerAct(() => api.play({ playlistId: draft!.id }))}
							><Play size={14} fill="currentColor" /> Play now</button
						>
						<button class="btn ghost icon sm" onclick={duplicate} aria-label="Duplicate playlist"
							><Copy size={15} /></button
						>
						<button class="btn ghost icon sm" onclick={remove} aria-label="Delete playlist"
							><Trash2 size={15} /></button
						>
					</header>
					<div class="opts">
						<label class="opt" title="Pick tonight’s songs by tags, length and history"
							><Switch checked={!!draft.smart} label="Smart" size="sm" onchange={setSmart} /><Sparkles
								size={14}
							/> Smart</label
						>
						{#if !draft.smart}
							<label class="opt"
								><Switch
									bind:checked={draft.shuffle}
									label="Shuffle"
									size="sm"
									onchange={queueSave}
								/><Shuffle size={14} /> Shuffle</label
							>
						{/if}
						<label class="opt"
							><Switch bind:checked={draft.repeat} label="Repeat" size="sm" onchange={queueSave} /><Repeat
								size={14}
							/> Repeat</label
						>
						<div class="opt xf">
							<span class="small">Crossfade</span>
							<input
								type="range"
								class="range"
								min="0"
								max="5000"
								step="250"
								bind:value={draft.crossfadeMs}
								oninput={queueSave}
								style:--pct="{(draft.crossfadeMs / 5000) * 100}%"
								aria-label="Crossfade"
							/>
							<span class="num small faint" style="width:44px"
								>{draft.crossfadeMs
									? `${(draft.crossfadeMs / 1000).toFixed(draft.crossfadeMs % 1000 ? 2 : 0)} s`
									: 'Off'}</span
							>
						</div>
						<span class="grow"></span>
						<span class="total num"><Clock size={14} /> {fmtDuration(total(draft), { long: true })}</span>
					</div>

					{#each sectionMeta as sec (sec.id)}
						{@const list = draft[sec.id]}
						<div
							class="section"
							class:target={target === sec.id}
							class:over={dragOver === sec.id}
							role="group"
							aria-label={sec.title}
							ondragover={(e) => {
								if (e.dataTransfer?.types.includes('application/x-pixelplus-item')) {
									e.preventDefault();
									dragOver = sec.id;
								}
							}}
							ondragleave={() => (dragOver = null)}
							ondrop={(e) => sectionDrop(e, sec.id)}
						>
							<button class="shead" onclick={() => (target = sec.id)}>
								<span class="stitle">{sec.title}</span><span class="faint tiny">{sec.hint}</span><span
									class="grow"
								></span>
								{#if list.length}<span class="faint tiny num"
										>{fmtDuration(list.reduce((n, it) => n + itemInfo(it, show).ms, 0))}</span
									>{/if}
							</button>
							{#if sec.id === 'items' && draft.smart}
								<div class="smart">
									<p class="faint small smart-intro">
										Songs are picked every night from your tagged library. Repeat plays a fresh pick each
										pass.
									</p>
									<SmartRulesEditor bind:rules={draft.smart} {show} onchange={queueSave} />
									<SmartTonight
										rules={draft.smart}
										playlistId={draft.id}
										{show}
										ontotal={(ms) => (smartTotal = ms)}
									/>
								</div>
							{:else}
								<ol use:sortable={{ onsort: (f, t) => reorder(sec.id, f, t) }}>
									{#each list as it, i (it.id)}
										{@const info = itemInfo(it, show)}
										<li class="item" data-sort-index={i}>
											<button class="drag-handle" aria-label="Move {info.name} (use arrow keys)"
												><GripVertical size={16} /></button
											>
											<span class="num idx faint">{i + 1}</span>
											<span class="iicon {info.tone}"><info.icon size={16} /></span>
											<span class="grow iname"
												><span class="ellipsis">{info.name}</span><span class="faint tiny">{info.sub}</span
												></span
											>
											{#if it.type === 'effect' || it.type === 'pause'}
												<label class="dur"
													><input
														class="input sm num"
														type="number"
														min="1"
														value={Math.round(it.durationMs / 1000)}
														onchange={(e) => {
															(it as any).durationMs = Number((e.target as HTMLInputElement).value) * 1000;
															queueSave();
														}}
														aria-label="Duration in seconds"
													/><span class="faint tiny">s</span></label
												>
											{:else if info.ms}
												<span class="faint small num">{fmtDuration(info.ms)}</span>
											{/if}
											{#if it.type === 'countdown'}
												<button
													class="btn ghost icon sm"
													onclick={() => editCountdown(sec.id, it)}
													aria-label="Edit the countdown"><Pencil size={14} /></button
												>
											{/if}
											<button
												class="btn ghost icon sm"
												onclick={() => removeItem(sec.id, i)}
												aria-label="Remove {info.name}"><X size={15} /></button
											>
										</li>
									{/each}
								</ol>
							{/if}
							{#if !list.length && !(sec.id === 'items' && draft.smart)}
								<button
									class="dropzone"
									onclick={() => {
										target = sec.id;
										addOpen = true;
									}}
								>
									{sec.id === 'items'
										? 'Add songs, DJ clips and effects'
										: `Optional — add a ${sec.id === 'intro' ? 'welcome message' : 'goodnight message'}`}
								</button>
							{/if}
						</div>
					{/each}
					<div class="mobile-add">
						<button class="btn block" onclick={() => (addOpen = true)}><Plus size={16} /> Add items</button>
					</div>
				</section>
				<aside class="library card">{@render libraryPanel()}</aside>
			{/if}
		</div>
	{/if}
</div>

<Modal
	open={!!editing}
	title="Countdown to showtime"
	subtitle="Shown just before the show starts"
	size="md"
	onclose={() => (editing = null)}
>
	{#if editing}
		<CountdownItemEditor bind:item={editing.item} onchange={saveCountdown} />
	{/if}
	{#snippet footer()}
		<button class="btn primary" onclick={() => (editing = null)}>Done</button>
	{/snippet}
</Modal>

<Modal bind:open={addOpen} title="Add to {sectionMeta.find((s) => s.id === target)?.title}" size="md">
	{@render libraryPanel()}
	{#snippet footer()}
		<span class="grow small muted added-count"
			>{addedNow.length ? `${addedNow.length} added` : 'Tap to add — add as many as you like'}</span
		>
		<button class="btn primary" onclick={() => (addOpen = false)}>Done</button>
	{/snippet}
</Modal>

<style>
	.smart {
		display: flex;
		flex-direction: column;
		gap: 12px;
		padding: 4px 16px 16px;
	}
	.smart-intro {
		margin: 0;
	}
	.layout {
		display: grid;
		grid-template-columns: 240px minmax(0, 1fr) 300px;
		gap: 16px;
		align-items: start;
	}
	.pls {
		display: flex;
		flex-direction: column;
		gap: 4px;
		position: sticky;
		top: 16px;
	}
	.pl {
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 10px;
		border-radius: 12px;
		text-align: left;
		border: 1px solid transparent;
		transition: all 150ms var(--ease);
	}
	.pl:hover {
		background: var(--surface);
	}
	.pl.on {
		background: var(--surface);
		border-color: var(--border-2);
		box-shadow: var(--shadow-1);
	}
	.plicon {
		width: 36px;
		height: 36px;
		border-radius: 10px;
		display: grid;
		place-items: center;
		background: var(--surface-3);
		color: var(--text-2);
		flex: 0 0 auto;
	}
	.pl.on .plicon {
		background: var(--accent-soft);
		color: var(--accent-text);
	}
	.pl .grow {
		display: flex;
		flex-direction: column;
		min-width: 0;
	}
	.plname {
		font-weight: 580;
		font-size: 13.5px;
	}
	.builder {
		overflow: hidden;
	}
	.bhead {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 14px 16px 10px 20px;
	}
	.title-input {
		font-size: 20px;
		font-weight: 650;
		letter-spacing: -0.02em;
		background: transparent;
		border: 1px solid transparent;
		border-radius: 8px;
		padding: 2px 6px;
		margin-left: -6px;
		min-width: 0;
		width: 100%;
		max-width: 360px;
	}
	.title-input:hover {
		border-color: var(--border-2);
	}
	.title-input:focus {
		border-color: var(--accent-line);
		background: var(--surface-2);
	}
	.save {
		display: inline-flex;
		align-items: center;
		gap: 4px;
		white-space: nowrap;
	}
	.save :global(.spin) {
		animation: spin 1s linear infinite;
	}
	@keyframes spin {
		to {
			transform: rotate(360deg);
		}
	}
	.opts {
		display: flex;
		align-items: center;
		flex-wrap: wrap;
		gap: 10px 20px;
		padding: 8px 20px 16px;
		border-bottom: 1px solid var(--border);
	}
	.opt {
		display: flex;
		align-items: center;
		gap: 8px;
		font-size: 13px;
		color: var(--text-2);
		cursor: pointer;
	}
	.xf .range {
		width: 120px;
	}
	.total {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		font-weight: 600;
		font-size: 13px;
	}
	.section {
		padding: 10px 12px 14px;
		border-bottom: 1px solid var(--border);
		transition: background 150ms;
	}
	.section:last-of-type {
		border-bottom: 0;
	}
	.section.target .stitle {
		color: var(--accent-text);
	}
	.section.over {
		background: var(--accent-soft);
	}
	.shead {
		display: flex;
		align-items: baseline;
		gap: 10px;
		width: 100%;
		padding: 6px 8px;
		text-align: left;
	}
	@media (pointer: coarse) {
		.shead {
			min-height: 44px;
			align-items: center;
		}
	}
	.stitle {
		font-weight: 650;
		font-size: 13px;
	}
	ol {
		list-style: none;
		margin: 4px 0 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 4px;
	}
	.item {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 4px 6px 4px 2px;
		border-radius: 10px;
		background: var(--surface-2);
		border: 1px solid var(--border);
		min-height: 52px;
	}
	.idx {
		width: 18px;
		text-align: right;
		font-size: 11.5px;
	}
	.iicon {
		width: 32px;
		height: 32px;
		border-radius: 9px;
		display: grid;
		place-items: center;
		background: var(--surface-3);
		color: var(--text-2);
		flex: 0 0 auto;
	}
	.iicon.accent {
		background: var(--accent-soft);
		color: var(--accent-text);
	}
	.iicon.purple {
		background: var(--purple-soft);
		color: var(--purple);
	}
	.iicon.blue {
		background: var(--blue-soft);
		color: var(--blue);
	}
	.iicon.green {
		background: var(--green-soft);
		color: var(--green);
	}
	.iname {
		display: flex;
		flex-direction: column;
		min-width: 0;
		font-size: 13.5px;
		font-weight: 540;
	}
	.dur {
		display: flex;
		align-items: center;
		gap: 4px;
	}
	.dur .input {
		width: 64px;
	}
	.dropzone {
		width: 100%;
		margin-top: 4px;
		padding: 14px;
		border-radius: 10px;
		border: 1.5px dashed var(--border-2);
		color: var(--text-3);
		font-size: 12.5px;
	}
	.dropzone:hover {
		border-color: var(--accent);
		color: var(--accent-text);
	}
	.library {
		position: sticky;
		top: 16px;
		padding: 14px;
		max-height: calc(100dvh - var(--transport-h) - 32px);
		display: flex;
		flex-direction: column;
	}
	.lib {
		display: flex;
		flex-direction: column;
		gap: 10px;
		min-height: 0;
		flex: 1;
	}
	.lib-target strong {
		color: var(--accent-text);
	}
	/* Five equal tabs that always fit the library column (no clipped "More"). */
	.lib-tabs :global(.seg) {
		display: flex;
		width: 100%;
	}
	.lib-tabs :global(.seg button) {
		flex: 1 1 0;
		justify-content: center;
		padding: 0 4px;
		min-width: 0;
	}
	.lib-item.added {
		border-color: var(--accent-line);
		background: var(--accent-soft);
	}
	.addedmark {
		display: inline-flex;
		align-items: center;
		gap: 2px;
		color: var(--accent-text);
		font-size: 12px;
		font-weight: 650;
	}
	.added-count {
		align-self: center;
	}
	@media (pointer: coarse) {
		.lib-item {
			min-height: 52px;
		}
	}
	.lib-list {
		display: flex;
		flex-direction: column;
		gap: 4px;
		overflow: auto;
		min-height: 120px;
		max-height: 60vh;
	}
	.lib-item {
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 8px 10px;
		border-radius: 10px;
		text-align: left;
		color: var(--text-3);
		border: 1px solid var(--border);
		cursor: grab;
	}
	.lib-item:hover {
		border-color: var(--accent-line);
		color: var(--accent-text);
		background: var(--accent-soft);
	}
	.lib-item strong {
		color: var(--text);
		font-weight: 560;
	}
	.mobile-add {
		display: none;
		padding: 12px;
	}
	@media (max-width: 1200px) {
		.layout {
			grid-template-columns: 220px minmax(0, 1fr);
		}
		.library {
			display: none;
		}
		.mobile-add {
			display: block;
		}
	}
	@media (max-width: 760px) {
		.layout {
			grid-template-columns: 1fr;
		}
		.pls {
			position: static;
			flex-direction: row;
			overflow-x: auto;
			margin: 0 -16px;
			padding: 0 16px 4px;
		}
		.pl {
			flex: 0 0 auto;
			max-width: 240px;
		}
		.bhead {
			flex-wrap: wrap;
			padding: 12px;
		}
		.opts {
			padding: 8px 12px 12px;
		}
	}
</style>
