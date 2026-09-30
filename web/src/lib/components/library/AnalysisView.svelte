<!--
	Beat / tempo / energy of a song (F2): tempo, how steady the beat is, and an energy
	timeline coloured by section with beat ticks. Fetches the full analysis lazily.
-->
<script lang="ts">
	import type { AudioAnalysis, Media } from '$lib/api/types';
	import { ApiError } from '$lib/api/client';
	import { library } from '$lib/library/api';
	import { activeJob, waitJob } from '$lib/library/jobs.svelte';
	import { energyWord } from '$lib/library/tags';
	import { toasts } from '$lib/stores/toasts.svelte';
	import { RefreshCw, Activity } from '@lucide/svelte';

	let { media }: { media: Media } = $props();

	let data = $state<AudioAnalysis | null>(null);
	let missing = $state(false);
	let busy = $state(false);
	let pct = $state(0);
	const job = $derived(activeJob(media.id, 'analysis'));
	const sum = $derived(media.analysis);

	async function fetchIt() {
		try {
			data = await library.analysis(media.id);
			missing = false;
		} catch (e) {
			data = null;
			missing = e instanceof ApiError && e.status === 404;
		}
	}

	$effect(() => {
		void media.id;
		void sum?.bpm;
		void fetchIt();
	});

	async function again() {
		busy = true;
		pct = 0;
		try {
			const { jobId } = await library.analyze(media.id);
			await waitJob(jobId, (p) => (pct = p));
			await fetchIt();
			toasts.success(`Listened to “${media.name}” again`);
		} catch (e) {
			toasts.error('Couldn’t analyze the song', (e as Error).message);
		} finally {
			busy = false;
		}
	}

	const steady = $derived(
		!sum
			? ''
			: sum.bpmConfidence >= 0.6
				? 'Steady beat'
				: sum.bpmConfidence >= 0.3
					? 'Clear beat'
					: 'Loose beat'
	);
	const W = 600;
	const H = 44;
	const bars = $derived.by(() => {
		const rms = data?.energy10Hz.rms ?? [];
		if (!rms.length) return [] as { x: number; h: number }[];
		const n = Math.min(150, rms.length);
		const per = rms.length / n;
		return Array.from({ length: n }, (_, i) => {
			let m = 0;
			for (let k = Math.floor(i * per); k < Math.floor((i + 1) * per); k++) m = Math.max(m, rms[k] ?? 0);
			return { x: (i / n) * W, h: Math.max(2, (m / 255) * (H - 4)) };
		});
	});
	const dur = $derived(Math.max(1, (data?.energy10Hz.rms.length ?? 0) * 100));
	const LEVEL = { low: 'var(--blue, #3b82f6)', mid: 'var(--accent, #f5a524)', high: 'var(--red, #ef4444)' };
</script>

<div class="av">
	<div class="head">
		<Activity size={15} />
		{#if sum && sum.bpm > 0}
			<strong class="num">{Math.round(sum.bpm)} BPM</strong>
			<span class="faint small">· {steady} · {energyWord(sum.energy)} · {sum.sections} parts</span>
		{:else if sum}
			<span class="small">No steady beat found — lights will follow the loudness instead.</span>
		{:else if job || busy}
			<span class="small">Listening for the beat… <span class="num">{job?.pct ?? pct}%</span></span>
		{:else}
			<span class="small faint">Not analyzed yet.</span>
		{/if}
		<span class="grow"></span>
		<button class="btn sm ghost" onclick={again} disabled={busy || !!job} title="Run the beat analysis again">
			<RefreshCw size={13} />
			{sum ? 'Analyze again' : 'Analyze'}
		</button>
	</div>
	{#if data && bars.length}
		<svg
			viewBox="0 0 {W} {H}"
			preserveAspectRatio="none"
			role="img"
			aria-label="Energy over the song, by part"
		>
			{#each data.sections as s, i (i)}
				<rect
					x={(s.startMs / dur) * W}
					y="0"
					width={Math.max(1, ((s.endMs - s.startMs) / dur) * W)}
					height={H}
					fill={LEVEL[s.level]}
					opacity="0.12"
				/>
			{/each}
			{#each bars as b, i (i)}
				<rect x={b.x} y={H - b.h} width={Math.max(1, W / bars.length - 1)} height={b.h} rx="1" class="bar" />
			{/each}
			{#each data.downbeats as d, i (i)}
				<line x1={(d / dur) * W} x2={(d / dur) * W} y1="0" y2="5" class="tick" />
			{/each}
		</svg>
		<div class="legend faint tiny">
			<span><i style:background={LEVEL.low}></i>Quiet</span>
			<span><i style:background={LEVEL.mid}></i>Building</span>
			<span><i style:background={LEVEL.high}></i>Big</span>
			<span class="grow"></span>
			<span>Ticks mark each bar</span>
		</div>
	{:else if missing && !job && !busy && sum}
		<div class="faint tiny">The details are being refreshed.</div>
	{/if}
</div>

<style>
	.av {
		display: flex;
		flex-direction: column;
		gap: 8px;
		padding: 10px 12px;
		border-radius: 12px;
		background: var(--surface-2);
	}
	.head {
		display: flex;
		align-items: center;
		gap: 8px;
		flex-wrap: wrap;
		color: var(--text-2);
	}
	svg {
		width: 100%;
		height: 44px;
		display: block;
	}
	.bar {
		fill: var(--text-3);
	}
	.tick {
		stroke: var(--text-2);
		stroke-width: 1;
		vector-effect: non-scaling-stroke;
	}
	.legend {
		display: flex;
		gap: 12px;
		align-items: center;
	}
	.legend i {
		display: inline-block;
		width: 8px;
		height: 8px;
		border-radius: 2px;
		margin-right: 4px;
		opacity: 0.7;
	}
</style>
