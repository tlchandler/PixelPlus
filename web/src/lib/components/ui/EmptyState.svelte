<script lang="ts">
	import type { Component, Snippet } from 'svelte';
	let {
		icon: Icon,
		title,
		message,
		children,
		compact = false
	}: {
		icon?: Component<any>;
		title: string;
		message?: string;
		children?: Snippet;
		compact?: boolean;
	} = $props();
</script>

<div class="empty" class:compact>
	{#if Icon}
		<div class="halo"><Icon size={compact ? 20 : 26} strokeWidth={1.6} /></div>
	{/if}
	<h3>{title}</h3>
	{#if message}<p>{message}</p>{/if}
	{#if children}<div class="actions">{@render children()}</div>{/if}
</div>

<style>
	.empty {
		display: flex;
		flex-direction: column;
		align-items: center;
		text-align: center;
		padding: 56px 24px;
		gap: 8px;
	}
	.empty.compact {
		padding: 28px 16px;
	}
	.halo {
		width: 60px;
		height: 60px;
		border-radius: 18px;
		display: grid;
		place-items: center;
		color: var(--accent);
		background: radial-gradient(circle at 50% 30%, var(--accent-soft), transparent 70%), var(--surface-2);
		border: 1px solid var(--border-2);
		margin-bottom: 8px;
	}
	.compact .halo {
		width: 44px;
		height: 44px;
		border-radius: 14px;
	}
	h3 {
		font-size: 15px;
	}
	p {
		color: var(--text-2);
		max-width: 420px;
		font-size: 13.5px;
	}
	.actions {
		display: flex;
		gap: 8px;
		margin-top: 12px;
		flex-wrap: wrap;
		justify-content: center;
	}
</style>
