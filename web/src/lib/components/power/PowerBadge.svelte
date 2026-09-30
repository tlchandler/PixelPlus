<!-- Dashboard badge while the power limiter dims (or, in warn mode, would dim)
     part of the display (F12, WS3): "Power limiting · Garage PSU · 80 %". -->
<script lang="ts">
	import { onDestroy, onMount } from 'svelte';
	import { app } from '$lib/stores/app.svelte';
	import { limitingSummary, type LiveNode } from '$lib/power/power';
	import { Zap } from '@lucide/svelte';

	let live = $state<LiveNode[]>([]);
	let off: (() => void) | undefined;
	onMount(() => (off = app.onMessage('power', (d) => (live = d.nodes as LiveNode[]))));
	onDestroy(() => off?.());

	const mode = $derived(app.show?.settings.power?.mode ?? 'warn');
	const summary = $derived(limitingSummary(app.show, live));
	const limiting = $derived(!!app.status?.power?.limiting || !!summary);
	const scale = $derived(summary?.scale ?? app.status?.power?.minScale ?? 1);
</script>

{#if limiting && mode !== 'off'}
	<a
		class="pbadge"
		class:warn={mode === 'warn'}
		href="/settings/power"
		title={mode === 'warn'
			? 'A fuse or power supply would be over its budget; the limiter is in warn-only mode.'
			: 'The power limiter is dimming part of the display to protect a fuse or power supply.'}
	>
		<Zap size={12} />
		{mode === 'warn' ? 'Over power budget' : 'Power limiting'}{summary ? ` · ${summary.group}` : ''} · {Math.round(
			scale * 100
		)} %
	</a>
{/if}

<style>
	.pbadge {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		margin-left: 6px;
		padding: 4px 9px;
		border-radius: 99px;
		background: rgba(0, 0, 0, 0.55);
		backdrop-filter: blur(8px);
		color: #ffc861;
		text-decoration: none;
		font-size: 11px;
		font-weight: 700;
		letter-spacing: 0.04em;
	}
	.pbadge.warn {
		color: #ffd98a;
	}
</style>
