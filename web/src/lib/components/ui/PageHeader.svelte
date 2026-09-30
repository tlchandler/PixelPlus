<script lang="ts">
	import type { Snippet } from 'svelte';
	let {
		title,
		subtitle,
		actions,
		eyebrow
	}: { title: string; subtitle?: string; actions?: Snippet; eyebrow?: Snippet } = $props();
</script>

<header class="ph">
	<div class="grow">
		{#if eyebrow}<div class="eb">{@render eyebrow()}</div>{/if}
		<h1>{title}</h1>
		{#if subtitle}<p class="muted">{subtitle}</p>{/if}
	</div>
	{#if actions}<div class="actions">{@render actions()}</div>{/if}
</header>

<style>
	.ph {
		display: flex;
		align-items: flex-end;
		gap: 16px;
		margin-bottom: 24px;
		flex-wrap: wrap;
	}
	.ph p {
		margin-top: 4px;
		font-size: 14px;
	}
	.eb {
		margin-bottom: 6px;
	}
	.actions {
		display: flex;
		gap: 8px;
		flex-wrap: wrap;
	}
	@media (max-width: 760px) {
		.ph {
			margin-bottom: 16px;
			align-items: flex-start;
		}
		.ph p {
			font-size: 13px;
		}
		.actions {
			width: 100%;
		}
	}
</style>
