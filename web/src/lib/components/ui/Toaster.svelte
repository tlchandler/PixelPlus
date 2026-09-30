<script lang="ts">
	import { toasts } from '$lib/stores/toasts.svelte';
	import { CircleCheck, CircleAlert, Info, TriangleAlert, X } from '@lucide/svelte';
	import { fly } from 'svelte/transition';
	import { flip } from 'svelte/animate';

	const icons = { success: CircleCheck, error: CircleAlert, info: Info, warning: TriangleAlert };
</script>

<div class="toaster" aria-live="polite" aria-relevant="additions">
	{#each toasts.items as t (t.id)}
		{@const Icon = icons[t.kind]}
		<div
			class="toast {t.kind}"
			role={t.kind === 'error' ? 'alert' : 'status'}
			in:fly={{ y: 16, duration: 220 }}
			out:fly={{ x: 40, duration: 180 }}
			animate:flip={{ duration: 200 }}
		>
			<span class="ico"><Icon size={18} /></span>
			<div class="grow">
				<div class="msg">{t.message}</div>
				{#if t.detail}<div class="detail">{t.detail}</div>{/if}
			</div>
			{#if t.action}
				<button
					class="btn sm soft"
					onclick={async () => {
						toasts.dismiss(t.id);
						await t.action?.run();
					}}>{t.action.label}</button
				>
			{/if}
			<button class="x" aria-label="Dismiss" onclick={() => toasts.dismiss(t.id)}><X size={14} /></button>
		</div>
	{/each}
</div>

<style>
	.toaster {
		position: fixed;
		right: 20px;
		bottom: calc(var(--transport-h) + 20px);
		display: flex;
		flex-direction: column;
		gap: 8px;
		z-index: 120;
		width: min(400px, calc(100vw - 32px));
		pointer-events: none;
	}
	.toast {
		pointer-events: auto;
		display: flex;
		align-items: center;
		gap: 12px;
		padding: 12px 12px 12px 14px;
		border-radius: 14px;
		background: color-mix(in srgb, var(--surface-2) 92%, transparent);
		backdrop-filter: blur(16px) saturate(1.4);
		-webkit-backdrop-filter: blur(16px) saturate(1.4);
		border: 1px solid var(--border-2);
		box-shadow: var(--shadow-2);
		font-size: 13.5px;
	}
	.ico {
		display: flex;
		color: var(--blue);
	}
	.success .ico {
		color: var(--green);
	}
	.error .ico {
		color: var(--red);
	}
	.warning .ico {
		color: var(--accent);
	}
	.msg {
		font-weight: 520;
	}
	.detail {
		color: var(--text-2);
		font-size: 12.5px;
		margin-top: 2px;
	}
	.x {
		color: var(--text-3);
		display: grid;
		place-items: center;
		width: 24px;
		height: 24px;
		border-radius: 6px;
	}
	.x:hover {
		color: var(--text);
		background: var(--surface-3);
	}
	@media (max-width: 760px) {
		.toaster {
			right: 16px;
			left: 16px;
			width: auto;
			bottom: calc(var(--tabbar-h) + 76px + env(safe-area-inset-bottom));
		}
	}
</style>
