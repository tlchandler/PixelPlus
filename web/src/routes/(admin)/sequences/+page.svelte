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
		AudioLines
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
	const seqs = $derived(
		show?.sequences.filter((s) => !q || s.name.toLowerCase().includes(q.toLowerCase())) ?? []
	);
	const media = $derived(
		show?.media.filter((m) => !q || m.name.toLowerCase().includes(q.toLowerCase())) ?? []
	);

	function mediaOf(s: Sequence): Media | undefined {
		return show?.media.find((m) => m.id === s.mediaId);
	}

	function loudness(m?: Media) {
		if (!m?.loudnessLufs) return null;
		const d = m.loudnessLufs - target;
		if (Math.abs(d) <= 2) return { label: 'Balanced', cls: 'green', d };
		return d > 0
			? { label: `Loud · ${d.toFixed(0)} dB`, cls: 'accent', d }
			: { label: `Quiet · ${(-d).toFixed(0)} dB`, cls: 'blue', d };
	}

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
			toasts.warn(`Skipped ${o.name} — only .fseq and audio files can be uploaded here`);
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
		await Promise.allSettled(jobs);
		await app.reloadShow();
		const ok = uploads.filter((u) => u.state === 'done').length;
		if (ok) toasts.success(`Uploaded ${plural(ok, 'file')}`);
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
		subtitle="Upload .fseq files from xLights together with their songs — PixelPlus links them up."
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
			><strong>Drop .fseq and audio files here</strong>&nbsp;<span class="faint small">
				— or click to choose. Several at once is fine; matching names are paired automatically.</span
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
	</div>

	{#if !show}
		<div class="card card-pad"><Skeleton count={6} h={40} /></div>
	{:else if tab === 'sequences'}
		{#if !show.sequences.length}
			<div class="card">
				<EmptyState
					icon={Film}
					title="No sequences yet"
					message="In xLights, save your sequence (File → Save) and upload the .fseq from your show folder along with the song."
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
					<div class="srow">
						<div class="list-row">
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
								<button class="sname ellipsis" onclick={() => toggleExpand(s.id, m?.id)}>{s.name}</button>
								<div class="faint small row wrap" style="gap:6px 10px">
									<span class="num">{fmtDuration(s.durationMs)}</span>
									<span class="num">{Math.round(1000 / s.frameMs)} fps</span>
									{#if m}<span class="linked"><Link2 size={12} /> {m.name}</span>{:else}<span class="nolink"
											><Link2Off size={12} /> No audio</span
										>{/if}
								</div>
							</div>
							{#if ld}<span
									class="badge {ld.cls} hide-sm"
									title="Loudness {m?.loudnessLufs} LUFS · target {target} LUFS{show.settings.audio.normalize
										? ' · normalized automatically'
										: ''}"><Volume2 size={12} /> {ld.label}</span
								>{/if}
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
								</div>
							</div>
						{/if}
					</div>
				{:else}
					<div class="card-body faint small">No sequences match “{q}”.</div>
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
					<div class="srow">
						<div class="list-row">
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
								<span class="sname ellipsis">{m.name}</span>
								<div class="faint small row" style="gap:10px">
									<span class="num">{fmtDuration(m.durationMs)}</span><span
										>{m.kind === 'song' ? 'Song' : m.kind === 'dj' ? 'DJ clip' : 'Sound effect'}</span
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
							<button
								class="btn sm ghost icon hide-sm"
								onclick={() => (renaming = { kind: 'media', id: m.id, name: m.name })}
								aria-label="Rename {m.name}"><Pencil size={14} /></button
							>
							<button class="btn sm ghost icon" onclick={() => removeMedia(m)} aria-label="Delete {m.name}"
								><Trash2 size={14} /></button
							>
						</div>
					</div>
				{/each}
			</div>
			<p class="faint tiny" style="margin-top:10px">
				Loudness is measured on upload. With normalization on (Settings → Audio) every song plays at {target} LUFS.
			</p>
		{/if}
	{/if}
</div>

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
		color: var(--accent-text);
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
		.detail {
			padding: 0 16px 16px;
		}
		.thumb {
			width: 52px;
		}
		.drop {
			padding: 14px;
		}
	}
</style>
