<script lang="ts">
	import type { Prop, PropSegment, Show } from '$lib/api/types';
	import { COLOR_ORDERS } from '$lib/api/types';
	import { api } from '$lib/api/client';
	import { app } from '$lib/stores/app.svelte';
	import { toasts } from '$lib/stores/toasts.svelte';
	import {
		jackOf,
		outputLabel,
		pixelsOnOutput,
		portOf,
		propsOnOutput,
		receiverFor,
		wiringChain,
		MAX_PIXELS_PER_OUTPUT,
		needsPort3Warning
	} from '$lib/util/boards';
	import { sortable, moveItem } from '$lib/actions/sortable';
	import Switch from '$lib/components/ui/Switch.svelte';
	import {
		Plus,
		Trash2,
		GripVertical,
		ChevronRight,
		TriangleAlert,
		Cable,
		ArrowLeftRight
	} from '@lucide/svelte';

	let { show, prop = $bindable() }: { show: Show; prop: Prop } = $props();

	const wired = $derived(prop.segments.reduce((n, s) => n + s.pixelCount, 0));
	const unwired = $derived(prop.pixelCount - wired);

	function outputOptions(nodeId: string) {
		const node = show.nodes.find((n) => n.id === nodeId);
		if (!node) return [];
		return node.outputs.map((o) => {
			const rx = receiverFor(show, node.id, o.index);
			const jack = jackOf(node.board, o.index);
			let label: string;
			if (rx)
				label = `${node.board === 'difftxlarge' ? `J${jack} · ` : ''}${rx.name} receiver · Port ${portOf(o.index)}`;
			else
				label =
					node.board === 'difftxlarge'
						? `J${jack} · Port ${portOf(o.index)}`
						: outputLabel(node.board, o.index);
			const used = pixelsOnOutput(show, node.id, o.index);
			return { value: o.index, label: used ? `${label} — ${used} px in use` : label };
		});
	}

	function addSegment() {
		const node = show.nodes[0];
		if (!node) return;
		// first empty output
		let out = node.outputs.find((o) => !pixelsOnOutput(show, node.id, o.index))?.index ?? 1;
		const seg: PropSegment = {
			nodeId: node.id,
			output: out,
			startPixel: pixelsOnOutput(show, node.id, out),
			pixelCount: Math.max(1, unwired || prop.pixelCount),
			propOffset: wired,
			reverse: false,
			nullPixels: 0
		};
		prop.segments = [...prop.segments, seg];
	}

	function removeSegment(i: number) {
		const segs = prop.segments.filter((_, k) => k !== i);
		let off = 0;
		for (const s of segs) {
			s.propOffset = off;
			off += s.pixelCount;
		}
		prop.segments = segs;
	}

	function placeAfterLast(seg: PropSegment) {
		const others = propsOnOutput(show, seg.nodeId, seg.output).filter((x) => x.prop.id !== prop.id);
		seg.startPixel =
			others.reduce((m, x) => Math.max(m, x.seg.startPixel + x.seg.pixelCount), 0) + seg.nullPixels;
	}

	/** Props chained on the output of the given segment, in physical order. */
	function chainFor(seg: PropSegment) {
		return propsOnOutput(show, seg.nodeId, seg.output);
	}

	async function reorderChain(seg: PropSegment, from: number, to: number) {
		const chain = moveItem(chainFor(seg), from, to);
		let cursor = 0;
		const ops: { op: 'update'; id: string; patch: Partial<Prop> }[] = [];
		const patched = new Map<string, Prop>();
		for (const { prop: p, seg: s } of chain) {
			cursor += s.nullPixels;
			const target = patched.get(p.id) ?? structuredClone($state.snapshot(p) as Prop);
			const ts = target.segments.find(
				(x) => x.nodeId === s.nodeId && x.output === s.output && x.propOffset === s.propOffset
			);
			if (ts) ts.startPixel = cursor;
			patched.set(p.id, target);
			cursor += s.pixelCount;
		}
		for (const [id, p] of patched) ops.push({ op: 'update', id, patch: { segments: p.segments } });
		const mine = patched.get(prop.id);
		if (mine) prop.segments = mine.segments.map((s) => ({ ...s }));
		await app.mutate(() => api.props.bulk(ops), { success: 'Chain order updated' });
	}

	async function saveOutput(nodeId: string, index: number, patch: Record<string, unknown>) {
		try {
			await api.saveOutput(nodeId, index, patch);
			await app.reloadShow();
			toasts.success('Port settings saved');
		} catch (e) {
			toasts.error('Could not save port settings', (e as Error).message);
		}
	}
</script>

<div class="wiring">
	{#if !prop.segments.length}
		<div class="notice warn">
			<Cable size={18} class="ico" />
			<div>
				<strong>Not wired yet.</strong> Tell PixelPlus which port this prop is plugged into and it will light up
				in sequences.
			</div>
		</div>
	{/if}

	{#each prop.segments as seg, i (i)}
		{@const node = show.nodes.find((n) => n.id === seg.nodeId)}
		{@const outCfg = node?.outputs.find((o) => o.index === seg.output)}
		{@const used = pixelsOnOutput(show, seg.nodeId, seg.output)}
		{@const chain = chainFor(seg)}
		<section class="seg card">
			<div class="chain" aria-label="Wiring path">
				{#each wiringChain(show, seg) as step, k (k)}
					{#if k}<ChevronRight size={14} class="sep" />{/if}
					<span class="step {step.kind}">{step.label}</span>
				{/each}
			</div>

			<div class="form-grid">
				<label class="field">
					<span class="label">Controller</span>
					<select
						class="select"
						bind:value={seg.nodeId}
						onchange={() => {
							seg.output = 1;
							placeAfterLast(seg);
						}}
					>
						{#each show.nodes as n (n.id)}<option value={n.id}>{n.name}</option>{/each}
					</select>
				</label>
				<label class="field">
					<span class="label">Plugged into</span>
					<select class="select" bind:value={seg.output} onchange={() => placeAfterLast(seg)}>
						{#each outputOptions(seg.nodeId) as o (o.value)}<option value={o.value}>{o.label}</option>{/each}
					</select>
				</label>
				<label class="field">
					<span class="label">First pixel on this port</span>
					<input
						class="input"
						type="number"
						min="1"
						value={seg.startPixel + 1}
						oninput={(e) => (seg.startPixel = Math.max(0, Number((e.target as HTMLInputElement).value) - 1))}
					/>
					<span class="hint"
						>1 = first pixel after the receiver. <button
							type="button"
							class="linkish"
							onclick={() => placeAfterLast(seg)}>Place after the last prop</button
						></span
					>
				</label>
				<label class="field">
					<span class="label">Pixels in this run</span>
					<input class="input" type="number" min="1" max={prop.pixelCount} bind:value={seg.pixelCount} />
				</label>
				<label class="field">
					<span class="label">Null pixels before</span>
					<input class="input" type="number" min="0" bind:value={seg.nullPixels} />
					<span class="hint">Spacer pixels that stay dark (e.g. a lead-in pixel).</span>
				</label>
				<div class="field">
					<span class="label">Direction</span>
					<div class="row" style="height:40px">
						<Switch bind:checked={seg.reverse} label="Reverse direction" />
						<span class="small muted"
							><ArrowLeftRight size={13} />
							{seg.reverse ? 'Reversed — starts at the far end' : 'Normal'}</span
						>
					</div>
				</div>
			</div>

			{#if used > MAX_PIXELS_PER_OUTPUT}
				<div class="notice danger small">
					<TriangleAlert size={16} />
					{used} pixels on this port — more than about {MAX_PIXELS_PER_OUTPUT} can refresh smoothly. Consider splitting
					it.
				</div>
			{/if}
			{#if node && needsPort3Warning(node) && portOf(seg.output) === 3}
				<div class="notice warn small">
					<TriangleAlert size={16} />
					{node.name} is a rev D board: port 3 needs a short patch lead with pins 4 and 5 swapped.
				</div>
			{/if}

			{#if chain.length > 1}
				<div class="chain-list">
					<div class="eyebrow">Daisy chain on this port · drag to reorder</div>
					<ol use:sortable={{ onsort: (f, t) => reorderChain(seg, f, t) }}>
						{#each chain as c, k (c.prop.id + c.seg.propOffset)}
							<li data-sort-index={k} class:me={c.prop.id === prop.id}>
								<button type="button" class="drag-handle" aria-label="Move {c.prop.name} (use arrow keys)"
									><GripVertical size={16} /></button
								>
								<span class="n num">{k + 1}</span>
								<span class="grow ellipsis">{c.prop.name}</span>
								<span class="faint small num"
									>px {c.seg.startPixel + 1}–{c.seg.startPixel + c.seg.pixelCount}</span
								>
							</li>
						{/each}
					</ol>
				</div>
			{/if}

			{#if outCfg && node}
				<details class="port">
					<summary
						>Port settings <span class="faint small"
							>· shared by everything on {outputLabel(node.board, seg.output)}</span
						></summary
					>
					<div class="form-grid" style="margin-top:12px">
						<label class="field">
							<span class="label">Color order</span>
							<select
								class="select"
								value={outCfg.colorOrder}
								onchange={(e) =>
									saveOutput(node.id, seg.output, { colorOrder: (e.target as HTMLSelectElement).value })}
							>
								{#each COLOR_ORDERS as c (c)}<option value={c}>{c}</option>{/each}
							</select>
							<span class="hint">If red shows as green, try GRB.</span>
						</label>
						<label class="field">
							<span class="label">Brightness limit · {outCfg.brightness}%</span>
							<input
								type="range"
								class="range"
								min="5"
								max="100"
								step="5"
								value={outCfg.brightness}
								style:--pct="{outCfg.brightness}%"
								onchange={(e) =>
									saveOutput(node.id, seg.output, {
										brightness: Number((e.target as HTMLInputElement).value)
									})}
							/>
						</label>
						<label class="field">
							<span class="label">Gamma</span>
							<select
								class="select"
								value={String(outCfg.gamma)}
								onchange={(e) =>
									saveOutput(node.id, seg.output, { gamma: Number((e.target as HTMLSelectElement).value) })}
							>
								<option value="1">None (1.0)</option>
								<option value="1.8">Soft (1.8)</option>
								<option value="2.2">Standard (2.2)</option>
								<option value="2.8">Strong (2.8)</option>
							</select>
						</label>
						<div class="field">
							<span class="label">Port enabled</span>
							<div style="height:40px" class="row">
								<Switch
									checked={outCfg.enabled}
									label="Port enabled"
									onchange={(v) => saveOutput(node.id, seg.output, { enabled: v })}
								/>
							</div>
						</div>
					</div>
				</details>
			{/if}

			<div class="row seg-foot">
				<span class="faint small">Prop pixels {seg.propOffset + 1}–{seg.propOffset + seg.pixelCount}</span>
				<span class="grow"></span>
				<button type="button" class="btn ghost sm" onclick={() => removeSegment(i)}
					><Trash2 size={14} /> Remove run</button
				>
			</div>
		</section>
	{/each}

	<div class="row">
		<button type="button" class="btn" onclick={addSegment}
			><Plus size={16} /> {prop.segments.length ? 'Add another run' : 'Wire this prop'}</button
		>
		{#if prop.segments.length}
			<span class="small {unwired ? 'warn-text' : 'faint'}"
				>{unwired > 0
					? `${unwired} pixels not wired yet`
					: unwired < 0
						? `${-unwired} more pixels wired than the prop has`
						: 'All pixels wired'}</span
			>
		{/if}
	</div>
	{#if prop.segments.length}
		<p class="faint tiny">
			Props longer than one port can be split into several runs (e.g. a mega tree across four ports).
		</p>
	{/if}
</div>

<style>
	.wiring {
		display: flex;
		flex-direction: column;
		gap: 14px;
	}
	.seg {
		padding: 16px;
		display: flex;
		flex-direction: column;
		gap: 16px;
		background: var(--surface-2);
	}
	.chain {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 4px 6px;
		padding: 10px 12px;
		border-radius: 10px;
		background: var(--surface);
		border: 1px solid var(--border);
		font-size: 13px;
		font-weight: 560;
	}
	.chain :global(.sep) {
		color: var(--text-3);
	}
	.step.pixels {
		color: var(--accent-text);
	}
	.step.node {
		color: var(--text);
	}
	.step.receiver,
	.step.jack,
	.step.port {
		color: var(--text-2);
	}
	.linkish {
		color: var(--accent-text);
		font-weight: 560;
		font-size: 12px;
	}
	.chain-list ol {
		list-style: none;
		margin: 8px 0 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 4px;
	}
	.chain-list li {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 2px 10px 2px 2px;
		border-radius: 8px;
		background: var(--surface);
		border: 1px solid var(--border);
		font-size: 13px;
		min-height: 40px;
	}
	.chain-list li.me {
		border-color: var(--accent-line);
		background: var(--accent-soft);
	}
	.n {
		width: 20px;
		height: 20px;
		border-radius: 6px;
		display: grid;
		place-items: center;
		font-size: 11px;
		background: var(--surface-3);
		color: var(--text-2);
	}
	details.port summary {
		cursor: pointer;
		font-weight: 560;
		font-size: 13px;
		list-style: none;
	}
	details.port summary::before {
		content: '▸';
		display: inline-block;
		margin-right: 6px;
		transition: transform 150ms;
		color: var(--text-3);
	}
	details.port[open] summary::before {
		transform: rotate(90deg);
	}
	.seg-foot {
		border-top: 1px solid var(--border);
		padding-top: 10px;
		margin-top: -4px;
	}
	.warn-text {
		color: var(--accent-text);
	}
</style>
