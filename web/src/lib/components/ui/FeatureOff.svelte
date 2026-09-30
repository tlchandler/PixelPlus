<!--
	Shown instead of a page (or a section) whose feature is turned off in Settings → Features:
	what it is, that nothing was deleted, and a one-tap "Turn on".
-->
<script lang="ts">
	import type { FeatureId } from '$lib/api/types';
	import { feature, isEnabled } from '$lib/features';
	import { toggleFeature } from '$lib/features-actions';
	import { Power, SlidersHorizontal } from '@lucide/svelte';

	let { id, compact = false }: { id: FeatureId; compact?: boolean } = $props();
	const f = $derived(feature(id));
	const needs = $derived(f.requires.filter((r) => !isEnabled(r)).map((r) => feature(r).name));
	let busy = $state(false);

	async function turnOn() {
		busy = true;
		await toggleFeature(id, true);
		busy = false;
	}
</script>

<div class="off" class:compact class:page={!compact} role="region" aria-labelledby="off-title-{id}">
	<div class="card inner">
		<div class="halo" aria-hidden="true">
			<f.icon size={compact ? 22 : 28} strokeWidth={1.6} />
			<span class="plug"><Power size={11} strokeWidth={2.6} /></span>
		</div>
		<span class="badge outline">Turned off</span>
		<svelte:element this={compact ? 'h2' : 'h1'} class="title" id="off-title-{id}"
			>{f.name} is turned off</svelte:element
		>
		<p class="desc">{f.description}</p>
		<p class="faint small">
			It was turned off in Settings → Features to keep things simple. Anything you set up is still here.
			{#if needs.length}Turning it on also turns on {needs.join(' and ')}.{/if}
		</p>
		<div class="actions">
			<button class="btn primary" onclick={turnOn} disabled={busy}>
				<Power size={16} />
				{busy ? 'Turning on…' : `Turn on ${f.name}`}
			</button>
			<a class="btn ghost" href="/settings/features"><SlidersHorizontal size={16} /> All features</a>
		</div>
	</div>
</div>

<style>
	.off.page {
		padding: 48px 32px;
		display: grid;
		place-items: start center;
		min-height: 60dvh;
	}
	.inner {
		max-width: 520px;
		width: 100%;
		display: flex;
		flex-direction: column;
		align-items: center;
		text-align: center;
		gap: 10px;
		padding: 36px 28px 28px;
		background: radial-gradient(420px 160px at 50% 0%, var(--accent-soft), transparent 70%), var(--surface);
	}
	.compact .inner {
		max-width: none;
		padding: 28px 20px 22px;
	}
	.halo {
		position: relative;
		width: 64px;
		height: 64px;
		border-radius: 20px;
		display: grid;
		place-items: center;
		color: var(--text-2);
		background: var(--surface-2);
		border: 1px solid var(--border-2);
		margin-bottom: 4px;
	}
	.plug {
		position: absolute;
		right: -6px;
		bottom: -6px;
		width: 22px;
		height: 22px;
		border-radius: 50%;
		display: grid;
		place-items: center;
		background: var(--surface-3);
		color: var(--text-2);
		border: 2px solid var(--surface);
	}
	.title {
		font-size: 19px;
		letter-spacing: -0.01em;
	}
	.compact .title {
		font-size: 16px;
	}
	.desc {
		color: var(--text-2);
		font-size: 14px;
		max-width: 420px;
	}
	.small {
		max-width: 420px;
	}
	.actions {
		display: flex;
		gap: 8px;
		margin-top: 10px;
		flex-wrap: wrap;
		justify-content: center;
	}
	.actions .btn {
		min-height: 44px;
	}
	@media (max-width: 760px) {
		.off.page {
			padding: 16px;
		}
		.inner {
			padding: 28px 18px 20px;
		}
		.actions {
			width: 100%;
			flex-direction: column;
		}
	}
</style>
