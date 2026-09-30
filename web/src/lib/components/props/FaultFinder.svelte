<script lang="ts">
	import type { FaultStep, Prop } from '$lib/api/types';
	import { api } from '$lib/api/client';
	import Modal from '$lib/components/ui/Modal.svelte';
	import PixelCount from './PixelCount.svelte';
	import { Search, CircleCheck, ThumbsUp, ThumbsDown, Wrench, NotebookPen, Ruler } from '@lucide/svelte';

	let {
		open = $bindable(false),
		prop,
		onnote
	}: { open?: boolean; prop: Prop | null; onnote?: (text: string) => void } = $props();

	/** Same estimate everywhere: the intro, the progress line and the result. */
	const estimate = $derived(Math.max(1, Math.ceil(Math.log2((prop?.pixelCount ?? 1) + 1))));

	let step = $state<FaultStep | null>(null);
	let countOpen = $state(false);
	let busy = $state(false);
	let error = $state('');

	async function start() {
		if (!prop) return;
		busy = true;
		error = '';
		try {
			step = await api.faultStart(prop.id);
		} catch (e) {
			error = (e as Error).message;
		} finally {
			busy = false;
		}
	}
	async function answer(lit: boolean) {
		if (!step) return;
		busy = true;
		try {
			step = await api.faultAnswer(step.session, lit);
		} catch (e) {
			error = (e as Error).message;
		} finally {
			busy = false;
		}
	}
	function close() {
		if (step && !step.done) api.faultStop().catch(() => {});
		step = null;
		open = false;
	}
	$effect(() => {
		if (!open) step = null;
	});
</script>

<Modal bind:open title="Find a faulty pixel" subtitle={prop?.name} size="md" onclose={close}>
	{#if !step}
		<div class="intro">
			<div class="halo"><Search size={26} /></div>
			<p>
				When part of a prop flickers, shows the wrong colors or stays dark, one pixel (or the joint just
				before it) is usually to blame.
			</p>
			<p class="muted">
				PixelPlus lights the prop a section at a time and asks you whether it looks right. It takes about {estimate}
				questions. Stand where you can see <strong>{prop?.name}</strong>.
			</p>
			{#if error}<p class="err small">{error}</p>{/if}
			{#if prop?.segments.length}
				<button
					class="btn ghost sm"
					onclick={() => {
						close();
						countOpen = true;
					}}><Ruler size={14} /> Only the end stays dark? Check the pixel count instead</button
				>
			{/if}
		</div>
	{:else if step.done}
		<div class="intro">
			<div class="halo {step.result?.pixelIndex == null ? 'ok' : ''}">
				{#if step.result?.pixelIndex == null}<CircleCheck size={26} />{:else}<Wrench size={26} />{/if}
			</div>
			{#if step.result?.pixelIndex != null}
				<div class="found">Pixel <span class="num">{step.result.pixelIndex + 1}</span></div>
			{/if}
			<p>{step.result?.message}</p>
		</div>
	{:else}
		<div class="q">
			<div class="row between small faint">
				<span>Question {step.step} of about {Math.max(estimate, step.step)}</span><span class="num"
					>{Math.round((step.step / Math.max(estimate, step.step)) * 100)}%</span
				>
			</div>
			<div class="progress"><span style:width="{(step.step / step.totalSteps) * 100}%"></span></div>
			<div class="strip" aria-hidden="true">
				{#each Array(Math.min(60, prop?.pixelCount ?? 0)) as _, i (i)}
					{@const idx = Math.floor((i / Math.min(60, prop?.pixelCount ?? 1)) * (prop?.pixelCount ?? 1))}
					<span class:lit={idx >= step.litFrom && idx < step.litTo}></span>
				{/each}
			</div>
			<p class="question">{step.question}</p>
		</div>
	{/if}
	{#snippet footer()}
		{#if !step}
			<button class="btn ghost" onclick={close}>Cancel</button>
			<button class="btn primary" onclick={start} disabled={busy}>Start — light the prop</button>
		{:else if step.done}
			{#if step.result?.pixelIndex != null && onnote}
				<button
					class="btn"
					onclick={() => {
						onnote?.(`Replace pixel ${(step?.result?.pixelIndex ?? 0) + 1}`);
						close();
					}}><NotebookPen size={16} /> Add “Replace pixel {step.result.pixelIndex + 1}” to notes</button
				>
			{/if}
			<button class="btn primary" onclick={close}>Done</button>
		{:else}
			<!-- Big, stacked answers: people tap these with gloves on while looking at the prop. -->
			<div class="answers">
				<button class="btn primary lg" disabled={busy} onclick={() => answer(true)}
					><ThumbsUp size={18} /> Yes, all good</button
				>
				<button class="btn lg" disabled={busy} onclick={() => answer(false)}
					><ThumbsDown size={18} /> No, something’s wrong</button
				>
				<button class="btn ghost stop" onclick={close}>Stop looking</button>
			</div>
		{/if}
	{/snippet}
</Modal>

<PixelCount bind:open={countOpen} {prop} />

<style>
	.intro {
		display: flex;
		flex-direction: column;
		align-items: center;
		text-align: center;
		gap: 12px;
		padding: 8px 8px 0;
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
	.found {
		font-size: 32px;
		font-weight: 700;
		letter-spacing: -0.03em;
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
		margin: 6px 0;
	}
	.strip span {
		flex: 1;
		height: 10px;
		border-radius: 3px;
		background: #1a1b20;
		transition: background 200ms;
	}
	.strip span.lit {
		background: #fff;
		box-shadow: 0 0 6px rgba(255, 255, 255, 0.7);
	}
	.question {
		font-size: 15px;
		font-weight: 540;
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
	.answers .stop {
		grid-column: 1;
		grid-row: 1;
	}
	.answers .primary {
		grid-column: 3;
		grid-row: 1;
	}
	@media (max-width: 640px) {
		.answers {
			grid-template-columns: 1fr;
		}
		.answers .primary {
			grid-column: auto;
			grid-row: auto;
		}
		.answers .btn {
			height: 56px;
			font-size: 16px;
		}
		.answers .stop {
			grid-row: auto;
			height: 44px;
			font-size: 14px;
		}
	}
</style>
