<script lang="ts">
	import { dialogs } from '$lib/stores/toasts.svelte';
	import Modal from './Modal.svelte';

	let open = $derived(!!dialogs.current);
</script>

{#if dialogs.current}
	{@const c = dialogs.current}
	<Modal {open} title={c.title} size="sm" onclose={() => dialogs.close(false)}>
		{#if c.message}<p class="muted">{c.message}</p>{/if}
		{#snippet footer()}
			<button class="btn ghost" onclick={() => dialogs.close(false)}>{c.cancelLabel ?? 'Cancel'}</button>
			<button class="btn {c.danger ? 'danger' : 'primary'}" data-autofocus onclick={() => dialogs.close(true)}
				>{c.confirmLabel ?? 'Confirm'}</button
			>
		{/snippet}
	</Modal>
{/if}
