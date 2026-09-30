<!--
	Settings → Power (F12, ARCHITECTURE §12.11). WS3.
	The brightness limiter (mode, safety, caps), power supplies and what they feed, late-night
	dimming, a live view of every budget, and a per-sequence check (supply view + simulated limiting).
-->
<script lang="ts">
	import { onDestroy, onMount } from 'svelte';
	import {
		ArrowLeft,
		Zap,
		Gauge,
		Moon,
		Plus,
		Trash2,
		TriangleAlert,
		Info,
		Activity,
		Plug,
		Check,
		Search
	} from '@lucide/svelte';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import Segmented from '$lib/components/ui/Segmented.svelte';
	import Slider from '$lib/components/ui/Slider.svelte';
	import TimeSpecPicker from '$lib/components/schedule/TimeSpecPicker.svelte';
	import { api, request } from '$lib/api/client';
	import type { DimWindow, LimiterMode, PowerSettings, PowerSupply, Weekday } from '$lib/api/types';
	import { app } from '$lib/stores/app.svelte';
	import { confirm, toasts } from '$lib/stores/toasts.svelte';
	import { newId } from '$lib/util/id';
	import {
		groupKind,
		groupLabel,
		load,
		nodeName,
		type LiveNode,
		type PowerEstimateX
	} from '$lib/power/power';

	const show = $derived(app.show);
	const isFollower = $derived(app.system?.role === 'follower');
	const DEFAULTS: PowerSettings = { mode: 'warn', safety: 0.9, dim: [], maxBrightness: 100 };
	const power = $derived<PowerSettings>({ ...DEFAULTS, ...(show?.settings.power ?? {}) });
	const supplies = $derived(show?.powerSupplies ?? []);
	const location = $derived(show?.schedule.location ?? { lat: 0, lon: 0, timezone: 'UTC', name: '' });

	// ----- settings -----
	let saving = $state(false);
	async function save(patch: Partial<PowerSettings>) {
		saving = true;
		app.updateShow((s) => {
			s.settings.power = { ...DEFAULTS, ...(s.settings.power ?? {}), ...patch };
		});
		try {
			await api.saveSettings({ power: patch });
			await app.reloadShow();
		} catch (e) {
			toasts.error('Couldn’t save the power settings', (e as Error).message);
			await app.reloadShow().catch(() => {});
		} finally {
			saving = false;
		}
	}
	const modeHelp: Record<LimiterMode, string> = {
		off: 'No estimates, no limits.',
		warn: 'Estimates current every frame and tells you when a budget would be exceeded, without dimming anything.',
		limit: 'Dims just the outputs on an overloaded fuse or supply, smoothly, so nothing trips.'
	};
	function numOrUndef(v: string): number | undefined {
		const n = Number(v);
		return v.trim() && Number.isFinite(n) && n > 0 ? n : undefined;
	}

	// ----- live view -----
	let live = $state<LiveNode[]>([]);
	let liveError = $state<string | null>(null);
	let poll: ReturnType<typeof setInterval> | undefined;
	let off: (() => void) | undefined;
	async function loadLive() {
		try {
			const r = await request<{ nodes: LiveNode[] }>('GET', '/power/live');
			live = r.nodes;
			liveError = null;
		} catch (e) {
			liveError = (e as Error).message;
		}
	}
	onMount(() => {
		void loadLive();
		poll = setInterval(loadLive, 3000);
		off = app.onMessage('power', (d) => (live = d.nodes as LiveNode[]));
	});
	onDestroy(() => {
		clearInterval(poll);
		off?.();
	});
	const liveGroups = $derived(live.flatMap((n) => n.groups.map((g) => ({ node: n, g }))));

	// ----- supplies -----
	let editing = $state<PowerSupply | null>(null);
	function newSupply() {
		editing = {
			id: newId(),
			name: `Power supply ${supplies.length + 1}`,
			volts: 12,
			amps: 29,
			receiverIds: [],
			directOutputs: []
		};
	}
	function edit(s: PowerSupply) {
		editing = structuredClone($state.snapshot(s)) as PowerSupply;
	}
	const isNew = $derived(!!editing && !supplies.some((s) => s.id === editing!.id));
	async function saveSupply() {
		if (!editing) return;
		const s = $state.snapshot(editing) as PowerSupply;
		try {
			if (isNew) await request('POST', '/power-supplies', s);
			else await request('PUT', `/power-supplies/${s.id}`, s);
			await app.reloadShow();
			toasts.success(`Saved “${s.name}”`);
			editing = null;
		} catch (e) {
			toasts.error('Couldn’t save the power supply', (e as Error).message);
		}
	}
	async function removeSupply(s: PowerSupply) {
		if (!(await confirm({ title: `Remove “${s.name}”?`, confirmLabel: 'Remove', danger: true }))) return;
		await app.mutate(() => request('DELETE', `/power-supplies/${s.id}`), { success: `Removed ${s.name}` });
	}
	function toggleReceiver(id: string) {
		if (!editing) return;
		const has = editing.receiverIds.includes(id);
		editing.receiverIds = has ? editing.receiverIds.filter((r) => r !== id) : [...editing.receiverIds, id];
	}
	const directCandidates = $derived(
		(show?.nodes ?? []).flatMap((n) =>
			n.outputs
				.filter((o) => !show?.receivers.some((r) => r.nodeId === n.id && Math.ceil(o.index / 4) === r.jack))
				.map((o) => ({ nodeId: n.id, output: o.index, label: `${n.name} ${o.label || `output ${o.index}`}` }))
		)
	);
	function toggleDirect(nodeId: string, output: number) {
		if (!editing) return;
		const has = editing.directOutputs.some((d) => d.nodeId === nodeId && d.output === output);
		editing.directOutputs = has
			? editing.directOutputs.filter((d) => !(d.nodeId === nodeId && d.output === output))
			: [...editing.directOutputs, { nodeId, output }];
	}
	function feeds(s: PowerSupply): string {
		const r = s.receiverIds.map((id) => show?.receivers.find((x) => x.id === id)?.name).filter(Boolean);
		const d = s.directOutputs.length
			? [`${s.directOutputs.length} output${s.directOutputs.length === 1 ? '' : 's'}`]
			: [];
		const all = [...r, ...d];
		return all.length ? all.join(', ') : 'Nothing yet';
	}

	// ----- dimming -----
	const WEEK: { v: Weekday; l: string }[] = [
		{ v: 'mon', l: 'Mo' },
		{ v: 'tue', l: 'Tu' },
		{ v: 'wed', l: 'We' },
		{ v: 'thu', l: 'Th' },
		{ v: 'fri', l: 'Fr' },
		{ v: 'sat', l: 'Sa' },
		{ v: 'sun', l: 'Su' }
	];
	let dim = $state<DimWindow[]>([]);
	let dimKey = '';
	$effect(() => {
		const k = JSON.stringify(power.dim);
		if (k !== dimKey) {
			dimKey = k;
			dim = structuredClone(power.dim);
		}
	});
	// Edits (times, brightness, days) save themselves shortly after.
	$effect(() => {
		if (JSON.stringify(dim) !== dimKey) dimChanged();
	});
	let dimTimer: ReturnType<typeof setTimeout> | undefined;
	function dimChanged() {
		clearTimeout(dimTimer);
		dimTimer = setTimeout(() => {
			const d = $state.snapshot(dim) as DimWindow[];
			dimKey = JSON.stringify(d);
			void save({ dim: d });
		}, 500);
	}
	function addDim() {
		dim = [
			...dim,
			{
				from: { kind: 'clock', time: '22:00' },
				to: { kind: 'clock', time: '06:00' },
				brightness: 40,
				days: []
			}
		];
		dimChanged();
	}
	function removeDim(i: number) {
		dim = dim.filter((_, j) => j !== i);
		dimChanged();
	}
	function toggleDay(i: number, d: Weekday) {
		const w = dim[i];
		w.days = w.days.includes(d) ? w.days.filter((x) => x !== d) : [...w.days, d];
		dimChanged();
	}

	// ----- sequence check -----
	let seqId = $state('');
	let est = $state<PowerEstimateX | null>(null);
	let checking = $state(false);
	async function check() {
		if (!seqId) return;
		checking = true;
		try {
			est = await request<PowerEstimateX>('GET', `/power/estimate?sequenceId=${encodeURIComponent(seqId)}`);
		} catch (e) {
			toasts.error('Couldn’t check that sequence', (e as Error).message);
		} finally {
			checking = false;
		}
	}
	const pct = (v: number) => `${Math.round(v * 100)} %`;
	const amps = (v: number | null | undefined) => (v == null ? '—' : `${v.toFixed(v < 10 ? 2 : 1)} A`);
</script>

<svelte:head><title>Power · Settings · PixelPlus</title></svelte:head>

<div class="page power">
	<a class="btn ghost sm back" href="/settings"><ArrowLeft size={16} /> Settings</a>
	<PageHeader
		title="Power"
		subtitle="Keep fuses and power supplies within their ratings — automatically, without visible pumping — and dim the display late at night."
	/>

	{#if isFollower}
		<div class="notice info">
			<Info size={18} />
			<div>
				This controller follows your show leader and uses the power budget the leader sends. Change it there.
			</div>
		</div>
	{:else if !show}
		<div class="card skeleton sk"></div>
	{:else}
		<!-- Limiter -->
		<section class="card">
			<div class="card-head">
				<Gauge size={18} />
				<h2 class="grow">Brightness limiter</h2>
				{#if saving}<span class="faint small">Saving…</span>{/if}
			</div>
			<div class="card-body col">
				<Segmented
					label="Limiter mode"
					value={power.mode}
					options={[
						{ value: 'off', label: 'Off' },
						{ value: 'warn', label: 'Warn only' },
						{ value: 'limit', label: 'Limit' }
					]}
					onchange={(v) => save({ mode: v as LimiterMode })}
				/>
				<p class="muted small">{modeHelp[power.mode]}</p>
				{#if power.mode !== 'off'}
					<div class="grid2">
						<div class="field">
							<div class="row between">
								<span class="label">Safety margin</span><span class="num v"
									>{pct(power.safety)} of each rating</span
								>
							</div>
							<Slider
								label="Safety margin"
								min={0.5}
								max={1}
								step={0.05}
								value={power.safety}
								onchange={(v) => save({ safety: v })}
								format={pct}
							/>
							<span class="faint tiny">The estimate can be off by ±20 %; 90 % is a good start.</span>
						</div>
						<div class="field">
							<span class="label">Whole-display cap (optional)</span>
							<div class="row" style="gap:8px">
								<label class="row cap"
									><input
										class="input num"
										type="number"
										min="0"
										step="0.5"
										placeholder="—"
										value={power.globalAmps ?? ''}
										onchange={(e) => save({ globalAmps: numOrUndef((e.target as HTMLInputElement).value) })}
									/><span class="faint small">A</span></label
								>
								<span class="faint small">or</span>
								<label class="row cap"
									><input
										class="input num"
										type="number"
										min="0"
										step="50"
										placeholder="—"
										value={power.globalWatts ?? ''}
										onchange={(e) => save({ globalWatts: numOrUndef((e.target as HTMLInputElement).value) })}
									/><span class="faint small">W at the wall</span></label
								>
							</div>
							<span class="faint tiny">For example a 15 A household circuit ≈ 1800 W.</span>
						</div>
					</div>
				{/if}
				<p class="faint tiny row" style="gap:6px">
					<Info size={13} /> Receiver ports use their fuse ratings (diffrx: 6 A) and the receiver's main fuse; set
					those on the Controllers page.
				</p>
			</div>
		</section>

		<!-- Live -->
		{#if power.mode !== 'off'}
			<section class="card">
				<div class="card-head">
					<Activity size={18} />
					<h2 class="grow">Right now</h2>
					{#if app.status?.power?.limiting}
						<span class="badge accent">Limiting · {pct(app.status.power.minScale)}</span>
					{:else}
						<span class="badge green"><Check size={12} /> Within budget</span>
					{/if}
				</div>
				<div class="card-body">
					{#if liveError}
						<p class="muted small">{liveError}</p>
					{:else if !liveGroups.length}
						<p class="muted small">
							No budgets yet: add a power supply below, or set receiver fuses on the Controllers page.
						</p>
					{:else}
						<div class="groups">
							{#each liveGroups as { node, g } (node.nodeId + g.id)}
								{@const l = load(g.amps, g.budget)}
								<div class="grp" class:limit={g.scale < 0.995}>
									<div class="row between">
										<span class="ellipsis"
											><span class="kind">{groupKind(g.id)}</span>
											{groupLabel(show, g.id)}
											{#if live.length > 1}<span class="faint small">
													· {nodeName(show, node.nodeId)}</span
												>{/if}</span
										>
										<span class="num small">{amps(g.amps)} / {amps(g.budget)}</span>
									</div>
									<div class="bar">
										<div class="fill" class:hot={l > 0.9} style:width="{Math.min(100, l * 100)}%"></div>
									</div>
									{#if g.scale < 0.995}
										<span class="tiny warn-t"
											>{power.mode === 'limit' ? 'Dimmed to' : 'Would dim to'} {pct(g.scale)}</span
										>
									{/if}
								</div>
							{/each}
						</div>
					{/if}
				</div>
			</section>
		{/if}

		<!-- Supplies -->
		<section class="card">
			<div class="card-head">
				<Plug size={18} />
				<h2 class="grow">Power supplies</h2>
				<button class="btn sm" onclick={newSupply}><Plus size={14} /> Add supply</button>
			</div>
			<div class="card-body col">
				{#if !supplies.length && !editing}
					<p class="muted small">
						Tell PixelPlus which supply feeds which receivers so it can keep each one within its rating. A 350
						W 12 V supply is about 29 A.
					</p>
				{/if}
				{#each supplies as s (s.id)}
					<div class="sup row between">
						<div class="grow">
							<div class="name">{s.name}</div>
							<div class="faint small">
								{s.volts} V · {s.amps} A ({Math.round(s.volts * s.amps)} W) · {feeds(s)}
							</div>
						</div>
						<button class="btn sm" onclick={() => edit(s)}>Edit</button>
						<button class="btn sm icon danger" aria-label="Remove {s.name}" onclick={() => removeSupply(s)}
							><Trash2 size={14} /></button
						>
					</div>
				{/each}
				{#if editing}
					<div class="editor">
						<div class="grid3">
							<label class="field"
								><span class="label">Name</span><input
									class="input"
									bind:value={editing.name}
									maxlength="120"
								/></label
							>
							<label class="field"
								><span class="label">Volts</span><input
									class="input num"
									type="number"
									min="1"
									max="60"
									step="0.1"
									bind:value={editing.volts}
								/></label
							>
							<label class="field"
								><span class="label">Amps</span><input
									class="input num"
									type="number"
									min="0.1"
									step="0.5"
									bind:value={editing.amps}
								/></label
							>
						</div>
						<div class="field">
							<span class="label">Feeds these receivers</span>
							<div class="row wrap">
								{#each show.receivers as r (r.id)}
									<button
										type="button"
										class="chip"
										aria-pressed={editing.receiverIds.includes(r.id)}
										onclick={() => toggleReceiver(r.id)}>{r.name}</button
									>
								{:else}<span class="faint small">No receivers yet.</span>{/each}
							</div>
						</div>
						{#if directCandidates.length}
							<details>
								<summary class="small">Outputs without a receiver ({editing.directOutputs.length})</summary>
								<div class="row wrap" style="margin-top:8px">
									{#each directCandidates as c (c.nodeId + c.output)}
										<button
											type="button"
											class="chip"
											aria-pressed={editing.directOutputs.some(
												(d) => d.nodeId === c.nodeId && d.output === c.output
											)}
											onclick={() => toggleDirect(c.nodeId, c.output)}>{c.label}</button
										>
									{/each}
								</div>
							</details>
						{/if}
						<div class="row" style="justify-content:flex-end;gap:8px">
							<button class="btn" onclick={() => (editing = null)}>Cancel</button>
							<button class="btn primary" onclick={saveSupply}
								><Check size={14} /> {isNew ? 'Add' : 'Save'}</button
							>
						</div>
					</div>
				{/if}
			</div>
		</section>

		<!-- Dimming -->
		<section class="card">
			<div class="card-head">
				<Moon size={18} />
				<h2 class="grow">Late-night dimming</h2>
				<button class="btn sm" onclick={addDim}><Plus size={14} /> Add window</button>
			</div>
			<div class="card-body col">
				<div class="field">
					<div class="row between">
						<span class="label">Brightest the display ever gets</span><span class="num v"
							>{power.maxBrightness} %</span
						>
					</div>
					<Slider
						label="Maximum brightness"
						min={10}
						max={100}
						step={5}
						value={power.maxBrightness}
						onchange={(v) => save({ maxBrightness: v })}
						format={(v) => `${v} %`}
					/>
				</div>
				{#if !dim.length}
					<p class="muted small">
						Dim the lights after a certain time, e.g. to 40 % from 10 pm — the neighbours will thank you.
						Followers dim too.
					</p>
				{/if}
				{#each dim as w, i (i)}
					<div class="dimw">
						<div class="grid2">
							<TimeSpecPicker label="From" bind:value={w.from} {location} />
							<TimeSpecPicker label="Until" bind:value={w.to} {location} />
						</div>
						<div class="row wrap between" style="gap:12px">
							<div class="row wrap days" role="group" aria-label="Days">
								{#each WEEK as d (d.v)}
									<button
										type="button"
										class="chip sm"
										aria-pressed={w.days.includes(d.v)}
										onclick={() => toggleDay(i, d.v)}>{d.l}</button
									>
								{/each}
								<span class="faint tiny">{w.days.length ? '' : 'every day'}</span>
							</div>
							<div class="row bright">
								<Slider
									label="Brightness"
									min={5}
									max={100}
									step={5}
									bind:value={w.brightness}
									onchange={dimChanged}
									format={(v) => `${v} %`}
								/>
								<span class="num v">{w.brightness} %</span>
								<button class="btn sm icon danger" aria-label="Remove window" onclick={() => removeDim(i)}
									><Trash2 size={14} /></button
								>
							</div>
						</div>
					</div>
				{/each}
			</div>
		</section>

		<!-- Check a sequence -->
		<section class="card">
			<div class="card-head">
				<Zap size={18} />
				<h2 class="grow">Check a sequence</h2>
			</div>
			<div class="card-body col">
				<div class="row" style="gap:8px">
					<select class="select grow" bind:value={seqId} aria-label="Sequence">
						<option value="">Pick a sequence…</option>
						{#each show.sequences as s (s.id)}<option value={s.id}>{s.name}</option>{/each}
					</select>
					<button class="btn" onclick={check} disabled={!seqId || checking}><Search size={14} /> Check</button
					>
				</div>
				{#if est}
					{#if est.perSupply?.length}
						<table class="tbl">
							<thead><tr><th>Supply</th><th>Peak</th><th>Average</th><th>Rating</th></tr></thead>
							<tbody>
								{#each est.perSupply as s (s.supplyId)}
									<tr class={s.status}>
										<td>{s.name}</td>
										<td class="num">{amps(s.peakAmps)} ({Math.round(s.peakWatts)} W)</td>
										<td class="num">{amps(s.avgAmps)}</td>
										<td class="num">{amps(s.ratedAmps)}</td>
									</tr>
								{/each}
							</tbody>
						</table>
					{/if}
					{#if est.limited?.length}
						<div class="notice warn">
							<TriangleAlert size={16} />
							<div>
								{#each est.limited as l (l.nodeId + l.groupId)}
									<div>
										<strong>{l.label}</strong> would be dimmed for {l.seconds.toFixed(0)} s (down to {pct(
											l.minScale
										)}).
									</div>
								{/each}
							</div>
						</div>
					{:else}
						<p class="row good small"><Check size={14} /> Stays within every budget.</p>
					{/if}
					{#each est.warnings as w, i (i)}<p class="faint small">{w}</p>{/each}
				{/if}
			</div>
		</section>
	{/if}
</div>

<style>
	.power {
		display: flex;
		flex-direction: column;
		gap: 16px;
	}
	.back {
		align-self: flex-start;
	}
	.sk {
		height: 240px;
	}
	.col {
		display: flex;
		flex-direction: column;
		gap: 12px;
	}
	.grid2 {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: 16px;
	}
	.grid3 {
		display: grid;
		grid-template-columns: 2fr 1fr 1fr;
		gap: 12px;
	}
	.cap .input {
		width: 90px;
	}
	.cap {
		gap: 6px;
	}
	.v {
		font-size: 12px;
	}
	.groups {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(260px, 1fr));
		gap: 12px;
	}
	.grp {
		display: flex;
		flex-direction: column;
		gap: 6px;
		padding: 10px 12px;
		border: 1px solid var(--border);
		border-radius: 10px;
	}
	.grp.limit {
		border-color: var(--accent-line, var(--accent));
	}
	.kind {
		font-size: 10px;
		text-transform: uppercase;
		letter-spacing: 0.06em;
		color: var(--text-3);
		margin-right: 4px;
	}
	.bar {
		height: 6px;
		border-radius: 3px;
		background: var(--surface-2);
		overflow: hidden;
	}
	.fill {
		height: 100%;
		background: var(--green);
		transition: width 400ms ease;
	}
	.fill.hot {
		background: var(--accent);
	}
	.warn-t {
		color: var(--accent-text);
	}
	.sup {
		gap: 10px;
		padding: 10px 0;
		border-bottom: 1px solid var(--border);
	}
	.name {
		font-weight: 590;
	}
	.editor {
		display: flex;
		flex-direction: column;
		gap: 12px;
		padding: 14px;
		border-radius: 12px;
		border: 1px solid var(--accent-line, var(--border-2));
	}
	.dimw {
		display: flex;
		flex-direction: column;
		gap: 10px;
		padding: 12px;
		border: 1px solid var(--border);
		border-radius: 12px;
	}
	.days {
		gap: 4px;
	}
	.bright {
		gap: 8px;
		min-width: 240px;
	}
	.tbl {
		width: 100%;
		border-collapse: collapse;
		font-size: 13px;
	}
	.tbl th,
	.tbl td {
		text-align: left;
		padding: 6px 8px;
		border-bottom: 1px solid var(--border);
	}
	.tbl tr.over td {
		color: var(--red);
	}
	.tbl tr.warn td {
		color: var(--accent-text);
	}
	.good {
		color: var(--green);
		gap: 6px;
	}
	@media (max-width: 640px) {
		.grid2,
		.grid3 {
			grid-template-columns: 1fr;
		}
		.bright {
			min-width: 0;
			width: 100%;
		}
	}
</style>
