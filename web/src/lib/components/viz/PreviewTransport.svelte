<script lang="ts">
	// Transport for a local sequence preview (F3): play/pause, a waveform scrubber,
	// speed, and a clear "your lights are not affected" note.
	import type { PreviewPlayer } from '$lib/preview/player.svelte';
	import { api } from '$lib/api/client';
	import { fmtDuration } from '$lib/util/format';
	import Waveform from '$lib/components/viz/Waveform.svelte';
	import Segmented from '$lib/components/ui/Segmented.svelte';
	import { Play, Pause, EyeOff, Loader2, RotateCcw, VolumeX } from '@lucide/svelte';

	let {
		player,
		mediaId,
		onclose
	}: {
		player: PreviewPlayer;
		/** Song for the waveform. */
		mediaId?: string;
		onclose?: () => void;
	} = $props();

	let peaks = $state<number[]>([]);
	let bar: HTMLDivElement | undefined = $state();
	let scrubbing = $state(false);
	let rate = $state<'0.5' | '1'>('1');

	$effect(() => {
		const id = mediaId;
		peaks = [];
		if (id)
			api.media
				.peaks(id, 160)
				.then((p) => (peaks = p))
				.catch(() => {});
	});

	const progress = $derived(player.durationMs ? player.posMs / player.durationMs : 0);

	function posFrom(e: PointerEvent) {
		const r = bar!.getBoundingClientRect();
		return Math.max(0, Math.min(1, (e.clientX - r.left) / r.width)) * player.durationMs;
	}
	function down(e: PointerEvent) {
		if (player.status !== 'ready') return;
		bar!.setPointerCapture(e.pointerId);
		scrubbing = true;
		void player.seek(posFrom(e));
	}
	function move(e: PointerEvent) {
		if (scrubbing) void player.seek(posFrom(e));
	}
	function key(e: KeyboardEvent) {
		const step = e.shiftKey ? 10000 : 5000;
		if (e.key === 'ArrowRight') void player.seek(player.posMs + step);
		else if (e.key === 'ArrowLeft') void player.seek(player.posMs - step);
		else if (e.key === 'Home') void player.seek(0);
		else if (e.key === ' ' || e.key === 'k') player.toggle();
		else return;
		e.preventDefault();
	}
</script>

<div class="pt" data-testid="preview-transport">
	<div class="note" role="note">
		<EyeOff size={14} />
		<span><strong>Preview only</strong> — your lights are not affected.</span>
		{#if onclose}<button class="btn sm ghost" onclick={onclose}>Back to live</button>{/if}
	</div>
	{#if player.status === 'preparing'}
		<div class="prep" aria-live="polite">
			<Loader2 size={16} class="spin" />
			<span class="grow small">Preparing preview… <span class="num">{player.pct}%</span></span>
			<div class="progress" style="width:40%"><span style:width="{player.pct}%"></span></div>
		</div>
	{:else if player.status === 'error'}
		<div class="prep err small" role="alert">{player.error}</div>
	{:else}
		<div class="row">
			<button
				class="btn icon primary playbtn"
				onclick={() => player.toggle()}
				disabled={player.status !== 'ready'}
				aria-label={player.playing ? 'Pause preview' : 'Play preview'}
			>
				{#if player.playing}<Pause
						size={18}
					/>{:else if player.posMs >= player.durationMs - 50 && player.durationMs}<RotateCcw
						size={18}
					/>{:else}<Play size={18} />{/if}
			</button>
			<div
				class="scrub"
				bind:this={bar}
				role="slider"
				tabindex="0"
				aria-label="Preview position"
				aria-valuemin={0}
				aria-valuemax={Math.round(player.durationMs / 1000)}
				aria-valuenow={Math.round(player.posMs / 1000)}
				aria-valuetext={fmtDuration(player.posMs)}
				onpointerdown={down}
				onpointermove={move}
				onpointerup={() => (scrubbing = false)}
				onpointercancel={() => (scrubbing = false)}
				onkeydown={key}
			>
				{#if peaks.length}
					<Waveform {peaks} {progress} height={36} />
				{:else}
					<div class="track"><span style:width="{progress * 100}%"></span></div>
				{/if}
				<span class="head" style:left="{progress * 100}%"></span>
			</div>
			<span class="time num small">
				{fmtDuration(player.posMs)}<span class="faint"> / {fmtDuration(player.durationMs)}</span>
			</span>
		</div>
		<div class="row sub">
			<Segmented
				size="sm"
				label="Speed"
				bind:value={rate}
				onchange={(v) => player.setRate(Number(v))}
				options={[
					{ value: '0.5', label: '½×' },
					{ value: '1', label: '1×' }
				]}
			/>
			{#if player.buffering}<span class="faint tiny">Loading…</span>{/if}
			{#if !player.withAudio && player.status === 'ready'}<span class="faint tiny row" style="gap:4px"
					><VolumeX size={12} /> No sound in this preview</span
				>{/if}
		</div>
	{/if}
</div>

<style>
	.pt {
		display: flex;
		flex-direction: column;
		gap: 10px;
		padding: 12px 14px;
		border-radius: 14px;
		background: var(--surface);
		border: 1px solid var(--border);
	}
	.note {
		display: flex;
		align-items: center;
		gap: 8px;
		font-size: 12.5px;
		color: var(--blue-text, var(--text-2));
		background: var(--blue-soft, var(--surface-2));
		padding: 6px 10px;
		border-radius: 10px;
	}
	.note span {
		flex: 1;
	}
	.row {
		display: flex;
		align-items: center;
		gap: 12px;
	}
	.sub {
		gap: 10px;
	}
	.playbtn {
		width: 44px;
		height: 44px;
		border-radius: 50%;
		flex: 0 0 auto;
	}
	.scrub {
		position: relative;
		flex: 1;
		min-width: 0;
		padding: 6px 0;
		cursor: pointer;
		touch-action: none;
		border-radius: 8px;
	}
	.scrub:focus-visible {
		outline: 2px solid var(--accent);
		outline-offset: 2px;
	}
	.track {
		height: 6px;
		border-radius: 3px;
		background: var(--surface-3);
		overflow: hidden;
		margin: 15px 0;
	}
	.track span {
		display: block;
		height: 100%;
		background: var(--accent);
	}
	.head {
		position: absolute;
		top: 2px;
		bottom: 2px;
		width: 2px;
		margin-left: -1px;
		background: var(--text);
		border-radius: 1px;
		pointer-events: none;
	}
	.time {
		white-space: nowrap;
	}
	.prep {
		display: flex;
		align-items: center;
		gap: 10px;
		min-height: 44px;
	}
	.prep.err {
		color: var(--red);
	}
	.prep .progress {
		height: 4px;
	}
	:global(.spin) {
		animation: spin 1s linear infinite;
	}
	@keyframes spin {
		to {
			transform: rotate(360deg);
		}
	}
	@media (max-width: 520px) {
		.time .faint {
			display: none;
		}
	}
</style>
