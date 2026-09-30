<script lang="ts">
	import { untrack } from 'svelte';
	import { api } from '$lib/api/client';
	import type { DjClip, DjLine, DjVoice, Pronunciation } from '$lib/api/types';
	import { DJ_PLACEHOLDERS } from '$lib/api/types';
	import { app } from '$lib/stores/app.svelte';
	import { toasts, confirm } from '$lib/stores/toasts.svelte';
	import { ENERGY_LEVELS, KOKORO_VOICES } from '$lib/util/voices';
	import { fmtDuration } from '$lib/util/format';
	import { newId } from '$lib/util/id';
	import { renderSpeech, renderWhere, playBlob, sampleText } from '$lib/tts';
		import { sortable, moveItem } from '$lib/actions/sortable';
	import { PLACEHOLDER_LABELS, PAUSES, tokenize } from '$lib/util/djtokens';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import Segmented from '$lib/components/ui/Segmented.svelte';
	import Switch from '$lib/components/ui/Switch.svelte';
	import Modal from '$lib/components/ui/Modal.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
		import Skeleton from '$lib/components/ui/Skeleton.svelte';
	import SaveState from '$lib/components/ui/SaveState.svelte';
	import VoiceEditor from '$lib/components/dj/VoiceEditor.svelte';
	import {
		Mic,
		Plus,
		Play,
		Square,
		Trash2,
		GripVertical,
		Wand2,
		LoaderCircle,
		ListPlus,
		Users,
		BookA,
		Pencil,
		Braces,
		Cpu,
		Globe,
		Check,
		Music2,
		Copy
	} from '@lucide/svelte';

	const show = $derived(app.show);
	let tab = $state<'clips' | 'voices' | 'words'>('clips');
	let clipId = $state<string | null>(null);
	let draft = $state<DjClip | null>(null);
	let saveState = $state<'saved' | 'saving' | 'dirty'>('saved');
	let saveTimer: ReturnType<typeof setTimeout>;
	let focused = $state<{ i: number; el: HTMLTextAreaElement } | null>(null);
	let rendering = $state<number | null>(null);
	let renderMsg = $state('');
	let where = $state<'device' | 'browser'>('device');
	let stopPlay = $state<(() => void) | null>(null);
	let auditioning = $state<string | null>(null);
	let editVoice = $state<DjVoice | null>(null);
	let addToPl = $state(false);
	let plTarget = $state('');
	let words = $state<Pronunciation[]>([]);

	$effect(() => {
		if (show) renderWhere(show).then((w) => (where = w));
	});
	$effect(() => {
		if (!show) return;
		if (!clipId || !show.djClips.some((c) => c.id === clipId)) clipId = show.djClips[0]?.id ?? null;
	});
	$effect(() => {
		const c = show?.djClips.find((x) => x.id === clipId);
		untrack(() => {
			if (c && (draft?.id !== c.id || saveState === 'saved'))
				draft = structuredClone($state.snapshot(c) as DjClip);
			if (!c) draft = null;
		});
	});
		$effect(() => {
		const list = show?.pronunciations;
		// Only take the server's list when it's really different (not while a row is half typed).
		if (list)
			untrack(() => {
				if (wordsSave === 'saved' && JSON.stringify(cleanWords(words)) !== JSON.stringify(list))
					words = structuredClone($state.snapshot(list) as Pronunciation[]);
			});
	});
	const cleanWords = (ws: Pronunciation[]) =>
		ws.filter((w) => w.word.trim() && w.say.trim()).map((w) => ({ word: w.word, say: w.say }));
	let wordsSave = $state<'saved' | 'saving' | 'dirty'>('saved');
	let wordsTimer: ReturnType<typeof setTimeout> | undefined;
	// The dictionary saves itself: complete rows are stored a moment after you stop typing.
	$effect(() => {
		const clean = JSON.stringify(cleanWords(words));
		if (clean === JSON.stringify(untrack(() => show?.pronunciations ?? []))) return;
		wordsSave = 'dirty';
		clearTimeout(wordsTimer);
		wordsTimer = setTimeout(async () => {
			wordsSave = 'saving';
			try {
				await api.savePronunciations(cleanWords($state.snapshot(words) as Pronunciation[]));
				await app.reloadShow();
			} catch (e) {
				toasts.error('Couldn’t save the pronunciations', (e as Error).message);
			} finally {
				wordsSave = 'saved';
			}
		}, 800);
	});

	function queueSave() {
		saveState = 'dirty';
		if (draft) draft.dynamic = draft.lines.some((l) => /\{\w+\}/.test(l.text));
		clearTimeout(saveTimer);
		saveTimer = setTimeout(save, 700);
	}
	async function save() {
		if (!draft) return;
		saveState = 'saving';
		try {
			await api.djClips.update(draft.id, $state.snapshot(draft) as DjClip);
			await app.reloadShow();
			saveState = 'saved';
		} catch (e) {
			toasts.error('Could not save clip', (e as Error).message);
			saveState = 'dirty';
		}
	}

	async function newClip() {
		const v = show?.djVoices[0]?.id ?? 'af_heart';
		const c = await app.mutate(() =>
			api.djClips.create({
				name: 'New DJ clip',
				dynamic: false,
				speed: 1,
				lines: [{ voice: v, text: '', pauseMs: 300, energy: 0.4 }]
			})
		);
				if (c) {
			clipId = c.id;
			tab = 'clips';
			focusTitle();
		}
	}
	let titleInput: HTMLInputElement | undefined = $state();
	/** New clip: name field focused and selected, ready to type over. */
	function focusTitle() {
		setTimeout(() => {
			titleInput?.focus();
			titleInput?.select();
		}, 80);
	}
	/** Lines whose voice picker shows every base voice (the curated DJs come first). */
	let allVoices = $state<Set<number>>(new Set());
	const isDj = (id: string) => !!show?.djVoices.some((v) => v.id === id);
	function pickVoice(line: DjLine, i: number, e: Event) {
		const el = e.target as HTMLSelectElement;
		if (el.value === '__more') {
			el.value = line.voice;
			allVoices = new Set([...allVoices, i]);
			// Re-open the picker, now with every voice in it.
			requestAnimationFrame(() => {
				try {
					(document.getElementById(`voice-${i}`) as HTMLSelectElement | null)?.showPicker?.();
				} catch {
					/* not supported: the longer list is there on the next tap */
				}
			});
			return;
		}
		line.voice = el.value;
		queueSave();
	}
	let tokensOpen = $state(false);
	/** Pauses are picked in words; older clips' odd values snap to the closest one. */
	function nearestPause(ms: number) {
		return PAUSES.reduce((best, p) => (Math.abs(p.ms - ms) < Math.abs(best - ms) ? p.ms : best), 0);
	}
	async function removeClip() {
		if (!draft) return;
		const d = structuredClone($state.snapshot(draft) as DjClip);
		if (!(await confirm({ title: `Delete “${d.name}”?`, confirmLabel: 'Delete', danger: true }))) return;
		await app.mutate(() => api.djClips.remove(d.id));
		toasts.success(`Deleted ${d.name}`, {
			label: 'Undo',
			run: () => app.mutate(() => api.djClips.create(d))
		});
	}
	async function duplicateClip() {
		if (!draft) return;
		const d = $state.snapshot(draft) as DjClip;
		const c = await app.mutate(
			() =>
				api.djClips.create({
					...d,
					id: undefined as unknown as string,
					name: `${d.name} copy`,
					mediaId: undefined
				}),
			{ success: 'Clip duplicated' }
		);
		if (c) clipId = c.id;
	}

		/** New lines go to the other DJ: two-voice banter is the common case. */
	function addLine() {
		if (!draft) return;
		const last = draft.lines[draft.lines.length - 1];
		const inClip = [...new Set(draft.lines.map((l) => l.voice))];
		const partner =
			[...draft.lines].reverse().find((l) => l.voice !== last?.voice)?.voice ??
			inClip.find((v) => v !== last?.voice) ??
			show?.djVoices.find((v) => v.id !== last?.voice)?.id ??
			last?.voice ??
			'af_heart';
		draft.lines = [...draft.lines, { voice: partner, text: '', pauseMs: 300, energy: 0.4 }];
		queueSave();
	}
	function removeLine(i: number) {
		if (!draft) return;
		draft.lines = draft.lines.filter((_, k) => k !== i);
		queueSave();
	}
		function insertPlaceholder(p: string) {
		tokensOpen = false;
		if (!draft) return;
		const token = `{${p}}`;
		if (focused) {
			const { i, el } = focused;
			const s = el.selectionStart ?? el.value.length;
			const e = el.selectionEnd ?? s;
			const line = draft.lines[i];
			line.text = line.text.slice(0, s) + token + line.text.slice(e);
			queueSave();
			requestAnimationFrame(() => {
				el.focus();
				el.setSelectionRange(s + token.length, s + token.length);
			});
		} else if (draft.lines.length) {
			draft.lines[draft.lines.length - 1].text +=
				(draft.lines[draft.lines.length - 1].text ? ' ' : '') + token;
			queueSave();
		}
	}

	function voiceName(id: string) {
		return (
			show?.djVoices.find((v) => v.id === id)?.name ?? KOKORO_VOICES.find((v) => v.id === id)?.name ?? id
		);
	}
	function voiceHue(id: string) {
		let h = 0;
		for (const c of id) h = (h * 31 + c.charCodeAt(0)) % 360;
		return h;
	}

	async function audition(key: string, lines: DjLine[], speed = 1) {
		if (!show) return;
		if (stopPlay) {
			stopPlay();
			stopPlay = null;
			if (auditioning === key) {
				auditioning = null;
				return;
			}
		}
		auditioning = key;
		try {
			const blob = await renderSpeech(
				show,
				lines.map((l) => ({ ...l, text: sampleText(l.text, show) })),
				speed
			);
			if (auditioning !== key) return;
			stopPlay = playBlob(blob, () => {
				stopPlay = null;
				auditioning = null;
			});
		} catch (e) {
			toasts.error('Couldn’t make the voice', (e as Error).message);
			auditioning = null;
		}
	}

	async function renderClip() {
		if (!draft || !show) return;
		if (saveState !== 'saved') await save();
		const id = draft.id;
		rendering = 0;
		try {
			if (where === 'device') {
				renderMsg = 'Making the voice on the controller…';
				const tick = setInterval(() => (rendering = Math.min(0.9, (rendering ?? 0) + 0.07)), 200);
				await api.djClips.render(id);
				clearInterval(tick);
			} else {
				renderMsg = 'Making the voice in this browser (the first time downloads the voices, about a minute)…';
				const blob = await renderSpeech(
					show,
					draft.lines.map((l) => ({ ...l, text: draft!.dynamic ? sampleText(l.text, show) : l.text })),
					draft.speed,
					(p) => (rendering = p * 0.85)
				);
				renderMsg = 'Sending it to the controller…';
				await api.djClips.upload(id, blob, (p) => (rendering = 0.85 + p * 0.15));
			}
			rendering = 1;
			await app.reloadShow();
			toasts.success('Voice ready — the clip can play');
		} catch (e) {
			toasts.error('Couldn’t make the voice', (e as Error).message);
		} finally {
			setTimeout(() => (rendering = null), 600);
		}
	}

	async function previewRendered() {
		if (!draft?.mediaId) return;
		if (stopPlay) {
			stopPlay();
			stopPlay = null;
			return;
		}
		try {
			const r = await fetch(api.media.fileUrl(draft.mediaId));
			const blob = await r.blob();
			stopPlay = playBlob(blob, () => (stopPlay = null));
		} catch {
			toasts.error('Couldn’t load the audio');
		}
	}

	async function addToPlaylist() {
		const pl = show?.playlists.find((p) => p.id === plTarget);
		if (!pl || !draft) return;
		await app.mutate(
			() =>
				api.playlists.update(pl.id, {
					...pl,
					items: [...pl.items, { id: newId(), type: 'dj', djClipId: draft!.id }]
				}),
			{ success: `Added to ${pl.name}` }
		);
		addToPl = false;
	}

	function newVoice() {
		editVoice = {
			id: newId(),
			name: 'New voice',
			description: '',
			blend: { af_heart: 0.5, af_bella: 0.5 },
			speed: 1,
			lang: 'en-us',
			defaultEnergy: 0.4,
			energy: {}
		};
	}

	
	const media = $derived(show?.media.find((m) => m.id === draft?.mediaId));
	const estSec = $derived(
		draft
			? Math.round(
					draft.lines.reduce(
						(n, l) => n + l.text.split(/\s+/).filter(Boolean).length * 0.38 + l.pauseMs / 1000,
						0
					) / (draft.speed || 1)
				)
			: 0
	);
</script>

<div class="page">
	<PageHeader
		title="DJ Studio"
		subtitle="Write radio-style announcements between songs, voiced by your own DJs."
	>
		{#snippet actions()}
			<span
				class="where badge {where === 'device' ? 'green' : 'blue'}"
				title={where === 'device'
					? 'Voices render on this controller'
					: 'Voices render in your browser, then upload'}
			>
								{#if where === 'device'}<Cpu size={12} /> Voices made on the controller{:else}<Globe size={12} /> Voices made in
					this browser{/if}
			</span>
			<button class="btn primary" onclick={newClip}><Plus size={16} /> New clip</button>
		{/snippet}
	</PageHeader>

	<div class="toolbar">
		<Segmented
			bind:value={tab}
			label="Studio sections"
			options={[
				{ value: 'clips', label: 'Clips', icon: Mic },
				{ value: 'voices', label: 'Voices', icon: Users },
				{ value: 'words', label: 'Pronunciation', icon: BookA }
			]}
		/>
	</div>

	{#if !show}
		<div class="card card-pad"><Skeleton count={8} h={30} /></div>
	{:else if tab === 'voices'}
		<div class="voices">
			{#each show.djVoices as v (v.id)}
				<article class="card vcard">
					<div class="vart" style:--h={voiceHue(v.id)}>
						<span class="initial">{v.name[0]}</span>
						<div class="wave" class:on={auditioning === 'v:' + v.id}>
							{#each Array(18) as _, i (i)}<i style:animation-delay="{-i * 70}ms"></i>{/each}
						</div>
					</div>
					<div class="vbody">
						<div class="row">
							<h3 class="grow">{v.name}</h3>
							<span class="badge outline">{v.lang === 'en-gb' ? 'British' : 'American'}</span>
						</div>
						<p class="muted small">{v.description || 'Custom voice'}</p>
						<div class="mix">
							{#each Object.entries(v.blend) as [id, w] (id)}<span class="mixchip"
									>{KOKORO_VOICES.find((x) => x.id === id)?.name ?? id}
									<b class="num">{Math.round(w * 100)}%</b></span
								>{/each}
						</div>
						<div class="row" style="margin-top:auto">
							<button
								class="btn soft sm grow"
								onclick={() =>
									audition(
										'v:' + v.id,
										[
											{
												voice: v.id,
												text: `Hi, I'm ${v.name}! Welcome to ${show!.name}. Grab some cocoa and enjoy the show!`,
												pauseMs: 0,
												energy: v.defaultEnergy
											}
										],
										v.speed
									)}
							>
								{#if auditioning === 'v:' + v.id && !stopPlay}<LoaderCircle size={14} class="spin" /> Rendering{:else if auditioning === 'v:' + v.id}<Square
										size={13}
									/> Stop{:else}<Play size={14} /> Audition{/if}
							</button>
							<button
								class="btn sm"
								onclick={() => (editVoice = structuredClone($state.snapshot(v) as DjVoice))}
								><Pencil size={14} /> Edit</button
							>
						</div>
					</div>
				</article>
			{/each}
			<button class="card vnew" onclick={newVoice}
				><span class="icon-tile accent"><Plus size={20} /></span><strong>Create a voice</strong><span
										class="faint small">Mix base voices to create a new DJ</span
				></button
			>
		</div>
	{:else if tab === 'words'}
		<div class="card wordscard">
			<div class="card-head">
				<BookA size={18} />
				<h2 class="grow">Pronunciation dictionary</h2>
								<SaveState state={wordsSave} />
			</div>
			<div class="card-body">
				<p class="muted small" style="margin-bottom:14px">
					Teach the DJs how to say names and words. Write it how it sounds (<em>Chand-ler</em>) or use IPA
					between slashes (<em>/noʊˈɛl/</em>). Whole words only.
				</p>
				<div class="words">
					{#each words as w, i (i)}
						<div class="wrow">
							<input class="input" placeholder="Word" bind:value={w.word} aria-label="Word" />
							<span class="faint">→</span>
							<input class="input" placeholder="Say it like" bind:value={w.say} aria-label="Pronunciation" />
							<button
								class="btn ghost icon sm"
								onclick={() =>
									audition('w:' + i, [
										{ voice: show!.djVoices[0]?.id ?? 'af_heart', text: `${w.word}.`, pauseMs: 0 }
									])}
								aria-label="Hear {w.word}"
								disabled={!w.word}
							>
								{#if auditioning === 'w:' + i}<LoaderCircle size={14} class="spin" />{:else}<Play
										size={14}
									/>{/if}
							</button>
							<button
								class="btn ghost icon sm"
								onclick={() => (words = words.filter((_, k) => k !== i))}
								aria-label="Remove"><Trash2 size={14} /></button
							>
						</div>
					{/each}
				</div>
				<button
					class="btn sm"
					style="margin-top:12px"
					onclick={() => (words = [...words, { word: '', say: '' }])}><Plus size={14} /> Add word</button
				>
			</div>
		</div>
	{:else if !show.djClips.length}
		<div class="card">
			<EmptyState
				icon={Mic}
				title="No DJ clips yet"
				message="Welcome messages, radio-station reminders, “up next” announcements — they make a show feel like a real event."
			>
				<button class="btn primary" onclick={newClip}><Plus size={16} /> Write your first clip</button>
			</EmptyState>
		</div>
	{:else}
		<div class="studio">
			<nav class="clips">
				{#each show.djClips as c (c.id)}
					<button class="clip" class:on={c.id === clipId} onclick={() => (clipId = c.id)}>
						<span class="grow"
							><span class="ellipsis cname">{c.name}</span><span class="faint tiny"
								>{c.lines.length} line{c.lines.length === 1 ? '' : 's'}{c.dynamic ? ' · live text' : ''}</span
							></span
						>
						{#if c.mediaId}<span class="rdot" title="Voice ready"></span>{/if}
					</button>
				{/each}
			</nav>

			{#if draft}
				<section class="editor card">
					<header class="ehead">
												<input
							class="title-input"
							bind:this={titleInput}
							bind:value={draft.name}
							oninput={queueSave}
							aria-label="Clip name"
						/>
												<span class="save"><SaveState state={saveState} /></span>
						<span class="grow"></span>
						<button class="btn ghost icon sm" onclick={duplicateClip} aria-label="Duplicate clip"
							><Copy size={15} /></button
						>
						<button class="btn ghost icon sm" onclick={removeClip} aria-label="Delete clip"
							><Trash2 size={15} /></button
						>
					</header>

										<div class="ph" class:open={tokensOpen}>
						<button
							class="ph-toggle btn sm"
							onmousedown={(e) => e.preventDefault()}
							onclick={() => (tokensOpen = !tokensOpen)}
							aria-expanded={tokensOpen}><Braces size={14} /> Insert live info</button
						>
						<span class="faint tiny ph-label"><Braces size={12} /> Insert live info:</span>
						<div class="ph-chips">
							{#each DJ_PLACEHOLDERS as p (p)}<button
									class="pchip"
									onmousedown={(e) => e.preventDefault()}
									onclick={() => insertPlaceholder(p)}
									title="Inserts {`{${p}}`}, filled in when the clip plays">{PLACEHOLDER_LABELS[p] ?? p}</button
								>{/each}
						</div>
					</div>

					<ol
						class="lines"
						use:sortable={{
							onsort: (f, t) => {
								if (draft) {
									draft.lines = moveItem(draft.lines, f, t);
									queueSave();
								}
							}
						}}
					>
						{#each draft.lines as line, i (i)}
							<li class="line" data-sort-index={i}>
								<button class="drag-handle" aria-label="Move line {i + 1}"><GripVertical size={16} /></button>
								<div class="who" style:--h={voiceHue(line.voice)}>
									<span class="av">{voiceName(line.voice)[0]}</span>
																		<select
										class="select sm"
										id="voice-{i}"
										value={line.voice}
										onchange={(e) => pickVoice(line, i, e)}
										aria-label="Voice for line {i + 1}"
									>
										{#each show.djVoices as v (v.id)}<option value={v.id}>{v.name}</option>{/each}
										{#if allVoices.has(i) || !isDj(line.voice) || !show.djVoices.length}
											<optgroup label="More voices"
												>{#each KOKORO_VOICES as v (v.id)}<option value={v.id}
														>{v.name} ({v.gender === 'male' ? 'male' : 'female'}, {v.accent})</option
													>{/each}</optgroup
											>
										{:else}
											<option value="__more">More voices…</option>
										{/if}
									</select>
								</div>
								<div class="grow ltext">
									<textarea
										class="textarea"
										rows="2"
										placeholder="What should they say?"
										bind:value={line.text}
										oninput={queueSave}
										onfocus={(e) => (focused = { i, el: e.currentTarget })}
																				aria-label="Line {i + 1} text"></textarea>
									{#if /\{\w+\}/.test(line.text)}
										<div class="said" aria-label="How line {i + 1} reads">
											{#each tokenize(line.text) as part, k (k)}{#if 'token' in part}<span class="tok"
														>{part.label}</span
													>{:else}{part.text}{/if}{/each}
										</div>
									{/if}
									<div class="lopts">
										<div class="energy" role="radiogroup" aria-label="Energy">
											{#each ENERGY_LEVELS as l (l.value)}
												<button
													type="button"
													role="radio"
													aria-checked={(line.energy ?? 0.4) === l.value}
													class="en en{String(l.value).replace('.', '')}"
													class:on={(line.energy ?? 0.4) === l.value}
													onclick={() => {
														line.energy = l.value;
														queueSave();
													}}>{l.label}</button
												>
											{/each}
										</div>
																				<select
											class="select sm pausesel"
											value={String(nearestPause(line.pauseMs))}
											onchange={(e) => {
												line.pauseMs = Number((e.target as HTMLSelectElement).value);
												queueSave();
											}}
											aria-label="Pause after line {i + 1}"
										>
																						{#each PAUSES as p (p.ms)}<option value={String(p.ms)}>{p.label}</option>{/each}
										</select>
										<span class="grow"></span>
										<button
											class="btn ghost icon sm"
											onclick={() => audition('l:' + i, [line], draft?.speed)}
											aria-label="Hear line {i + 1}"
											disabled={!line.text.trim()}
										>
											{#if auditioning === 'l:' + i && !stopPlay}<LoaderCircle
													size={14}
													class="spin"
												/>{:else if auditioning === 'l:' + i}<Square size={13} />{:else}<Play
													size={14}
												/>{/if}
										</button>
										<button
											class="btn ghost icon sm"
											onclick={() => removeLine(i)}
											aria-label="Remove line {i + 1}"
											disabled={draft.lines.length < 2}><Trash2 size={14} /></button
										>
									</div>
								</div>
							</li>
						{/each}
					</ol>
					<div class="addline">
						<button class="btn sm" onclick={addLine}><Plus size={14} /> Add line</button><span
							class="faint tiny">Tip: wrap a word in *asterisks* to make it the punchline.</span
						>
					</div>

					<div class="clipopts">
						<label class="field"
							><span class="label">Speed · {draft.speed.toFixed(2)}×</span>
							<input
								type="range"
								class="range"
								min="0.5"
								max="2"
								step="0.05"
								bind:value={draft.speed}
								oninput={queueSave}
								style:--pct="{((draft.speed - 0.5) / 1.5) * 100}%"
							/>
						</label>
						<label class="field"
							><span class="label"><Music2 size={12} /> Music bed</span>
							<select
								class="select"
								value={draft.musicBedMediaId ?? ''}
								onchange={(e) => {
									if (draft) {
										draft.musicBedMediaId = (e.target as HTMLSelectElement).value || undefined;
										queueSave();
									}
								}}
							>
								<option value="">None</option>
								{#each show.media.filter((m) => m.kind !== 'dj') as m (m.id)}<option value={m.id}
										>{m.name}</option
									>{/each}
							</select>
							<span class="hint">Plays quietly underneath the voices.</span>
						</label>
						<div class="field">
														<span class="label">Say live info fresh each time</span>
							<div class="row" style="min-height:40px">
								<Switch
									checked={draft.dynamic}
									label="Say live info fresh each time"
									onchange={(v) => {
										if (draft) {
											draft.dynamic = v;
											queueSave();
										}
									}}
								/><span class="small muted"
									>{draft.dynamic ? 'The voice is made again as it plays' : 'Made once, played the same'}</span
								>
							</div>
						</div>
					</div>
					{#if draft.dynamic && where === 'browser'}
						<div class="notice info small" style="margin:0 20px 16px">
							<Globe size={16} /><span
								>Live placeholders are filled in at showtime on a Pi 4/5 or Docker leader. On this controller
								they’ll use the values at render time.</span
							>
						</div>
					{/if}

					<footer class="rfoot">
						{#if rendering != null}
							<div class="grow col" style="gap:6px">
								<span class="small muted">{renderMsg}</span>
								<div class="progress"><span style:width="{rendering * 100}%"></span></div>
							</div>
						{:else}
							<div class="grow small">
																{#if media}<span class="ok"
										><Check size={14} /> Voice ready · {fmtDuration(media.durationMs)}</span
									>{:else}<span class="faint">No voice yet · about {estSec} s long</span>{/if}
							</div>
						{/if}
						{#if media}<button class="btn" onclick={previewRendered}
								>{#if stopPlay && !auditioning}<Square size={14} /> Stop{:else}<Play size={15} /> Preview{/if}</button
							>{/if}
						<button
							class="btn"
							onclick={() => {
								plTarget = show?.playlists[0]?.id ?? '';
								addToPl = true;
							}}><ListPlus size={15} /> Add to playlist</button
						>
						<button
							class="btn primary"
							onclick={renderClip}
							disabled={rendering != null || !draft.lines.some((l) => l.text.trim())}
														><Wand2 size={15} /> {media ? 'Update voice' : 'Make the voice'}</button
						>
					</footer>
				</section>
			{/if}
		</div>
	{/if}
</div>

<VoiceEditor bind:voice={editVoice} />

<Modal bind:open={addToPl} title="Add “{draft?.name}” to a playlist" size="sm">
	<label class="field"
		><span class="label">Playlist</span>
		<select class="select" bind:value={plTarget}
			>{#each show?.playlists ?? [] as p (p.id)}<option value={p.id}>{p.name}</option>{/each}</select
		>
		<span class="hint">It’s added at the end — drag it into place on the Playlists page.</span>
	</label>
	{#snippet footer()}
		<button class="btn ghost" onclick={() => (addToPl = false)}>Cancel</button>
		<button class="btn primary" onclick={addToPlaylist} disabled={!plTarget}>Add</button>
	{/snippet}
</Modal>

<style>
	.where {
		height: 32px;
		padding: 0 12px;
		align-self: center;
	}
	:global(.spin) {
		animation: spin 1s linear infinite;
	}
	@keyframes spin {
		to {
			transform: rotate(360deg);
		}
	}
	.voices {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(260px, 1fr));
		gap: 16px;
	}
	.vcard {
		overflow: hidden;
		display: flex;
		flex-direction: column;
	}
	.vart {
		height: 120px;
		position: relative;
		display: grid;
		place-items: center;
		background:
			radial-gradient(circle at 30% 20%, hsl(var(--h) 80% 60% / 0.55), transparent 60%),
			radial-gradient(circle at 80% 90%, hsl(calc(var(--h) + 60) 80% 55% / 0.45), transparent 60%),
			var(--surface-2);
	}
	.initial {
		width: 64px;
		height: 64px;
		border-radius: 50%;
		display: grid;
		place-items: center;
		font-size: 28px;
		font-weight: 700;
		color: #fff;
		background: hsl(var(--h) 55% 40%);
		box-shadow:
			0 8px 24px hsl(var(--h) 60% 30% / 0.5),
			inset 0 0 0 2px rgba(255, 255, 255, 0.2);
	}
	.wave {
		position: absolute;
		bottom: 10px;
		left: 50%;
		transform: translateX(-50%);
		display: flex;
		gap: 3px;
		align-items: flex-end;
		height: 18px;
		opacity: 0;
		transition: opacity 200ms;
	}
	.wave.on {
		opacity: 1;
	}
	.wave i {
		width: 3px;
		height: 6px;
		border-radius: 2px;
		background: rgba(255, 255, 255, 0.8);
		animation: bar 0.8s ease-in-out infinite;
	}
	@keyframes bar {
		50% {
			height: 18px;
		}
	}
	.vbody {
		padding: 16px;
		display: flex;
		flex-direction: column;
		gap: 10px;
		flex: 1;
	}
	.mix {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
	}
	.mixchip {
		font-size: 11.5px;
		padding: 3px 8px;
		border-radius: 99px;
		background: var(--surface-3);
		color: var(--text-2);
	}
	.mixchip b {
		color: var(--text);
		font-weight: 600;
	}
	.vnew {
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: 8px;
		min-height: 300px;
		border-style: dashed;
		background: transparent;
	}
	.vnew:hover {
		border-color: var(--accent);
	}
	.studio {
		display: grid;
		grid-template-columns: 240px minmax(0, 1fr);
		gap: 16px;
		align-items: start;
	}
	.clips {
		display: flex;
		flex-direction: column;
		gap: 4px;
		position: sticky;
		top: 16px;
	}
	.clip {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 10px 12px;
		border-radius: 12px;
		text-align: left;
		border: 1px solid transparent;
	}
	.clip:hover {
		background: var(--surface);
	}
	.clip.on {
		background: var(--surface);
		border-color: var(--border-2);
	}
	.clip .grow {
		display: flex;
		flex-direction: column;
		min-width: 0;
	}
	.cname {
		font-weight: 580;
		font-size: 13.5px;
	}
	.rdot {
		width: 8px;
		height: 8px;
		border-radius: 50%;
		background: var(--green);
	}
	.editor {
		overflow: hidden;
	}
	.ehead {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 14px 16px 6px 20px;
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
		width: 100%;
		max-width: 380px;
	}
	.title-input:hover {
		border-color: var(--border-2);
	}
	.title-input:focus {
		border-color: var(--accent-line);
		background: var(--surface-2);
	}
	.ph {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 6px;
		padding: 8px 20px 14px;
		border-bottom: 1px solid var(--border);
	}
	.ph .faint {
		display: inline-flex;
		align-items: center;
		gap: 4px;
		margin-right: 4px;
	}
	.pchip {
		font-size: 12px;
		font-weight: 560;
		padding: 5px 10px;
		border-radius: 99px;
		background: var(--blue-soft);
		color: var(--blue);
	}
	.ph-chips {
		display: contents;
	}
	.ph-toggle {
		display: none;
	}
	.said {
		margin-top: 6px;
		font-size: 12.5px;
		color: var(--text-2);
		line-height: 1.9;
	}
	.tok {
		display: inline-block;
		padding: 0 8px;
		margin: 0 1px;
		border-radius: 99px;
		background: var(--blue-soft);
		color: var(--blue);
		font-weight: 600;
		font-size: 11.5px;
		line-height: 20px;
	}
	.pausesel {
		width: auto;
		min-width: 132px;
	}
	/* Phones: the live-info chips fold into one "Insert live info" button. */
	@media (max-width: 760px) {
		.ph .ph-label {
			display: none;
		}
		.ehead {
			flex-wrap: wrap;
			padding: 12px 12px 6px 16px;
		}
		.ehead .title-input {
			flex: 1 1 100%;
			max-width: none;
			font-size: 18px;
		}
		.ph-toggle {
			display: inline-flex;
		}
		.ph-chips {
			display: none;
		}
		.ph.open .ph-chips {
			display: flex;
			flex-wrap: wrap;
			gap: 6px;
			width: 100%;
		}
	}
	@media (pointer: coarse) {
		.pchip {
			min-height: 44px;
			padding: 0 14px;
			font-size: 13px;
		}
		.en {
			height: 40px !important;
			padding: 0 12px !important;
			font-size: 12.5px !important;
		}
	}
	.pchip:hover {
		background: var(--blue);
		color: #fff;
	}
	.lines {
		list-style: none;
		margin: 0;
		padding: 12px 12px 0;
		display: flex;
		flex-direction: column;
		gap: 8px;
	}
	.line {
		display: flex;
		gap: 10px;
		align-items: flex-start;
		padding: 10px 10px 10px 2px;
		border-radius: 14px;
		background: var(--surface-2);
		border: 1px solid var(--border);
	}
	.who {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 6px;
		width: 110px;
		flex: 0 0 auto;
	}
	.av {
		width: 36px;
		height: 36px;
		border-radius: 50%;
		display: grid;
		place-items: center;
		font-weight: 700;
		color: #fff;
		background: hsl(var(--h) 55% 42%);
	}
	.ltext {
		display: flex;
		flex-direction: column;
		gap: 8px;
		min-width: 0;
	}
	.ltext .textarea {
		min-height: 64px;
		font-size: 14.5px;
	}
	.lopts {
		display: flex;
		align-items: center;
		gap: 10px;
		flex-wrap: wrap;
	}
	.energy {
		display: inline-flex;
		gap: 3px;
		padding: 3px;
		border-radius: 9px;
		background: var(--surface);
		border: 1px solid var(--border);
	}
	.en {
		height: 26px;
		padding: 0 9px;
		border-radius: 6px;
		font-size: 11.5px;
		font-weight: 560;
		color: var(--text-3);
	}
	.en.on {
		color: #fff;
	}
	.en0.on {
		background: #4b7bd6;
	}
	.en04.on {
		background: #5b6472;
	}
	.en1.on {
		background: #e0831a;
	}
	.en15.on {
		background: linear-gradient(90deg, #f2555a, #f5a524);
	}
	.addline {
		display: flex;
		align-items: center;
		gap: 12px;
		padding: 12px 20px 16px;
	}
	.clipopts {
		display: grid;
		grid-template-columns: 1fr 1fr 1fr;
		gap: 16px;
		padding: 16px 20px;
		border-top: 1px solid var(--border);
	}
	.rfoot {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 14px 20px;
		border-top: 1px solid var(--border);
		background: var(--surface-2);
		flex-wrap: wrap;
	}
	.ok {
		color: var(--green);
		display: inline-flex;
		align-items: center;
		gap: 6px;
		font-weight: 560;
	}
	.wordscard {
		max-width: 760px;
	}
	.words {
		display: flex;
		flex-direction: column;
		gap: 8px;
	}
	.wrow {
		display: grid;
		grid-template-columns: 1fr auto 1fr auto auto;
		gap: 8px;
		align-items: center;
	}
	@media (max-width: 1100px) {
		.clipopts {
			grid-template-columns: 1fr;
		}
	}
	@media (max-width: 760px) {
		.studio {
			grid-template-columns: 1fr;
		}
		.clips {
			position: static;
			flex-direction: row;
			overflow-x: auto;
			margin: 0 -16px;
			padding: 0 16px;
		}
		.clip {
			flex: 0 0 auto;
			max-width: 220px;
		}
		.line {
			flex-wrap: wrap;
		}
		.who {
			flex-direction: row;
			width: auto;
		}
		.ltext {
			flex-basis: 100%;
		}
		.wrow {
			grid-template-columns: 1fr 1fr auto auto;
		}
		.wrow > span {
			display: none;
		}
	}
</style>
