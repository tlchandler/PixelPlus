<script lang="ts">
	import { page } from '$app/state';
	import { TABS, isActive, visibleNav } from './nav';
	import { Ellipsis, SlidersHorizontal } from '@lucide/svelte';
	import { fade, fly } from 'svelte/transition';

	let more = $state(false);
	let sheet: HTMLDivElement | undefined = $state();
	let moreBtn: HTMLButtonElement | undefined = $state();
	const nav = $derived(visibleNav());
	const tabs = $derived(TABS.map((h) => nav.find((n) => n.href === h)!).filter(Boolean));
	const rest = $derived(nav.filter((n) => !TABS.includes(n.href)));
	const moreActive = $derived(rest.some((n) => isActive(n.href, page.url.pathname)));
	const short: Record<string, string> = { '/sequences': 'Sequences' };

	function close(returnFocus = false) {
		more = false;
		if (returnFocus) moreBtn?.focus();
	}
	$effect(() => {
		if (more) requestAnimationFrame(() => sheet?.querySelector<HTMLElement>('a')?.focus());
	});
</script>

<svelte:window onkeydown={(e) => more && e.key === 'Escape' && close(true)} />

<nav class="tabbar" aria-label="Main">
	{#each tabs as t (t.href)}
		{@const active = isActive(t.href, page.url.pathname)}
		<a href={t.href} class="tab" class:active aria-current={active ? 'page' : undefined}>
			<t.icon size={22} strokeWidth={active ? 2.2 : 1.8} />
			<span>{short[t.href] ?? t.label}</span>
		</a>
	{/each}
	<button
		bind:this={moreBtn}
		class="tab"
		class:active={moreActive || more}
		onclick={() => (more = !more)}
		aria-expanded={more}
		aria-haspopup="dialog"
		aria-label="More pages"
	>
		<Ellipsis size={22} />
		<span>More</span>
	</button>
</nav>

{#if more}
	<div class="scrim" transition:fade={{ duration: 150 }} onclick={() => close()} aria-hidden="true"></div>
	<div
		bind:this={sheet}
		class="sheet"
		transition:fly={{ y: 300, duration: 240, opacity: 1 }}
		role="dialog"
		aria-modal="true"
		aria-label="More pages"
	>
		<div class="grabber"></div>
		<div class="grid">
			{#each rest as t (t.href)}
				{@const active = isActive(t.href, page.url.pathname)}
				<a
					href={t.href}
					class="tile"
					class:active
					aria-current={active ? 'page' : undefined}
					onclick={() => close()}
				>
					<t.icon size={22} />
					<span>{short[t.href] ?? t.label}</span>
				</a>
			{/each}
		</div>
		<a class="customize" href="/settings/features" onclick={() => close()}
			><SlidersHorizontal size={16} /> Customize what you see</a
		>
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
		background: var(--sidebar);
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
		min-width: 0;
		color: var(--text-3);
		font-size: 10.5px;
		font-weight: 560;
		-webkit-tap-highlight-color: transparent;
	}
	.tab span {
		max-width: 100%;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.tab.active {
		color: var(--accent-text);
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
		padding: 0 4px;
		border-radius: 14px;
		color: var(--text-2);
		font-size: 11.5px;
		font-weight: 540;
		text-align: center;
		background: var(--surface-2);
	}
	.customize {
		display: flex;
		align-items: center;
		justify-content: center;
		gap: 8px;
		min-height: 44px;
		margin-top: 8px;
		border-radius: 12px;
		color: var(--text-2);
		font-size: 13px;
		font-weight: 560;
	}
	.customize:hover {
		background: var(--surface-2);
		color: var(--text);
	}
	.tile.active {
		color: var(--accent-text);
		background: var(--accent-soft);
	}
</style>
