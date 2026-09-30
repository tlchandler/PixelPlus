<script lang="ts">
	import { api } from '$lib/api/client';
	import { toasts } from '$lib/stores/toasts.svelte';
	import Modal from './Modal.svelte';
	import Slider from './Slider.svelte';
	import { Play, Square, Minus, Plus } from '@lucide/svelte';

	/**
	 * "Sync lights to sound": plays a click every second while every prop on every controller
	 * flashes white; the user moves the slider until flash and click coincide where the
	 * audience is. The value is `settings.audio.outputDelayMs`, saved through `onsave` as it
	 * changes (the lights move live).
	 */
	let {
		open = $bindable(false),
		delayMs,
		onsave
	}: {
		open?: boolean;
		delayMs: number;
		onsave: (ms: number) => Promise<void> | void;
	} = $props();

	const MIN = -500;
	const MAX = 1000;
	const PRESETS = [
		{ label: 'Speakers by the lights', ms: 0 },
		{ label: 'FM transmitter', ms: 20 },
		{ label: 'TV / HDMI sound bar', ms: 80 },
		{ label: 'Bluetooth speaker', ms: 200 }
	];

	let value = $state(0);
	let playing = $state(false);
	let busy = $state(false);
	let timer: ReturnType<typeof setTimeout>;

	$effect(() => {
		if (open) value = delayMs;
	});

	function set(v: number) {
		value = Math.max(MIN, Math.min(MAX, Math.round(v)));
		clearTimeout(timer);
		timer = setTimeout(() => onsave(value), 250);
	}

	async function start() {
		busy = true;
		try {
			await api.calibration(true);
			playing = true;
		} catch (e) {
			toasts.error('Could not start the test', (e as Error).message);
		} finally {
			busy = false;
		}
	}
	async function stop() {
		playing = false;
		await api.calibration(false).catch(() => {});
	}
	async function finish() {
		clearTimeout(timer);
		await onsave(value);
		if (playing) await stop();
		open = false;
	}
	function onclose() {
		clearTimeout(timer);
		if (playing) stop();
	}
	const fmt = (v: number) => (v === 0 ? '0 ms' : `${v > 0 ? '+' : '−'}${Math.abs(v)} ms`);
	const metres = $derived(Math.round(Math.max(0, value) / 2.9));
</script>

<Modal
	bind:open
	title="Sync lights to sound"
	subtitle="Match the lights to what the audience hears"
	{onclose}
>
	<ol class="steps">
		<li>
			Stand where your audience listens — on the sidewalk, or in a car tuned to your FM station — with the
			lights in view.
		</li>
		<li>Start the test: every prop flashes white and a click plays, once a second, on every controller.</li>
		<li>
			Move the slider until the flash and the click happen together. <strong>Flash first?</strong> Move right
			(the lights wait longer). <strong>Click first?</strong> Move left.
		</li>
	</ol>

	<div class="row wrap play">
		{#if playing}
			<button class="btn" onclick={stop}><Square size={16} /> Stop the test</button>
			<span class="badge green"><span class="dot"></span> Flashing and clicking</span>
		{:else}
			<button class="btn primary" onclick={start} disabled={busy} data-autofocus
				><Play size={16} /> Start the test</button
			>
		{/if}
	</div>

	<div class="delay">
		<div class="row">
			<span class="title grow">Sound delay</span>
			<span class="num big" aria-live="polite">{fmt(value)}</span>
		</div>
		<Slider
			label="Sound delay"
			min={MIN}
			max={MAX}
			step={5}
			bind:value
			oninput={(v) => set(v)}
			format={fmt}
		/>
		<div class="row nudge">
			<button class="btn sm" onclick={() => set(value - 10)} aria-label="10 ms earlier"
				><Minus size={14} /> 10</button
			>
			<button class="btn sm" onclick={() => set(value - 1)} aria-label="1 ms earlier"
				><Minus size={14} /> 1</button
			>
			<span class="grow"></span>
			<button class="btn sm" onclick={() => set(value + 1)} aria-label="1 ms later"
				><Plus size={14} /> 1</button
			>
			<button class="btn sm" onclick={() => set(value + 10)} aria-label="10 ms later"
				><Plus size={14} /> 10</button
			>
		</div>
		<div class="presets row wrap">
			<span class="faint small">Start from:</span>
			{#each PRESETS as p (p.label)}
				<button class="btn sm ghost" onclick={() => set(p.ms)}>{p.label}</button>
			{/each}
		</div>
		<p class="faint small">
			Sound travels about 3 ms per metre: each 10 m between the speakers and the audience adds about 30 ms.{#if metres >= 5}
				{fmt(value)} is like standing {metres} m from the speakers.{/if} Lights that come a little before the sound
			look more natural than lights that come after it.
		</p>
	</div>

	{#snippet footer()}
		<button class="btn primary" onclick={finish}>Done</button>
	{/snippet}
</Modal>

<style>
	.steps {
		margin: 0 0 14px;
		padding-left: 20px;
		display: grid;
		gap: 6px;
		color: var(--text-2);
	}
	.play {
		gap: 10px;
		margin-bottom: 16px;
	}
	.delay {
		display: grid;
		gap: 10px;
	}
	.title {
		font-weight: 600;
	}
	.big {
		font-size: 20px;
		font-weight: 650;
		font-variant-numeric: tabular-nums;
	}
	.nudge {
		gap: 6px;
	}
	.presets {
		gap: 6px;
		align-items: center;
	}
</style>
