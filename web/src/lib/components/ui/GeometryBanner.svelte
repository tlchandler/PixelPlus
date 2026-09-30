<script lang="ts">
	// Shown when the longest string is longer than the pixel output was set up for at boot
	// (DPI geometry, /boot/firmware/pixelplus.conf): offers "Apply & reboot".
	import { onMount } from 'svelte';
	import { api } from '$lib/api/client';
	import type { OutputGeometry } from '$lib/api/types';
	import { app } from '$lib/stores/app.svelte';
	import { confirm, toasts } from '$lib/stores/toasts.svelte';
	import { RotateCcw, Ruler } from '@lucide/svelte';

	let geo = $state<OutputGeometry | null>(null);
	let busy = $state(false);

	async function load() {
		if (app.system?.role === 'follower' && app.system?.needsSetup) return;
		geo = await api.outputGeometry().catch(() => geo);
	}

	onMount(() => {
		load();
		const t = setInterval(load, 60_000);
		return () => clearInterval(t);
	});

	// Refresh when the boot-settings helper finishes, or when strings change.
	const job = $derived(app.helpers['config-txt']);
	let lastJob = '';
	$effect(() => {
		const key = job ? `${job.state}:${job.updatedAt}` : '';
		if (key && key !== lastJob) {
			lastJob = key;
			if (job?.state !== 'running') load();
		}
	});
	let lastVersion = 0;
	$effect(() => {
		const v = app.show?.version ?? 0;
		if (v !== lastVersion) {
			lastVersion = v;
			// the player re-checks the geometry right after a show change
			setTimeout(load, 1500);
		}
	});

	const running = $derived(busy || job?.state === 'running');

	async function apply() {
		if (!geo) return;
		const target = geo.targetPixels ?? geo.longestString;
		if (
			!(await confirm({
				title: 'Apply the new string length and restart?',
				message: `The pixel output will be set up for ${target.toLocaleString()} pixels per output. The controller restarts (about a minute) and the show stops meanwhile.`,
				confirmLabel: 'Apply & restart'
			}))
		)
			return;
		busy = true;
		try {
			await api.applyOutputGeometry(true);
			toasts.info('Saving the new boot settings…');
		} catch (e) {
			toasts.error('Could not apply the new string length', (e as Error).message);
		} finally {
			busy = false;
		}
	}

	async function restart() {
		if (
			!(await confirm({
				title: 'Restart the controller?',
				message: 'The new string length is saved. The show stops for about a minute.',
				confirmLabel: 'Restart'
			}))
		)
			return;
		await api.reboot().catch((e) => toasts.error('Restart failed', (e as Error).message));
	}
</script>

{#if geo && !geo.ok}
	<section class="geo card" role="status">
		<span class="icon-tile accent"><Ruler size={20} /></span>
		<div class="grow">
			<div class="title">
				{geo.pendingReboot
					? 'Restart to light your longest string'
					: 'A string is longer than the pixel output allows'}
			</div>
			<div class="muted small">
				{#if running && job}{job.message}{:else}{geo.message}{/if}
			</div>
		</div>
		{#if geo.canApply}
			<button class="btn primary" onclick={apply} disabled={running}
				><RotateCcw size={15} />{running ? 'Applying…' : 'Apply & reboot'}</button
			>
		{:else if geo.pendingReboot && app.system?.platform?.power !== false}
			<button class="btn primary" onclick={restart}><RotateCcw size={15} /> Restart now</button>
		{/if}
	</section>
{/if}

<style>
	.geo {
		display: flex;
		align-items: center;
		gap: 14px;
		padding: 14px 16px;
		border-color: var(--accent-line);
		background: linear-gradient(0deg, var(--accent-soft), var(--accent-soft)), var(--surface);
		margin-bottom: 16px;
	}
	.title {
		font-weight: 600;
	}
	.grow {
		flex: 1;
		min-width: 0;
	}
	@media (max-width: 640px) {
		.geo {
			flex-wrap: wrap;
		}
		.geo .btn {
			width: 100%;
			justify-content: center;
		}
	}
</style>
