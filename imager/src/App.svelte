<script lang="ts">
	import { onMount } from 'svelte';
	import { api, isMock } from './lib/api';
	import type { Defaults, Drive, ImageChoice, ImagerSettings, Progress } from './lib/types';
	import { emptySettings } from './lib/types';
	import { normalizeHostname, validate } from './lib/validate';
	import StepImage from './components/StepImage.svelte';
	import StepDrive from './components/StepDrive.svelte';
	import StepSettings from './components/StepSettings.svelte';
	import StepWrite from './components/StepWrite.svelte';

	const steps = ['Image', 'SD card', 'Settings', 'Write'] as const;
	let step = $state(0);
	let defaults = $state<Defaults>({ timezone: '', country: '', timezones: [], platform: 'unknown' });
	let image = $state<ImageChoice | null>(null);
	let drive = $state<Drive | null>(null);
	let settings = $state<ImagerSettings>(emptySettings());
	let writing = $state(false);
	let finished = $state(false);
	let progress = $state<Progress | null>(null);

	onMount(async () => {
		defaults = await api.defaults();
		settings = emptySettings(defaults);
	});

	const errors = $derived(validate(settings));
	const canNext = $derived(
		step === 0 ? !!image : step === 1 ? !!drive && !drive.tooSmall : step === 2 ? errors.length === 0 : false
	);

	function go(i: number) {
		if (writing) return;
		// only allow jumping back, or forward through completed steps
		if (i <= step || (i === step + 1 && canNext)) step = i;
	}

	function restart() {
		finished = false;
		progress = null;
		drive = null;
		step = 1;
	}
</script>

<div class="shell">
	<header>
		<div class="brand">
			<svg width="28" height="28" viewBox="0 0 40 40" aria-hidden="true">
				<rect width="40" height="40" rx="9" fill="var(--surface-2)" />
				<g fill="#F5A524">
					<circle cx="10" cy="10" r="3.4" /><circle cx="20" cy="10" r="3.4" opacity=".55" /><circle cx="30" cy="10" r="3.4" />
					<circle cx="10" cy="20" r="3.4" opacity=".55" /><circle cx="20" cy="20" r="4" /><circle cx="30" cy="20" r="3.4" opacity=".55" />
					<circle cx="10" cy="30" r="3.4" /><circle cx="20" cy="30" r="3.4" opacity=".55" /><circle cx="30" cy="30" r="3.4" />
				</g>
			</svg>
			<div>
				<div class="title">PixelPlus Imager</div>
				<div class="hint">Make a ready-to-go SD card for your pixel controller</div>
			</div>
		</div>
		{#if isMock}<span class="badge muted" title="Running in a browser: nothing is written">Preview mode</span>{/if}
	</header>

	<nav aria-label="Steps">
		{#each steps as label, i (label)}
			<button
				class="step"
				class:active={i === step}
				class:done={i < step || finished}
				disabled={writing || (i > step && !(i === step + 1 && canNext))}
				onclick={() => go(i)}
				aria-current={i === step ? 'step' : undefined}
			>
				<span class="num">{i < step || finished ? '✓' : i + 1}</span>
				{label}
			</button>
			{#if i < steps.length - 1}<span class="sep" aria-hidden="true"></span>{/if}
		{/each}
	</nav>

	<main>
		{#if step === 0}
			<StepImage bind:image />
		{:else if step === 1}
			<StepDrive bind:drive {image} />
		{:else if step === 2}
			<StepSettings bind:settings {defaults} {errors} />
		{:else}
			<StepWrite
				{image}
				{drive}
				settings={{ ...settings, hostname: normalizeHostname(settings.hostname) }}
				bind:writing
				bind:finished
				bind:progress
				onrestart={restart}
			/>
		{/if}
	</main>

	{#if step < 3}
		<footer>
			<button class="btn" onclick={() => (step = Math.max(0, step - 1))} disabled={step === 0}>Back</button>
			<button class="btn primary" onclick={() => (step += 1)} disabled={!canNext}>
				{step === 2 ? 'Review' : 'Next'}
			</button>
		</footer>
	{/if}
</div>

<style>
	.shell {
		display: grid;
		grid-template-rows: auto auto 1fr auto;
		height: 100vh;
		max-width: 880px;
		margin: 0 auto;
		padding: 20px 24px;
		gap: 20px;
	}
	header {
		display: flex;
		align-items: center;
		justify-content: space-between;
	}
	.brand {
		display: flex;
		gap: 12px;
		align-items: center;
	}
	.title {
		font-size: 17px;
		font-weight: 650;
		letter-spacing: -0.01em;
	}
	nav {
		display: flex;
		align-items: center;
		gap: 8px;
	}
	.step {
		display: inline-flex;
		align-items: center;
		gap: 8px;
		padding: 6px 12px 6px 6px;
		border-radius: 999px;
		border: 1px solid transparent;
		background: none;
		color: var(--text-3);
		cursor: pointer;
		font-weight: 500;
	}
	.step:disabled {
		cursor: default;
	}
	.step.active {
		color: var(--text);
		border-color: var(--border-2);
		background: var(--surface);
	}
	.step.done {
		color: var(--text-2);
	}
	.num {
		display: grid;
		place-items: center;
		width: 24px;
		height: 24px;
		border-radius: 50%;
		background: var(--surface-3);
		font-size: 12px;
		font-weight: 650;
	}
	.active .num {
		background: var(--accent);
		color: var(--accent-fg);
	}
	.done .num {
		background: var(--green-soft);
		color: var(--green);
	}
	.sep {
		flex: 1;
		height: 1px;
		background: var(--border-2);
		min-width: 12px;
	}
	main {
		overflow: auto;
		padding: 2px;
	}
	footer {
		display: flex;
		justify-content: space-between;
		padding-top: 8px;
		border-top: 1px solid var(--border);
	}
</style>
