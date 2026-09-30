<!--
	A self-contained sequence preview (F3): the display drawn on this device in time
	with the song, plus the transport. Nothing is sent to the lights.

	  <SequencePreview sequenceId={s.id} mediaId={s.mediaId} autoplay />
-->
<script lang="ts">
	import { app } from '$lib/stores/app.svelte';
	import { PreviewPlayer } from '$lib/preview/player.svelte';
	import LayoutCanvas from '$lib/components/viz/LayoutCanvas.svelte';
	import PreviewTransport from '$lib/components/viz/PreviewTransport.svelte';
	import { onDestroy, untrack } from 'svelte';

	let {
		sequenceId,
		mediaId,
		autoplay = false,
		height = '320px'
	}: { sequenceId: string; mediaId?: string; autoplay?: boolean; height?: string } = $props();

	const player = new PreviewPlayer();
	const showProps = $derived(app.show?.props ?? []);

	$effect(() => {
		const id = sequenceId;
		const m = mediaId;
		// The player's own state must not re-run this effect.
		untrack(() => {
			void player.load(id, m).then(() => {
				if (autoplay && player.status === 'ready' && player.seqId === id) void player.play();
			});
		});
	});

	onDestroy(() => player.unload());
</script>

<div class="sp">
	<div class="stage" style:height>
		<LayoutCanvas props={showProps} source={player.source} height="100%" />
		{#if !showProps.length}
			<div class="none faint small">Add props (or import your xLights layout) to see the preview.</div>
		{/if}
	</div>
	<PreviewTransport {player} {mediaId} />
</div>

<style>
	.sp {
		display: flex;
		flex-direction: column;
		gap: 12px;
	}
	.stage {
		position: relative;
		border-radius: 14px;
		overflow: hidden;
		background: #060608;
	}
	.none {
		position: absolute;
		inset: 0;
		display: grid;
		place-items: center;
		padding: 20px;
		text-align: center;
		color: #a9a9b3;
	}
</style>
