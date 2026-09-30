<script lang="ts">
	import { isEnabled } from '$lib/features';
	import type { ImportPreview } from '$lib/api/types';
	import { api } from '$lib/api/client';
	import { app } from '$lib/stores/app.svelte';
	import { toasts } from '$lib/stores/toasts.svelte';
	import Modal from '$lib/components/ui/Modal.svelte';
	import {
		FileUp,
		FileCheck2,
		TriangleAlert,
		ArrowRight,
		CircleCheck,
		Sparkles,
		Pencil
	} from '@lucide/svelte';
	import type { Prop } from '$lib/api/types';
	import { portName } from '$lib/util/boards';

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
	/** Compare with the current show: what's new, what changed, what stays the same. */
	const diff = $derived.by(() => {
		const out = { added: [] as Prop[], changed: [] as Prop[], same: [] as Prop[] };
		if (!preview) return out;
		const cur = app.show?.props ?? [];
		for (const p of preview.props) {
			const old =
				cur.find((x) => x.id === p.id) ??
				cur.find((x) => !!p.xlightsModel && x.xlightsModel === p.xlightsModel) ??
				cur.find((x) => x.name === p.name);
			if (!old) out.added.push(p);
			else if (old.pixelCount !== p.pixelCount || old.kind !== p.kind || old.name !== p.name)
				out.changed.push(p);
			else out.same.push(p);
		}
		return out;
	});
	/** "J3 · Port 2 on Main Controller" for a new prop, once its controller is matched. */
	function whereItGoes(p: Prop): string {
		const seg = p.segments[0];
		if (!seg) return 'Not connected in xLights — wire it here afterwards';
		const node = app.show?.nodes.find((n) => n.id === map[seg.nodeId]);
		if (!node) return 'Stays unwired for now';
		return `${portName(node.board, seg.output)} on ${node.name}`;
	}
	/** Daemon import warnings, in plain words. */
	function friendly(w: string): string {
		const m = /^skipped '(.+?)': (.+)$/i.exec(w);
		if (m)
			return `“${m[1]}” was skipped: ${m[2].replace(/'/g, '’')}. You can add and wire it here after importing.`;
		return w.charAt(0).toUpperCase() + w.slice(1);
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
			<strong>Where are they?</strong> Both files are in your xLights show folder (the folder you picked in
			xLights under <em>Show Folder</em>).
		</div>
		<details class="xl-help">
			<summary>Setting up xLights for PixelPlus</summary>
			<ol>
				<li>In xLights, open <em>Controllers</em> and add one controller for each PixelPlus box.</li>
				<li>
					Choose <em>Ethernet</em>, vendor <em>PixelPlus</em> (or <em>Generic</em>), protocol <em>DDP</em>,
					and turn on <em>Auto size</em>.
				</li>
				<li>Plug each prop into the port it uses in real life, then save.</li>
			</ol>
			<p class="faint small">That’s all — PixelPlus works out the rest when you import.</p>
			{#if isEnabled('xlightsUpload')}
				<p class="faint small">
					Later, xLights can send your sequences straight to PixelPlus: see
					<a href="/settings/xlights">Settings → xLights</a>.
				</p>
			{/if}
		</details>
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
		<p class="summary">
			Found <strong>{preview.props.length} props</strong> — {diff.added.length} new, {diff.changed.length} changed,
			{diff.same.length} unchanged.
		</p>
		{#if diff.added.length}
			<h3 class="eyebrow sect"><Sparkles size={13} /> New · {diff.added.length}</h3>
			<div class="props">
				{#each diff.added as p (p.id)}
					<div class="prow">
						<span class="grow ellipsis"><strong>{p.name}</strong></span>
						<span class="faint small where ellipsis">{whereItGoes(p)}</span>
						<span class="faint small num">{p.pixelCount} px</span>
					</div>
				{/each}
			</div>
		{/if}
		{#if diff.changed.length}
			<h3 class="eyebrow sect"><Pencil size={13} /> Changed · {diff.changed.length}</h3>
			<div class="props">
				{#each diff.changed as p (p.id)}
					<div class="prow">
						<span class="grow ellipsis">{p.name}</span><span class="faint small num">{p.pixelCount} px</span>
					</div>
				{/each}
			</div>
		{/if}
		{#if diff.same.length}
			<details class="same">
				<summary class="eyebrow"><CircleCheck size={13} /> Unchanged · {diff.same.length}</summary>
				<div class="props">
					{#each diff.same as p (p.id)}
						<div class="prow">
							<span class="grow ellipsis">{p.name}</span><span class="faint small num">{p.pixelCount} px</span
							>
						</div>
					{/each}
				</div>
			</details>
		{/if}
		{#each preview.warnings as w (w)}
			<div class="notice warn small" style="margin-top:10px">
				<TriangleAlert size={16} class="ico" /><span>{friendly(w)}</span>
			</div>
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
				>{busy
					? 'Importing…'
					: diff.added.length || diff.changed.length
						? `Import ${diff.added.length + diff.changed.length} ${diff.added.length + diff.changed.length === 1 ? 'prop' : 'props'}`
						: 'Everything’s up to date'}</button
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
	.summary {
		margin-top: 18px;
		font-size: 14px;
	}
	.sect,
	.same summary {
		display: flex;
		align-items: center;
		gap: 6px;
		margin-top: 16px;
	}
	.same summary {
		cursor: pointer;
		list-style: none;
	}
	.where {
		max-width: 46%;
	}
	.xl-help {
		margin-top: 10px;
		font-size: 13px;
		color: var(--text-2);
	}
	.xl-help summary {
		cursor: pointer;
		font-weight: 560;
		color: var(--text);
	}
	.xl-help ol {
		margin: 8px 0;
		padding-left: 20px;
		display: flex;
		flex-direction: column;
		gap: 4px;
	}
	.xl-help em {
		font-style: normal;
		font-weight: 560;
		color: var(--text);
	}
</style>
