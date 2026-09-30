<script lang="ts">
	import { onDestroy, onMount } from 'svelte';
	import { api } from '../lib/api';
	import type { Drive, ImageChoice } from '../lib/types';

	let { drive = $bindable(), image }: { drive: Drive | null; image: ImageChoice | null } = $props();

	let drives = $state<Drive[] | null>(null);
	let error = $state<string | null>(null);
	let timer: ReturnType<typeof setInterval> | undefined;

	async function refresh() {
		try {
			const list = await api.listDrives();
			drives = list;
			error = null;
			// drop the selection if the card was removed
			if (drive && !list.some((d) => d.device === drive!.device)) drive = null;
			if (!drive) {
				const usable = list.filter((d) => !d.tooSmall);
				if (usable.length === 1) drive = usable[0];
			}
		} catch (e) {
			error = String(e);
			drives = [];
		}
	}

	onMount(() => {
		refresh();
		timer = setInterval(refresh, 2500); // cards get plugged in while this screen is open
	});
	onDestroy(() => clearInterval(timer));

	const needed = $derived(image?.kind === 'release' ? image.image.extractSize : null);
</script>

<section>
	<h2>Choose the SD card</h2>
	<p class="hint">
		Insert the microSD card (4 GB or larger) into your computer. <strong>Everything on it will be erased.</strong>
		Only removable drives are shown - never your computer's own disks.
	</p>

	<div class="list">
		{#if drives === null}
			<div class="skeleton"></div>
		{:else if drives.length === 0}
			<div class="card empty">
				<svg width="40" height="40" viewBox="0 0 24 24" aria-hidden="true"
					><path fill="var(--text-3)" d="M7 2h8l4 4v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2Zm2 2v4h1.5V4H9Zm3 0v4h1.5V4H12Zm3 .5V8h1.5V6.1L15 4.5Z" /></svg
				>
				<strong>No SD card found</strong>
				<span class="hint">Insert a card or USB card reader - it will show up here automatically.</span>
				{#if error}<span class="err">{error}</span>{/if}
			</div>
		{:else}
			{#each drives as d (d.device)}
				<button
					class="option"
					aria-pressed={drive?.device === d.device}
					disabled={d.tooSmall || (!!needed && d.size < needed)}
					onclick={() => (drive = d)}
				>
					<svg width="28" height="28" viewBox="0 0 24 24" aria-hidden="true"
						><path fill="var(--accent)" d="M7 2h8l4 4v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2Zm2 2v4h1.5V4H9Zm3 0v4h1.5V4H12Zm3 .5V8h1.5V6.1L15 4.5Z" /></svg
					>
					<div class="grow">
						<strong>{d.name}</strong>
						<div class="hint">
							{d.device}{d.mountpoints.length ? ` · ${d.mountpoints.join(', ')}` : ''}
							{#if d.tooSmall || (needed && d.size < needed)}<span class="err"> · too small</span>{/if}
						</div>
					</div>
				</button>
			{/each}
		{/if}
		<button class="linkish" onclick={refresh}>Refresh</button>
	</div>
</section>

<style>
	h2 {
		margin: 0 0 4px;
		font-size: 20px;
	}
	.list {
		display: grid;
		gap: 10px;
		margin-top: 16px;
	}
	.grow {
		flex: 1;
		display: grid;
		gap: 2px;
	}
	.empty {
		display: grid;
		justify-items: center;
		gap: 6px;
		padding: 32px 16px;
		text-align: center;
	}
	.linkish {
		justify-self: start;
		background: none;
		border: 0;
		color: var(--accent);
		cursor: pointer;
		padding: 4px 0;
	}
</style>
