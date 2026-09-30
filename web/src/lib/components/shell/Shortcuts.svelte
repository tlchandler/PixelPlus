<script lang="ts">
	import Modal from '$lib/components/ui/Modal.svelte';
	import { NAV } from './nav';
	let { open = $bindable(false) }: { open?: boolean } = $props();
	const general = [
		['Space', 'Play / pause the show'],
		['/', 'Search on this page'],
		['Shift B', 'Lights off / back on'],
		['?', 'Show this help'],
		['Esc', 'Close panels and dialogs']
	];
</script>

<Modal bind:open title="Keyboard shortcuts" size="md">
	<div class="cols">
		<section>
			<h3 class="eyebrow">General</h3>
			{#each general as [k, d] (k)}
				<div class="sc"><span>{d}</span><span class="kbd">{k}</span></div>
			{/each}
		</section>
		<section>
			<h3 class="eyebrow">Go to</h3>
			{#each NAV as n (n.href)}
				<div class="sc">
					<span>{n.label}</span><span class="keys"
						><span class="kbd">G</span><span class="kbd">{n.key?.toUpperCase()}</span></span
					>
				</div>
			{/each}
		</section>
	</div>
</Modal>

<style>
	.cols {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: 24px;
	}
	@media (max-width: 640px) {
		.cols {
			grid-template-columns: 1fr;
		}
	}
	h3 {
		margin-bottom: 8px;
	}
	.sc {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 7px 0;
		border-bottom: 1px solid var(--border);
		font-size: 13px;
		color: var(--text-2);
	}
	.keys {
		display: flex;
		gap: 4px;
	}
</style>
