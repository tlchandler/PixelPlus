<script lang="ts">
	import { api } from '$lib/api/client';
	import type { Media, Sequence } from '$lib/api/types';
	import { app } from '$lib/stores/app.svelte';
	import { toasts, confirm } from '$lib/stores/toasts.svelte';
	import { fmtDuration, plural } from '$lib/util/format';
	import { playerAct } from '$lib/player';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import Segmented from '$lib/components/ui/Segmented.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import Skeleton from '$lib/components/ui/Skeleton.svelte';
	import Modal from '$lib/components/ui/Modal.svelte';
	import Waveform from '$lib/components/viz/Waveform.svelte';
	import SequencePreview from '$lib/components/viz/SequencePreview.svelte';
	import TagChips from '$lib/components/library/TagChips.svelte';
	import TagInput from '$lib/components/library/TagInput.svelte';
	import AnalysisView from '$lib/components/library/AnalysisView.svelte';
	import AutoShowDialog from '$lib/components/library/AutoShowDialog.svelte';
	import { library, type HistoryRow } from '$lib/library/api';
	import { activeJob, waitJob } from '$lib/library/jobs.svelte';
	import { allTags, energyGlyph, energyWord, parseTags } from '$lib/library/tags';
	import {
		UploadCloud,
		Music,
		Film,
		Play,
		Pause,
		Trash2,
		Pencil,
		Link2,
		Link2Off,
		Search,
		CircleCheck,
		CircleAlert,
		Volume2,
		Mic,
		AudioLines,
		Eye,
		Sparkles,
		Tags,
		X,
		RefreshCw,
		ListChecks
	} from '@lucide/svelte';
	import { slide } from 'svelte/transition';

	const show = $derived(app.show);
	let tab = $state<'sequences' | 'audio'>('sequences');
	let q = $state('');
	let dragging = $state(false);
	let uploads = $state<
		{
			id: number;
			name: string;
			detail: string;
			progress: number;
			state: 'uploading' | 'done' | 'error';
			error?: string;
		}[]
	>([]);
	let expanded = $state<string | null>(null);
	let peaks = $state<Record<string, number[]>>({});
	let playingMedia = $state<string | null>(null);
	let audioEl: HTMLAudioElement | undefined = $state();
	let audioPos = $state(0);
	let renaming = $state<{ kind: 'seq' | 'media'; id: string; name: string } | null>(null);
	let fileInput: HTMLInputElement | undefined = $state();
	let upSeq = 0;

	const target = $derived(show?.settings.audio.targetLufs ?? -14);
	/** Tags the list is filtered by (all must match). */
	let tagFilter = $state<string[]>([]);
	let selecting = $state(false);
	let picked = $state<string[]>([]);
	let bulkText = $state('');
	let previewSeq = $state<Sequence | null>(null);
	let autoFor = $state<Media | null>(null);
	let history = $state<Record<string, HistoryRow>>({});

	const tags = $derived(allTags(show));
	const defs = $derived(show?.tagDefs ?? []);
	function matches(x: { name: string; tags?: string[] }) {
		const t = x.tags ?? [];
		const needle = q.trim().toLowerCase();
		return (
			(!needle || x.name.toLowerCase().includes(needle) || t.some((tag) => tag.includes(needle))) &&
			tagFilter.every((f) => t.includes(f))
		);
	}
	const seqs = $derived(show?.sequences.filter(matches) ?? []);
	const media = $derived(show?.media.filter(matches) ?? []);
	const visibleIds = $derived((tab === 'sequences' ? seqs : media).map((x) => x.id));

	$effect(() => {
		// Play counts for the last two weeks (from the show journal).
		library
			.history(14)
			.then((rows) => (history = Object.fromEntries(rows.map((r) => [r.sequenceId, r]))))
			.catch(() => {});
	});

	function toggleTag(t: string) {
		tagFilter = tagFilter.includes(t) ? tagFilter.filter((x) => x !== t) : [...tagFilter, t];
	}
	function togglePick(id: string) {
		picked = picked.includes(id) ? picked.filter((x) => x !== id) : [...picked, id];
	}
	function endSelect() {
		selecting = false;
		picked = [];
		bulkText = '';
	}
	async function bulk(mode: 'add' | 'remove') {
		const t = parseTags(bulkText);
		if (!t.length || !picked.length) return;
		const n = picked.length;
		await app.mutate(() => library.bulkTags(picked, mode === 'add' ? t : [], mode === 'remove' ? t : []), {
			success: mode === 'add' ? `Tagged ${plural(n, 'item')}` : `Removed the tag from ${plural(n, 'item')}`
		});
		bulkText = '';
	}
	async function setSeqTags(s: Sequence, t: string[]) {
		await app.mutate(() => api.sequences.update(s.id, { tags: t }));
	}
	async function setMediaTags(m: Media, t: string[]) {
		await app.mutate(() => api.media.update(m.id, { tags: t }));
	}
	async function regenerate(s: Sequence, fresh: boolean) {
		try {
			const { jobId } = await library.regenerate(
				s.id,
				fresh ? { seed: Math.floor(Math.random() * 1e6) } : {}
			);
			toasts.info(fresh ? `Making new moves for “${s.name}”…` : `Updating “${s.name}”…`);
			await waitJob(jobId);
			await app.reloadShow();
			toasts.success(`“${s.name}” is updated`, { label: 'Preview', run: () => openPreview(s.id) });
		} catch (e) {
			toasts.error('Couldn’t update the light show', (e as Error).message);
		}
	}
	function openPreview(id: string) {
		const s = app.show?.sequences.find((x) => x.id === id);
		if (s) previewSeq = s;
	}
	function playsText(id: string) {
		const h = history[id];
		if (!h?.plays) return '';
		return `Played ${plural(h.plays, 'time')} in 2 weeks`;
	}

	function mediaOf(s: Sequence): Media | undefined {
		return show?.media.find((m) => m.id === s.mediaId);
	}

	/**
	 * With volume leveling on, every song already plays at the same loudness: nothing to show.
	 * With it off, point out the songs that will sound much louder or quieter than the rest.
	 */
	function loudness(m?: Media) {
		if (!m?.loudnessLufs || show?.settings.audio.normalize) return null;
		const d = m.loudnessLufs - target;
		if (Math.abs(d) <= 3) return null;
		return d > 0
			? { label: 'Louder than the rest', cls: 'accent', d }
			: { label: 'Quieter than the rest', cls: 'blue', d };
	}
	/** An audio file whose name matches this sequence (offered as a one-tap link). */
	function audioMatch(s: Sequence): Media | undefined {
		if (s.mediaId || !show) return undefined;
		const n = norm(s.name);
		return show.media.find(
			(m) => m.kind === 'song' && (norm(m.name) === n || norm(m.name).includes(n) || n.includes(norm(m.name)))
		);
	}
	/** Smoothness of a sequence, in words (fps stays out of sight). */
	function smoothness(frameMs: number) {
		const fps = Math.round(1000 / frameMs);
		return fps >= 40 ? 'Extra smooth' : fps >= 25 ? 'Smooth' : 'Standard';
	}
	let fresh = $state<Set<string>>(new Set());

	function norm(name: string) {
		return name
			.toLowerCase()
			.replace(/\.[a-z0-9]+$/, '')
			.replace(/[^a-z0-9]+/g, '');
	}

	async function handleFiles(files: File[]) {
		const fseqs = files.filter((f) => /\.fseq$/i.test(f.name));
		const audios = files.filter((f) => /\.(mp3|ogg|m4a|wav|flac|aac)$/i.test(f.name));
		const others = files.filter((f) => !fseqs.includes(f) && !audios.includes(f));
		for (const o of others)
			toasts.warn(`Skipped ${o.name} — only light sequences from xLights and songs can be uploaded here`);
		const used = new Set<File>();
		const jobs: Promise<unknown>[] = [];
		for (const f of fseqs) {
			const a = audios.find(
				(x) => !used.has(x) && (norm(x.name).includes(norm(f.name)) || norm(f.name).includes(norm(x.name)))
			);
			if (a) used.add(a);
			jobs.push(
				run(f.name, a ? `with ${a.name}` : 'audio linked automatically if it’s already here', (p) =>
					api.sequences.upload(f, a, p)
				)
			);
		}
		for (const a of audios.filter((x) => !used.has(x)))
			jobs.push(run(a.name, 'audio file', (p) => api.media.upload(a, 'song', p)));
		const before = new Set([...(show?.sequences ?? []), ...(show?.media ?? [])].map((x) => x.id));
		await Promise.allSettled(jobs);
		await app.reloadShow();
		const ok = uploads.filter((u) => u.state === 'done').length;
		if (ok) toasts.success(`Uploaded ${plural(ok, 'file')}`);
		// Scroll to and highlight what just arrived.
		const added = [...(app.show?.sequences ?? []), ...(app.show?.media ?? [])]
			.map((x) => x.id)
			.filter((id) => !before.has(id));
		if (added.length) {
			fresh = new Set(added);
			if (!fseqs.length && audios.length) tab = 'audio';
			requestAnimationFrame(() =>
				document.getElementById(`row-${added[0]}`)?.scrollIntoView({ behavior: 'smooth', block: 'center' })
			);
			setTimeout(() => (fresh = new Set()), 4000);
		}
		setTimeout(() => (uploads = uploads.filter((u) => u.state !== 'done')), 4000);
	}

	async function run(name: string, detail: string, fn: (p: (x: number) => void) => Promise<unknown>) {
		const id = ++upSeq;
		uploads = [...uploads, { id, name, detail, progress: 0, state: 'uploading' }];
		const upd = (patch: Partial<(typeof uploads)[number]>) =>
			(uploads = uploads.map((u) => (u.id === id ? { ...u, ...patch } : u)));
		try {
			await fn((p) => upd({ progress: p }));
			upd({ progress: 1, state: 'done' });
		} catch (e) {
			upd({ state: 'error', error: (e as Error).message });
		}
	}

	function onDrop(e: DragEvent) {
		e.preventDefault();
		dragging = false;
		const files = [...(e.dataTransfer?.files ?? [])];
		if (files.length) handleFiles(files);
	}

	async function linkAudio(s: Sequence, mediaId: string) {
		await app.mutate(() => api.sequences.update(s.id, { mediaId: mediaId || undefined }), {
			success: mediaId ? 'Audio linked' : 'Audio unlinked'
		});
	}

	async function removeSeq(s: Sequence) {
		const used =
			show?.playlists.filter((p) =>
				[...p.items, ...p.intro, ...p.outro].some((i) => i.type === 'sequence' && i.sequenceId === s.id)
			) ?? [];
		if (
			!(await confirm({
				title: `Delete “${s.name}”?`,
				message: used.length
					? `It is used in ${used.map((p) => p.name).join(', ')}. It will be removed from those playlists.`
					: 'The sequence file is removed from all controllers.',
				confirmLabel: 'Delete',
				danger: true
			}))
		)
			return;
		const copy = structuredClone($state.snapshot(s) as Sequence);
		// The daemon also takes it out of playlists: undo puts those back too.
		const playlistsBefore = used.map((p) => structuredClone($state.snapshot(p)) as typeof p);
		await app.mutate(() => api.sequences.remove(s.id));
		toasts.success(`Deleted ${s.name}`, {
			label: 'Undo',
			run: () =>
				app.mutate(async () => {
					await api.sequences.create(copy);
					for (const p of playlistsBefore) await api.playlists.update(p.id, p);
				})
		});
	}
	async function removeMedia(m: Media) {
		if (
			!(await confirm({
				title: `Delete “${m.name}”?`,
				message: 'Sequences using it will play without audio.',
				confirmLabel: 'Delete',
				danger: true
			}))
		)
			return;
		const copy = structuredClone($state.snapshot(m) as Media);
		await app.mutate(() => api.media.remove(m.id));
		toasts.success(`Deleted ${m.name}`, {
			label: 'Undo',
			run: () => app.mutate(() => api.media.create(copy))
		});
	}
	async function doRename(e: SubmitEvent) {
		e.preventDefault();
		if (!renaming) return;
		const r = renaming;
		if (r.kind === 'seq') await app.mutate(() => api.sequences.update(r.id, { name: r.name }));
		else await app.mutate(() => api.media.update(r.id, { name: r.name }));
		renaming = null;
	}

	async function toggleExpand(id: string, mediaId?: string) {
		expanded = expanded === id ? null : id;
		if (mediaId && !peaks[mediaId]) peaks[mediaId] = await api.media.peaks(mediaId, 180).catch(() => []);
	}

	function listen(m: Media) {
		if (!audioEl) return;
		if (playingMedia === m.id) {
			audioEl.pause();
			playingMedia = null;
			return;
		}
		audioEl.src = api.media.fileUrl(m.id);
		audioEl.play().catch(() => toasts.info('Audio preview isn’t available in demo mode'));
		playingMedia = m.id;
		if (!peaks[m.id])
			api.media
				.peaks(m.id, 180)
				.then((p) => (peaks[m.id] = p))
				.catch(() => {});
	}

	/** Colorful strip generated from the sequence hash, used until the daemon's PNG thumbnail loads. */
	function strip(s: Sequence): string {
		let h = 0;
		for (const c of s.hash || s.id) h = (h * 31 + c.charCodeAt(0)) >>> 0;
		const stops = Array.from({ length: 6 }, (_, i) => {
			const hue = (h >> (i * 5)) % 360;
			return `hsl(${hue} 85% ${45 + ((h >> i) % 15)}%) ${(i / 5) * 100}%`;
		});
		return `linear-gradient(90deg, ${stops.join(', ')})`;
	}
</script>

<audio
	bind:this={audioEl}
	ontimeupdate={() => (audioPos = audioEl && audioEl.duration ? audioEl.currentTime / audioEl.duration : 0)}
	onended={() => (playingMedia = null)}
></audio>

<div
	class="page"
	ondragover={(e) => {
		e.preventDefault();
		dragging = true;
	}}
	ondragleave={(e) => {
		if (!(e.currentTarget as HTMLElement).contains(e.relatedTarget as Node)) dragging = false;
	}}
	ondrop={onDrop}
	role="region"
	aria-label="Sequences and audio"
>
	<PageHeader
		title="Sequences & Audio"
		subtitle="Upload your xLights sequences and their songs — PixelPlus pairs them up for you."
	>
		{#snippet actions()}
			<button class="btn primary" onclick={() => fileInput?.click()}
				><UploadCloud size={16} /> Upload files</button
			>
		{/snippet}
	</PageHeader>
	<input
		bind:this={fileInput}
		type="file"
		multiple
		accept=".fseq,audio/*,.mp3,.ogg,.m4a,.wav,.flac"
		class="sr-only"
		onchange={(e) => {
			const f = [...((e.target as HTMLInputElement).files ?? [])];
			if (f.length) handleFiles(f);
			(e.target as HTMLInputElement).value = '';
		}}
	/>

	<button class="drop" class:active={dragging} onclick={() => fileInput?.click()}>
		<span class="dicon"><UploadCloud size={24} /></span>
		<span
			><strong>Drop your sequences and songs here</strong>&nbsp;<span class="faint small">
				— or click to choose. Use the light sequence files from your xLights show folder; songs with matching
				names are paired automatically.</span
			></span
		>
	</button>

	{#if uploads.length}
		<div class="uploads card" transition:slide>
			{#each uploads as u (u.id)}
				<div class="up">
					<span class="uic {u.state}"
						>{#if u.state === 'done'}<CircleCheck size={16} />{:else if u.state === 'error'}<CircleAlert
								size={16}
							/>{:else}<UploadCloud size={16} />{/if}</span
					>
					<div class="grow">
						<div class="row between">
							<span class="ellipsis small"
								><strong>{u.name}</strong> <span class="faint">{u.detail}</span></span
							><span class="faint tiny num"
								>{u.state === 'error' ? u.error : `${Math.round(u.progress * 100)}%`}</span
							>
						</div>
						<div class="progress">
							<span
								style:width="{u.progress * 100}%"
								style:background={u.state === 'error'
									? 'var(--red)'
									: u.state === 'done'
										? 'var(--green)'
										: ''}
							></span>
						</div>
					</div>
				</div>
			{/each}
		</div>
	{/if}

	<div class="toolbar">
		<Segmented
			bind:value={tab}
			label="Library"
			options={[
				{ value: 'sequences', label: `Sequences${show ? ` · ${show.sequences.length}` : ''}`, icon: Film },
				{ value: 'audio', label: `Audio${show ? ` · ${show.media.length}` : ''}`, icon: Music }
			]}
		/>
		<span class="grow"></span>
		<div class="input-group" style="max-width:280px">
			<span class="prefix"><Search size={16} /></span><input
				class="input"
				placeholder="Search"
				bind:value={q}
				data-search
				aria-label="Search"
			/>
		</div>
		{#if show && (show.sequences.length || show.media.length)}
			<button
				class="btn sm"
				class:primary={selecting}
				onclick={() => (selecting ? endSelect() : (selecting = true))}
				aria-pressed={selecting}
				aria-label={selecting ? 'Done selecting' : 'Select songs to tag'}
				><ListChecks size={15} /><span class="hide-sm">{selecting ? 'Done' : 'Select'}</span></button
			>
		{/if}
	</div>

	{#if tags.length}
		<div class="tagbar" role="group" aria-label="Filter by tag">
			<Tags size={14} />
			{#each tags as t (t.name)}
				<button
					class="tf"
					class:on={tagFilter.includes(t.name)}
					aria-pressed={tagFilter.includes(t.name)}
					onclick={() => toggleTag(t.name)}>{t.name}<span class="faint num">{t.count}</span></button
				>
			{/each}
			{#if tagFilter.length}<button class="tf clear" onclick={() => (tagFilter = [])}
					><X size={12} /> Clear</button
				>{/if}
		</div>
	{/if}

	{#if !show}
		<div class="card card-pad"><Skeleton count={6} h={40} /></div>
	{:else if tab === 'sequences'}
		{#if !show.sequences.length}
			<div class="card">
				<EmptyState
					icon={Film}
					title="No sequences yet"
					message="In xLights, save your sequence, then upload it from your xLights show folder together with its song. Several at once is fine."
				>
					<button class="btn primary" onclick={() => fileInput?.click()}
						><UploadCloud size={16} /> Upload sequences</button
					>
				</EmptyState>
			</div>
		{:else}
			<div class="card list">
				{#each seqs as s (s.id)}
					{@const m = mediaOf(s)}
					{@const ld = loudness(m)}
					{@const am = audioMatch(s)}
					{@const busy = activeJob(s.id, 'autoshow')}
					<div
						class="srow"
						id="row-{s.id}"
						class:fresh={fresh.has(s.id)}
						class:picked={picked.includes(s.id)}
					>
						<div class="list-row">
							{#if selecting}
								<input
									type="checkbox"
									class="pick"
									checked={picked.includes(s.id)}
									onchange={() => togglePick(s.id)}
									aria-label="Select {s.name}"
								/>
							{/if}
							<button
								class="thumb"
								style:background={strip(s)}
								onclick={() => toggleExpand(s.id, m?.id)}
								aria-label="Show details for {s.name}"
							>
								{#if s.thumbnail}<img
										src={api.sequences.thumbnailUrl(s.id)}
										alt=""
										loading="lazy"
										onerror={(e) => ((e.target as HTMLImageElement).style.display = 'none')}
									/>{/if}
							</button>
							<div class="grow sinfo">
								<button class="sname ellipsis" onclick={() => toggleExpand(s.id, m?.id)}
									>{s.name}{#if s.generated}<span class="auto" title="Made by PixelPlus"
											><Sparkles size={11} /> Auto</span
										>{/if}</button
								>
								<div class="faint small row wrap" style="gap:6px 10px">
									<span class="num">{fmtDuration(s.durationMs)}</span>
									{#if m?.analysis && m.analysis.bpm > 0}<span
											class="bpm num"
											title="{energyWord(m.analysis.energy)} · {m.analysis.sections} parts"
											>{Math.round(m.analysis.bpm)} BPM · {energyGlyph(m.analysis.energy)}</span
										>{/if}
									{#if m}<span class="linked"><Link2 size={12} /> {m.name}</span>{:else if am}
										<button class="suggest" onclick={() => linkAudio(s, am.id)}
											><Link2 size={12} /> Link “{am.name}”?</button
										>
									{:else}<span class="nolink"><Link2Off size={12} /> Light-only (no song)</span>{/if}
									{#if busy}<span class="updating">Updating… {busy.pct}%</span>{/if}
									<span class="hide-sm"
										><TagChips
											tags={s.tags ?? []}
											{defs}
											active={tagFilter}
											onpick={toggleTag}
											max={3}
										/></span
									>
								</div>
							</div>
							{#if ld}<span
									class="badge {ld.cls} hide-sm"
									title="Turn on volume leveling (Settings → Audio) to even this out"
									><Volume2 size={12} /> {ld.label}</span
								>{/if}
							<button
								class="btn sm ghost icon"
								onclick={() => (previewSeq = s)}
								aria-label="Preview {s.name} on this device"
								title="Preview here (lights stay as they are)"><Eye size={15} /></button
							>
							<button
								class="btn sm ghost icon"
								onclick={() => playerAct(() => api.play({ sequenceId: s.id }))}
								aria-label="Play {s.name} on the display"
								title="Play on the display"><Play size={15} /></button
							>
							<button
								class="btn sm ghost icon hide-sm"
								onclick={() => (renaming = { kind: 'seq', id: s.id, name: s.name })}
								aria-label="Rename {s.name}"><Pencil size={14} /></button
							>
							<button class="btn sm ghost icon" onclick={() => removeSeq(s)} aria-label="Delete {s.name}"
								><Trash2 size={14} /></button
							>
						</div>
						{#if expanded === s.id}
							<div class="detail" transition:slide={{ duration: 180 }}>
								{#if s.generated}
									<div class="gen">
										<Sparkles size={15} />
										<span class="grow small"
											>Made by PixelPlus from “{show.media.find((x) => x.id === s.generated?.mediaId)?.name ??
												'a song'}”. It updates itself when you change your props.</span
										>
										<button class="btn sm" onclick={() => regenerate(s, true)} disabled={!!busy}
											><RefreshCw size={13} /> New moves</button
										>
									</div>
								{/if}
								{#if m}
									<div class="wave">
										<button
											class="btn icon sm"
											onclick={() => listen(m)}
											aria-label={playingMedia === m.id ? 'Pause preview' : 'Preview audio'}
											>{#if playingMedia === m.id}<Pause size={15} />{:else}<Play size={15} />{/if}</button
										>
										<div class="grow">
											<Waveform
												peaks={peaks[m.id] ?? []}
												progress={playingMedia === m.id ? audioPos : 0}
												height={44}
											/>
										</div>
									</div>
								{/if}
								{#if m}<AnalysisView media={m} />{/if}
								<div class="field">
									<span class="label">Tags</span>
									<TagInput
										tags={s.tags ?? []}
										suggestions={tags.map((t) => t.name)}
										{defs}
										label="Add a tag to {s.name}"
										onchange={(t) => setSeqTags(s, t)}
									/>
								</div>
								<div class="form-grid">
									<label class="field"
										><span class="label">Audio</span>
										<select
											class="select"
											value={s.mediaId ?? ''}
											onchange={(e) => linkAudio(s, (e.target as HTMLSelectElement).value)}
										>
											<option value="">No audio (lights only)</option>
											{#each show.media.filter((x) => x.kind === 'song') as mm (mm.id)}<option value={mm.id}
													>{mm.name}</option
												>{/each}
										</select>
									</label>
									<div class="field">
										<span class="label">From xLights</span>
										<div class="faint small mono" style="padding-top:10px">{s.xlightsName ?? s.file}</div>
									</div>
									<details class="adv span-2">
										<summary class="small muted">Advanced</summary>
										<div class="faint small" style="margin-top:6px">
											Smoothness: <strong>{smoothness(s.frameMs)}</strong> ({Math.round(1000 / s.frameMs)} updates
											a second, {s.generated
												? 'as PixelPlus made it'
												: 'set in xLights when the sequence was made'})
										</div>
										{#if playsText(s.id)}<div class="faint small">{playsText(s.id)}</div>{/if}
									</details>
								</div>
							</div>
						{/if}
					</div>
				{:else}
					<div class="card-body faint small">
						No sequences match{q ? ` “${q}”` : ''}{tagFilter.length ? ` with ${tagFilter.join(' + ')}` : ''}.
						{#if tagFilter.length}<button class="linkbtn" onclick={() => (tagFilter = [])}>Clear tags</button
							>{/if}
					</div>
				{/each}
			</div>
		{/if}
	{:else}
		{#if !show.media.length}
			<div class="card">
				<EmptyState
					icon={Music}
					title="No audio yet"
					message="Upload songs (mp3, ogg, m4a, wav or flac). They’re loudness-matched so every song plays at the same volume."
				/>
			</div>
		{:else}
			<div class="card list">
				{#each media as m (m.id)}
					{@const ld = loudness(m)}
					<div
						class="srow"
						id="row-{m.id}"
						class:fresh={fresh.has(m.id)}
						class:picked={picked.includes(m.id)}
					>
						<div class="list-row">
							{#if selecting}
								<input
									type="checkbox"
									class="pick"
									checked={picked.includes(m.id)}
									onchange={() => togglePick(m.id)}
									aria-label="Select {m.name}"
								/>
							{/if}
							<button
								class="play-dot"
								class:on={playingMedia === m.id}
								onclick={() => listen(m)}
								aria-label={playingMedia === m.id ? `Pause ${m.name}` : `Play ${m.name}`}
							>
								{#if playingMedia === m.id}<Pause size={16} />{:else if m.kind === 'dj'}<Mic
										size={16}
									/>{:else if m.kind === 'sfx'}<AudioLines size={16} />{:else}<Music size={16} />{/if}
							</button>
							<div class="grow sinfo">
								<button class="sname ellipsis" onclick={() => (expanded = expanded === m.id ? null : m.id)}
									>{m.name}</button
								>
								<div class="faint small row wrap" style="gap:6px 10px">
									<span class="num">{fmtDuration(m.durationMs)}</span><span
										>{m.kind === 'song' ? 'Song' : m.kind === 'dj' ? 'DJ clip' : 'Sound effect'}</span
									>
									{#if m.analysis && m.analysis.bpm > 0}<span class="bpm num"
											>{Math.round(m.analysis.bpm)} BPM · {energyGlyph(m.analysis.energy)}</span
										>{:else if activeJob(m.id, 'analysis')}<span class="updating"
											>Listening for the beat…</span
										>{/if}
									<span class="hide-sm"
										><TagChips
											tags={m.tags ?? []}
											{defs}
											active={tagFilter}
											onpick={toggleTag}
											max={3}
										/></span
									>
									{#if show.sequences.some((s) => s.mediaId === m.id)}<span class="linked"
											><Link2 size={12} /> {show.sequences.find((s) => s.mediaId === m.id)?.name}</span
										>{/if}
								</div>
							</div>
							{#if playingMedia === m.id}<div class="mini-wave hide-sm">
									<Waveform peaks={peaks[m.id] ?? []} progress={audioPos} height={28} />
								</div>{/if}
							{#if ld}<span class="badge {ld.cls} hide-sm"><Volume2 size={12} /> {ld.label}</span>{/if}
							{#if m.kind !== 'sfx'}
								<button
									class="btn sm ghost make"
									onclick={() => (autoFor = m)}
									aria-label="Make a light show for {m.name}"
									title="Make a light show for this song"
									><Sparkles size={14} /><span class="hide-sm">Light show</span></button
								>
							{/if}
							<button
								class="btn sm ghost icon hide-sm"
								onclick={() => (renaming = { kind: 'media', id: m.id, name: m.name })}
								aria-label="Rename {m.name}"><Pencil size={14} /></button
							>
							<button class="btn sm ghost icon" onclick={() => removeMedia(m)} aria-label="Delete {m.name}"
								><Trash2 size={14} /></button
							>
						</div>
						{#if expanded === m.id}
							<div class="detail media-detail" transition:slide={{ duration: 180 }}>
								<AnalysisView media={m} />
								<div class="field">
									<span class="label">Tags</span>
									<TagInput
										tags={m.tags ?? []}
										suggestions={tags.map((t) => t.name)}
										{defs}
										label="Add a tag to {m.name}"
										onchange={(t) => setMediaTags(m, t)}
									/>
								</div>
								{#if m.kind !== 'sfx' && !show.sequences.some((x) => x.mediaId === m.id)}
									<div class="gen">
										<Sparkles size={15} />
										<span class="grow small">No light sequence for this song yet.</span>
										<button class="btn sm primary" onclick={() => (autoFor = m)}>Make a light show</button>
									</div>
								{/if}
							</div>
						{/if}
					</div>
				{:else}
					<div class="card-body faint small">No audio matches your search.</div>
				{/each}
			</div>
			<p class="faint tiny" style="margin-top:10px">
				{show.settings.audio.normalize
					? 'Volume leveling is on: every song plays at the same loudness.'
					: 'Volume leveling is off. Turn it on in Settings → Audio so every song plays at the same loudness.'}
			</p>
		{/if}
	{/if}
</div>

{#if selecting}
	<div class="bulkbar" transition:slide={{ duration: 160 }} role="region" aria-label="Tag selected items">
		<span class="small"
			><strong class="num">{picked.length}</strong> selected
			{#if visibleIds.length && picked.length < visibleIds.length}<button
					class="linkbtn"
					onclick={() => (picked = [...new Set([...picked, ...visibleIds])])}>Select all</button
				>{/if}</span
		>
		<input
			class="input sm"
			bind:value={bulkText}
			placeholder="Tag, e.g. kids"
			aria-label="Tag to add or remove"
			list="bulk-tags"
			onkeydown={(e) => e.key === 'Enter' && bulk('add')}
		/>
		<datalist id="bulk-tags"
			>{#each tags as t (t.name)}<option value={t.name}></option>{/each}</datalist
		>
		<button class="btn sm primary" onclick={() => bulk('add')} disabled={!picked.length || !bulkText.trim()}
			>Add tag</button
		>
		<button class="btn sm" onclick={() => bulk('remove')} disabled={!picked.length || !bulkText.trim()}
			>Remove</button
		>
		<button class="btn sm ghost icon" onclick={endSelect} aria-label="Stop selecting"><X size={15} /></button>
	</div>
{/if}

<Modal
	open={!!previewSeq}
	title={previewSeq ? previewSeq.name : 'Preview'}
	subtitle="Plays on this device only"
	size="lg"
	onclose={() => (previewSeq = null)}
>
	{#if previewSeq}
		<SequencePreview sequenceId={previewSeq.id} mediaId={previewSeq.mediaId} autoplay />
	{/if}
	{#snippet footer()}
		{#if previewSeq}
			<a class="btn ghost" href="/layout?preview={previewSeq.id}">Open on the Layout page</a>
			<button
				class="btn"
				onclick={() => {
					const s = previewSeq;
					previewSeq = null;
					if (s) playerAct(() => api.play({ sequenceId: s.id }));
				}}><Play size={15} /> Play on the lights</button
			>
		{/if}
	{/snippet}
</Modal>

<AutoShowDialog
	bind:media={autoFor}
	oncreated={(id) => {
		tab = 'sequences';
		fresh = new Set([id]);
		setTimeout(() => (fresh = new Set()), 4000);
		requestAnimationFrame(() =>
			document.getElementById(`row-${id}`)?.scrollIntoView({ behavior: 'smooth', block: 'center' })
		);
	}}
/>

<Modal open={!!renaming} title="Rename" size="sm" onclose={() => (renaming = null)}>
	{#if renaming}<form id="ren" onsubmit={doRename}>
			<input class="input" bind:value={renaming.name} aria-label="Name" />
		</form>{/if}
	{#snippet footer()}
		<button class="btn ghost" onclick={() => (renaming = null)}>Cancel</button>
		<button class="btn primary" type="submit" form="ren">Save</button>
	{/snippet}
</Modal>

<style>
	.drop {
		display: flex;
		align-items: center;
		gap: 16px;
		width: 100%;
		padding: 20px 22px;
		margin-bottom: 20px;
		border-radius: 16px;
		border: 1.5px dashed var(--border-3);
		background: var(--surface);
		text-align: left;
		transition: all 160ms var(--ease);
	}
	.drop:hover,
	.drop.active {
		border-color: var(--accent);
		background: var(--accent-soft);
	}
	.drop.active {
		transform: scale(1.005);
	}
	.dicon {
		width: 48px;
		height: 48px;
		border-radius: 14px;
		display: grid;
		place-items: center;
		background: var(--accent-soft);
		color: var(--accent-text);
		flex: 0 0 auto;
	}
	.uploads {
		padding: 8px 16px;
		margin-bottom: 20px;
	}
	.up {
		display: flex;
		align-items: center;
		gap: 12px;
		padding: 10px 0;
		border-bottom: 1px solid var(--border);
	}
	.up:last-child {
		border-bottom: 0;
	}
	.up .progress {
		margin-top: 6px;
		height: 4px;
	}
	.uic {
		color: var(--accent-text);
		display: flex;
	}
	.uic.done {
		color: var(--green);
	}
	.uic.error {
		color: var(--red);
	}
	.srow {
		border-bottom: 1px solid var(--border);
	}
	.srow:last-child {
		border-bottom: 0;
	}
	.srow .list-row {
		border-bottom: 0;
	}
	.thumb {
		width: 72px;
		height: 44px;
		border-radius: 8px;
		flex: 0 0 auto;
		position: relative;
		overflow: hidden;
		box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.08);
	}
	.thumb::after {
		content: '';
		position: absolute;
		inset: 0;
		background:
			repeating-linear-gradient(90deg, transparent 0 3px, rgba(0, 0, 0, 0.35) 3px 4px),
			linear-gradient(180deg, transparent 40%, rgba(0, 0, 0, 0.45));
	}
	.thumb img {
		width: 100%;
		height: 100%;
		object-fit: cover;
	}
	.sinfo {
		display: flex;
		flex-direction: column;
		gap: 2px;
		min-width: 0;
	}
	.sname {
		font-weight: 580;
		text-align: left;
		max-width: 100%;
	}
	.linked {
		display: inline-flex;
		align-items: center;
		gap: 4px;
		color: var(--text-2);
		max-width: 260px;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.nolink {
		display: inline-flex;
		align-items: center;
		gap: 4px;
		color: var(--text-3);
	}
	.suggest {
		display: inline-flex;
		align-items: center;
		gap: 4px;
		min-height: 28px;
		padding: 0 10px;
		border-radius: 99px;
		background: var(--accent-soft);
		color: var(--accent-text);
		font-size: 12px;
		font-weight: 600;
	}
	.suggest:hover {
		background: var(--accent);
		color: var(--accent-fg);
	}
	@media (pointer: coarse) {
		.suggest {
			min-height: 36px;
		}
	}
	.srow.fresh {
		animation: fresh 3.6s var(--ease);
	}
	@keyframes fresh {
		0%,
		60% {
			background: var(--accent-soft);
			box-shadow: inset 3px 0 0 var(--accent);
		}
	}
	.adv summary {
		cursor: pointer;
	}
	.tagbar {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 6px;
		margin: -6px 0 14px;
		color: var(--text-3);
	}
	.tf {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		height: 30px;
		padding: 0 12px;
		border-radius: 99px;
		font-size: 12.5px;
		font-weight: 550;
		color: var(--text-2);
		background: var(--surface);
		border: 1px solid var(--border);
	}
	.tf:hover {
		color: var(--text);
		border-color: var(--border-3);
	}
	.tf.on {
		background: var(--accent-soft);
		border-color: var(--accent);
		color: var(--accent-text);
	}
	.tf.clear {
		border-style: dashed;
	}
	.pick {
		width: 20px;
		height: 20px;
		flex: 0 0 auto;
		accent-color: var(--accent);
	}
	.srow.picked {
		background: var(--accent-soft);
	}
	.auto {
		display: inline-flex;
		align-items: center;
		gap: 3px;
		margin-left: 8px;
		padding: 1px 7px;
		border-radius: 99px;
		font-size: 11px;
		font-weight: 600;
		vertical-align: 2px;
		color: var(--accent-text);
		background: var(--accent-soft);
	}
	.bpm {
		color: var(--text-2);
		white-space: nowrap;
	}
	.updating {
		color: var(--accent-text);
		font-size: 12px;
	}
	.gen {
		display: flex;
		align-items: center;
		gap: 10px;
		flex-wrap: wrap;
		padding: 10px 12px;
		border-radius: 12px;
		background: var(--accent-soft);
		color: var(--accent-text);
	}
	.gen .small {
		color: var(--text-2);
	}
	.make {
		gap: 6px;
		color: var(--accent-text);
	}
	.linkbtn {
		color: var(--accent-text);
		font-weight: 600;
		margin-left: 6px;
		text-decoration: underline;
		text-underline-offset: 2px;
	}
	.bulkbar {
		position: fixed;
		left: 50%;
		transform: translateX(-50%);
		bottom: calc(var(--transport-h, 72px) + 12px);
		z-index: 40;
		display: flex;
		align-items: center;
		gap: 8px;
		flex-wrap: wrap;
		width: min(720px, calc(100% - 24px));
		padding: 10px 12px;
		border-radius: 16px;
		background: var(--surface);
		border: 1px solid var(--border-2);
		box-shadow: var(--shadow-3, 0 10px 30px rgba(0, 0, 0, 0.35));
	}
	.bulkbar .input {
		flex: 1 1 140px;
		min-width: 120px;
	}
	.media-detail {
		padding-left: 72px;
	}
	.detail {
		padding: 0 20px 18px 104px;
		display: flex;
		flex-direction: column;
		gap: 14px;
	}
	.wave {
		display: flex;
		align-items: center;
		gap: 12px;
		padding: 10px 12px;
		border-radius: 12px;
		background: var(--surface-2);
	}
	.play-dot {
		width: 40px;
		height: 40px;
		border-radius: 12px;
		display: grid;
		place-items: center;
		background: var(--surface-3);
		color: var(--text-2);
		flex: 0 0 auto;
		transition: all 150ms;
	}
	.play-dot:hover,
	.play-dot.on {
		background: var(--accent);
		color: var(--accent-fg);
	}
	.mini-wave {
		width: 160px;
	}
	@media (max-width: 760px) {
		.hide-sm {
			display: none;
		}
		.detail,
		.media-detail {
			padding: 0 16px 16px;
		}
		.bulkbar {
			bottom: calc(var(--tabbar-h, 64px) + 84px + env(safe-area-inset-bottom));
		}
		.thumb {
			width: 52px;
		}
		.drop {
			padding: 14px;
		}
	}
</style>
