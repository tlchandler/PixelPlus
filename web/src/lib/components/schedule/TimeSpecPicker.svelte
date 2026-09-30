<script lang="ts">
	import type { Location, TimeSpec } from '$lib/api/types';
	import { fmtTime, resolveTimeSpec, zonedParts } from '$lib/util/time';
	import Segmented from '$lib/components/ui/Segmented.svelte';
	import { Clock, Sunset, Sunrise } from '@lucide/svelte';

	let {
		value = $bindable(),
		location,
		label
	}: { value: TimeSpec; location: Location; label: string } = $props();

	let kind = $state(value.kind);
	let clock = $state(value.kind === 'clock' ? value.time : '18:00');
	let offset = $state(value.kind !== 'clock' ? Math.abs(value.offsetMin) : 0);
	let dir = $state<'after' | 'before'>(value.kind !== 'clock' && value.offsetMin < 0 ? 'before' : 'after');

	function emit() {
		value =
			kind === 'clock' ? { kind, time: clock } : { kind, offsetMin: dir === 'before' ? -offset : offset };
	}

	const today = $derived.by(() => {
		const p = zonedParts(new Date(), location.timezone);
		return resolveTimeSpec(value, p.y, p.m, p.d, location);
	});
</script>

<div class="tsp" role="group" aria-label={label}>
	<Segmented
		bind:value={kind}
		size="sm"
		{label}
		onchange={emit}
		options={[
			{ value: 'clock', label: 'Time', icon: Clock },
			{ value: 'sunset', label: 'Sunset', icon: Sunset },
			{ value: 'sunrise', label: 'Sunrise', icon: Sunrise }
		]}
	/>
	{#if kind === 'clock'}
		<input class="input" type="time" bind:value={clock} oninput={emit} aria-label="{label} time" />
	{:else}
		<div class="row">
			<div class="input-group" style="width:110px">
				<input
					class="input num"
					type="number"
					min="0"
					max="240"
					step="5"
					bind:value={offset}
					oninput={emit}
					aria-label="Minutes"
				/>
				<span class="suffix">min</span>
			</div>
			<select class="select" style="width:auto" bind:value={dir} onchange={emit} aria-label="Before or after">
				<option value="after">after {kind}</option>
				<option value="before">before {kind}</option>
			</select>
		</div>
	{/if}
	{#if kind !== 'clock'}
		<span class="calc faint small">≈ {fmtTime(today, location.timezone)} today — moves with the season</span>
	{/if}
</div>

<style>
	.tsp {
		display: flex;
		flex-direction: column;
		gap: 8px;
		align-items: flex-start;
	}
	.tsp :global(.input[type='time']) {
		width: 160px;
	}
	.calc {
		color: var(--accent-text);
	}
</style>
