<!--
	Pixel-count check (F7, WS4; ARCHITECTURE §12.6): how many pixels really answer on a prop's
	output, by camera (lights past the configured end too) or by a few yes/no taps (binary search).
	Result: configured vs measured, then "Update the count" or "Keep it".
-->
<script lang="ts">
	import { untrack } from 'svelte';
	import {
		Camera,
		Hand,
		Ruler,
		CircleCheck,
		TriangleAlert,
		ThumbsUp,
		ThumbsDown,
		Undo2
	} from '@lucide/svelte';
	import type { Prop, PropSegment } from '$lib/api/types';
	import Modal from '$lib/components/ui/Modal.svelte';
	import SecureGate from '$lib/components/ui/SecureGate.svelte';
	import CameraScan from '$lib/cv/CameraScan.svelte';
	import { pixelCountApi } from '$lib/cv/api';
	import { schedule } from '$lib/cv/mapcode';
	import { buildPlan, outputName } from '$lib/cv/plan';
	import type { DecodeResult } from '$lib/cv/decode';
	import type { CountStep, MapStart } from '$lib/cv/types';
	import { app } from '$lib/stores/app.svelte';
	import { toasts } from '$lib/stores/toasts.svelte';
	import { api } from '$lib/api/client';

	let { open = $bindable(false), prop }: { open?: boolean; prop: Prop | null } = $props();

	type Stage = 'choose' | 'camera' | 'manual' | 'result';
	let stage = $state<Stage>('choose');
	let segIndex = $state(0);
	let step = $state<CountStep | null>(null);
	let busy = $state(false);
	let error = $state('');
	let result = $state<{
		id: string;
		count: number;
		configured: number;
		dead: number[];
		method: 'camera' | 'manual';
	} | null>(null);

	const show = $derived(app.show);
	const segs = $derived(prop?.segments ?? []);
	const seg = $derived<PropSegment | undefined>(segs[segIndex]);
	const where = $derived(show && seg ? outputName(show, seg.nodeId, seg.output) : '');

	$effect(() => {
		if (!open) untrack(reset);
	});

	function reset() {
		if (step && step.count == null) pixelCountApi.stop(step.session).catch(() => {});
		stage = 'choose';
		step = null;
		result = null;
		error = '';
		segIndex = 0;
	}

	const estimateMs = (bitMs: number) => {
		if (!show || !seg) return 0;
		const { plan } = buildPlan(show, [[seg.nodeId, seg.output]], { bitMs, probeExtra: true }, 2);
		return schedule(plan).totalMs;
	};

	const begin = (bitMs: number): Promise<MapStart> =>
		pixelCountApi.startCamera(seg!.nodeId, seg!.output, bitMs);

	async function cameraDone(r: { result: DecodeResult; start: MapStart }) {
		if (!r.result.ok) {
			error = r.result.error ?? 'The scan did not work.';
			stage = 'choose';
			return;
		}
		const idx = r.result.lights.map((l) => l.idx).sort((a, b) => a - b);
		if (!idx.length) {
			error = 'No pixels of this output were seen. Is the prop in the picture and powered?';
			stage = 'choose';
			return;
		}
		const count = idx[idx.length - 1] + 1;
		const seen = new Set(idx);
		// Gaps inside the lit run are suspects (dead pixel, or just hidden from the camera).
		const dead: number[] = [];
		for (let i = 0; i < count && dead.length < 64; i++) if (!seen.has(i)) dead.push(i);
		const configured = r.start.configured ?? r.start.targets[0]?.configured ?? 0;
		await pixelCountApi.result(r.start.runId, count, dead.length < count * 0.3 ? dead : []).catch(() => {});
		result = {
			id: r.start.runId,
			count,
			configured,
			dead: dead.length < count * 0.3 ? dead : [],
			method: 'camera'
		};
		stage = 'result';
	}

	async function startManual() {
		if (!seg) return;
		busy = true;
		error = '';
		try {
			step = await pixelCountApi.startManual(seg.nodeId, seg.output);
			stage = 'manual';
		} catch (e) {
			error = (e as Error).message;
		} finally {
			busy = false;
		}
	}

	async function answer(seen: boolean | null) {
		if (!step) return;
		busy = true;
		try {
			step =
				seen === null
					? await pixelCountApi.undo(step.session)
					: await pixelCountApi.answer(step.session, seen);
			if (step.count != null) {
				result = {
					id: step.session,
					count: step.count,
					configured: step.configured,
					dead: [],
					method: 'manual'
				};
				stage = 'result';
			}
		} catch (e) {
			error = (e as Error).message;
		} finally {
			busy = false;
		}
	}

	async function apply(updatePropCount: boolean) {
		if (!result) return;
		busy = true;
		try {
			const res = await pixelCountApi.apply(result.id, {
				updatePropCount,
				count: result.count,
				dead: result.dead
			});
			await app.reloadShow();
			toasts.push({
				kind: 'success',
				message: res.message ?? 'Saved',
				action: {
					label: 'Undo',
					run: async () => {
						await api.restoreSnapshot(res.snapshotId);
						await app.reloadShow();
					}
				}
			});
			step = null;
			open = false;
		} catch (e) {
			error = (e as Error).message;
		} finally {
			busy = false;
		}
	}

	const diff = $derived(result ? result.count - result.configured : 0);
</script>

<Modal bind:open title="Check pixel count" subtitle={prop?.name} size="md">
	{#if stage === 'choose'}
		<div class="stack">
			<p class="muted">
				Finds how many pixels actually answer on {prop?.name}'s string — handy when the end stays dark or the
				xLights count might be off. Pixels past the configured end are tried too.
			</p>
			{#if segs.length > 1}
				<label class="field">
					<span class="eyebrow">Which string</span>
					<select class="select" bind:value={segIndex}>
						{#each segs as s, i (i)}<option value={i}
								>{show ? outputName(show, s.nodeId, s.output) : s.output} · {s.pixelCount} px</option
							>{/each}
					</select>
				</label>
			{:else if seg}
				<p class="faint small">On {where}.</p>
			{/if}
			{#if !seg}
				<div class="notice warn small">
					<TriangleAlert size={16} /> Wire this prop to a controller port first.
				</div>
			{/if}
			{#if error}<p class="err small" role="alert">{error}</p>{/if}
			<div class="methods">
				<button class="method" disabled={!seg} onclick={() => (stage = 'camera')}>
					<span class="icon-tile accent"><Camera size={20} /></span>
					<span
						><strong>With the camera</strong><span class="faint small"
							>About 10 s. Point the phone at the prop.</span
						></span
					>
					<span class="badge accent">Best</span>
				</button>
				<button class="method" disabled={!seg || busy} onclick={startManual}>
					<span class="icon-tile"><Hand size={20} /></span>
					<span
						><strong>By looking</strong><span class="faint small"
							>About 10 yes/no taps while you watch the end of the string.</span
						></span
					>
				</button>
			</div>
		</div>
	{:else if stage === 'camera'}
		{#if app.mock}
			<CameraScan
				{begin}
				{estimateMs}
				ondone={cameraDone}
				oncancel={() => (stage = 'choose')}
				startLabel="Count pixels"
			/>
		{:else}
			<SecureGate need="camera" purpose="count the pixels">
				<CameraScan
					{begin}
					{estimateMs}
					ondone={cameraDone}
					oncancel={() => (stage = 'choose')}
					startLabel="Count pixels"
				/>
			</SecureGate>
		{/if}
	{:else if stage === 'manual' && step?.step}
		<div class="q">
			<div class="row between small faint">
				<span>Question {step.step.number} of about {step.step.number + step.step.maxRemaining - 1}</span>
				<span class="num">{step.maxProbe} px tried</span>
			</div>
			<div class="strip" aria-hidden="true">
				{#each Array(40) as _, i (i)}
					{@const at = Math.floor(((i + 0.5) / 40) * step.maxProbe)}
					{@const endAt = Math.floor(((step.step.litUntil + 0.5) / step.maxProbe) * 40)}
					<span class:lit={at < step.step.litUntil} class:end={i === Math.min(39, endAt)}></span>
				{/each}
			</div>
			<p class="question">{step.step.ask}</p>
			{#if error}<p class="err small" role="alert">{error}</p>{/if}
		</div>
	{:else if stage === 'result' && result}
		<div class="res">
			<span class="halo" class:ok={diff === 0}
				>{#if diff === 0}<CircleCheck size={26} />{:else}<Ruler size={26} />{/if}</span
			>
			<div class="big"><span class="num">{result.count}</span> pixels answer</div>
			<div class="compare">
				<div><span class="faint small">Configured</span><strong class="num">{result.configured}</strong></div>
				<div><span class="faint small">Measured</span><strong class="num">{result.count}</strong></div>
			</div>
			<p class="muted">
				{#if diff === 0}
					The count matches the configuration.
				{:else if diff < 0}
					The last {-diff} pixel{diff === -1 ? '' : 's'} don't respond. Either the string is shorter than configured,
					or it's broken after pixel {result.count} (the fault finder can tell).
				{:else}
					{diff} more pixel{diff === 1 ? '' : 's'} answer than configured — the end of the string is dark in shows.
				{/if}
				{#if result.dead.length}<br /><span class="small"
						>Not seen inside the string: {result.dead
							.slice(0, 8)
							.map((d) => d + 1)
							.join(', ')}{result.dead.length > 8 ? '…' : ''}.</span
					>{/if}
			</p>
			{#if error}<p class="err small" role="alert">{error}</p>{/if}
		</div>
	{/if}

	{#snippet footer()}
		{#if stage === 'manual' && step?.step}
			<div class="answers">
				<button class="btn ghost" disabled={busy || !step.canUndo} onclick={() => answer(null)}
					><Undo2 size={16} /> Back</button
				>
				<button class="btn lg" disabled={busy} onclick={() => answer(false)}
					><ThumbsDown size={18} /> No red pixel</button
				>
				<button class="btn primary lg" disabled={busy} onclick={() => answer(true)}
					><ThumbsUp size={18} /> I see it</button
				>
			</div>
		{:else if stage === 'result' && result}
			{#if diff !== 0}
				<button class="btn" disabled={busy} onclick={() => apply(false)}
					>Keep {result.configured}, note the check</button
				>
				<button class="btn primary" disabled={busy} onclick={() => apply(true)}
					>Update to {result.count}</button
				>
			{:else}
				<button class="btn primary" disabled={busy} onclick={() => apply(false)}>Done</button>
			{/if}
		{:else}
			<button class="btn ghost" onclick={() => (open = false)}>Close</button>
		{/if}
	{/snippet}
</Modal>

<style>
	.stack {
		display: flex;
		flex-direction: column;
		gap: 12px;
	}
	.field {
		display: flex;
		flex-direction: column;
		gap: 6px;
	}
	.methods {
		display: flex;
		flex-direction: column;
		gap: 8px;
	}
	.method {
		display: flex;
		align-items: center;
		gap: 12px;
		padding: 14px;
		border-radius: 14px;
		border: 1px solid var(--border-2);
		background: var(--surface-2);
		color: var(--text);
		text-align: left;
		cursor: pointer;
		min-height: 64px;
	}
	.method:disabled {
		opacity: 0.45;
	}
	.method > span:nth-child(2) {
		display: flex;
		flex-direction: column;
		flex: 1;
	}
	.q {
		display: flex;
		flex-direction: column;
		gap: 12px;
	}
	.strip {
		display: flex;
		gap: 2px;
		padding: 14px;
		border-radius: 12px;
		background: #060608;
	}
	.strip span {
		flex: 1;
		height: 10px;
		border-radius: 3px;
		background: #1a1b20;
	}
	.strip span.lit {
		background: #1f7a3a;
	}
	.strip span.end {
		background: #e0282e;
		box-shadow: 0 0 6px rgba(224, 40, 46, 0.8);
	}
	.question {
		font-size: 15px;
		font-weight: 540;
	}
	.res {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 12px;
		text-align: center;
	}
	.halo {
		width: 60px;
		height: 60px;
		border-radius: 18px;
		display: grid;
		place-items: center;
		background: var(--accent-soft);
		color: var(--accent-text);
	}
	.halo.ok {
		background: var(--green-soft);
		color: var(--green);
	}
	.big {
		font-size: 20px;
		font-weight: 600;
	}
	.big .num {
		font-size: 32px;
	}
	.compare {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: 8px;
		width: 100%;
		max-width: 320px;
	}
	.compare div {
		display: flex;
		flex-direction: column;
		padding: 10px;
		border-radius: 12px;
		background: var(--surface-2);
	}
	.compare strong {
		font-size: 20px;
	}
	.err {
		color: var(--red);
	}
	.answers {
		display: grid;
		grid-template-columns: auto 1fr 1fr;
		gap: 8px;
		width: 100%;
	}
	@media (max-width: 640px) {
		.answers {
			grid-template-columns: 1fr 1fr;
		}
		.answers .ghost {
			grid-column: 1 / -1;
			order: 3;
		}
		.answers .lg {
			height: 56px;
		}
	}
</style>
