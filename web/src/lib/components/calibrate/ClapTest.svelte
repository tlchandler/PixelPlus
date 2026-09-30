<!--
	"Fine-tune for this phone": 10 claps in front of the camera measure this phone's own
	sound-vs-picture timing (see lib/sensing/clap.ts). The result is kept on this phone.
-->
<script lang="ts">
	import { onDestroy } from 'svelte';
	import { Hand, Check, RotateCcw, TriangleAlert } from '@lucide/svelte';
	import type { SensingSession } from '$lib/sensing/session';
	import { clapContacts, detectClaps, estimateBias } from '$lib/sensing/clap';
	import { clearBias, saveBias, type StoredBias } from '$lib/sensing/device';
	import ProgressRing from './ProgressRing.svelte';

	let {
		session,
		model,
		current,
		ondone
	}: {
		session: SensingSession;
		model: string;
		current: StoredBias | null;
		ondone: (b: StoredBias | null) => void;
	} = $props();

	const RECORD_MS = 14_000;
	let phase = $state<'ready' | 'recording' | 'done' | 'failed'>('ready');
	let progress = $state(0);
	let result = $state<StoredBias | null>(null);
	let heard = $state(0);
	let seen = $state(0);
	let timer: ReturnType<typeof setInterval> | undefined;

	function start() {
		phase = 'recording';
		progress = 0;
		session.startRaw();
		const t0 = performance.now();
		timer = setInterval(() => {
			const t = performance.now() - t0;
			progress = t / RECORD_MS;
			if (t >= RECORD_MS) finish(t0, performance.now());
		}, 100);
	}

	function finish(t0: number, t1: number) {
		clearInterval(timer);
		const raw = session.stopRaw();
		if (!raw) {
			phase = 'failed';
			return;
		}
		const claps = detectClaps(raw.samples, raw.sampleRate).map(
			(s) => raw.startMs + (s / raw.sampleRate) * 1000
		);
		const contacts = clapContacts(session.motionBetween(t0 - 500, t1));
		heard = claps.length;
		seen = contacts.length;
		const est = estimateBias(claps, contacts, 1);
		if (!est) {
			phase = 'failed';
			return;
		}
		result = { ...est, at: new Date().toISOString() };
		saveBias(model, result);
		phase = 'done';
	}

	onDestroy(() => {
		clearInterval(timer);
		if (phase === 'recording') session.stopRaw();
	});

	const fmt = (ms: number) => `${Math.abs(Math.round(ms))} ms ${ms >= 0 ? 'later' : 'earlier'}`;
</script>

<div class="clap">
	{#if phase === 'ready'}
		<div class="icon"><Hand size={26} strokeWidth={1.6} /></div>
		<h2>Fine-tune for this phone</h2>
		<p class="muted">
			Every phone hears a little earlier or later than it sees. Measure it once and results get about twice as
			precise.
		</p>
		<ol class="steps">
			<li>Stand somewhere quiet and well lit.</li>
			<li>Hold the phone at arm's length (about 1 m), camera pointing at your hands.</li>
			<li>Tap <strong>Start</strong> and clap about 10 times, a second apart.</li>
		</ol>
		{#if current}
			<p class="faint small">
				Already measured for {model}: it hears {fmt(current.biasMs)} than it sees.
				<button
					class="linkish"
					onclick={() => {
						clearBias(model);
						ondone(null);
					}}>Forget it</button
				>
			</p>
		{/if}
		<div class="row wrap actions">
			<button class="btn primary lg" onclick={start}><Hand size={18} /> Start</button>
			<button class="btn ghost" onclick={() => ondone(current)}>Not now</button>
		</div>
	{:else if phase === 'recording'}
		<ProgressRing value={progress} label="Clap test">
			<div>
				<div class="big">Clap!</div>
				<div class="faint small">{Math.max(0, Math.ceil(((1 - progress) * RECORD_MS) / 1000))} s</div>
			</div>
		</ProgressRing>
		<p class="muted center">Clap about once a second, in front of the camera.</p>
	{:else if phase === 'done' && result}
		<div class="icon ok"><Check size={26} strokeWidth={2} /></div>
		<h2>Saved for {model}</h2>
		<p class="muted">
			This phone hears {fmt(result.biasMs)} than it sees (from {result.pairs} claps, ±{Math.max(
				1,
				Math.round(result.spreadMs)
			)}
			ms). Measurements will allow for it.
		</p>
		<div class="row wrap actions">
			<button class="btn primary lg" onclick={() => ondone(result)}>Continue</button>
		</div>
	{:else}
		<div class="icon warn"><TriangleAlert size={26} strokeWidth={1.6} /></div>
		<h2>Couldn't match the claps</h2>
		<p class="muted">
			{#if heard < 5}I heard {heard} clap{heard === 1 ? '' : 's'}. Clap louder, closer to the phone.
			{:else if seen < 5}I saw {seen} clap{seen === 1 ? '' : 's'}. Keep your hands in view, in good light, in
				front of a plain background.
			{:else}The claps didn't line up consistently. Hold the phone still and clap crisply.{/if}
		</p>
		<div class="row wrap actions">
			<button class="btn primary" onclick={start}><RotateCcw size={16} /> Try again</button>
			<button class="btn ghost" onclick={() => ondone(current)}>Skip</button>
		</div>
	{/if}
</div>

<style>
	.clap {
		display: flex;
		flex-direction: column;
		align-items: center;
		text-align: center;
		gap: 12px;
		padding: var(--s-5) var(--s-4);
	}
	.icon {
		width: 56px;
		height: 56px;
		border-radius: 50%;
		display: grid;
		place-items: center;
		background: var(--accent-soft);
		color: var(--accent-text);
	}
	.icon.ok {
		background: var(--green-soft);
		color: var(--green);
	}
	h2 {
		font-size: 19px;
	}
	.steps {
		text-align: left;
		margin: 0;
		padding-left: 20px;
		display: grid;
		gap: 6px;
		color: var(--text-2);
		max-width: 420px;
	}
	.actions {
		gap: 8px;
		justify-content: center;
		margin-top: 4px;
	}
	.big {
		font-size: 24px;
		font-weight: 700;
	}
	.center {
		text-align: center;
	}
	.linkish {
		background: none;
		border: 0;
		padding: 0;
		color: var(--accent-text);
		font: inherit;
		cursor: pointer;
		text-decoration: underline;
	}
</style>
