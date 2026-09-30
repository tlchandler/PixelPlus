<script lang="ts">
	import { page } from '$app/state';
	import { app } from '$lib/stores/app.svelte';
	import { theme } from '$lib/stores/theme.svelte';
	import { GROUPS, isActive, visibleNav } from './nav';
	import Logo from './Logo.svelte';
	import { Moon, Sun, Keyboard, FlaskConical, SlidersHorizontal } from '@lucide/svelte';
	import { exitMock } from '$lib/api/mode';

	let { onshortcuts }: { onshortcuts: () => void } = $props();

	const online = $derived(app.nodes.filter((n) => n.online).length);
	const conn = $derived(app.connection);
	const nav = $derived(visibleNav());
	/** Groups that still have a page (a group whose pages are all turned off disappears). */
	const groups = $derived(GROUPS.filter((g) => nav.some((n) => n.group === g.id)));
</script>

<nav class="sidebar" aria-label="Main">
	<div class="brand">
		<Logo size={32} />
		<div class="grow">
			<div class="name ellipsis">{app.show?.name ?? 'PixelPlus'}</div>
			<div class="sub">
				<span class="dot" class:ok={conn === 'open'} class:bad={conn === 'closed'}></span>
				{#if conn === 'open'}
					{online || 1} controller{(online || 1) === 1 ? '' : 's'} online
				{:else if conn === 'connecting'}
					Connecting…
				{:else}
					Reconnecting…
				{/if}
			</div>
		</div>
	</div>

	{#if app.mock}
		<div class="demo" title="Running against the in-browser demo backend">
			<FlaskConical size={14} /> <span class="grow">Demo show</span>
			{#if !app.mockAuto}
				<button
					class="exit"
					onclick={() => {
						exitMock();
						location.href = '/';
					}}>Exit</button
				>
			{/if}
		</div>
	{/if}

	<div class="scroll">
		{#each groups as g (g.id)}
			<div class="group">
				<div class="glabel">{g.label}</div>
				{#each nav.filter((n) => n.group === g.id) as item (item.href)}
					{@const active = isActive(item.href, page.url.pathname)}
					<a href={item.href} class="item" class:active aria-current={active ? 'page' : undefined}>
						<item.icon size={18} strokeWidth={active ? 2.2 : 1.8} />
						<span>{item.label}</span>
					</a>
				{/each}
			</div>
		{/each}
	</div>

	<div class="foot">
		{#each nav.filter((n) => n.group === 'system') as item (item.href)}
			{@const active = isActive(item.href, page.url.pathname)}
			<a href={item.href} class="item" class:active aria-current={active ? 'page' : undefined}>
				<item.icon size={18} strokeWidth={active ? 2.2 : 1.8} />
				<span>{item.label}</span>
			</a>
		{/each}
		<a
			class="customize"
			href="/settings/features"
			aria-current={page.url.pathname === '/settings/features' ? 'page' : undefined}
			><SlidersHorizontal size={14} /> Customize what you see</a
		>
		<div class="tools">
			<button
				class="btn ghost icon sm"
				onclick={() => theme.toggle()}
				aria-label="Switch to {theme.current === 'dark' ? 'light' : 'dark'} theme"
				title="Toggle theme"
			>
				{#if theme.current === 'dark'}<Sun size={16} />{:else}<Moon size={16} />{/if}
			</button>
			<button
				class="btn ghost icon sm"
				onclick={onshortcuts}
				aria-label="Keyboard shortcuts"
				title="Keyboard shortcuts (?)"
			>
				<Keyboard size={16} />
			</button>
			<span class="ver faint tiny">v{app.system?.version ?? '—'}</span>
		</div>
	</div>
</nav>

<style>
	.sidebar {
		position: fixed;
		top: 0;
		left: 0;
		bottom: 0;
		width: var(--sidebar-w);
		background: var(--sidebar);
		border-right: 1px solid var(--border);
		display: flex;
		flex-direction: column;
		z-index: 40;
	}
	.brand {
		display: flex;
		align-items: center;
		gap: 12px;
		padding: 20px 18px 16px;
	}
	.name {
		font-weight: 650;
		letter-spacing: -0.01em;
		font-size: 14.5px;
	}
	.sub {
		display: flex;
		align-items: center;
		gap: 6px;
		color: var(--text-3);
		font-size: 12px;
	}
	.dot {
		width: 7px;
		height: 7px;
		border-radius: 50%;
		background: var(--text-3);
	}
	.dot.ok {
		background: var(--green);
		box-shadow: 0 0 8px var(--green);
	}
	.dot.bad {
		background: var(--red);
	}
	.demo {
		margin: 0 14px 8px;
		padding: 7px 10px;
		border-radius: 8px;
		font-size: 11.5px;
		font-weight: 550;
		display: flex;
		align-items: center;
		gap: 6px;
		color: var(--purple);
		background: var(--purple-soft);
	}
	.exit {
		font-size: 11px;
		font-weight: 600;
		padding: 2px 8px;
		border-radius: 6px;
		color: var(--purple);
		background: var(--surface);
		box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--purple) 35%, transparent);
	}
	.exit:hover {
		background: var(--purple);
		color: #fff;
	}
	.scroll {
		flex: 1;
		overflow-y: auto;
		padding: 4px 10px;
	}
	.group {
		margin-bottom: 14px;
	}
	.glabel {
		font-size: 11px;
		font-weight: 600;
		letter-spacing: 0.06em;
		text-transform: uppercase;
		color: var(--text-3);
		padding: 8px 10px 6px;
	}
	.item {
		position: relative;
		display: flex;
		align-items: center;
		gap: 12px;
		height: 38px;
		padding: 0 10px;
		border-radius: 9px;
		color: var(--text-2);
		font-weight: 520;
		font-size: 13.5px;
		transition:
			background 150ms var(--ease),
			color 150ms var(--ease);
	}
	.item:hover {
		background: var(--surface-2);
		color: var(--text);
	}
	.item.active {
		background: var(--surface-3);
		color: var(--text);
	}
	/* On the pale sidebar a white pill with a hairline reads as "you are here". */
	:global([data-theme='light']) .item.active {
		background: var(--surface);
		box-shadow:
			var(--shadow-1),
			inset 0 0 0 1px var(--border-2);
	}
	.item.active::before {
		content: '';
		position: absolute;
		left: -10px;
		top: 9px;
		bottom: 9px;
		width: 3px;
		border-radius: 0 3px 3px 0;
		background: var(--accent);
	}
	.item.active :global(svg) {
		color: var(--accent-text);
	}
	.foot {
		padding: 10px;
		border-top: 1px solid var(--border);
		padding-bottom: calc(var(--transport-h) + 10px);
	}
	.tools {
		display: flex;
		align-items: center;
		gap: 2px;
		padding: 6px 2px 0;
	}
	.customize {
		display: flex;
		align-items: center;
		gap: 8px;
		height: 30px;
		padding: 0 12px;
		margin-top: 2px;
		border-radius: 8px;
		font-size: 12.5px;
		color: var(--text-3);
		font-weight: 520;
	}
	.customize:hover {
		background: var(--surface-2);
	}
	.customize:hover,
	.customize[aria-current='page'] {
		color: var(--text);
	}
	.ver {
		margin-left: auto;
		padding-right: 8px;
	}
</style>
