<script lang="ts">
	import type { Prop, PropSegment, Show } from '$lib/api/types';
	import { COLOR_ORDERS } from '$lib/api/types';
	import { api } from '$lib/api/client';
	import { app } from '$lib/stores/app.svelte';
	import { toasts } from '$lib/stores/toasts.svelte';
	import {
		pixelsOnOutput,
		portName,
		portOf,
		propsOnOutput,
		wiringChain,
		MAX_PIXELS_PER_OUTPUT,
		needsPort3Warning
	} from '$lib/util/boards';
	import { sortable } from '$lib/actions/sortable';
	import { reorderChain, flashPort, COLOR_CORRECTION, correctionIndex } from '$lib/wiring';
	import Switch from '$lib/components/ui/Switch.svelte';
	import PortPicker from './PortPicker.svelte';
	import {
		Plus,
		Trash2,
		GripVertical,
		ChevronRight,
		ChevronUp,
		ChevronDown,
		TriangleAlert,
		Cable,
		ArrowLeftRight,
		Zap
	} from '@lucide/svelte';

	let { show, prop = $bindable() }: { show: Show; prop: Prop } = $props();

	const wired = $derived(prop.segments.reduce((n, s) => n + s.pixelCount, 0));
	const unwired = $derived(prop.pixelCount - wired);
	const wirableNodes = $derived(show.nodes.filter((n) => n.outputs.length));

	function addSegment() {
		const node = wirableNodes[0];
		if (!node) return;
		// first empty output
		const out = node.outputs.find((o) => !pixelsOnOutput(show, node.id, o.index))?.index ?? 1;
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

	async function moveInChain(seg: PropSegment, from: number, to: number) {
		await reorderChain(show, seg.nodeId, seg.output, from, to);
		// Pick up the new start pixel for this prop without discarding other edits in the panel.
		const fresh = app.show?.props.find((p) => p.id === prop.id);
		const fs = fresh?.segments.find((x) => x.nodeId === seg.nodeId && x.output === seg.output);
		if (fs) seg.startPixel = fs.startPixel;
	}

	async function saveOutput(nodeId: string, index: number, patch: Record<string, unknown>) {
		try {
			await api.saveOutput(nodeId, index, patch);
			await app.reloadShow();
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

			{#if wirableNodes.length > 1}
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
						{#each wirableNodes as n (n.id)}<option value={n.id}>{n.name}</option>{/each}
					</select>
				</label>
			{/if}

			<div class="field">
				<div class="row between">
					<span class="label">Plugged into</span>
					<button
						type="button"
						class="btn ghost sm"
						onclick={() => flashPort(seg.nodeId, seg.output)}
						title="Lights everything on this port for a few seconds so you can find the cable"
						><Zap size={14} /> Flash this port</button
					>
				</div>
				<PortPicker
					{show}
					nodeId={seg.nodeId}
					value={seg.output}
					propId={prop.id}
					onchange={(o) => {
						seg.output = o;
						placeAfterLast(seg);
					}}
				/>
			</div>

			<div class="form-grid">
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
						>1 = the first pixel on the cable. <button
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
					<span class="label">Skip pixels (dark spacers)</span>
					<input class="input" type="number" min="0" bind:value={seg.nullPixels} />
					<span class="hint">Pixels before this prop that stay dark, e.g. a lead-in pixel or a gap.</span>
				</label>
				<div class="field">
					<span class="label">Direction</span>
					<div class="row dir">
						<Switch bind:checked={seg.reverse} label="Reverse direction" />
						<span class="small muted dirtext"
							><ArrowLeftRight size={14} />
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
					<div class="eyebrow">Daisy chain on this port · drag or use the arrows to reorder</div>
					<ol use:sortable={{ onsort: (f, t) => moveInChain(seg, f, t) }}>
						{#each chain as c, k (c.prop.id + c.seg.propOffset)}
							<li data-sort-index={k} class:me={c.prop.id === prop.id}>
								<button type="button" class="drag-handle" aria-label="Move {c.prop.name} (use arrow keys)"
									><GripVertical size={16} /></button
								>
								<span class="n num">{k + 1}</span>
								<span class="grow ellipsis">{c.prop.name}</span>
								<span class="faint small num hide-xs"
									>px {c.seg.startPixel + 1}–{c.seg.startPixel + c.seg.pixelCount}</span
								>
								<button
									type="button"
									class="btn ghost icon sm"
									disabled={k === 0}
									onclick={() => moveInChain(seg, k, k - 1)}
									aria-label="Move {c.prop.name} earlier in the chain"><ChevronUp size={15} /></button
								>
								<button
									type="button"
									class="btn ghost icon sm"
									disabled={k === chain.length - 1}
									onclick={() => moveInChain(seg, k, k + 1)}
									aria-label="Move {c.prop.name} later in the chain"><ChevronDown size={15} /></button
								>
							</li>
						{/each}
					</ol>
				</div>
			{/if}

			{#if outCfg && node}
				{@const ci = correctionIndex(outCfg.gamma)}
				<details class="port">
					<summary
						>Port settings <span class="faint small"
							>· shared by everything on {portName(node.board, seg.output)}</span
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
							<span class="hint">If red shows up as green, try GRB.</span>
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
							<span class="label">Color correction · {COLOR_CORRECTION[ci].label}</span>
							<input
								type="range"
								class="range"
								min="0"
								max={COLOR_CORRECTION.length - 1}
								step="1"
								value={ci}
								style:--pct="{(ci / (COLOR_CORRECTION.length - 1)) * 100}%"
								onchange={(e) =>
									saveOutput(node.id, seg.output, {
										gamma: COLOR_CORRECTION[Number((e.target as HTMLInputElement).value)].gamma
									})}
								aria-valuetext={COLOR_CORRECTION[ci].label}
							/>
							<span class="hint">Makes dim colors look natural. Normal suits most pixels.</span>
						</label>
						<div class="field">
							<span class="label">Port on</span>
							<div class="row dir">
								<Switch
									checked={outCfg.enabled}
									label="Port on"
									onchange={(v) => saveOutput(node.id, seg.output, { enabled: v })}
								/>
								<span class="small muted">{outCfg.enabled ? 'On' : 'Off — nothing on it lights'}</span>
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

	<div class="row wrap">
		<button type="button" class="btn" onclick={addSegment} disabled={!wirableNodes.length}
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
	{#if !wirableNodes.length}
		<p class="faint small">Adopt a controller with pixel outputs first (Controllers page).</p>
	{:else if prop.segments.length}
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
		text-decoration: underline;
		text-underline-offset: 2px;
	}
	.dir {
		min-height: 44px;
	}
	.dirtext {
		display: inline-flex;
		align-items: center;
		gap: 6px;
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
		gap: 6px;
		padding: 2px 4px 2px 2px;
		border-radius: 8px;
		background: var(--surface);
		border: 1px solid var(--border);
		font-size: 13px;
		min-height: 44px;
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
		flex: 0 0 auto;
	}
	details.port summary {
		cursor: pointer;
		font-weight: 560;
		font-size: 13px;
		list-style: none;
		min-height: 32px;
		display: flex;
		align-items: center;
		flex-wrap: wrap;
		gap: 4px;
	}
	details.port summary::before {
		content: '▸';
		display: inline-block;
		margin-right: 2px;
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
	@media (max-width: 420px) {
		.hide-xs {
			display: none;
		}
	}
</style>
