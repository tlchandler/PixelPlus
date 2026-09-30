<!--
	Settings → Triggers (moved here from the main Settings page by WS6, F20). Buttons on
	the controller, web links and ESP32 sensor inputs, and what they do — including
	surprises layered over the running song. Saves automatically.
-->
<script lang="ts">
	import { onDestroy } from 'svelte';
	import { ArrowLeft, Zap, Info } from '@lucide/svelte';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import SaveState from '$lib/components/ui/SaveState.svelte';
	import TriggerEditor from '$lib/components/triggers/TriggerEditor.svelte';
	import type { Trigger } from '$lib/api/types';
	import { api } from '$lib/api/client';
	import { app } from '$lib/stores/app.svelte';
	import { toasts } from '$lib/stores/toasts.svelte';

	let triggers = $state<Trigger[] | null>(null);
	let lastSaved = '';
	let saveState = $state<'saved' | 'saving'>('saved');
	let timer: ReturnType<typeof setTimeout> | undefined;

	$effect(() => {
		// Load once (and again if the show changes while nothing is pending).
		const t = app.show?.settings.triggers;
		if (t && saveState === 'saved') {
			const json = JSON.stringify(t);
			if (json !== lastSaved) {
				lastSaved = json;
				triggers = structuredClone(t);
			}
		}
	});

	$effect(() => {
		// Deep watch: any edit (including the time pickers' bindings) saves.
		const json = triggers ? JSON.stringify(triggers) : '';
		if (!triggers || json === lastSaved) return;
		saveState = 'saving';
		clearTimeout(timer);
		timer = setTimeout(() => save(json), 700);
	});

	async function save(json: string) {
		try {
			await api.saveSettings({ triggers: JSON.parse(json) });
			lastSaved = json;
			await app.reloadShow();
		} catch (e) {
			toasts.error("Couldn't save the triggers", (e as Error).message);
		} finally {
			saveState = 'saved';
		}
	}
	onDestroy(() => clearTimeout(timer));
</script>

<svelte:head><title>Triggers · Settings · PixelPlus</title></svelte:head>

<div class="page trg">
	<a class="btn ghost sm back" href="/settings"><ArrowLeft size={16} /> Settings</a>
	<PageHeader
		title="Triggers"
		subtitle="Start things with a button, a link, or a sensor in the yard — like a sparkle on the candy canes when someone walks by."
	>
		{#snippet actions()}<SaveState state={saveState} />{/snippet}
	</PageHeader>

	<section class="card">
		<div class="card-head">
			<Zap size={18} />
			<h2 class="grow">Triggers</h2>
		</div>
		<div class="card-body">
			{#if triggers && app.show}
				<TriggerEditor bind:triggers show={app.show} onchange={() => (triggers = triggers)} />
			{:else}
				<div class="skeleton" style="height:120px"></div>
			{/if}
		</div>
	</section>

	<div class="notice info">
		<Info size={18} />
		<div class="small">
			A <b>surprise</b> plays a look or a short sequence on the chosen props on top of whatever is playing
			(the song keeps going), then fades back. Use <b>Limits</b> so a busy sidewalk doesn't set it off
			constantly. Sensors are added in <a href="/settings/sensors">Settings → Sensors</a>.
		</div>
	</div>
</div>

<style>
	.trg {
		max-width: 900px;
		display: flex;
		flex-direction: column;
		gap: 16px;
	}
	.back {
		align-self: flex-start;
		margin-bottom: -8px;
	}
</style>
