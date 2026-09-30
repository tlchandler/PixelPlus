<script lang="ts">
	import { app } from '$lib/stores/app.svelte';
	import { BOARDS } from '$lib/util/boards';
	import Logo from './Logo.svelte';
	import { Radio, CircleCheck } from '@lucide/svelte';

	const sys = $derived(app.system);
	const adopted = $derived(!!sys?.leaderName);
</script>

<div class="wrap">
	<div class="card follower">
		<Logo size={48} />
		{#if adopted}
			<div class="state ok"><CircleCheck size={16} /> Part of a show</div>
			<h1>This controller follows <em>{sys?.leaderName}</em></h1>
			<p class="muted">
				Everything is configured on the show leader. This controller receives its settings, sequences and
				commands automatically — there is nothing to set up here.
			</p>
		{:else}
			<div class="state"><span class="radar"><Radio size={16} /></span> Waiting to be adopted…</div>
			<h1>Ready to join a show</h1>
			<p class="muted">
				Open PixelPlus on your show leader and go to <strong>Controllers → New controllers found</strong>.
				This controller will appear there as:
			</p>
		{/if}
		<dl>
			<div>
				<dt>Name</dt>
				<dd>{sys?.hostname}</dd>
			</div>
			<div>
				<dt>Address</dt>
				<dd>{sys?.ips?.[0] ?? '—'}</dd>
			</div>
			<div>
				<dt>Board</dt>
				<dd>{sys?.board ? BOARDS[sys.board].name : 'Unknown'}</dd>
			</div>
			<div>
				<dt>Version</dt>
				<dd>{sys?.version}</dd>
			</div>
		</dl>
	</div>
</div>

<style>
	.wrap {
		min-height: 100dvh;
		display: grid;
		place-items: center;
		padding: 24px;
	}
	.follower {
		width: min(520px, 100%);
		padding: 36px;
		display: flex;
		flex-direction: column;
		gap: 14px;
	}
	h1 em {
		font-style: normal;
		color: var(--accent);
	}
	.state {
		display: inline-flex;
		align-items: center;
		gap: 8px;
		color: var(--blue);
		font-weight: 560;
		font-size: 13px;
	}
	.state.ok {
		color: var(--green);
	}
	.radar {
		display: grid;
		place-items: center;
		animation: pulse 1.8s infinite;
		border-radius: 50%;
	}
	dl {
		margin: 8px 0 0;
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: 12px;
		padding: 16px;
		border-radius: 12px;
		background: var(--surface-2);
	}
	dt {
		font-size: 12px;
		color: var(--text-3);
	}
	dd {
		margin: 0;
		font-weight: 560;
	}
</style>
