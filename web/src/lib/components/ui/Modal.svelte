<script lang="ts">
	import type { Snippet } from 'svelte';
	import { X } from '@lucide/svelte';
	import { fade, scale } from 'svelte/transition';
	import { cubicOut } from 'svelte/easing';

	let {
		open = $bindable(false),
		title,
		subtitle,
		size = 'md',
		onclose,
		children,
		footer,
		dismissable = true
	}: {
		open?: boolean;
		title?: string;
		subtitle?: string;
		size?: 'sm' | 'md' | 'lg' | 'xl';
		onclose?: () => void;
		children: Snippet;
		footer?: Snippet;
		dismissable?: boolean;
	} = $props();

	let panel: HTMLDivElement | undefined = $state();
	let prevFocus: Element | null = null;

	function close() {
		if (!dismissable) return;
		open = false;
		onclose?.();
	}

	$effect(() => {
		if (open) {
			prevFocus = document.activeElement;
			queueMicrotask(() => {
				const f =
					panel?.querySelector<HTMLElement>('[data-autofocus]') ??
					panel?.querySelector<HTMLElement>('input, select, textarea, button:not(.modal-x)');
				(f ?? panel)?.focus();
			});
			document.body.style.overflow = 'hidden';
			return () => {
				document.body.style.overflow = '';
				(prevFocus as HTMLElement | null)?.focus?.();
			};
		}
	});

	function onkey(e: KeyboardEvent) {
		if (!open) return;
		if (e.key === 'Escape') {
			e.stopPropagation();
			close();
		}
		if (e.key === 'Tab' && panel) {
			const els = [...panel.querySelectorAll<HTMLElement>('button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])')].filter(
				(el) => !el.hasAttribute('disabled')
			);
			if (!els.length) return;
			const first = els[0],
				last = els[els.length - 1];
			if (e.shiftKey && document.activeElement === first) {
				last.focus();
				e.preventDefault();
			} else if (!e.shiftKey && document.activeElement === last) {
				first.focus();
				e.preventDefault();
			}
		}
	}
</script>

<svelte:window onkeydown={onkey} />

{#if open}
	<div class="backdrop" transition:fade={{ duration: 160 }} onclick={close} aria-hidden="true"></div>
	<div class="wrap" role="presentation">
		<div
			class="modal {size}"
			role="dialog"
			aria-modal="true"
			aria-label={title}
			tabindex="-1"
			bind:this={panel}
			transition:scale={{ start: 0.96, duration: 200, easing: cubicOut }}
		>
			{#if title}
				<header>
					<div class="grow">
						<h2>{title}</h2>
						{#if subtitle}<p class="muted small">{subtitle}</p>{/if}
					</div>
					{#if dismissable}
						<button class="btn ghost icon sm modal-x" onclick={close} aria-label="Close"><X size={18} /></button>
					{/if}
				</header>
			{/if}
			<div class="body">{@render children()}</div>
			{#if footer}<footer>{@render footer()}</footer>{/if}
		</div>
	</div>
{/if}

<style>
	.backdrop {
		position: fixed;
		inset: 0;
		background: var(--overlay);
		backdrop-filter: blur(6px);
		-webkit-backdrop-filter: blur(6px);
		z-index: 90;
	}
	.wrap {
		position: fixed;
		inset: 0;
		display: grid;
		place-items: center;
		padding: 24px;
		z-index: 91;
		pointer-events: none;
	}
	.modal {
		pointer-events: auto;
		width: 100%;
		max-height: calc(100dvh - 48px);
		display: flex;
		flex-direction: column;
		background: var(--surface);
		border: 1px solid var(--border-2);
		border-radius: var(--r-4);
		box-shadow: var(--shadow-3);
		overflow: hidden;
	}
	.sm {
		max-width: 420px;
	}
	.md {
		max-width: 560px;
	}
	.lg {
		max-width: 760px;
	}
	.xl {
		max-width: 1040px;
	}
	header {
		display: flex;
		align-items: flex-start;
		gap: 12px;
		padding: 20px 20px 4px 24px;
	}
	header h2 {
		font-size: 17px;
	}
	header p {
		margin-top: 2px;
	}
	.body {
		padding: 16px 24px 24px;
		overflow: auto;
	}
	footer {
		display: flex;
		justify-content: flex-end;
		gap: 8px;
		padding: 14px 20px;
		border-top: 1px solid var(--border);
		background: var(--surface-2);
	}
	@media (max-width: 640px) {
		.wrap {
			padding: 0;
			place-items: end stretch;
		}
		.modal {
			max-width: none;
			border-radius: var(--r-4) var(--r-4) 0 0;
			max-height: 92dvh;
			padding-bottom: env(safe-area-inset-bottom);
		}
		.body {
			padding: 12px 16px 20px;
		}
		header {
			padding: 18px 16px 4px 16px;
		}
		footer {
			padding: 12px 16px;
		}
		footer :global(.btn) {
			flex: 1;
		}
	}
</style>
