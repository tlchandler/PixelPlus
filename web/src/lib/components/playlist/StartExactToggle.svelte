<!-- "Start exactly on time" for a schedule entry (F4, WS3): the playlist's intro
     (e.g. a countdown) starts early so the first song begins at the show time. -->
<script lang="ts">
	import type { Playlist, Show } from '$lib/api/types';
	import Switch from '$lib/components/ui/Switch.svelte';
	import { introLeadMs } from '$lib/playlist/countdown';

	let {
		checked = $bindable(false),
		playlist,
		show
	}: { checked?: boolean; playlist: Playlist | undefined; show: Show | null } = $props();

	const lead = $derived(playlist && show ? introLeadMs(show, playlist) : 0);
	const fmt = (ms: number) => {
		const s = Math.round(ms / 1000);
		return s >= 60 ? `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')} min` : `${s} s`;
	};
</script>

<div class="row between exact">
	<div>
		<div class="label" style="margin:0">Start exactly on time</div>
		<div class="faint tiny">
			{#if lead > 0}
				The intro ({fmt(lead)}) starts early so the first song begins right at the start time.
			{:else}
				Add an intro (for example a countdown) to the playlist to use this.
			{/if}
		</div>
	</div>
	<Switch label="Start exactly on time" bind:checked disabled={lead === 0 && !checked} />
</div>

<style>
	.exact {
		gap: 12px;
	}
</style>
