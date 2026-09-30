<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { app } from '$lib/stores/app.svelte';
	import Sidebar from '$lib/components/shell/Sidebar.svelte';
	import TabBar from '$lib/components/shell/TabBar.svelte';
	import TransportBar from '$lib/components/shell/TransportBar.svelte';
	import Logo from '$lib/components/shell/Logo.svelte';
	import Login from '$lib/components/shell/Login.svelte';
	import FollowerScreen from '$lib/components/shell/FollowerScreen.svelte';
	import Shortcuts from '$lib/components/shell/Shortcuts.svelte';
	import { NAV } from '$lib/components/shell/nav';
	import { togglePlay, setLightsOff } from '$lib/player';
	import { FlaskConical, Power, X } from '@lucide/svelte';

	let { children } = $props();
	let shortcuts = $state(false);
	let gPressed = false;
	let gTimer: ReturnType<typeof setTimeout>;
	let bannerHidden = $state(false);

	$effect(() => {
		if (app.ready && app.system?.needsSetup) goto('/setup', { replaceState: true });
	});

	function typing(e: KeyboardEvent) {
		const t = e.target as HTMLElement | null;
		if (!t) return false;
		return t.isContentEditable || ['INPUT', 'TEXTAREA', 'SELECT'].includes(t.tagName);
	}

	function onkey(e: KeyboardEvent) {
		if (e.metaKey || e.ctrlKey || e.altKey || typing(e)) return;
		if (document.querySelector('[aria-modal="true"]')) return;
		if (e.key === ' ' && !(e.target as HTMLElement)?.closest('button, a, [role="button"], [role="switch"]')) {
			e.preventDefault();
			togglePlay();
		} else if (e.key === '/') {
			const el = document.querySelector<HTMLInputElement>('[data-search]');
			if (el) {
				e.preventDefault();
				el.focus();
				el.select();
			}
		} else if (e.key === '?') {
			shortcuts = true;
		} else if (e.key === 'B' && e.shiftKey) {
			// Shift+B, not a bare "b": one stray key press shouldn't turn the whole show dark.
			setLightsOff(!app.status?.blackout);
		} else if (e.key === 'g') {
			gPressed = true;
			clearTimeout(gTimer);
			gTimer = setTimeout(() => (gPressed = false), 1200);
		} else if (gPressed) {
			const n = NAV.find((x) => x.key === e.key);
			gPressed = false;
			if (n) goto(n.href);
		}
	}
</script>

<svelte:window onkeydown={onkey} />

{#if !app.ready || (app.booting && !app.fatal)}
	<div class="splash" aria-busy="true" aria-label="Loading PixelPlus">
		<div class="pulse"><Logo size={56} /></div>
	</div>
{:else if app.fatal}
	<div class="splash">
		<div class="fatal card card-pad">
			<Logo size={40} />
			<h2>Can’t reach this PixelPlus controller</h2>
			<p class="muted">{app.fatal}</p>
			<div class="row">
				<button class="btn primary" onclick={() => location.reload()}>Try again</button>
				<a class="btn" href="?mock=1">Open the demo instead</a>
			</div>
		</div>
	</div>
{:else if app.needsLogin}
	<Login />
{:else if app.system?.role === 'follower'}
	<FollowerScreen />
{:else if app.system?.needsSetup}
	<div class="splash"><div class="pulse"><Logo size={56} /></div></div>
{:else}
	<div class="shell">
		<div class="desk"><Sidebar onshortcuts={() => (shortcuts = true)} /></div>
		<main class="main" id="main">
			{#if app.mock && app.mockAuto && !bannerHidden}
				<div class="banner">
					<FlaskConical size={15} />
					<span class="grow"
						>Couldn’t reach pixelplusd, so you’re looking at the <strong>demo show</strong>. Changes stay in
						this browser tab.</span
					>
					<button class="btn ghost icon sm" aria-label="Hide" onclick={() => (bannerHidden = true)}
						><X size={15} /></button
					>
				</div>
			{/if}
			{#if app.status?.blackout}
				<div class="lights-off" role="status">
					<span class="lo-ic"><Power size={18} /></span>
					<div class="grow">
						<strong>Lights are off</strong>
						<span class="lo-sub">Every light is dark until you turn them back on.</span>
					</div>
					<button class="btn lo-btn" onclick={() => setLightsOff(false)}>Turn lights back on</button>
				</div>
			{/if}
			{#key page.url.pathname}
				{@render children()}
			{/key}
		</main>
		<TransportBar />
		<div class="mob"><TabBar /></div>
	</div>
{/if}

<Shortcuts bind:open={shortcuts} />

<style>
	.splash {
		min-height: 100dvh;
		display: grid;
		place-items: center;
		padding: 24px;
	}
	.pulse {
		animation: breathe 1.6s ease-in-out infinite;
		filter: drop-shadow(0 8px 30px rgba(245, 165, 36, 0.35));
	}
	@keyframes breathe {
		0%,
		100% {
			transform: scale(0.94);
			opacity: 0.7;
		}
		50% {
			transform: scale(1);
			opacity: 1;
		}
	}
	.fatal {
		max-width: 440px;
		display: flex;
		flex-direction: column;
		gap: 12px;
		align-items: flex-start;
	}
	.main {
		margin-left: var(--sidebar-w);
		min-height: 100dvh;
		padding-bottom: var(--transport-h);
		min-width: 0;
	}
	.mob {
		display: none;
	}
	.banner {
		display: flex;
		align-items: center;
		gap: 10px;
		margin: 16px 32px 0;
		padding: 8px 8px 8px 14px;
		border-radius: 12px;
		background: var(--purple-soft);
		color: var(--text);
		font-size: 13px;
	}
	.banner :global(svg) {
		color: var(--purple);
	}
	.lights-off {
		position: sticky;
		top: 0;
		z-index: 30;
		display: flex;
		align-items: center;
		gap: 12px;
		margin: 16px 32px 0;
		padding: 10px 10px 10px 14px;
		border-radius: 14px;
		background: #c9302c;
		color: #fff;
		box-shadow: 0 8px 28px rgba(201, 48, 44, 0.35);
		animation: page-in 220ms var(--ease);
	}
	.lo-ic {
		display: grid;
		place-items: center;
		width: 36px;
		height: 36px;
		border-radius: 10px;
		background: rgba(255, 255, 255, 0.16);
		flex: 0 0 auto;
	}
	.lights-off strong {
		display: block;
		font-size: 14px;
	}
	.lo-sub {
		font-size: 12.5px;
		opacity: 0.9;
	}
	.lo-btn {
		background: #fff;
		color: #8f1d1a;
		border-color: transparent;
		font-weight: 650;
	}
	.lo-btn:hover {
		background: #ffe9e8;
		border-color: transparent;
	}
	@media (max-width: 760px) {
		.desk {
			display: none;
		}
		.mob {
			display: block;
		}
		.main {
			margin-left: 0;
			padding-bottom: calc(var(--tabbar-h) + 76px + env(safe-area-inset-bottom));
		}
		.banner {
			margin: 12px 16px 0;
		}
		.lights-off {
			margin: 8px 8px 0;
			flex-wrap: wrap;
		}
		.lights-off .lo-btn {
			width: 100%;
		}
	}
</style>
