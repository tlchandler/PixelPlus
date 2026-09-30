<!--
	Settings → Features (ARCHITECTURE §12.17): turn optional parts of PixelPlus on and off so
	the rest of the interface shows only what this controller actually uses. Presets
	(Essentials / Everything / Custom), search, one card per group, "in use" facts from the
	show, dependency notes; turning off something in use asks first. Nothing is ever deleted.
-->
<script lang="ts">
	import {
		ArrowLeft,
		Check,
		Link2,
		LockKeyhole,
		Search,
		Sparkles,
		SlidersHorizontal,
		X,
		Layers
	} from '@lucide/svelte';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import { api } from '$lib/api/client';
	import type { FeatureId } from '$lib/api/types';
	import { app } from '$lib/stores/app.svelte';
	import {
		ALWAYS_ON,
		ESSENTIALS,
		FEATURES,
		FEATURE_GROUPS,
		dependents,
		disabledOf,
		feature,
		presetOf,
		usage,
		type FeatureDef,
		type PresetId
	} from '$lib/features';
	import { applyPreset, toggleFeature } from '$lib/features-actions';

	const disabled = $derived(disabledOf(app.show?.settings));
	const on = (id: FeatureId) => !disabled.includes(id);
	const onCount = $derived(FEATURES.filter((f) => on(f.id)).length);
	const preset = $derived(presetOf(disabled));

	let gameRoms = $state<number | undefined>(undefined);
	$effect(() => {
		if (!app.show || !on('games')) return;
		api.games
			.status()
			.then((s) => (gameRoms = s.roms?.length))
			.catch(() => {});
	});
	const use = $derived(usage(app.show, { gameRoms }));

	let q = $state('');
	const query = $derived(q.trim().toLowerCase());
	const matches = (f: FeatureDef) =>
		!query || `${f.name} ${f.description} ${f.keywords}`.toLowerCase().includes(query);
	const shown = $derived(FEATURES.filter(matches));

	/** Features being switched right now (their switch shows progress). */
	let busy = $state<FeatureId | PresetId | null>(null);
	async function flip(f: FeatureDef) {
		if (busy) return;
		busy = f.id;
		await toggleFeature(f.id, !on(f.id));
		busy = null;
	}
	async function pick(p: Exclude<PresetId, 'custom'>) {
		if (busy || preset === p) return;
		busy = p;
		await applyPreset(p);
		busy = null;
	}

	const names = (ids: FeatureId[]) => ids.map((id) => feature(id).name).join(' and ');
	function depNote(f: FeatureDef): string | null {
		const needs = f.requires;
		const neededBy = dependents(f.id);
		if (needs.length) return `Needs ${names(needs)}`;
		if (neededBy.length) return `${names(neededBy)} ${neededBy.length === 1 ? 'needs' : 'need'} this`;
		return null;
	}

	const presetCards = $derived([
		{
			id: 'essentials' as const,
			label: 'Essentials',
			blurb: 'The everyday tools for a great show',
			count: ESSENTIALS.length,
			icon: Sparkles
		},
		{
			id: 'everything' as const,
			label: 'Everything',
			blurb: 'Every tool and every page',
			count: FEATURES.length,
			icon: Layers
		},
		{
			id: 'custom' as const,
			label: 'Custom',
			blurb: preset === 'custom' ? 'Your own mix' : 'Flip any switch below',
			count: onCount,
			icon: SlidersHorizontal
		}
	]);
</script>

<svelte:head><title>Features · Settings · PixelPlus</title></svelte:head>

<div class="page feat">
	<a class="btn ghost sm back" href="/settings"><ArrowLeft size={16} /> Settings</a>
	<PageHeader
		title="Features"
		subtitle="Keep PixelPlus to what you use on this controller. Features you turn off disappear from menus and pages — nothing is deleted, and you can turn them back on any time."
	/>

	<div class="presets" role="radiogroup" aria-label="Presets">
		{#each presetCards as p (p.id)}
			{@const selected = preset === p.id}
			<button
				class="preset"
				class:selected
				class:custom={p.id === 'custom'}
				role="radio"
				aria-checked={selected}
				disabled={p.id === 'custom' ? !selected : !!busy}
				onclick={() => p.id !== 'custom' && pick(p.id)}
			>
				<span class="p-ic"><p.icon size={18} /></span>
				<span class="p-txt">
					<span class="p-label">{p.label}</span>
					<span class="p-blurb">{p.blurb}</span>
				</span>
				{#if p.id !== 'custom' || selected}<span class="p-count">{p.count}</span>{/if}
				{#if selected}<span class="p-tick" aria-hidden="true"><Check size={12} strokeWidth={3} /></span>{/if}
			</button>
		{/each}
	</div>

	<div class="searchbar">
		<div class="input-group grow">
			<span class="prefix"><Search size={16} /></span>
			<input
				class="input"
				type="search"
				placeholder="Search features"
				aria-label="Search features"
				data-search
				bind:value={q}
			/>
		</div>
		{#if q}
			<button class="btn ghost icon" aria-label="Clear search" onclick={() => (q = '')}
				><X size={16} /></button
			>
		{/if}
	</div>

	{#if !shown.length}
		<div class="card">
			<EmptyState
				icon={Search}
				title="No feature matches “{q}”"
				message="Try a shorter word, like “voice”, “camera” or “power”."
			>
				<button class="btn" onclick={() => (q = '')}>Show all features</button>
			</EmptyState>
		</div>
	{/if}

	{#each FEATURE_GROUPS as g (g.id)}
		{@const items = shown.filter((f) => f.group === g.id)}
		{#if items.length}
			{@const all = FEATURES.filter((f) => f.group === g.id)}
			<section class="card group" aria-labelledby="g-{g.id}">
				<header class="g-head">
					<div class="grow">
						<h2 id="g-{g.id}">{g.label}</h2>
						<p class="faint small">{g.blurb}</p>
					</div>
					<span class="badge outline">{all.filter((f) => on(f.id)).length} of {all.length} on</span>
				</header>
				<ul class="rows">
					{#each items as f (f.id)}
						{@const isOn = on(f.id)}
						{@const u = use[f.id]}
						{@const dep = depNote(f)}
						<li class="row-f" class:off={!isOn}>
							<span class="f-ic" aria-hidden="true"><f.icon size={20} /></span>
							<div class="f-txt">
								<div class="f-top">
									<span class="f-name" id="f-{f.id}">{f.name}</span>
									{#if u.inUse}
										<span class="badge {isOn ? 'green' : ''} inuse"
											><span class="dot"></span>{isOn ? 'In use' : 'Set up'}</span
										>
									{/if}
								</div>
								<p class="f-desc" id="fd-{f.id}">{f.description}</p>
								{#if u.facts.length || dep}
									<div class="f-meta">
										{#if u.facts.length}<span class="fact">{u.facts.join(' · ')}</span>{/if}
										{#if dep}<span class="dep"><Link2 size={12} /> {dep}</span>{/if}
									</div>
								{/if}
							</div>
							<button
								type="button"
								role="switch"
								class="fswitch"
								class:busy={busy === f.id}
								aria-checked={isOn}
								aria-labelledby="f-{f.id}"
								aria-describedby="fd-{f.id}"
								disabled={!!busy}
								onclick={() => flip(f)}
							>
								<span class="knob"></span>
							</button>
						</li>
					{/each}
				</ul>
			</section>
		{/if}
	{/each}

	{#if !query}
		<section class="card always" aria-labelledby="always-title">
			<header class="g-head">
				<span class="lock" aria-hidden="true"><LockKeyhole size={16} /></span>
				<div class="grow">
					<h2 id="always-title">Always included</h2>
					<p class="faint small">The heart of every show — these can’t be turned off.</p>
				</div>
			</header>
			<ul class="core">
				{#each ALWAYS_ON as c (c)}<li class="badge outline">{c}</li>{/each}
			</ul>
		</section>
	{/if}
</div>

<style>
	.feat {
		max-width: 920px;
	}
	.back {
		margin: 0 0 12px -8px;
	}

	/* ---- presets */
	.presets {
		display: grid;
		grid-template-columns: repeat(3, minmax(0, 1fr));
		gap: 12px;
		margin-bottom: 16px;
	}
	.preset {
		position: relative;
		display: flex;
		align-items: center;
		gap: 12px;
		min-height: 72px;
		padding: 14px 16px;
		border-radius: var(--r-3);
		background: var(--surface);
		border: 1px solid var(--border-2);
		text-align: left;
		color: var(--text);
		transition:
			border-color var(--dur) var(--ease),
			background var(--dur) var(--ease),
			transform var(--dur) var(--ease);
	}
	.preset:not(:disabled):hover {
		border-color: var(--border-3);
		transform: translateY(-1px);
	}
	.preset.selected {
		border-color: var(--accent-line);
		background: radial-gradient(260px 90px at 0% 0%, var(--accent-soft), transparent 75%), var(--surface);
		box-shadow: inset 0 0 0 1px var(--accent-line);
	}
	.preset:disabled {
		cursor: default;
	}
	.preset.custom:not(.selected) {
		border-style: dashed;
		background: transparent;
	}
	.preset.custom:not(.selected) .p-txt,
	.preset.custom:not(.selected) .p-count {
		opacity: 0.85;
	}
	.p-ic {
		display: grid;
		place-items: center;
		width: 36px;
		height: 36px;
		border-radius: 10px;
		background: var(--surface-3);
		color: var(--text-2);
		flex: 0 0 auto;
	}
	.selected .p-ic {
		background: var(--accent-soft);
		color: var(--accent-text);
	}
	.p-txt {
		display: flex;
		flex-direction: column;
		min-width: 0;
		flex: 1;
	}
	.p-label {
		font-weight: 650;
		font-size: 14.5px;
	}
	.p-blurb {
		color: var(--text-3);
		font-size: 12.5px;
		line-height: 1.35;
	}
	.p-count {
		font-size: 20px;
		font-weight: 650;
		color: var(--text-3);
		font-variant-numeric: tabular-nums;
	}
	.selected .p-count {
		color: var(--accent-text);
	}
	.p-tick {
		position: absolute;
		top: -7px;
		right: -7px;
		width: 20px;
		height: 20px;
		border-radius: 50%;
		display: grid;
		place-items: center;
		background: var(--accent);
		color: var(--accent-fg);
		box-shadow: 0 0 0 3px var(--bg);
	}

	/* ---- search */
	.searchbar {
		display: flex;
		gap: 8px;
		margin-bottom: 16px;
	}
	.searchbar .input {
		width: 100%;
		height: 44px;
	}

	/* ---- groups */
	.group,
	.always {
		margin-bottom: 16px;
		overflow: hidden;
	}
	.g-head {
		display: flex;
		align-items: center;
		gap: 12px;
		padding: 16px 20px 12px;
		border-bottom: 1px solid var(--border);
	}
	.g-head h2 {
		font-size: 15px;
	}
	.rows {
		list-style: none;
		margin: 0;
		padding: 0;
	}
	.row-f {
		display: flex;
		align-items: flex-start;
		gap: 14px;
		padding: 14px 20px;
		min-height: 64px;
	}
	.row-f + .row-f {
		border-top: 1px solid var(--border);
	}
	.f-ic {
		display: grid;
		place-items: center;
		width: 40px;
		height: 40px;
		border-radius: 11px;
		background: var(--accent-soft);
		color: var(--accent-text);
		flex: 0 0 auto;
		transition:
			background var(--dur) var(--ease),
			color var(--dur) var(--ease);
	}
	.off .f-ic {
		background: var(--surface-3);
		color: var(--text-3);
	}
	.f-txt {
		flex: 1;
		min-width: 0;
	}
	.f-top {
		display: flex;
		align-items: center;
		gap: 8px;
		flex-wrap: wrap;
		min-height: 22px;
	}
	.f-name {
		font-weight: 620;
		font-size: 14.5px;
	}
	.off .f-name {
		color: var(--text-2);
	}
	.inuse {
		height: 20px;
		font-size: 11px;
	}
	.inuse .dot {
		width: 6px;
		height: 6px;
	}
	.f-desc {
		margin-top: 2px;
		color: var(--text-2);
		font-size: 13px;
		line-height: 1.45;
	}
	.f-meta {
		display: flex;
		flex-wrap: wrap;
		gap: 4px 14px;
		margin-top: 6px;
		font-size: 12px;
		color: var(--text-3);
	}
	.fact {
		color: var(--text-2);
	}
	.dep {
		display: inline-flex;
		align-items: center;
		gap: 4px;
	}

	/* A switch with a 44 × 44 touch target. */
	.fswitch {
		position: relative;
		flex: 0 0 auto;
		width: 44px;
		height: 26px;
		margin-top: 7px;
		border-radius: 99px;
		background: var(--surface-3);
		border: 1px solid var(--border-3);
		transition:
			background 200ms var(--ease),
			border-color 200ms var(--ease);
	}
	.fswitch::before {
		content: '';
		position: absolute;
		inset: -10px -4px;
	}
	.fswitch .knob {
		position: absolute;
		top: 2px;
		left: 2px;
		width: 20px;
		height: 20px;
		border-radius: 50%;
		background: #fff;
		box-shadow: 0 1px 3px rgba(0, 0, 0, 0.35);
		transition: transform 220ms var(--ease-spring);
	}
	.fswitch[aria-checked='true'] {
		background: var(--accent);
		border-color: var(--accent);
	}
	.fswitch[aria-checked='true'] .knob {
		transform: translateX(18px);
	}
	.fswitch:focus-visible {
		box-shadow: var(--ring);
	}
	/* Bigger on touch screens: 52 × 32, plus the extended hit area. */
	@media (pointer: coarse) {
		.fswitch {
			width: 52px;
			height: 32px;
			margin-top: 4px;
		}
		.fswitch .knob {
			width: 26px;
			height: 26px;
		}
		.fswitch[aria-checked='true'] .knob {
			transform: translateX(20px);
		}
	}
	.fswitch.busy {
		opacity: 0.6;
	}
	.fswitch:disabled:not(.busy) {
		cursor: progress;
	}

	/* ---- always on */
	.lock {
		display: grid;
		place-items: center;
		width: 32px;
		height: 32px;
		border-radius: 9px;
		background: var(--surface-3);
		color: var(--text-3);
	}
	.core {
		list-style: none;
		display: flex;
		flex-wrap: wrap;
		gap: 8px;
		margin: 0;
		padding: 14px 20px 18px;
	}
	.core .badge {
		height: 28px;
		padding: 0 12px;
		font-size: 12.5px;
	}

	@media (max-width: 760px) {
		.presets {
			grid-template-columns: 1fr;
			gap: 8px;
		}
		.preset {
			min-height: 60px;
			padding: 10px 14px;
		}
		.g-head {
			padding: 14px 16px 10px;
		}
		.row-f {
			padding: 14px 16px;
			gap: 12px;
		}
		.f-ic {
			width: 36px;
			height: 36px;
			border-radius: 10px;
		}
		.core {
			padding: 12px 16px 16px;
		}
	}
</style>
