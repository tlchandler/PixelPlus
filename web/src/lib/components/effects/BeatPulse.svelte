<!-- "Pulse to a beat" (F2 beat-reactive looks, WS3): the beatBpm / beatPhaseMs /
     beatDepth / beatDecayMs parameters every look accepts, with tap tempo. -->
<script lang="ts">
	import type { EffectParams } from '$lib/api/types';
	import Switch from '$lib/components/ui/Switch.svelte';
	import Slider from '$lib/components/ui/Slider.svelte';
	import { Activity, Hand, Minus, Plus } from '@lucide/svelte';

	let { params = $bindable(), onchange }: { params: EffectParams; onchange?: () => void } = $props();

	const num = (k: string, d: number) => {
		const v = Number(params[k]);
		return Number.isFinite(v) ? v : d;
	};
	const bpm = $derived(num('beatBpm', 0));
	const on = $derived(bpm > 0);
	const depth = $derived(num('beatDepth', 0.6));
	const decay = $derived(num('beatDecayMs', 220));
	const phase = $derived(num('beatPhaseMs', 0));
	const follow = $derived(params.beatFollowSong === true);

	// The last tempo used, so switching off and on again keeps it.
	let remembered = $state(120);

	function set(patch: Record<string, number>) {
		params = { ...params, ...patch };
		onchange?.();
	}
	function toggle(v: boolean) {
		if (v) set({ beatBpm: remembered, beatDepth: depth || 0.6, beatDecayMs: decay || 220 });
		else {
			if (bpm > 0) remembered = bpm;
			set({ beatBpm: 0 });
		}
	}
	function setBpm(v: number) {
		if (!Number.isFinite(v)) return;
		const b = Math.round(Math.min(300, Math.max(20, v)) * 10) / 10;
		remembered = b;
		set({ beatBpm: b });
	}

	// Tap tempo: the mean interval of the last taps (a pause of 2 s starts over).
	let taps = $state<number[]>([]);
	function tap() {
		const now = performance.now();
		const last = taps[taps.length - 1];
		const next = last !== undefined && now - last > 2000 ? [now] : [...taps, now].slice(-8);
		taps = next;
		if (next.length >= 2) {
			const span = next[next.length - 1] - next[0];
			setBpm((60000 * (next.length - 1)) / span);
		}
	}
	const tapHint = $derived(
		taps.length === 0 ? 'Tap along to the music' : taps.length === 1 ? 'Keep tapping…' : `${taps.length} taps`
	);
	function nudge(ms: number) {
		const period = bpm > 0 ? 60000 / bpm : 500;
		set({ beatPhaseMs: Math.round((((phase + ms) % period) + period) % period) });
	}
</script>

<div class="beat">
	<div class="row between head">
		<div class="row" style="gap:8px">
			<Activity size={15} />
			<div>
				<div class="label" style="margin:0">Pulse to a beat</div>
				<div class="faint tiny">Brightness swells on every beat — also on followers, perfectly in step.</div>
			</div>
		</div>
		<Switch checked={on} label="Pulse to a beat" onchange={toggle} />
	</div>
	{#if on}
		<div class="grid">
			<label class="field">
				<span class="label">Tempo</span>
				<div class="row" style="gap:6px">
					<input
						class="input num"
						type="number"
						min="20"
						max="300"
						step="0.1"
						value={bpm}
						aria-label="Beats per minute"
						onchange={(e) => setBpm(Number((e.target as HTMLInputElement).value))}
					/>
					<span class="faint small">BPM</span>
				</div>
			</label>
			<div class="field">
				<span class="label">Tap tempo</span>
				<button type="button" class="btn tapbtn" onclick={tap} aria-label="Tap the beat">
					<Hand size={15} /> Tap
				</button>
				<span class="faint tiny">{tapHint}</span>
			</div>
		</div>
		<div class="field">
			<div class="row between">
				<span class="label">Pulse depth</span><span class="num v">{Math.round(depth * 100)} %</span>
			</div>
			<Slider
				label="Pulse depth"
				min={0}
				max={1}
				step={0.05}
				value={depth}
				onchange={(v) => set({ beatDepth: v })}
				format={(v) => `${Math.round(v * 100)} %`}
			/>
		</div>
		<div class="field">
			<div class="row between">
				<span class="label">Pulse length</span><span class="num v">{decay} ms</span>
			</div>
			<Slider
				label="Pulse length"
				min={30}
				max={2000}
				step={10}
				value={decay}
				onchange={(v) => set({ beatDecayMs: v })}
				format={(v) => `${v} ms`}
			/>
		</div>
		<div class="row between follow">
			<div>
				<div class="label" style="margin:0">Follow the song’s beat</div>
				<div class="faint tiny">
					As the idle look under a song or DJ clip, pulse on the song’s own beats (once its beat has been
					analysed).
				</div>
			</div>
			<Switch
				size="sm"
				label="Follow the song’s beat"
				checked={follow}
				onchange={(v) => {
					params = { ...params, beatFollowSong: v };
					onchange?.();
				}}
			/>
		</div>
		<div class="row between">
			<span class="faint small">Beat offset {phase} ms</span>
			<div class="row" style="gap:6px">
				<button type="button" class="btn sm icon" onclick={() => nudge(-25)} aria-label="Beat earlier"
					><Minus size={14} /></button
				>
				<button type="button" class="btn sm icon" onclick={() => nudge(25)} aria-label="Beat later"
					><Plus size={14} /></button
				>
			</div>
		</div>
	{/if}
</div>

<style>
	.beat {
		margin-top: 18px;
		padding: 14px;
		border-radius: var(--r-3, 12px);
		border: 1px solid var(--border);
		display: flex;
		flex-direction: column;
		gap: 12px;
	}
	.head {
		gap: 12px;
	}
	.grid {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: 12px;
	}
	.num.input {
		width: 96px;
	}
	.tapbtn {
		width: 100%;
		justify-content: center;
		min-height: 40px;
	}
	.v {
		font-size: 12px;
	}
	.follow {
		gap: 12px;
	}
	@media (max-width: 480px) {
		.grid {
			grid-template-columns: 1fr;
		}
	}
</style>
