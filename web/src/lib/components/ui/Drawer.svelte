<script lang="ts">
	import type { Snippet } from 'svelte';
	import { X } from '@lucide/svelte';
	import { fade, fly } from 'svelte/transition';
	import { cubicOut } from 'svelte/easing';

	let {
		open = $bindable(false),
		title,
		width = 560,
		onclose,
		header,
		children,
		footer
	}: {
		open?: boolean;
		title?: string;
		width?: number;
		onclose?: () => void;
		header?: Snippet;
		children: Snippet;
		footer?: Snippet;
	} = $props();

	let mobile = $state(false);
	$effect(() => {
		const mq = window.matchMedia('(max-width: 760px)');
		mobile = mq.matches;
		const fn = () => (mobile = mq.matches);
		mq.addEventListener('change', fn);
		return () => mq.removeEventListener('change', fn);
	});

	function close() {
		open = false;
		onclose?.();
	}
</script>

<svelte:window onkeydown={(e) => open && e.key === 'Escape' && close()} />

{#if open}
	<div class="backdrop" transition:fade={{ duration: 180 }} onclick={close} aria-hidden="true"></div>
	<div
		class="drawer"
		style:--w="{width}px"
		role="dialog"
		aria-modal="true"
		aria-label={title}
		transition:fly={mobile ? { y: 500, duration: 260, easing: cubicOut, opacity: 1 } : { x: width, duration: 260, easing: cubicOut, opacity: 1 }}
	>
		<header>
			<div class="grow">
				{#if header}{@render header()}{:else}<h2>{title}</h2>{/if}
			</div>
			<button class="btn ghost icon sm" onclick={close} aria-label="Close panel"><X size={18} /></button>
		</header>
		<div class="body">{@render children()}</div>
		{#if footer}<footer>{@render footer()}</footer>{/if}
	</div>
{/if}

<style>
	.backdrop {
		position: fixed;
		inset: 0;
		background: var(--overlay);
		z-index: 80;
	}
	.drawer {
		position: fixed;
		top: 8px;
		right: 8px;
		bottom: 8px;
		width: min(var(--w), calc(100vw - 16px));
		background: var(--surface);
		border: 1px solid var(--border-2);
		border-radius: var(--r-4);
		box-shadow: var(--shadow-3);
		z-index: 81;
		display: flex;
		flex-direction: column;
		overflow: hidden;
	}
	header {
		display: flex;
		align-items: flex-start;
		gap: 12px;
		padding: 18px 16px 14px 24px;
		border-bottom: 1px solid var(--border);
	}
	.body {
		flex: 1;
		overflow: auto;
		padding: 20px 24px 32px;
		overscroll-behavior: contain;
	}
	footer {
		display: flex;
		gap: 8px;
		padding: 14px 20px;
		border-top: 1px solid var(--border);
		background: var(--surface-2);
	}
	@media (max-width: 760px) {
		.drawer {
			top: auto;
			left: 0;
			right: 0;
			bottom: 0;
			width: 100%;
			height: 92dvh;
			border-radius: var(--r-4) var(--r-4) 0 0;
			padding-bottom: env(safe-area-inset-bottom);
		}
		.body {
			padding: 16px;
		}
		header {
			padding: 14px 12px 12px 16px;
		}
	}
</style>
