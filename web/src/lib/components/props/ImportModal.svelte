<script lang="ts">
	import type { ImportPreview } from '$lib/api/types';
	import { api } from '$lib/api/client';
	import { app } from '$lib/stores/app.svelte';
	import { toasts } from '$lib/stores/toasts.svelte';
	import Modal from '$lib/components/ui/Modal.svelte';
	import { FileUp, FileCheck2, TriangleAlert, ArrowRight } from '@lucide/svelte';

	let { open = $bindable(false) }: { open?: boolean } = $props();

	let rgb = $state<File | null>(null);
	let net = $state<File | null>(null);
	let preview = $state<ImportPreview | null>(null);
	let map = $state<Record<string, string>>({});
	let busy = $state(false);

	async function analyze() {
		if (!rgb) return;
		busy = true;
		try {
			preview = await api.importXlights(rgb, net ?? undefined);
			// Suggested match, else the first controller with enough outputs, else leave it unwired.
			const nodes = app.show?.nodes ?? [];
			map = Object.fromEntries(
				preview.controllers.map((c) => [
					c.name,
					c.suggestedNodeId ?? nodes.find((n) => n.outputs.length >= c.ports)?.id ?? ''
				])
			);
		} catch (e) {
			toasts.error('Could not read the layout', (e as Error).message);
		} finally {
			busy = false;
		}
	}
	async function apply() {
		if (!preview) return;
		busy = true;
		const n = preview.props.length;
		await app.mutate(() => api.applyImport(preview!, map), { success: `Imported ${n} props from xLights` });
		busy = false;
		reset();
	}
	function reset() {
		open = false;
		rgb = net = null;
		preview = null;
	}
	/** "Porch has 4 outputs" when the chosen controller has fewer outputs than the xLights one uses. */
	function portWarning(controller: string, ports: number): string | null {
		const node = app.show?.nodes.find((n) => n.id === map[controller]);
		if (!node || node.outputs.length >= ports) return null;
		return `${node.name} has ${node.outputs.length} outputs; props on higher ports stay unwired until you wire them by hand.`;
	}
	function pick(e: Event, which: 'rgb' | 'net') {
		const f = (e.target as HTMLInputElement).files?.[0] ?? null;
		if (which === 'rgb') rgb = f;
		else net = f;
	}
</script>

<Modal
	bind:open
	title="Import from xLights"
	subtitle="Bring in your props straight from your xLights layout"
	size="lg"
	onclose={reset}
>
	{#if !preview}
		<div class="grid grid-2">
			<label class="drop" class:has={rgb}>
				<input type="file" accept=".xml" class="sr-only" onchange={(e) => pick(e, 'rgb')} />
				{#if rgb}<FileCheck2 size={26} />{:else}<FileUp size={26} />{/if}
				<strong>{rgb?.name ?? 'xlights_rgbeffects.xml'}</strong>
				<span class="faint small">Required — your props and their layout</span>
			</label>
			<label class="drop" class:has={net}>
				<input type="file" accept=".xml" class="sr-only" onchange={(e) => pick(e, 'net')} />
				{#if net}<FileCheck2 size={26} />{:else}<FileUp size={26} />{/if}
				<strong>{net?.name ?? 'xlights_networks.xml'}</strong>
				<span class="faint small">Optional — helps match controllers</span>
			</label>
		</div>
		<div class="tip">
			<strong>Tip:</strong> Both files are in your xLights show folder. In xLights, set up each PixelPlus
			controller as
			<em>PixelPlus / Generic</em> with protocol <em>DDP</em> and “Auto size” — you never need to type a universe
			or channel.
		</div>
	{:else}
		<h3 class="eyebrow">Match controllers</h3>
		<div class="maps">
			{#each preview.controllers as c (c.name)}
				<div class="map">
					<span class="grow"
						><strong>{c.name}</strong> <span class="faint small">· {c.ports} ports in xLights</span></span
					>
					<ArrowRight size={16} class="faint" />
					<select
						class="select sm"
						style="max-width:220px"
						bind:value={map[c.name]}
						aria-label="PixelPlus controller for {c.name}"
					>
						{#each app.show?.nodes ?? [] as n (n.id)}<option value={n.id}
								>{n.name} · {n.outputs.length} outputs</option
							>{/each}
						<option value="">Leave unwired</option>
					</select>
				</div>
				{#if portWarning(c.name, c.ports)}
					<div class="notice warn small">
						<TriangleAlert size={16} /><span>{portWarning(c.name, c.ports)}</span>
					</div>
				{/if}
			{/each}
		</div>
		<h3 class="eyebrow" style="margin-top:20px">{preview.props.length} props to add or update</h3>
		<div class="props">
			{#each preview.props as p (p.id)}
				<div class="prow">
					<span class="grow ellipsis">{p.name}</span><span class="faint small num">{p.pixelCount} px</span>
				</div>
			{/each}
		</div>
		{#each preview.warnings as w (w)}
			<div class="notice warn small" style="margin-top:8px"><TriangleAlert size={16} /><span>{w}</span></div>
		{/each}
	{/if}
	{#snippet footer()}
		<button class="btn ghost" onclick={reset}>Cancel</button>
		{#if !preview}
			<button class="btn primary" disabled={!rgb || busy} onclick={analyze}
				>{busy ? 'Reading layout…' : 'Continue'}</button
			>
		{:else}
			<button class="btn primary" disabled={busy} onclick={apply}
				>{busy ? 'Importing…' : `Import ${preview.props.length} props`}</button
			>
		{/if}
	{/snippet}
</Modal>

<style>
	.drop {
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: 6px;
		text-align: center;
		min-height: 150px;
		padding: 20px;
		border: 1.5px dashed var(--border-3);
		border-radius: 14px;
		cursor: pointer;
		color: var(--text-3);
		transition: all 150ms var(--ease);
	}
	.drop strong {
		color: var(--text);
		font-size: 13.5px;
		word-break: break-all;
	}
	.drop:hover,
	.drop:focus-within {
		border-color: var(--accent);
		background: var(--accent-soft);
	}
	.drop.has {
		border-style: solid;
		border-color: var(--green);
		color: var(--green);
		background: var(--green-soft);
	}
	.tip {
		margin-top: 16px;
		font-size: 13px;
		color: var(--text-2);
		padding: 12px 14px;
		border-radius: 10px;
		background: var(--surface-2);
	}
	.tip em {
		font-style: normal;
		color: var(--text);
		font-weight: 560;
	}
	.maps,
	.props {
		display: flex;
		flex-direction: column;
		border: 1px solid var(--border);
		border-radius: 12px;
		margin-top: 8px;
		overflow: hidden;
	}
	.map,
	.prow {
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 10px 14px;
		border-bottom: 1px solid var(--border);
		font-size: 13.5px;
	}
	.map:last-child,
	.prow:last-child {
		border-bottom: 0;
	}
	.props {
		max-height: 200px;
		overflow: auto;
	}
</style>
