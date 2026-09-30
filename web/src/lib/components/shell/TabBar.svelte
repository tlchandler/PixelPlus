<script lang="ts">
	import { page } from '$app/state';
	import { NAV, TABS, isActive } from './nav';
	import { Ellipsis } from '@lucide/svelte';
	import { fade, fly } from 'svelte/transition';

	let more = $state(false);
	const tabs = $derived(NAV.filter((n) => TABS.includes(n.href)));
	const rest = $derived(NAV.filter((n) => !TABS.includes(n.href)));
	const moreActive = $derived(rest.some((n) => isActive(n.href, page.url.pathname)));
	const short: Record<string, string> = { '/': 'Home', '/sequences': 'Sequences' };
</script>

<nav class="tabbar" aria-label="Main">
	{#each tabs as t (t.href)}
		{@const active = isActive(t.href, page.url.pathname)}
		<a href={t.href} class="tab" class:active aria-current={active ? 'page' : undefined}>
			<t.icon size={22} strokeWidth={active ? 2.2 : 1.8} />
			<span>{short[t.href] ?? t.label}</span>
		</a>
	{/each}
	<button class="tab" class:active={moreActive || more} onclick={() => (more = !more)} aria-expanded={more} aria-label="More pages">
		<Ellipsis size={22} />
		<span>More</span>
	</button>
</nav>

{#if more}
	<div class="scrim" transition:fade={{ duration: 150 }} onclick={() => (more = false)} aria-hidden="true"></div>
	<div class="sheet" transition:fly={{ y: 300, duration: 240, opacity: 1 }} role="dialog" aria-label="More pages">
		<div class="grabber"></div>
		<div class="grid">
			{#each rest as t (t.href)}
				{@const active = isActive(t.href, page.url.pathname)}
				<a href={t.href} class="tile" class:active onclick={() => (more = false)}>
					<t.icon size={22} />
					<span>{short[t.href] ?? t.label}</span>
				</a>
			{/each}
		</div>
	</div>
{/if}

<style>
	.tabbar {
		position: fixed;
		left: 0;
		right: 0;
		bottom: 0;
		height: calc(var(--tabbar-h) + env(safe-area-inset-bottom));
		padding-bottom: env(safe-area-inset-bottom);
		display: grid;
		grid-template-columns: repeat(5, 1fr);
		background: color-mix(in srgb, var(--sidebar) 88%, transparent);
		backdrop-filter: blur(20px) saturate(1.5);
		-webkit-backdrop-filter: blur(20px) saturate(1.5);
		border-top: 1px solid var(--border);
		z-index: 60;
	}
	.tab {
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: 3px;
		color: var(--text-3);
		font-size: 10.5px;
		font-weight: 560;
		-webkit-tap-highlight-color: transparent;
	}
	.tab.active {
		color: var(--accent);
	}
	.scrim {
		position: fixed;
		inset: 0;
		background: var(--overlay);
		z-index: 58;
	}
	.sheet {
		position: fixed;
		left: 8px;
		right: 8px;
		bottom: calc(var(--tabbar-h) + env(safe-area-inset-bottom) + 8px);
		background: var(--surface);
		border: 1px solid var(--border-2);
		border-radius: 20px;
		padding: 8px 12px 14px;
		z-index: 59;
		box-shadow: var(--shadow-3);
	}
	.grabber {
		width: 36px;
		height: 4px;
		border-radius: 4px;
		background: var(--border-3);
		margin: 2px auto 10px;
	}
	.grid {
		display: grid;
		grid-template-columns: repeat(4, 1fr);
		gap: 6px;
	}
	.tile {
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: 6px;
		height: 76px;
		border-radius: 14px;
		color: var(--text-2);
		font-size: 11.5px;
		font-weight: 540;
		text-align: center;
		background: var(--surface-2);
	}
	.tile.active {
		color: var(--accent);
		background: var(--accent-soft);
	}
</style>
