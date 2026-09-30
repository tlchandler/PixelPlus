<script lang="ts">
	import type { DjVoice } from '$lib/api/types';
	import { api } from '$lib/api/client';
	import { app } from '$lib/stores/app.svelte';
	import { toasts, confirm } from '$lib/stores/toasts.svelte';
	import { KOKORO_VOICES, ENERGY_KEYS, ENERGY_LEVELS } from '$lib/util/voices';
	import { renderSpeech, playBlob } from '$lib/tts';
	import Modal from '$lib/components/ui/Modal.svelte';
	import { Play, Plus, X, Trash2, LoaderCircle, Square } from '@lucide/svelte';

	let { voice = $bindable(null) }: { voice: DjVoice | null } = $props();

	let draft = $state<DjVoice | null>(null);
	let addId = $state('');
	let busy = $state(false);
	let playing = $state<(() => void) | null>(null);
	let sample = $state('Hey hey, welcome to the show! Tune your radio to eighty-eight point three.');

	$effect(() => {
		draft = voice ? structuredClone($state.snapshot(voice) as DjVoice) : null;
	});

	const total = $derived(draft ? Object.values(draft.blend).reduce((a, b) => a + b, 0) || 1 : 1);
	const isNew = $derived(!!draft && !app.show?.djVoices.some((v) => v.id === draft!.id));

	function addBase() {
		if (!draft || !addId || draft.blend[addId] != null) return;
		draft.blend = { ...draft.blend, [addId]: 0.3 };
		addId = '';
	}
	function removeBase(id: string) {
		if (!draft) return;
		const b = { ...draft.blend };
		delete b[id];
		draft.blend = b;
	}

	async function audition() {
		if (!draft || !app.show) return;
		if (playing) {
			playing();
			playing = null;
			return;
		}
		busy = true;
		try {
			const show = { ...app.show, djVoices: [...app.show.djVoices.filter((v) => v.id !== draft!.id), $state.snapshot(draft) as DjVoice] };
			const blob = await renderSpeech(show, [{ voice: draft.id, text: sample, pauseMs: 0, energy: draft.defaultEnergy }], draft.speed);
			playing = playBlob(blob, () => (playing = null));
		} catch (e) {
			toasts.error('Couldn’t play the sample', (e as Error).message);
		} finally {
			busy = false;
		}
	}

	async function save() {
		if (!draft) return;
		const d = $state.snapshot(draft) as DjVoice;
		// normalize weights
		const sum = Object.values(d.blend).reduce((a, b) => a + b, 0) || 1;
		d.blend = Object.fromEntries(Object.entries(d.blend).map(([k, v]) => [k, +(v / sum).toFixed(3)]));
		if (isNew) await app.mutate(() => api.djVoices.create(d), { success: `Created voice ${d.name}` });
		else await app.mutate(() => api.djVoices.update(d.id, d), { success: `Saved ${d.name}` });
		voice = null;
	}
	async function remove() {
		if (!draft) return;
		const d = structuredClone($state.snapshot(draft) as DjVoice);
		if (!(await confirm({ title: `Delete voice ${d.name}?`, message: 'Clips using it fall back to a default voice.', confirmLabel: 'Delete', danger: true }))) return;
		await app.mutate(() => api.djVoices.remove(d.id));
		voice = null;
		toasts.success(`Deleted ${d.name}`, { label: 'Undo', run: () => app.mutate(() => api.djVoices.create(d)) });
	}
</script>

<Modal open={!!draft} title={isNew ? 'New voice' : `Edit ${draft?.name}`} subtitle="Mix Kokoro base voices into a character, then tune how it sounds when hyped." size="lg" onclose={() => (voice = null)}>
	{#if draft}
		<div class="form-grid">
			<label class="field"><span class="label">Name</span><input class="input" bind:value={draft.name} /></label>
			<label class="field"><span class="label">Accent</span>
				<select class="select" bind:value={draft.lang}><option value="en-us">American English</option><option value="en-gb">British English</option></select>
			</label>
			<label class="field span-2"><span class="label">Description</span><input class="input" bind:value={draft.description} placeholder="e.g. Warm late-night radio host" /></label>
		</div>

		<h3 class="eyebrow sect">Voice blend</h3>
		<div class="blend">
			{#each Object.entries(draft.blend) as [id, w] (id)}
				{@const base = KOKORO_VOICES.find((v) => v.id === id)}
				<div class="brow">
					<span class="bname"><strong>{base?.name ?? id}</strong><span class="faint tiny">{base ? `${base.gender === 'male' ? 'Male' : 'Female'} · ${base.accent}` : id}</span></span>
					<input type="range" class="range" min="0" max="1" step="0.05" value={w} style:--pct="{w * 100}%" oninput={(e) => draft && (draft.blend = { ...draft.blend, [id]: Number((e.target as HTMLInputElement).value) })} aria-label="{base?.name} weight" />
					<span class="pct num">{Math.round((w / total) * 100)}%</span>
					<button class="btn ghost icon sm" onclick={() => removeBase(id)} aria-label="Remove {base?.name}" disabled={Object.keys(draft.blend).length < 2}><X size={14} /></button>
				</div>
			{/each}
			<div class="row">
				<select class="select sm" bind:value={addId} aria-label="Add a base voice" style="max-width:260px">
					<option value="">Add a base voice…</option>
					{#each KOKORO_VOICES.filter((v) => draft && draft.blend[v.id] == null) as v (v.id)}<option value={v.id}>{v.name} — {v.gender} · {v.accent}</option>{/each}
				</select>
				<button class="btn sm" onclick={addBase} disabled={!addId}><Plus size={14} /> Add</button>
			</div>
			<div class="bar" aria-hidden="true">
				{#each Object.entries(draft.blend) as [id, w], i (id)}<span style:flex={w} style:background="hsl({(i * 67 + 30) % 360} 70% 55%)"></span>{/each}
			</div>
		</div>

		<div class="form-grid" style="margin-top:18px">
			<label class="field"><span class="label">Speed · {draft.speed.toFixed(2)}×</span>
				<input type="range" class="range" min="0.7" max="1.4" step="0.01" bind:value={draft.speed} style:--pct="{((draft.speed - 0.7) / 0.7) * 100}%" />
			</label>
			<div class="field"><span class="label">Default energy</span>
				<div class="row wrap">
					{#each ENERGY_LEVELS as l (l.value)}<button type="button" class="chip" aria-pressed={draft.defaultEnergy === l.value} onclick={() => draft && (draft.defaultEnergy = l.value)}>{l.label}</button>{/each}
				</div>
			</div>
		</div>

		<details class="tune">
			<summary>Energy tuning <span class="faint small">· how the voice changes from calm to extra hype</span></summary>
			<div class="energy">
				{#each ENERGY_KEYS as k (k.key)}
					{@const v = draft.energy[k.key] ?? k.def}
					<label class="field">
						<span class="label">{k.label} · <span class="num">{v}</span></span>
						<input type="range" class="range" min={k.min} max={k.max} step={k.step} value={v} style:--pct="{((v - k.min) / (k.max - k.min)) * 100}%" oninput={(e) => draft && (draft.energy = { ...draft.energy, [k.key]: Number((e.target as HTMLInputElement).value) })} />
						<span class="hint">{k.hint}</span>
					</label>
				{/each}
			</div>
		</details>

		<div class="audition">
			<input class="input" bind:value={sample} aria-label="Sample sentence" />
			<button class="btn soft" onclick={audition} disabled={busy}>
				{#if busy}<LoaderCircle size={15} class="spin" /> Rendering…{:else if playing}<Square size={14} /> Stop{:else}<Play size={15} /> Audition{/if}
			</button>
		</div>
	{/if}
	{#snippet footer()}
		{#if !isNew}<button class="btn danger" onclick={remove}><Trash2 size={14} /></button><span class="grow"></span>{/if}
		<button class="btn ghost" onclick={() => (voice = null)}>Cancel</button>
		<button class="btn primary" onclick={save} disabled={!draft?.name.trim()}>{isNew ? 'Create voice' : 'Save voice'}</button>
	{/snippet}
</Modal>

<style>
	.sect {
		margin: 22px 0 10px;
	}
	.blend {
		display: flex;
		flex-direction: column;
		gap: 8px;
	}
	.brow {
		display: grid;
		grid-template-columns: 140px 1fr 44px 32px;
		align-items: center;
		gap: 12px;
	}
	.bname {
		display: flex;
		flex-direction: column;
		line-height: 1.3;
	}
	.pct {
		text-align: right;
		font-size: 12.5px;
		color: var(--text-2);
	}
	.bar {
		display: flex;
		height: 8px;
		border-radius: 99px;
		overflow: hidden;
		gap: 2px;
		margin-top: 4px;
	}
	.tune {
		margin-top: 18px;
		border: 1px solid var(--border);
		border-radius: 12px;
		padding: 12px 14px;
	}
	.tune summary {
		cursor: pointer;
		font-weight: 560;
		font-size: 13px;
	}
	.energy {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: 14px 20px;
		margin-top: 14px;
	}
	.audition {
		display: flex;
		gap: 8px;
		margin-top: 18px;
		padding: 12px;
		border-radius: 12px;
		background: var(--surface-2);
	}
	.audition :global(.spin) {
		animation: spin 1s linear infinite;
	}
	@keyframes spin {
		to {
			transform: rotate(360deg);
		}
	}
	@media (max-width: 640px) {
		.brow {
			grid-template-columns: 100px 1fr 40px 32px;
		}
		.energy {
			grid-template-columns: 1fr;
		}
		.audition {
			flex-direction: column;
		}
	}
</style>
