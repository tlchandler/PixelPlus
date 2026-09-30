<script lang="ts">
	import { onMount } from 'svelte';
	import { api } from '../lib/api';
	import type { ImageChoice, OsImage } from '../lib/types';
	import { formatBytes } from '../lib/validate';

	let { image = $bindable() }: { image: ImageChoice | null } = $props();

	let releases = $state<OsImage[] | null>(null);
	let error = $state<string | null>(null);
	let showAll = $state(false);

	async function load() {
		error = null;
		releases = null;
		try {
			releases = await api.releases();
			if (!image) {
				const rec = releases.find((r) => r.recommended && !r.prerelease) ?? releases[0];
				if (rec) image = { kind: 'release', image: rec };
			}
		} catch (e) {
			error = String(e);
			releases = [];
		}
	}
	onMount(load);

	async function pickFile() {
		const f = await api.pickImageFile();
		if (f) image = { kind: 'file', ...f };
	}

	const visible = $derived(
		(releases ?? []).filter((r, i) => showAll || i === 0 || (r.recommended !== releases?.[0]?.recommended && r.version === releases?.[0]?.version))
	);
	const selected = (r: OsImage) => image?.kind === 'release' && image.image.url === r.url;
</script>

<section>
	<h2>Choose the PixelPlus version</h2>
	<p class="hint">The newest release is selected for you. It works on Raspberry Pi Zero 2 W, 3, 4 and 5.</p>

	<div class="list">
		{#if releases === null}
			<div class="skeleton"></div>
			<div class="skeleton"></div>
		{:else}
			{#each visible as r (r.url)}
				<button class="option" aria-pressed={selected(r)} onclick={() => (image = { kind: 'release', image: r })}>
					<div class="grow">
						<div class="row">
							<strong>{r.name}</strong>
							{#if r.recommended && !r.prerelease}<span class="badge">Recommended</span>{/if}
							{#if r.prerelease}<span class="badge muted">Beta</span>{/if}
						</div>
						<div class="hint">{r.releaseDate} · {formatBytes(r.downloadSize)} download</div>
					</div>
				</button>
			{/each}
			{#if error}
				<div class="card notice">
					<strong>Couldn't check for releases.</strong>
					<span class="hint">Are you online? You can still use an image file you downloaded.</span>
					<button class="btn" onclick={load}>Try again</button>
				</div>
			{/if}
			{#if (releases?.length ?? 0) > visible.length}
				<button class="linkish" onclick={() => (showAll = true)}>Show older versions</button>
			{/if}
		{/if}

		<button class="option" aria-pressed={image?.kind === 'file'} onclick={pickFile}>
			<div class="grow">
				<strong>Use an image file on this computer…</strong>
				<div class="hint">
					{#if image?.kind === 'file'}{image.name} · {formatBytes(image.size)}{:else}.img or .img.xz{/if}
				</div>
			</div>
		</button>
	</div>
</section>

<style>
	h2 {
		margin: 0 0 4px;
		font-size: 20px;
		letter-spacing: -0.01em;
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
	.notice {
		display: grid;
		gap: 6px;
		padding: 14px 16px;
		justify-items: start;
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
