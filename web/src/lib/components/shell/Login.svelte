<script lang="ts">
	import { api } from '$lib/api/client';
	import { app } from '$lib/stores/app.svelte';
	import Logo from './Logo.svelte';
	import { LockKeyhole } from '@lucide/svelte';

	let password = $state('');
	let error = $state('');
	let busy = $state(false);

	async function submit(e: SubmitEvent) {
		e.preventDefault();
		busy = true;
		error = '';
		try {
			await api.login(password);
			await app.afterLogin();
		} catch (err) {
			error = err instanceof Error ? err.message : 'Could not sign in';
		} finally {
			busy = false;
		}
	}
</script>

<div class="wrap">
	<form class="card card-pad login" onsubmit={submit}>
		<Logo size={48} />
		<h1>Welcome back</h1>
		<p class="muted">Enter the password for this PixelPlus show.</p>
		<div class="input-group">
			<span class="prefix"><LockKeyhole size={16} /></span>
			<input
				class="input"
				type="password"
				autocomplete="current-password"
				placeholder="Password"
				aria-label="Password"
				bind:value={password}
			/>
		</div>
		{#if error}<p class="err small">{error}</p>{/if}
		<button class="btn primary lg block" disabled={busy || !password}
			>{busy ? 'Signing in…' : 'Sign in'}</button
		>
	</form>
</div>

<style>
	.wrap {
		min-height: 100dvh;
		display: grid;
		place-items: center;
		padding: 24px;
	}
	.login {
		width: min(400px, 100%);
		display: flex;
		flex-direction: column;
		gap: 14px;
		align-items: stretch;
	}
	.login h1 {
		margin-top: 8px;
	}
	.err {
		color: var(--red);
	}
</style>
