<!--
	Countdown playlist item editor (F4, WS3). Embedded by the playlists page:

	  <CountdownItemEditor bind:item={countdownItem} onchange={save} />

	Edits a `CountdownItem` in place; `onchange` fires after every change (debounce
	saving on the caller's side if needed). `newCountdownItem()` in
	`$lib/playlist/countdown` makes a fresh one.
-->
<script lang="ts">
	import type { CountdownItem } from '$lib/api/types';
	import { app } from '$lib/stores/app.svelte';
	import Segmented from '$lib/components/ui/Segmented.svelte';
	import Slider from '$lib/components/ui/Slider.svelte';
	import Switch from '$lib/components/ui/Switch.svelte';
	import { countdownText, pickMatrix, TEXT_PRESETS } from '$lib/playlist/countdown';
	import { Play, Square, Timer } from '@lucide/svelte';
	import { onDestroy } from 'svelte';

	let { item = $bindable(), onchange }: { item: CountdownItem; onchange?: () => void } = $props();

	const show = $derived(app.show);
	const matrices = $derived((show?.props ?? []).filter((p) => p.matrix && p.matrix.width > 0));
	const autoMatrix = $derived(pickMatrix(show?.props ?? []));
	const clips = $derived(show?.djClips ?? []);
	const seconds = $derived(Math.round(item.durationMs / 1000));
	const textMode = $derived(TEXT_PRESETS.find((p) => p.value === (item.text ?? '{s}'))?.value ?? 'custom');
	const color = $derived(item.color ?? '#ffffff');
	const others = $derived(item.others ?? 'fill');
	const colors = ['#ffffff', '#ff2a2a', '#00d060', '#ffb000', '#2a7bff', '#c040ff'];

	function set(patch: Partial<CountdownItem>) {
		item = { ...item, ...patch };
		onchange?.();
	}

	// ----- preview -----
	let playing = $state(false);
	let t = $state(0);
	let raf = 0;
	let started = 0;
	function play() {
		if (playing) return stop();
		playing = true;
		started = performance.now();
		const step = () => {
			t = performance.now() - started;
			if (t >= item.durationMs + 600) return stop();
			raf = requestAnimationFrame(step);
		};
		raf = requestAnimationFrame(step);
	}
	function stop() {
		playing = false;
		cancelAnimationFrame(raf);
		t = 0;
	}
	onDestroy(() => cancelAnimationFrame(raf));
	const pt = $derived(playing ? t : 0);
	const left = $derived(Math.max(0, Math.ceil((item.durationMs - pt) / 1000)));
	const flashing = $derived(
		(item.finale ?? 'flash') === 'flash' && pt >= item.durationMs - 200 && pt < item.durationMs
	);
	const since = $derived.by(() => {
		const rem = item.durationMs - pt;
		if (rem <= 0) return 1e9;
		const f = rem % 1000;
		return f === 0 ? 0 : 1000 - f;
	});
	const accent = $derived(playing ? Math.exp(-since / 260) : 0);
	const fill = $derived(playing ? Math.min(1, pt / item.durationMs) : 0.35);
	const done = $derived(playing && pt >= item.durationMs);
</script>

<div class="cd">
	<div class="pv" class:flash={flashing} aria-hidden="true">
		<div class="matrix" style:--c={color}>
			{#if done}<span class="go">SHOW!</span>{:else}<span class="digits" style:opacity={0.8 + 0.2 * accent}
					>{countdownText(item.text ?? '{s}', playing ? left : seconds)}</span
				>{/if}
		</div>
		<div class="others">
			{#if others === 'fill'}
				<div class="bar">
					<div class="fillbar" style:width="{fill * 100}%" style:background={color}></div>
				</div>
			{:else if others === 'pulse'}
				<div class="bar">
					<div
						class="fillbar"
						style:width="100%"
						style:background={color}
						style:opacity={0.12 + 0.88 * accent}
					></div>
				</div>
			{:else}
				<div class="bar"></div>
			{/if}
			<span class="faint tiny">Other props</span>
		</div>
		<button type="button" class="btn sm pvbtn" onclick={play}>
			{#if playing}<Square size={13} /> Stop{:else}<Play size={13} /> Preview{/if}
		</button>
	</div>

	<div class="field">
		<div class="row between"><span class="label">Length</span><span class="num v">{seconds} s</span></div>
		<Slider
			label="Countdown length"
			min={5}
			max={60}
			step={1}
			value={seconds}
			onchange={(v) => set({ durationMs: v * 1000 })}
			format={(v) => `${v} seconds`}
		/>
	</div>

	<div class="field">
		<span class="label">Text on the matrix</span>
		<Segmented
			label="Text format"
			size="sm"
			value={textMode}
			options={[
				...TEXT_PRESETS.map((p) => ({ value: p.value, label: p.label })),
				{ value: 'custom', label: 'Custom' }
			]}
			onchange={(v) => set({ text: v === 'custom' ? 'SHOW IN {s}' : v })}
		/>
		{#if textMode === 'custom'}
			<input
				class="input"
				maxlength="40"
				value={item.text ?? ''}
				aria-label="Custom countdown text"
				placeholder="SHOW IN {'{s}'}"
				onchange={(e) => set({ text: (e.target as HTMLInputElement).value || '{s}' })}
			/>
			<span class="faint tiny"
				>Use {'{s}'} for the seconds left, {'{mm}'}:{'{ss}'} for minutes and seconds.</span
			>
		{/if}
	</div>

	<div class="grid">
		<label class="field">
			<span class="label">Matrix</span>
			<select
				class="select"
				value={item.matrixPropId ?? ''}
				onchange={(e) => set({ matrixPropId: (e.target as HTMLSelectElement).value || undefined })}
			>
				<option value="">{autoMatrix ? `Automatic (${autoMatrix.name})` : 'Automatic'}</option>
				{#each matrices as m (m.id)}<option value={m.id}>{m.name}</option>{/each}
			</select>
			{#if !matrices.length}<span class="faint tiny">No matrix prop: the other props still count down.</span
				>{/if}
		</label>
		<div class="field">
			<span class="label">Colour</span>
			<div class="swatches">
				{#each colors as c (c)}
					<button
						type="button"
						class="sw"
						style:background={c}
						aria-label="Colour {c}"
						aria-pressed={color.toLowerCase() === c}
						onclick={() => set({ color: c })}
					></button>
				{/each}
				<input
					type="color"
					class="sw pick"
					value={color}
					aria-label="Pick a colour"
					onchange={(e) => set({ color: (e.target as HTMLInputElement).value })}
				/>
			</div>
		</div>
	</div>

	<div class="field">
		<span class="label">Other props</span>
		<Segmented
			label="Other props"
			size="sm"
			value={others}
			options={[
				{ value: 'fill', label: 'Fill up' },
				{ value: 'pulse', label: 'Pulse each second' },
				{ value: 'dark', label: 'Dark' }
			]}
			onchange={(v) => set({ others: v as CountdownItem['others'] })}
		/>
	</div>

	<div class="row between opt">
		<div>
			<div class="label" style="margin:0">White flash at zero</div>
			<div class="faint tiny">Every prop flashes white just before the first song.</div>
		</div>
		<Switch
			label="White flash at zero"
			checked={(item.finale ?? 'flash') === 'flash'}
			onchange={(v) => set({ finale: v ? 'flash' : 'none' })}
		/>
	</div>

	<div class="field">
		<span class="label">DJ voice</span>
		<select
			class="select"
			value={item.djClipId ?? ''}
			onchange={(e) => set({ djClipId: (e.target as HTMLSelectElement).value || undefined })}
		>
			<option value="">None</option>
			{#each clips as c (c.id)}<option value={c.id}>{c.name}</option>{/each}
		</select>
		{#if item.djClipId}
			<div class="row between" style="margin-top:6px">
				<span class="faint small">
					{(item.djOffsetMs ?? 0) === 0
						? 'The clip ends exactly at zero.'
						: `The clip ends ${Math.abs(item.djOffsetMs ?? 0) / 1000} s ${(item.djOffsetMs ?? 0) > 0 ? 'after' : 'before'} zero.`}
				</span>
				<label class="row" style="gap:6px">
					<span class="faint tiny">Offset</span>
					<input
						class="input num off"
						type="number"
						step="100"
						min="-10000"
						max="10000"
						value={item.djOffsetMs ?? 0}
						aria-label="Clip offset in milliseconds"
						onchange={(e) =>
							set({ djOffsetMs: Math.round(Number((e.target as HTMLInputElement).value) || 0) })}
					/>
					<span class="faint tiny">ms</span>
				</label>
			</div>
		{:else}
			<div class="row between opt" style="margin-top:6px">
				<span class="faint small">Tick sound every second</span>
				<Switch label="Tick sound" size="sm" checked={!!item.tick} onchange={(v) => set({ tick: v })} />
			</div>
		{/if}
	</div>

	<p class="hint faint tiny">
		<Timer size={12} /> Put the countdown last in the intro and turn on <strong>Start exactly on time</strong> in
		the schedule: the intro then starts early so the first song begins right at show time.
	</p>
</div>

<style>
	.cd {
		display: flex;
		flex-direction: column;
		gap: 14px;
	}
	.pv {
		position: relative;
		display: grid;
		grid-template-columns: 1.4fr 1fr;
		gap: 12px;
		align-items: center;
		padding: 12px;
		border-radius: var(--r-3, 12px);
		background: #06070a;
		border: 1px solid var(--border);
		transition: background 80ms linear;
	}
	.pv.flash {
		background: #fff;
	}
	.matrix {
		height: 84px;
		border-radius: 8px;
		display: grid;
		place-items: center;
		background-color: #0b0d12;
		background-image: radial-gradient(circle, rgba(255, 255, 255, 0.07) 1px, transparent 1.5px);
		background-size: 6px 6px;
		overflow: hidden;
	}
	.digits,
	.go {
		font-family: ui-monospace, 'SF Mono', Menlo, monospace;
		font-weight: 800;
		font-size: 42px;
		letter-spacing: 0.04em;
		color: var(--c);
		text-shadow: 0 0 14px var(--c);
		white-space: nowrap;
	}
	.go {
		font-size: 26px;
		color: #fff;
	}
	.others {
		display: flex;
		flex-direction: column;
		gap: 6px;
	}
	.bar {
		height: 10px;
		border-radius: 5px;
		background: #1a1d24;
		overflow: hidden;
	}
	.fillbar {
		height: 100%;
		border-radius: 5px;
		box-shadow: 0 0 10px currentColor;
	}
	.pvbtn {
		position: absolute;
		right: 10px;
		bottom: 10px;
	}
	.grid {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: 12px;
	}
	.swatches {
		display: flex;
		gap: 6px;
		flex-wrap: wrap;
		align-items: center;
	}
	.sw {
		width: 26px;
		height: 26px;
		border-radius: 50%;
		border: 2px solid var(--border-2);
		padding: 0;
		cursor: pointer;
	}
	.sw[aria-pressed='true'] {
		outline: 2px solid var(--accent);
		outline-offset: 2px;
	}
	.sw.pick {
		background: none;
		overflow: hidden;
	}
	.opt {
		gap: 12px;
	}
	.off {
		width: 90px;
	}
	.v {
		font-size: 12px;
	}
	.hint {
		display: flex;
		gap: 6px;
		align-items: flex-start;
		margin: 0;
	}
	@media (max-width: 520px) {
		.pv,
		.grid {
			grid-template-columns: 1fr;
		}
		.pvbtn {
			position: static;
			justify-self: end;
		}
	}
</style>
