<!--
	"Map my yard" (F6, WS4; ARCHITECTURE §12.5). Phone-first:
	  scope → camera (frame, lock exposure, scan) → review (photo with outlines, proposals with
	  checkboxes, Apply selected with Undo) — plus earlier runs to reopen.
-->
<script lang="ts">
	import { onMount } from 'svelte';
	import {
		Camera,
		Cpu,
		Shapes,
		Sparkles,
		ArrowLeftRight,
		RotateCcw,
		Ruler,
		EyeOff,
		LayoutTemplate,
		Info,
		Check,
		Image as ImageIcon,
		Trash2,
		History,
		ChevronRight,
		TriangleAlert
	} from '@lucide/svelte';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import SecureGate from '$lib/components/ui/SecureGate.svelte';
	import CameraScan from '$lib/cv/CameraScan.svelte';
	import { app } from '$lib/stores/app.svelte';
	import { toasts, confirm } from '$lib/stores/toasts.svelte';
	import { api } from '$lib/api/client';
	import { mappingApi, type MapScope } from '$lib/cv/api';
	import { analyze, type Analysis } from '$lib/cv/analyze';
	import { buildPlan, scopeTargets } from '$lib/cv/plan';
	import type { DecodeResult } from '$lib/cv/decode';
	import type { CvProposal, MapStart, RunTarget, StoredRun } from '$lib/cv/types';
	import { fmtRelative } from '$lib/util/format';

	type Stage = 'scope' | 'camera' | 'review';
	let stage = $state<Stage>('scope');
	let scopeKind = $state<'all' | 'node' | 'props'>('all');
	let nodeId = $state('');
	let propIds = $state<string[]>([]);
	let propFilter = $state('');

	let runId = $state('');
	let targets = $state<RunTarget[]>([]);
	let analysis = $state.raw<Analysis | null>(null);
	let proposals = $state<CvProposal[]>([]);
	let photoUrl = $state<string | null>(null);
	let imgW = $state(320);
	let imgH = $state(180);
	let stats = $state<DecodeResult['stats'] | null>(null);
	let failure = $state<string | null>(null);
	let applying = $state(false);
	let appliedIds = $state<string[]>([]);
	let hasPhoto = $state(false);
	let runs = $state<StoredRun[]>([]);

	const show = $derived(app.show);
	const wiredProps = $derived(show?.props.filter((p) => p.segments.length) ?? []);
	const scope = $derived<MapScope>(
		scopeKind === 'node' && nodeId
			? { nodeId }
			: scopeKind === 'props' && propIds.length
				? { propIds }
				: { all: true }
	);
	const outs = $derived(show ? scopeTargets(show, scope) : []);
	const scopeProps = $derived(
		new Set(
			show?.props
				.filter((p) => p.segments.some((s) => outs.some(([n, o]) => n === s.nodeId && o === s.output)))
				.map((p) => p.id)
		)
	);
	const estimateMs = (bitMs: number) =>
		show && outs.length ? buildPlan(show, outs, { bitMs }).schedule.totalMs : 0;
	const estimateS = $derived(Math.ceil(estimateMs(200) / 1000));
	const scopeOk = $derived(outs.length > 0 && (scopeKind !== 'props' || propIds.length > 0));

	$effect(() => {
		if (!nodeId && show?.nodes.length) nodeId = show.nodes[0].id;
	});

	onMount(loadRuns);
	async function loadRuns() {
		runs = await mappingApi.list().catch(() => []);
	}

	const begin = (bitMs: number) => mappingApi.start({ scope, bitMs });

	async function done(r: {
		result: DecodeResult;
		start: MapStart;
		photo: Blob | null;
		photoUrl: string | null;
	}) {
		runId = r.start.runId;
		targets = r.start.targets;
		stats = r.result.stats;
		photoUrl = r.photoUrl;
		hasPhoto = !!r.photo;
		appliedIds = [];
		if (!r.result.ok || !show) {
			failure = r.result.error ?? 'The scan did not work.';
			stage = 'review';
			analysis = null;
			proposals = [];
			return;
		}
		failure = null;
		imgW = r.result.width;
		imgH = r.result.height;
		analysis = analyze(show, targets, r.result.detected, { width: imgW, height: imgH });
		proposals = analysis.proposals;
		stage = 'review';
		const { heat: _heat, ...plain } = r.result;
		void _heat;
		await mappingApi
			.results(runId, {
				detected: plain.detected,
				proposals,
				stats: { ...plain.stats, width: imgW, height: imgH }
			})
			.catch((e) => toasts.warn(`The result wasn't saved on the controller: ${(e as Error).message}`));
		if (r.photo && !app.mock) await mappingApi.uploadPhoto(runId, r.photo).catch(() => (hasPhoto = false));
		loadRuns();
	}

	async function openRun(id: string) {
		const run = await mappingApi.get(id).catch((e) => {
			toasts.error("Couldn't open that scan", (e as Error).message);
			return null;
		});
		if (!run || !show) return;
		runId = run.id;
		targets = run.targets;
		const st = (run.results?.stats ?? {}) as Record<string, unknown>;
		imgW = Number(st.width) || 320;
		imgH = Number(st.height) || 180;
		stats = (run.results?.stats as unknown as DecodeResult['stats']) ?? null;
		failure = run.results ? null : 'This scan has no results (it was stopped or never decoded).';
		analysis = run.results
			? analyze(show, run.targets, run.results.detected, { width: imgW, height: imgH })
			: null;
		proposals = run.results?.proposals ?? [];
		appliedIds = run.appliedProposalIds ?? [];
		hasPhoto = !!run.photo;
		photoUrl = run.photo ? mappingApi.photoUrl(run.id) : null;
		stage = 'review';
	}

	async function removeRun(r: StoredRun) {
		if (
			!(await confirm({
				title: 'Delete this scan?',
				message: 'Its photo and results are removed. Applied changes stay.',
				confirmLabel: 'Delete',
				danger: true
			}))
		)
			return;
		await mappingApi.remove(r.id).catch(() => {});
		loadRuns();
	}

	const selected = $derived(proposals.filter((p) => p.selected && p.data && !appliedIds.includes(p.id)));

	async function apply() {
		if (!selected.length) return;
		applying = true;
		try {
			const res = await mappingApi.apply(
				runId,
				selected.map((p) => p.id)
			);
			appliedIds = [...appliedIds, ...selected.map((p) => p.id)];
			await app.reloadShow();
			toasts.push({
				kind: 'success',
				message: `Applied ${res.applied?.length ?? selected.length} change${(res.applied?.length ?? selected.length) === 1 ? '' : 's'}`,
				action: {
					label: 'Undo',
					run: async () => {
						await api.restoreSnapshot(res.snapshotId);
						appliedIds = [];
						await app.reloadShow();
						toasts.info('Changes undone');
					}
				}
			});
		} catch (e) {
			toasts.error("Couldn't apply the changes", (e as Error).message);
		} finally {
			applying = false;
		}
	}

	async function useAsBackground() {
		try {
			await mappingApi.photoAsBackground(runId);
			toasts.success('Saved as the layout background');
		} catch (e) {
			toasts.error("Couldn't save the photo", (e as Error).message);
		}
	}

	function restart() {
		stage = 'scope';
		analysis = null;
		proposals = [];
		failure = null;
		loadRuns();
	}

	const ICON = {
		swap: ArrowLeftRight,
		reverse: RotateCcw,
		pixelCount: Ruler,
		notSeen: EyeOff,
		layout: LayoutTemplate,
		info: Info
	} as const;
	const GROUPS: { title: string; kinds: CvProposal['kind'][] }[] = [
		{ title: 'Wiring fixes', kinds: ['swap', 'reverse', 'pixelCount'] },
		{ title: 'Layout from the photo', kinds: ['layout'] },
		{ title: 'Worth a look', kinds: ['notSeen', 'info'] }
	];
	const PALETTE = ['#f5a524', '#5b9dff', '#3fcf8e', '#a88bfa', '#ff6b70', '#f5d547', '#4dd4d4', '#ff9ad5'];
	const colorOf = (i: number) => PALETTE[i % PALETTE.length];
	const outline = (a: Analysis) =>
		a.props
			.filter((f) => f.centroid)
			.map((f, i) => ({
				id: f.propId,
				name: f.name,
				color: colorOf(i),
				pts: [...f.hits]
					.sort((x, y) => x.propPixel - y.propPixel)
					.map((h) => `${h.x.toFixed(1)},${h.y.toFixed(1)}`)
					.join(' '),
				dots: f.regions.filter((r) => Number.isFinite(r[0])),
				c: f.centroid!,
				flagged: proposals.some((p) => p.propId === f.propId && ['swap', 'reverse', 'info'].includes(p.kind))
			}));
</script>

<div class="page map">
	<PageHeader
		title="Map my yard"
		subtitle="Point your phone at the house — PixelPlus finds which prop is where."
	/>

	{#if stage === 'scope'}
		<section class="card card-pad stack">
			<div class="hero">
				<span class="icon-tile accent"><Camera size={22} /></span>
				<div>
					<h2>How it works</h2>
					<p class="muted small">
						Every output blinks its own code for about {estimateS || 30} seconds while your phone films the display.
						PixelPlus then shows which prop lit up where, strings that run backwards or are on the wrong port, and
						new layout positions — nothing changes until you apply it.
					</p>
				</div>
			</div>

			<div class="field">
				<span class="eyebrow">What to map</span>
				<div class="scopes" role="radiogroup" aria-label="What to map">
					<button
						role="radio"
						aria-checked={scopeKind === 'all'}
						class:on={scopeKind === 'all'}
						onclick={() => (scopeKind = 'all')}
					>
						<Sparkles size={18} /><strong>Whole display</strong><span class="faint small"
							>{wiredProps.length} props</span
						>
					</button>
					<button
						role="radio"
						aria-checked={scopeKind === 'node'}
						class:on={scopeKind === 'node'}
						onclick={() => (scopeKind = 'node')}
					>
						<Cpu size={18} /><strong>One controller</strong><span class="faint small"
							>{show?.nodes.length ?? 0} controllers</span
						>
					</button>
					<button
						role="radio"
						aria-checked={scopeKind === 'props'}
						class:on={scopeKind === 'props'}
						onclick={() => (scopeKind = 'props')}
					>
						<Shapes size={18} /><strong>Some props</strong><span class="faint small"
							>{propIds.length ? `${propIds.length} picked` : 'pick'}</span
						>
					</button>
				</div>
			</div>

			{#if scopeKind === 'node'}
				<label class="field">
					<span class="eyebrow">Controller</span>
					<select class="select" bind:value={nodeId}>
						{#each show?.nodes ?? [] as n (n.id)}<option value={n.id}>{n.name}</option>{/each}
					</select>
				</label>
			{:else if scopeKind === 'props'}
				<div class="field">
					<input class="input" placeholder="Find a prop" bind:value={propFilter} aria-label="Find a prop" />
					<div class="pick">
						{#each wiredProps.filter((p) => !propFilter || p.name
									.toLowerCase()
									.includes(propFilter.toLowerCase())) as p (p.id)}
							<label class="chip" class:on={propIds.includes(p.id)}>
								<input
									type="checkbox"
									class="sr-only"
									checked={propIds.includes(p.id)}
									onchange={() =>
										(propIds = propIds.includes(p.id)
											? propIds.filter((x) => x !== p.id)
											: [...propIds, p.id])}
								/>
								{#if propIds.includes(p.id)}<Check size={13} />{/if}{p.name}
							</label>
						{/each}
					</div>
				</div>
			{/if}

			<div class="row between wrap summary">
				<span class="muted small">
					{#if outs.length}{scopeProps.size} props on {outs.length} output{outs.length === 1 ? '' : 's'} · about
						{estimateS} s{:else}Nothing wired in this choice yet{/if}
				</span>
				<button class="btn primary lg" disabled={!scopeOk} onclick={() => (stage = 'camera')}
					><Camera size={18} /> Next: the camera</button
				>
			</div>
			{#if !wiredProps.length}
				<div class="notice warn small">
					<TriangleAlert size={16} />
					<div>Wire your props to controller ports first (Props → Wiring, or import from xLights).</div>
				</div>
			{/if}
		</section>

		{#if runs.length}
			<section class="card">
				<div class="card-head"><h3><History size={16} /> Earlier scans</h3></div>
				<div class="list">
					{#each runs.slice(0, 12) as r (r.id)}
						{@const n = r.results?.proposals?.length ?? 0}
						<div class="list-row">
							<button class="grow runbtn" onclick={() => openRun(r.id)}>
								<span class="grow">
									<strong
										>{r.kind === 'pixelCount'
											? 'Pixel count'
											: r.scope?.all
												? 'Whole display'
												: r.scope?.nodeId
													? (show?.nodes.find((x) => x.id === r.scope.nodeId)?.name ?? 'Controller')
													: `${r.scope?.propIds?.length ?? 0} props`}</strong
									>
									<span class="faint small">
										{fmtRelative(r.startedAt)} · {r.results
											? `${n} finding${n === 1 ? '' : 's'}`
											: 'no result'}{r.appliedSnapshotId ? ' · applied' : ''}
									</span>
								</span>
								<ChevronRight size={16} />
							</button>
							<button class="btn ghost icon sm" onclick={() => removeRun(r)} aria-label="Delete this scan"
								><Trash2 size={14} /></button
							>
						</div>
					{/each}
				</div>
			</section>
		{/if}
	{:else if stage === 'camera'}
		<section class="card card-pad">
			{#if app.mock}
				<CameraScan
					{begin}
					{estimateMs}
					ondone={done}
					oncancel={() => (stage = 'scope')}
					startLabel="Map my yard"
				/>
			{:else}
				<SecureGate need="camera" purpose="map your yard">
					<CameraScan
						{begin}
						{estimateMs}
						ondone={done}
						oncancel={() => (stage = 'scope')}
						startLabel="Map my yard"
					/>
				</SecureGate>
			{/if}
		</section>
	{:else}
		<section class="card card-pad stack">
			{#if failure}
				<div class="notice warn">
					<TriangleAlert size={18} />
					<div>
						<strong>{failure}</strong>
						<ul class="small muted">
							<li>Keep every prop you want mapped inside the picture, and the phone still.</li>
							<li>Very bright props? Turn the brightness down a little and scan again.</li>
							<li>Streetlight or porch light shining into the lens? Change the angle.</li>
						</ul>
					</div>
				</div>
				<div class="row">
					<button class="btn primary lg" onclick={() => (stage = 'camera')}
						><RotateCcw size={16} /> Scan again</button
					>
					<button class="btn ghost lg" onclick={restart}>Back</button>
				</div>
			{:else if analysis}
				<div class="row between wrap">
					<div>
						<h2 class="found">Found {analysis.seen} of {analysis.total} props</h2>
						<p class="faint small">
							{proposals.filter((p) => p.kind !== 'layout' && p.kind !== 'info' && p.kind !== 'notSeen')
								.length || 'No'} wiring fix{proposals.filter((p) =>
								['swap', 'reverse', 'pixelCount'].includes(p.kind)
							).length === 1
								? ''
								: 'es'} ·
							{proposals.filter((p) => p.kind === 'layout').length} layout update{proposals.filter(
								(p) => p.kind === 'layout'
							).length === 1
								? ''
								: 's'}
						</p>
					</div>
					{#if stats}
						<div class="chips">
							{#if stats.fps}<span class="chip">{stats.fps} fps</span>{/if}
							{#if stats.movedPasses?.length}<span class="chip warn"
									>Phone moved: {stats.passesUsed} of 3 passes used</span
								>{/if}
							{#if stats.saturatedPct > 20}<span
									class="chip warn"
									title="Lower the brightness for the next scan">Very bright lights</span
								>{/if}
						</div>
					{/if}
				</div>

				<figure class="photo" style:aspect-ratio="{imgW} / {imgH}">
					{#if photoUrl}<img src={photoUrl} alt="Your display during the scan" />{:else}<div
							class="nophoto"
						></div>{/if}
					<svg viewBox="0 0 {imgW} {imgH}" preserveAspectRatio="none" aria-label="Where each prop was seen">
						{#each outline(analysis) as o (o.id)}
							{#if o.pts}<polyline points={o.pts} stroke={o.color} class:flag={o.flagged} />{/if}
							{#each o.dots as d, j (j)}<circle cx={d[0]} cy={d[1]} r="2.2" stroke={o.color} />{/each}
							<text x={o.c[0]} y={Math.max(8, o.c[1] - 6)} fill={o.color}>{o.name}</text>
						{/each}
					</svg>
				</figure>
				{#if hasPhoto && !app.mock}
					<button class="btn ghost sm bg" onclick={useAsBackground}
						><ImageIcon size={14} /> Use this photo as the layout background</button
					>
				{/if}

				{#each GROUPS as g (g.title)}
					{@const items = proposals.filter((p) => g.kinds.includes(p.kind))}
					{#if items.length}
						<div class="group">
							<div class="eyebrow">{g.title}</div>
							{#each items as p (p.id)}
								{@const Icon = ICON[p.kind]}
								{@const done = appliedIds.includes(p.id)}
								{@const actionable = !!p.data && p.kind !== 'notSeen' && p.kind !== 'info'}
								<label class="prop" class:done class:info={!actionable}>
									{#if actionable}
										<input type="checkbox" class="check" bind:checked={p.selected} disabled={done} />
									{:else}
										<span class="ico"><Icon size={16} /></span>
									{/if}
									<span class="grow">
										<span class="pm"
											>{#if actionable}<Icon size={14} />{/if}
											{p.message}{#if done}<span class="badge green">Applied</span>{/if}</span
										>
										{#if p.detail}<span class="faint small">{p.detail}</span>{/if}
									</span>
								</label>
							{/each}
						</div>
					{/if}
				{/each}
				{#if !proposals.length}
					<div class="notice ok small"><Check size={16} /> Everything matches — nothing to change.</div>
				{/if}

				<div class="sticky-actions">
					<button class="btn ghost lg" onclick={restart}>{selected.length ? 'Discard' : 'Done'}</button>
					<button class="btn primary lg grow" disabled={!selected.length || applying} onclick={apply}>
						<Check size={18} />
						{applying
							? 'Applying…'
							: selected.length
								? `Apply ${selected.length} selected`
								: 'Nothing selected'}
					</button>
				</div>
				<p class="faint tiny">A snapshot is taken first, so every change can be undone.</p>
			{/if}
		</section>
	{/if}
</div>

<style>
	.stack {
		display: flex;
		flex-direction: column;
		gap: 16px;
	}
	.hero {
		display: flex;
		gap: 14px;
		align-items: flex-start;
	}
	.hero h2 {
		font-size: 16px;
		margin: 2px 0 4px;
	}
	.field {
		display: flex;
		flex-direction: column;
		gap: 8px;
	}
	.scopes {
		display: grid;
		grid-template-columns: repeat(3, 1fr);
		gap: 8px;
	}
	.scopes button {
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: 4px;
		padding: 14px;
		min-height: 88px;
		border-radius: 14px;
		border: 1px solid var(--border-2);
		background: var(--surface-2);
		color: var(--text);
		text-align: left;
		cursor: pointer;
	}
	.scopes button.on {
		border-color: var(--accent-line);
		background: var(--accent-soft);
		color: var(--accent-text);
	}
	.pick {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
		max-height: 220px;
		overflow: auto;
	}
	.pick .chip {
		cursor: pointer;
		min-height: 36px;
		display: inline-flex;
		align-items: center;
		gap: 4px;
	}
	.pick .chip.on {
		background: var(--accent-soft);
		color: var(--accent-text);
	}
	.summary {
		gap: 10px;
	}
	.card-head h3 {
		display: flex;
		align-items: center;
		gap: 8px;
		font-size: 14px;
	}
	.runbtn {
		display: flex;
		align-items: center;
		gap: 10px;
		background: none;
		border: 0;
		color: inherit;
		text-align: left;
		cursor: pointer;
		padding: 4px 0;
		min-height: 44px;
	}
	.runbtn .grow {
		display: flex;
		flex-direction: column;
	}
	.found {
		font-size: 22px;
		letter-spacing: -0.02em;
		margin: 0;
	}
	.chips {
		display: flex;
		gap: 6px;
		flex-wrap: wrap;
	}
	.chip.warn {
		background: var(--accent-soft);
		color: var(--accent-text);
	}
	.photo {
		position: relative;
		margin: 0;
		width: 100%;
		border-radius: 14px;
		overflow: hidden;
		background: #030305;
	}
	.photo img,
	.photo svg,
	.nophoto {
		position: absolute;
		inset: 0;
		width: 100%;
		height: 100%;
	}
	.photo img {
		object-fit: cover;
		filter: brightness(0.8);
	}
	.photo polyline {
		fill: none;
		stroke-width: 1.6;
		stroke-linejoin: round;
		stroke-linecap: round;
		vector-effect: non-scaling-stroke;
		opacity: 0.95;
	}
	.photo circle {
		fill: none;
		stroke-width: 1.4;
		vector-effect: non-scaling-stroke;
	}
	.photo polyline.flag {
		stroke-dasharray: 4 3;
	}
	.photo text {
		font-size: 7px;
		font-weight: 600;
		paint-order: stroke;
		stroke: rgba(0, 0, 0, 0.8);
		stroke-width: 2px;
		text-anchor: middle;
	}
	.bg {
		align-self: flex-start;
	}
	.group {
		display: flex;
		flex-direction: column;
		gap: 6px;
	}
	.prop {
		display: flex;
		gap: 12px;
		align-items: flex-start;
		padding: 12px;
		border-radius: 12px;
		background: var(--surface-2);
		border: 1px solid var(--border);
		cursor: pointer;
		min-height: 44px;
	}
	.prop.info {
		cursor: default;
	}
	.prop.done {
		opacity: 0.6;
	}
	.prop .grow {
		display: flex;
		flex-direction: column;
		gap: 3px;
	}
	.pm {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		font-weight: 560;
		flex-wrap: wrap;
	}
	.prop .check {
		width: 22px;
		height: 22px;
		margin-top: 1px;
		accent-color: var(--accent);
	}
	.ico {
		color: var(--text-3);
		width: 22px;
		display: grid;
		place-items: center;
	}
	.sticky-actions {
		position: sticky;
		bottom: calc(var(--tabbar-h, 0px) + 8px);
		display: flex;
		gap: 8px;
		padding: 10px;
		margin: 0 -10px;
		border-radius: 14px;
		background: color-mix(in srgb, var(--surface) 88%, transparent);
		backdrop-filter: blur(8px);
	}
	.sticky-actions .grow {
		flex: 1;
	}
	.notice ul {
		margin: 6px 0 0;
		padding-left: 18px;
	}
	@media (max-width: 640px) {
		.scopes {
			grid-template-columns: 1fr;
		}
		.scopes button {
			min-height: 56px;
			flex-direction: row;
			align-items: center;
			gap: 10px;
		}
		.scopes button .faint {
			margin-left: auto;
		}
		.summary .btn {
			width: 100%;
		}
	}
</style>
