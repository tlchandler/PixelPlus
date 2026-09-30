<!--
	F10 (WS5): "Replace with…" for a dead controller (ARCHITECTURE §12.9).
	A follower: pick the freshly flashed controller that announces itself; it takes over the old
	one's id, name, wiring, props and sequences in one step (its key is revoked, a blank board
	EEPROM is written as the old board type). The show leader: download the transfer file, then
	choose "Restore a show" when setting up the new Pi.
	WS4 embeds it on the Controllers page:
		<ReplaceDialog bind:open={replacing} node={n} online={status.online} onreplaced={reload} />
-->
<script lang="ts">
	import { onDestroy } from 'svelte';
	import { ArrowRight, Cpu, RefreshCw, Replace, TriangleAlert } from '@lucide/svelte';
	import Modal from '$lib/components/ui/Modal.svelte';
	import TransferExport from './TransferExport.svelte';
	import { api, ApiError } from '$lib/api/client';
	import { fleetApi } from '$lib/api/fleet';
	import type { DiscoveredNode, Node } from '$lib/api/types';
	import { confirm, toasts } from '$lib/stores/toasts.svelte';

	let {
		open = $bindable(false),
		node,
		online = false,
		onreplaced
	}: {
		open?: boolean;
		node: Node;
		/** The old controller is online right now (replacing it then needs a confirmation). */
		online?: boolean;
		onreplaced?: (n: Node) => void;
	} = $props();

	let found = $state<DiscoveredNode[]>([]);
	let pick = $state<string | null>(null);
	let busy = $state(false);
	let timer: ReturnType<typeof setInterval> | undefined;

	const isLeader = $derived(node.role === 'leader');
	/** Fresh (unadopted) controllers only; not duplicates, retired ones or other shows' leaders. */
	const candidates = $derived(
		found.filter((d) => !d.adoptedBy && !d.duplicate && !d.retired && d.role !== 'leader')
	);
	const chosen = $derived(candidates.find((d) => d.id === pick) ?? null);
	const blankBoard = (b: string) => b === 'bare-pi' || b === 'virtual';
	const boardNote = $derived.by(() => {
		if (!chosen) return null;
		if (chosen.board === node.board) return null;
		if (blankBoard(chosen.board) && !blankBoard(node.board))
			return `Its board memory is blank: PixelPlus writes it as a ${node.board} board.`;
		return `The new controller is a ${chosen.board}; ${node.name} was a ${node.board}. Outputs it doesn't have will be unwired.`;
	});

	async function refresh() {
		try {
			found = await api.discovered();
			if (pick && !candidates.some((d) => d.id === pick)) pick = null;
			if (!pick && candidates.length === 1) pick = candidates[0].id;
		} catch {
			/* keep the last list */
		}
	}

	$effect(() => {
		if (open && !isLeader) {
			void refresh();
			timer = setInterval(refresh, 3000);
			return () => clearInterval(timer);
		}
	});
	onDestroy(() => clearInterval(timer));

	async function replace(force = false) {
		if (!chosen) return;
		busy = true;
		try {
			const n = await fleetApi.replace(node.id, chosen.id, force);
			toasts.success(`${n.name} now runs on the new controller; it's getting its show`);
			open = false;
			onreplaced?.(n);
		} catch (e) {
			const err = e as ApiError;
			if (!force && (err.code === 'board_mismatch' || err.code === 'node_online')) {
				const ok = await confirm({
					title: err.code === 'node_online' ? `${node.name} is online` : 'Different board',
					message: err.message,
					confirmLabel: 'Replace anyway',
					danger: true
				});
				if (ok) return replace(true);
			} else {
				toasts.error("Couldn't replace it", err.message);
			}
		} finally {
			busy = false;
		}
	}
</script>

<Modal
	bind:open
	title={isLeader ? `Replace the show leader` : `Replace ${node.name}`}
	subtitle={isLeader
		? 'Move the whole show to new hardware'
		: 'A new controller takes over its name, wiring, props and sequences'}
	size="md"
>
	{#if isLeader}
		<ol class="steps small">
			<li>Download the <strong>transfer file</strong> below and keep it with its passphrase.</li>
			<li>
				Flash a new SD card with PixelPlus, start the new Pi and open it in the browser. In the welcome screen
				choose
				<strong>Restore a show from a transfer file</strong>.
			</li>
			<li>
				The new Pi becomes this leader: same show, same address name, same followers (they find it by
				themselves), and phones keep trusting it.
			</li>
		</ol>
		<TransferExport compact />
	{:else}
		{#if online}
			<div class="notice warn small">
				<TriangleAlert size={16} />
				<div>
					{node.name} is online. Replace a controller when it is broken; the old one stops taking part in the show
					at once.
				</div>
			</div>
		{/if}
		<p class="muted small intro">
			Flash a new SD card with PixelPlus, put it in the new controller (or the same board with a new Pi), and
			power it up on the same network. It shows up here within a minute.
		</p>
		<div class="head row">
			<strong class="grow">New controllers found</strong>
			<button class="btn ghost sm" onclick={refresh} aria-label="Look again"><RefreshCw size={14} /></button>
		</div>
		{#if candidates.length === 0}
			<div class="empty">
				<div class="pulse" aria-hidden="true"></div>
				<span class="muted">Waiting for a new controller…</span>
			</div>
		{:else}
			<div class="list" role="radiogroup" aria-label="New controller">
				{#each candidates as d (d.id)}
					<label class="cand" class:on={pick === d.id}>
						<input type="radio" name="replace-candidate" value={d.id} bind:group={pick} />
						<Cpu size={18} />
						<div class="grow">
							<div><strong>{d.name}</strong> <span class="faint small">{d.board}</span></div>
							<div class="faint small mono">
								{d.ip ?? ''}{d.pi ? ` · ${d.pi.replace(/ Rev [\d.]+$/, '')}` : ''}{d.ver
									? ` · v${d.ver}`
									: ''}
							</div>
						</div>
					</label>
				{/each}
			</div>
		{/if}
		{#if chosen}
			<div class="summary row small">
				<span class="mono">{chosen.name}</span>
				<ArrowRight size={14} />
				<strong>{node.name}</strong>
			</div>
			{#if boardNote}
				<div class="notice info small">
					<Cpu size={16} />
					<div>{boardNote}</div>
				</div>
			{/if}
		{/if}
	{/if}
	{#snippet footer()}
		{#if isLeader}
			<button class="btn" onclick={() => (open = false)}>Close</button>
		{:else}
			<button class="btn" onclick={() => (open = false)} disabled={busy}>Cancel</button>
			<button class="btn primary" onclick={() => replace()} disabled={!chosen || busy}>
				<Replace size={16} />
				{busy ? 'Replacing…' : 'Replace'}
			</button>
		{/if}
	{/snippet}
</Modal>

<style>
	.steps {
		margin: 0 0 14px;
		padding-left: 20px;
		display: grid;
		gap: 6px;
	}
	.intro {
		margin: 0 0 12px;
	}
	.row {
		display: flex;
		align-items: center;
		gap: 8px;
	}
	.grow {
		flex: 1 1 auto;
		min-width: 0;
	}
	.head {
		margin-bottom: 6px;
	}
	.list {
		display: grid;
		gap: 6px;
	}
	.cand {
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 10px 12px;
		border: 1px solid var(--border-2);
		border-radius: var(--r-2);
		cursor: pointer;
		background: var(--surface-2);
	}
	.cand.on {
		border-color: var(--accent);
		box-shadow: 0 0 0 1px var(--accent) inset;
	}
	.cand input {
		margin: 0;
	}
	.empty {
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 16px 12px;
		border: 1px dashed var(--border-2);
		border-radius: var(--r-2);
	}
	.pulse {
		width: 10px;
		height: 10px;
		border-radius: 50%;
		background: var(--accent);
		animation: pulse 1.4s ease-in-out infinite;
	}
	@keyframes pulse {
		50% {
			opacity: 0.25;
		}
	}
	@media (prefers-reduced-motion: reduce) {
		.pulse {
			animation: none;
		}
	}
	.summary {
		margin: 12px 0 8px;
	}
</style>
