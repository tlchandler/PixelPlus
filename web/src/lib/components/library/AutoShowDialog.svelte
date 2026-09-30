<!--
	"Make a light show" (F2): pick a style and props, preview it on this device, create it.
	The result is a normal sequence tagged "auto".
-->
<script lang="ts">
	import type { AutoshowStyle, Media } from '$lib/api/types';
	import { app } from '$lib/stores/app.svelte';
	import { toasts } from '$lib/stores/toasts.svelte';
	import { library } from '$lib/library/api';
	import { waitJob } from '$lib/library/jobs.svelte';
	import { energyWord } from '$lib/library/tags';
	import Modal from '$lib/components/ui/Modal.svelte';
	import Switch from '$lib/components/ui/Switch.svelte';
	import SequencePreview from '$lib/components/viz/SequencePreview.svelte';
	import { Sparkles, Eye, Check, Dices } from '@lucide/svelte';
	import { untrack } from 'svelte';

	let {
		media = $bindable(null),
		oncreated
	}: { media: Media | null; oncreated?: (sequenceId: string) => void } = $props();

	let styles = $state<AutoshowStyle[]>([]);
	let style = $state('classic');
	let allProps = $state(true);
	let picked = $state<string[]>([]);
	let seed = $state<number | undefined>(undefined);
	let stage = $state<'pick' | 'preparing' | 'preview' | 'creating'>('pick');
	let pct = $state(0);
	let tmpId = $state<string | null>(null);
	let error = $state<string | null>(null);
	const showProps = $derived(app.show?.props ?? []);
	const open = $derived(!!media);

	$effect(() => {
		if (!open) return;
		const kind = media?.kind;
		untrack(() => reset(kind));
	});

	function reset(kind: Media['kind'] | undefined) {
		stage = 'pick';
		tmpId = null;
		error = null;
		seed = undefined;
		style = kind === 'dj' ? 'voice' : 'classic';
		if (!styles.length)
			library
				.styles()
				.then((s) => (styles = s))
				.catch(() => {});
	}

	function req() {
		return {
			mediaId: media!.id,
			style,
			propIds: allProps ? [] : picked,
			seed
		};
	}

	async function preview() {
		if (!media) return;
		error = null;
		stage = 'preparing';
		pct = 0;
		try {
			const r = await library.previewShow(req());
			seed = r.seed;
			const j = await waitJob(r.jobId, (p) => (pct = p));
			tmpId = j.result?.sequenceId ?? r.sequenceId ?? null;
			stage = 'preview';
		} catch (e) {
			error = (e as Error).message;
			stage = 'pick';
		}
	}

	async function create() {
		if (!media) return;
		error = null;
		stage = 'creating';
		pct = 0;
		try {
			const r = await library.createShow(req());
			const j = await waitJob(r.jobId, (p) => (pct = p));
			await app.reloadShow();
			const id = j.result?.sequenceId;
			toasts.success('Your light show is ready', {
				label: 'Preview',
				run: () => id && oncreated?.(id)
			});
			if (id) oncreated?.(id);
			media = null;
		} catch (e) {
			error = (e as Error).message;
			stage = tmpId ? 'preview' : 'pick';
		}
	}

	function reroll() {
		seed = Math.floor(Math.random() * 1_000_000);
		void preview();
	}

	function toggle(id: string) {
		picked = picked.includes(id) ? picked.filter((x) => x !== id) : [...picked, id];
	}
</script>

<Modal
	{open}
	size="lg"
	title="Make a light show"
	subtitle={media ? `For “${media.name}”` : undefined}
	onclose={() => (media = null)}
>
	{#if media}
		{#if media.analysis && media.analysis.bpm > 0}
			<p class="small faint intro">
				PixelPlus found <strong class="num">{Math.round(media.analysis.bpm)} BPM</strong> and {media.analysis
					.sections}
				parts ({energyWord(media.analysis.energy).toLowerCase()}). Lights will hit on the beats and swell with
				the big parts.
			</p>
		{:else}
			<p class="small faint intro">
				PixelPlus listens to the song first (a few seconds), then choreographs your props.
			</p>
		{/if}

		{#if stage === 'preview' && tmpId}
			<SequencePreview sequenceId={tmpId} mediaId={media.id} autoplay height="280px" />
		{:else}
			<fieldset class="styles" disabled={stage !== 'pick'}>
				<legend class="label">Style</legend>
				{#each styles.filter((x) => x.id !== 'voice' || media?.kind === 'dj') as s (s.id)}
					<label class="style" class:on={style === s.id}>
						<input type="radio" name="style" value={s.id} bind:group={style} />
						<strong>{s.name}</strong>
						<span class="faint small">{s.description}</span>
					</label>
				{:else}
					<div class="faint small">Loading styles…</div>
				{/each}
			</fieldset>

			<div class="props">
				<label class="row lbl">
					<Switch bind:checked={allProps} label="Use every prop" size="sm" />
					<span>Use every prop</span>
				</label>
				{#if !allProps}
					<div class="plist">
						{#each showProps as p (p.id)}
							<label class="pp"
								><input type="checkbox" checked={picked.includes(p.id)} onchange={() => toggle(p.id)} />
								<span class="ellipsis">{p.name}</span></label
							>
						{:else}
							<div class="faint small">No props yet.</div>
						{/each}
					</div>
				{/if}
			</div>
		{/if}

		{#if stage === 'preparing' || stage === 'creating'}
			<div class="prog" aria-live="polite">
				<span class="small"
					>{stage === 'creating' ? 'Making your light show…' : 'Choreographing a preview…'}
					<span class="num">{pct}%</span></span
				>
				<div class="progress"><span style:width="{pct}%"></span></div>
			</div>
		{/if}
		{#if error}<div class="err small" role="alert">{error}</div>{/if}
	{/if}

	{#snippet footer()}
		{#if stage === 'preview'}
			<button class="btn ghost" onclick={() => (stage = 'pick')}>Change style</button>
			<button class="btn ghost" onclick={reroll} title="Same style, different moves"
				><Dices size={15} /> Try another</button
			>
			<button class="btn primary" onclick={create}><Check size={15} /> Create</button>
		{:else}
			<button class="btn ghost" onclick={() => (media = null)}>Cancel</button>
			<button
				class="btn"
				onclick={preview}
				disabled={stage !== 'pick' || (!allProps && !picked.length) || !showProps.length}
				><Eye size={15} /> Preview</button
			>
			<button
				class="btn primary"
				onclick={create}
				disabled={stage !== 'pick' || (!allProps && !picked.length) || !showProps.length}
				><Sparkles size={15} /> Create</button
			>
		{/if}
	{/snippet}
</Modal>

<style>
	.intro {
		margin: 0 0 14px;
	}
	.styles {
		border: 0;
		padding: 0;
		margin: 0 0 16px;
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
		gap: 8px;
	}
	.styles legend {
		margin-bottom: 8px;
	}
	.style {
		display: flex;
		flex-direction: column;
		gap: 2px;
		padding: 12px;
		border-radius: 12px;
		border: 1px solid var(--border);
		background: var(--surface-2);
		cursor: pointer;
		min-height: 44px;
	}
	.style input {
		position: absolute;
		opacity: 0;
		pointer-events: none;
	}
	.style.on {
		border-color: var(--accent);
		background: var(--accent-soft);
	}
	.style:focus-within {
		outline: 2px solid var(--accent);
		outline-offset: 2px;
	}
	.props {
		display: flex;
		flex-direction: column;
		gap: 10px;
	}
	.lbl {
		gap: 8px;
		font-size: 13.5px;
		cursor: pointer;
	}
	.plist {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(160px, 1fr));
		gap: 4px 12px;
		max-height: 180px;
		overflow: auto;
	}
	.pp {
		display: flex;
		align-items: center;
		gap: 8px;
		min-height: 32px;
		font-size: 13px;
		min-width: 0;
	}
	.prog {
		display: flex;
		flex-direction: column;
		gap: 6px;
		margin-top: 14px;
	}
	.prog .progress {
		height: 4px;
	}
	.err {
		margin-top: 12px;
		color: var(--red);
	}
</style>
