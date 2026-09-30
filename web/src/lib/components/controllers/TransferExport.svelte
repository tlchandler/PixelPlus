<!--
	F10 (WS5): download the passphrase-encrypted controller transfer file (.ppxfer): the whole
	show, its sequences and media, the cluster keys and the HTTPS certificate authority. Used by
	Settings → Updates (Controller transfer file) and by "Replace…" on the show leader's card.
-->
<script lang="ts">
	import { Download, Eye, EyeOff, KeyRound, TriangleAlert } from '@lucide/svelte';
	import { fleetApi, MIN_PASSPHRASE } from '$lib/api/fleet';
	import { toasts } from '$lib/stores/toasts.svelte';

	let { compact = false }: { compact?: boolean } = $props();

	let pass = $state('');
	let again = $state('');
	let show = $state(false);
	let busy = $state(false);

	const tooShort = $derived(pass.length > 0 && [...pass].length < MIN_PASSPHRASE);
	const mismatch = $derived(again.length > 0 && again !== pass);
	const ok = $derived([...pass].length >= MIN_PASSPHRASE && again === pass);

	async function download() {
		busy = true;
		try {
			const { url } = await fleetApi.transferExport(pass);
			// A plain navigation: the browser saves it like any download (no page memory).
			const a = document.createElement('a');
			a.href = url;
			a.rel = 'noopener';
			document.body.appendChild(a);
			a.click();
			a.remove();
			toasts.success('Transfer file download started — keep it and the passphrase somewhere safe');
			pass = again = '';
		} catch (e) {
			toasts.error("Couldn't make the transfer file", (e as Error).message);
		} finally {
			busy = false;
		}
	}
</script>

<div class="xfer" class:compact>
	{#if !compact}
		<p class="muted small">
			If this controller ever dies, set up a fresh SD card with <strong>Restore a show</strong> and this file: the
			new one takes over its show, sequences, music, followers and phone certificates in one step.
		</p>
	{/if}
	<div class="notice warn small">
		<TriangleAlert size={16} />
		<div>
			The file holds everything, including passwords and keys. It is encrypted with your passphrase; without
			it, nobody (not even you) can open it. Download it again after big changes.
		</div>
	</div>
	<form
		class="grid"
		onsubmit={(e) => {
			e.preventDefault();
			if (ok) void download();
		}}
	>
		<label class="field">
			<span class="label"><KeyRound size={14} /> Passphrase</span>
			<div class="pw">
				<input
					class="input"
					type={show ? 'text' : 'password'}
					autocomplete="new-password"
					bind:value={pass}
					aria-invalid={tooShort}
					placeholder="At least {MIN_PASSPHRASE} characters"
				/>
				<button
					type="button"
					class="btn icon ghost sm"
					aria-label={show ? 'Hide passphrase' : 'Show passphrase'}
					onclick={() => (show = !show)}
					>{#if show}<EyeOff size={16} />{:else}<Eye size={16} />{/if}</button
				>
			</div>
			{#if tooShort}<span class="hint bad">Use at least {MIN_PASSPHRASE} characters.</span>{/if}
		</label>
		<label class="field">
			<span class="label">Passphrase again</span>
			<input
				class="input"
				type={show ? 'text' : 'password'}
				autocomplete="new-password"
				bind:value={again}
				aria-invalid={mismatch}
			/>
			{#if mismatch}<span class="hint bad">The two don't match.</span>{/if}
		</label>
		<div class="actions">
			<button class="btn primary" type="submit" disabled={!ok || busy}>
				<Download size={16} />
				{busy ? 'Preparing…' : 'Download transfer file'}
			</button>
		</div>
	</form>
</div>

<style>
	.xfer {
		display: flex;
		flex-direction: column;
		gap: 12px;
	}
	.xfer p {
		margin: 0;
	}
	.grid {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: 12px;
		align-items: start;
	}
	.actions {
		grid-column: 1 / -1;
	}
	.pw {
		display: flex;
		gap: 6px;
		align-items: center;
	}
	.pw .input {
		flex: 1 1 auto;
		min-width: 0;
	}
	.bad {
		color: var(--red);
	}
	@media (max-width: 640px) {
		.grid {
			grid-template-columns: 1fr;
		}
	}
</style>
