<script lang="ts">
	import { api } from '$lib/api/client';
	import type { BoardKind, DiscoveredNode, Node, OutputConfig, Receiver, ReceiverKind } from '$lib/api/types';
	import { COLOR_ORDERS } from '$lib/api/types';
	import { app } from '$lib/stores/app.svelte';
	import { toasts, confirm } from '$lib/stores/toasts.svelte';
	import {
		BOARDS,
		RECEIVERS,
		needsPort3Warning,
		nodeUsage,
		portName,
		pixelsOnOutput,
		propsOnOutput,
		MAX_PIXELS_PER_OUTPUT
	} from '$lib/util/boards';
	import { sortable } from '$lib/actions/sortable';
	import { reorderChain, wirePropToPort, flashPort, COLOR_CORRECTION, correctionIndex } from '$lib/wiring';
	import { fmtTemp, tempUnitOf } from '$lib/util/units';
	import { fmtRelative } from '$lib/util/format';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import GeometryBanner from '$lib/components/ui/GeometryBanner.svelte';
	import Modal from '$lib/components/ui/Modal.svelte';
	import Switch from '$lib/components/ui/Switch.svelte';
	import Skeleton from '$lib/components/ui/Skeleton.svelte';
	import BoardDiagram from '$lib/components/viz/BoardDiagram.svelte';
	import SyncBadge from '$lib/components/ui/SyncBadge.svelte';
	import {
		Radar,
		Plus,
		Lightbulb,
		TriangleAlert,
		Info,
		Pencil,
		Trash2,
		Thermometer,
		Zap,
		Activity,
		Gauge,
		RefreshCw,
		Radio,
		HardDrive,
		CircleCheck,
		ChevronDown,
		ChevronUp,
		Cpu,
		Settings2,
		GripVertical,
		Search
	} from '@lucide/svelte';
	import { slide } from 'svelte/transition';

	const show = $derived(app.show);
	let discovered = $state<DiscoveredNode[]>([]);
	let scanning = $state(false);
	let adopting = $state<DiscoveredNode | null>(null);
	let adoptName = $state('');
	let adoptBusy = $state(false);
	let selJack = $state<Record<string, number | null>>({});
	let editOut = $state<string | null>(null);
	let rxModal = $state(false);
	let rxDraft = $state<Partial<Receiver>>({});
	let renameNode = $state<Node | null>(null);
	let renameValue = $state('');
	let eepromOpen = $state(false);
	let eepromBoard = $state<BoardKind>('difftx');
	let eepromRev = $state('E');

	async function scan(manual = false) {
		scanning = true;
		try {
			discovered = await api.discovered();
			if (manual)
				toasts.info(
					discovered.length
						? `Found ${discovered.length} new controller${discovered.length > 1 ? 's' : ''}`
						: 'No new controllers on the network'
				);
		} catch {
			/* ignore */
		} finally {
			scanning = false;
		}
	}
	$effect(() => {
		scan();
		const t = setInterval(scan, 5000);
		return () => clearInterval(t);
	});

	/** A real name to start from ("Controller 3"), so nobody adopts a box called "pixelplus-3f2a". */
	function suggestName() {
		const taken = new Set(show?.nodes.map((n) => n.name.toLowerCase()) ?? []);
		for (let i = (show?.nodes.length ?? 0) + 1; ; i++)
			if (!taken.has(`controller ${i}`)) return `Controller ${i}`;
	}
	// ---- "Join another show": this leader becomes a follower of another one.
	let joinOpen = $state(false);
	let joinAddr = $state('');
	let joinBusy = $state(false);
	async function joinShow() {
		joinBusy = true;
		try {
			await api.joinShow(joinAddr.trim() || undefined);
			toasts.success(
				'Ready to join: open PixelPlus on the other show leader and adopt this controller within 15 minutes.'
			);
			joinOpen = false;
		} catch (e) {
			toasts.error('Could not get ready to join', (e as Error).message);
		} finally {
			joinBusy = false;
		}
	}

	async function adopt() {
		if (!adopting || !adoptName.trim()) return;
		adoptBusy = true;
		await app.mutate(() => api.adopt(adopting!.id, adoptName.trim(), adopting!.joining || undefined));
		adoptBusy = false;
		adopting = null;
		scan();
	}

	function jacksOf(n: Node): { jack: number | null; outputs: number[] }[] {
		if (n.board === 'difftxlarge')
			return Array.from({ length: 15 }, (_, j) => ({
				jack: j + 1,
				outputs: [1, 2, 3, 4].map((p) => j * 4 + p)
			}));
		if (n.board === 'difftx') return [{ jack: 1, outputs: [1, 2, 3, 4] }];
		if (n.board === 'diffsmart') return [{ jack: null, outputs: [1, 2, 3, 4] }];
		return [];
	}
	function currentJack(n: Node) {
		const groups = jacksOf(n);
		const j = selJack[n.id];
		if (j === undefined) {
			// first used jack
			return groups.find((g) => g.outputs.some((o) => pixelsOnOutput(show!, n.id, o))) ?? groups[0];
		}
		return groups.find((g) => g.jack === j) ?? groups[0];
	}
	function rxOn(n: Node, jack: number | null) {
		return jack == null ? undefined : show?.receivers.find((r) => r.nodeId === n.id && r.jack === jack);
	}
	function portPixels(n: Node) {
		return n.outputs.map((o) => pixelsOnOutput(show!, n.id, o.index));
	}
	function rxNames(n: Node) {
		const m: Record<number, string> = {};
		for (const r of show?.receivers ?? []) if (r.nodeId === n.id) m[r.jack] = r.name;
		return m;
	}

	async function saveOutput(n: Node, o: OutputConfig, patch: Partial<OutputConfig>) {
		app.updateShow((s) => {
			const t = s.nodes.find((x) => x.id === n.id)?.outputs.find((x) => x.index === o.index);
			if (t) Object.assign(t, patch);
		});
		try {
			await api.saveOutput(n.id, o.index, patch);
		} catch (e) {
			toasts.error('Could not save output', (e as Error).message);
			app.reloadShow();
		}
	}

	function newReceiver(n: Node, jack: number) {
		rxDraft = { name: '', kind: 'diffrx', nodeId: n.id, jack, fuseAmps: 6, location: '' };
		rxModal = true;
	}
	function editReceiver(r: Receiver) {
		rxDraft = structuredClone($state.snapshot(r) as Receiver);
		rxModal = true;
	}
	async function saveReceiver(e: SubmitEvent) {
		e.preventDefault();
		const d = $state.snapshot(rxDraft) as Receiver;
		if (!d.name?.trim()) return;
		if (d.id) await app.mutate(() => api.receivers.update(d.id, d), { success: `Saved ${d.name}` });
		else await app.mutate(() => api.receivers.create(d), { success: `Added ${d.name} receiver` });
		rxModal = false;
	}
	async function deleteReceiver(r: Receiver) {
		if (
			!(await confirm({
				title: `Remove ${r.name} receiver?`,
				message: 'Props stay wired to the same ports; they just won’t show the receiver name.',
				confirmLabel: 'Remove',
				danger: true
			}))
		)
			return;
		const copy = structuredClone($state.snapshot(r) as Receiver);
		await app.mutate(() => api.receivers.remove(r.id));
		rxModal = false;
		toasts.success(`Removed ${r.name}`, {
			label: 'Undo',
			run: () => app.mutate(() => api.receivers.create(copy))
		});
	}
	async function doRename(e: SubmitEvent) {
		e.preventDefault();
		if (!renameNode) return;
		await app.mutate(() => api.nodes.update(renameNode!.id, { name: renameValue.trim() }), {
			success: 'Renamed'
		});
		renameNode = null;
	}
	async function removeNode(n: Node) {
		if (
			!(await confirm({
				title: `Release ${n.name}?`,
				message:
					'It stops following this show and goes back to “waiting to be adopted”. Props wired to it stay in your show but won’t light.',
				confirmLabel: 'Release controller',
				danger: true
			}))
		)
			return;
		await app.mutate(() => api.nodes.remove(n.id), { success: `${n.name} released` });
	}
	async function identify(n: Node) {
		await api.identifyNode(n.id).catch((e) => toasts.error('Identify failed', e.message));
	}
	async function writeEeprom() {
		if (
			!(await confirm({
				title: 'Write board EEPROM?',
				message: `This stores “${BOARDS[eepromBoard].name} rev ${eepromRev}” on the board’s memory chip. Only do this if the board was detected wrongly.`,
				confirmLabel: 'Write EEPROM',
				danger: true
			}))
		)
			return;
		try {
			await api.writeEeprom(eepromBoard, eepromRev);
			toasts.success('EEPROM written — reboot to apply');
			eepromOpen = false;
		} catch (e) {
			toasts.error('EEPROM write failed', (e as Error).message);
		}
	}

	const sensorIcon = { temperature: Thermometer, voltage: Zap, current: Activity, power: Gauge };
	const tunit = $derived(tempUnitOf(show));

	// ---- "+ Add a prop to this port"
	let addTo = $state<{ nodeId: string; output: number } | null>(null);
	let addQ = $state('');
	const addChoices = $derived.by(() => {
		if (!show || !addTo) return [];
		const needle = addQ.trim().toLowerCase();
		return show.props
			.filter((p) => !needle || p.name.toLowerCase().includes(needle))
			.map((p) => ({ p, left: p.pixelCount - p.segments.reduce((n, sg) => n + sg.pixelCount, 0) }))
			.sort((a, b) => Number(b.left > 0) - Number(a.left > 0) || a.p.name.localeCompare(b.p.name));
	});
</script>

<div class="page">
	<PageHeader
		title="Controllers"
		subtitle="Set everything up here on the leader — followers receive their settings and sequences automatically."
	>
		{#snippet actions()}
			<button class="btn ghost" onclick={() => (joinOpen = true)}>Join another show…</button>
			<button class="btn" onclick={() => scan(true)} disabled={scanning}
				><span class:spin={scanning} class="ic"><RefreshCw size={16} /></span> Scan network</button
			>
		{/snippet}
	</PageHeader>

	<GeometryBanner />

	{#if discovered.length}
		<section class="found card" transition:slide>
			<div class="found-head">
				<span class="radar"><Radar size={20} /></span>
				<div class="grow">
					<h2>New controllers found</h2>
					<p class="muted small">
						These PixelPlus controllers are on your network and waiting to join a show.
					</p>
				</div>
			</div>
			{#each discovered as d (d.id)}
				<div class="found-row">
					<div class="mini-board"><BoardDiagram board={d.board} compact /></div>
					<div class="grow">
						<strong>{d.name}</strong>
						<div class="faint small">
							{BOARDS[d.board]?.name ?? d.board}{d.boardRev ? ` · rev ${d.boardRev}` : ''} · {d.ip ??
								'unknown address'}{d.pi ? ` · ${d.pi.replace(' Rev 1.0', '')}` : ''}
						</div>
						{#if d.duplicate}<div class="small" style="color:var(--red)">
								Possible duplicate: two devices on the network claim to be this controller. Make sure only one
								uses this SD card.
							</div>{:else if d.joining}<div class="small muted">
								A show leader that wants to join this show. Adopting it replaces its own show.
							</div>{/if}
					</div>
					<button
						class="btn primary"
						disabled={d.duplicate}
						onclick={() => {
							adopting = d;
							adoptName = suggestName();
						}}><Plus size={16} /> Adopt</button
					>
				</div>
			{/each}
		</section>
	{/if}

	{#if !show}
		<div class="card card-pad"><Skeleton count={6} /></div>
	{:else}
		{#each show.nodes as n (n.id)}
			{@const live = app.nodes.find((x) => x.id === n.id)}
			{@const usage = nodeUsage(show, n)}
			{@const grp = currentJack(n)}
			{@const rx = rxOn(n, grp?.jack ?? null)}
			{@const sensors = app.sensors.filter(
				(s) => (s.nodeId ?? show.nodes.find((x) => x.role === 'leader')?.id) === n.id
			)}
			<section class="node card" id={n.id}>
				<header class="nhead">
					<span class="icon-tile {n.role === 'leader' ? 'accent' : 'blue'}"><Cpu size={20} /></span>
					<div class="grow">
						<div class="row wrap">
							<h2>{n.name}</h2>
							<span class="badge {n.role === 'leader' ? 'accent' : 'blue'}"
								>{n.role === 'leader' ? 'Leader' : 'Follower'}</span
							>
							{#if live}
								<span
									class="badge {live.online ? (live.syncState === 'syncing' ? 'accent' : 'green') : 'red'}"
									title={live.online && n.role !== 'leader'
										? `Clock within ${Math.abs(live.syncOffsetMs).toFixed(1)} ms of the leader`
										: undefined}
								>
									<span class="dot"></span>
									{#if !live.online}Offline · last seen {fmtRelative(
											live.lastSeen
										)}{:else if n.role === 'leader'}Online{:else if live.syncState === 'syncing'}Syncing files {live
											.files.total - live.files.pending}/{live.files.total}{:else}In sync{/if}
								</span>
								{#if live.online && n.role === 'follower'}
									<SyncBadge node={live} />
								{:else if live.online && live.wifiPowerSave}
									<span
										class="badge red"
										title="Wi-Fi power saving delays packets by up to a second and hurts sync between controllers"
										>Wi-Fi power saving on</span
									>
								{/if}
							{/if}
						</div>
						<div class="faint small">
							{BOARDS[n.board].name}{n.boardRev ? ` · rev ${n.boardRev}` : ''} · {n.hostname}.local{n.piModel
								? ` · ${n.piModel.replace(/ Rev [\d.]+$/, '')}`
								: ''}
						</div>
					</div>
					<div class="nact">
						<button
							class="btn sm"
							onclick={() => identify(n)}
							title="Blink the status light so you can find this controller"
							><Lightbulb size={14} /> Identify</button
						>
						<button
							class="btn sm ghost icon"
							onclick={() => {
								renameNode = n;
								renameValue = n.name;
							}}
							aria-label="Rename {n.name}"><Pencil size={14} /></button
						>
						{#if n.role === 'follower'}<button
								class="btn sm ghost icon"
								onclick={() => removeNode(n)}
								aria-label="Release {n.name}"><Trash2 size={14} /></button
							>{/if}
					</div>
				</header>

				{#if needsPort3Warning(n)}
					<div class="notice warn nwarn">
						<TriangleAlert size={18} class="ico" />
						<div>
							<strong>Rev D board — port 3 needs a special lead.</strong> This board’s port 3 pair is
							reversed. Use a short patch cable with <strong>pins 4 and 5 swapped</strong> at one end (blue and
							white/blue) for anything on port 3. Rev E boards don’t need this.
						</div>
					</div>
				{/if}
				{#if n.board === 'diffsmart'}
					<div class="notice info nwarn">
						<Info size={18} />
						<div>
							Make sure switch <strong>SW1</strong> on the Smart Receiver is set to <strong>PI</strong>. In RX
							mode the board ignores PixelPlus and acts as a plain receiver.
						</div>
					</div>
				{/if}

				{#if n.outputs.length}
					<div class="nbody">
						<div class="diagram">
							<BoardDiagram
								board={n.board}
								rev={n.boardRev}
								portPixels={portPixels(n)}
								receivers={rxNames(n)}
								selectedJack={grp?.jack ?? null}
								onjack={(j) => (selJack = { ...selJack, [n.id]: j })}
								warnPort3={needsPort3Warning(n)}
							/>
							<div class="legend">
								<span><i class="lg on"></i> In use</span><span><i class="lg"></i> Free</span><span
									><i class="lg bad"></i> Too many pixels</span
								>
								<span class="grow"></span>
								<span class="num"
									>{usage.used} of {n.outputs.length} outputs · {usage.pixels.toLocaleString()} px</span
								>
							</div>
						</div>

						{#if grp}
							<div class="jack">
								<div class="jhead">
									<div class="grow">
										<div class="eyebrow">
											{grp.jack != null && n.board === 'difftxlarge'
												? `Jack J${grp.jack}`
												: n.board === 'diffsmart'
													? 'Outputs'
													: 'Network jack'}
										</div>
										{#if rx}
											<div class="rxname"><Radio size={15} /> {rx.name} receiver</div>
											<div class="faint tiny">
												{RECEIVERS[rx.kind].name}{rx.location ? ` · ${rx.location}` : ''}{rx.fuseAmps
													? ` · ${rx.fuseAmps} A fuses`
													: ''}
											</div>
										{:else if grp.jack != null}
											<div class="rxname faint">No receiver</div>
										{/if}
									</div>
									{#if rx}
										<button class="btn sm ghost" onclick={() => editReceiver(rx)}
											><Pencil size={14} /> Edit</button
										>
									{:else if grp.jack != null}
										<button class="btn sm" onclick={() => newReceiver(n, grp.jack!)}
											><Plus size={14} /> Add receiver</button
										>
									{/if}
								</div>
								<div class="ports">
									{#each grp.outputs as oi, k (oi)}
										{@const o = n.outputs.find((x) => x.index === oi)}
										{@const chain = propsOnOutput(show, n.id, oi)}
										{@const px = pixelsOnOutput(show, n.id, oi)}
										{@const key = n.id + ':' + oi}
										{#if o}
											<div class="port" class:empty={!px} class:off={!o.enabled}>
												<button
													class="prow"
													onclick={() => (editOut = editOut === key ? null : key)}
													aria-expanded={editOut === key}
												>
													<span class="pnum">{rx ? `Port ${k + 1}` : portName(n.board, oi)}</span>
													<span class="grow pprops ellipsis">
														{#if chain.length}{chain.map((c) => c.prop.name).join(' → ')}{:else}<span
																class="faint">Nothing plugged in</span
															>{/if}
													</span>
													{#if needsPort3Warning(n) && k === 2}<TriangleAlert
															size={14}
															class="warn-ic"
														/>{/if}
													<span class="num small {px > MAX_PIXELS_PER_OUTPUT ? 'bad' : 'faint'}"
														>{px ? `${px} px` : ''}</span
													>
													<span class="faint tiny co"
														>{o.colorOrder}{o.brightness < 100 ? ` · ${o.brightness}%` : ''}</span
													>
													<ChevronDown size={15} class="chev {editOut === key ? 'open' : ''}" />
												</button>
												{#if editOut === key}
													{@const ci = correctionIndex(o.gamma)}
													<div class="oedit" transition:slide={{ duration: 160 }}>
														<div class="ochain-wrap">
															<div class="row between">
																<span class="eyebrow"
																	>{chain.length > 1
																		? 'Plugged in, in order · drag or use the arrows'
																		: 'Plugged in'}</span
																>
																<button class="btn ghost sm" onclick={() => flashPort(n.id, oi)}
																	><Zap size={14} /> Flash this port</button
																>
															</div>
															{#if chain.length}
																<ol
																	class="ochain"
																	use:sortable={{ onsort: (f, t) => reorderChain(show, n.id, oi, f, t) }}
																>
																	{#each chain as c, ci2 (c.prop.id + c.seg.propOffset)}
																		<li data-sort-index={ci2}>
																			<button
																				type="button"
																				class="drag-handle"
																				aria-label="Move {c.prop.name} (use arrow keys)"
																				><GripVertical size={15} /></button
																			>
																			<span class="on-n num">{ci2 + 1}</span>
																			<a class="grow ellipsis on-name" href="/props#{c.prop.id}"
																				>{c.prop.name}</a
																			>
																			<span class="faint small num on-px"
																				>{c.seg.startPixel + 1}–{c.seg.startPixel + c.seg.pixelCount}</span
																			>
																			<button
																				type="button"
																				class="btn ghost icon sm"
																				disabled={ci2 === 0}
																				onclick={() => reorderChain(show, n.id, oi, ci2, ci2 - 1)}
																				aria-label="Move {c.prop.name} earlier in the chain"
																				><ChevronUp size={15} /></button
																			>
																			<button
																				type="button"
																				class="btn ghost icon sm"
																				disabled={ci2 === chain.length - 1}
																				onclick={() => reorderChain(show, n.id, oi, ci2, ci2 + 1)}
																				aria-label="Move {c.prop.name} later in the chain"
																				><ChevronDown size={15} /></button
																			>
																		</li>
																	{/each}
																</ol>
															{/if}
															<button
																class="btn sm add-prop"
																onclick={() => {
																	addQ = '';
																	addTo = { nodeId: n.id, output: oi };
																}}
																><Plus size={14} />
																{chain.length
																	? 'Add another prop to this port'
																	: 'Add a prop to this port'}</button
															>
														</div>
														<label class="field"
															><span class="label">Color order</span>
															<select
																class="select sm"
																value={o.colorOrder}
																onchange={(e) =>
																	saveOutput(n, o, {
																		colorOrder: (e.target as HTMLSelectElement)
																			.value as OutputConfig['colorOrder']
																	})}
															>
																{#each COLOR_ORDERS as c (c)}<option value={c}>{c}</option>{/each}
															</select>
														</label>
														<label class="field"
															><span class="label">Brightness limit · {o.brightness}%</span>
															<input
																type="range"
																class="range"
																min="5"
																max="100"
																step="5"
																value={o.brightness}
																style:--pct="{o.brightness}%"
																onchange={(e) =>
																	saveOutput(n, o, {
																		brightness: Number((e.target as HTMLInputElement).value)
																	})}
															/>
														</label>
														<label class="field"
															><span class="label">Color correction · {COLOR_CORRECTION[ci].label}</span>
															<input
																type="range"
																class="range"
																min="0"
																max={COLOR_CORRECTION.length - 1}
																step="1"
																value={ci}
																style:--pct="{(ci / (COLOR_CORRECTION.length - 1)) * 100}%"
																aria-valuetext={COLOR_CORRECTION[ci].label}
																onchange={(e) =>
																	saveOutput(n, o, {
																		gamma:
																			COLOR_CORRECTION[Number((e.target as HTMLInputElement).value)].gamma
																	})}
															/>
														</label>
														<div class="field">
															<span class="label">Port on</span>
															<div class="row" style="min-height:32px">
																<Switch
																	size="sm"
																	checked={o.enabled}
																	label="Port on"
																	onchange={(v) => saveOutput(n, o, { enabled: v })}
																/>
															</div>
														</div>
													</div>
												{/if}
											</div>
										{/if}
									{/each}
								</div>
							</div>
						{/if}
					</div>
				{:else}
					<div class="card-body faint small">
						This controller has no pixel outputs. It runs the show, schedule and audio and drives the
						followers.
					</div>
				{/if}

				{#if sensors.length}
					<div class="sensors">
						{#each sensors as s (s.id)}
							{@const Icon = sensorIcon[s.kind]}
							<div class="sensor">
								<Icon size={14} /><span class="faint small">{s.label.replace(n.name + ' ', '')}</span><strong
									class="num"
									>{s.kind === 'temperature'
										? fmtTemp(s.value, tunit)
										: `${s.value.toFixed(s.kind === 'voltage' || s.kind === 'current' ? 1 : 0)} ${s.unit}`}</strong
								>
							</div>
						{/each}
					</div>
				{/if}
			</section>
		{/each}

		<div class="section-title">
			<h2>Receivers</h2>
			<span class="grow"></span>
		</div>
		<div class="card">
			{#if !show.receivers.length}
				<div class="card-body faint small">
					No receivers yet. Click a jack on a controller above to add the receiver plugged into it.
				</div>
			{/if}
			{#each show.receivers as r (r.id)}
				{@const n = show.nodes.find((x) => x.id === r.nodeId)}
				<button class="list-row clickable rxrow" onclick={() => editReceiver(r)}>
					<span class="icon-tile"><Radio size={18} /></span>
					<div class="grow">
						<strong>{r.name}</strong>
						<div class="faint small">
							{RECEIVERS[r.kind].short} · {n?.name ?? 'Unknown'}{n?.board === 'difftxlarge'
								? ` › J${r.jack}`
								: ''}{r.location ? ` · ${r.location}` : ''}
						</div>
					</div>
					<span class="faint small num"
						>{[1, 2, 3, 4].filter((p) => pixelsOnOutput(show, r.nodeId, (r.jack - 1) * 4 + p)).length}/4 ports
						used</span
					>
					<Pencil size={14} class="faint" />
				</button>
			{/each}
		</div>

		<details class="adv card">
			<summary><Settings2 size={16} /> Advanced · board identity (EEPROM)</summary>
			<div class="card-body">
				<p class="muted small">
					PixelPlus reads the board type from a small memory chip on the board. If a board was detected
					wrongly (or is blank), you can write the right type here. You rarely need this.
				</p>
				<button class="btn sm" style="margin-top:12px" onclick={() => (eepromOpen = true)}
					><HardDrive size={14} /> Write board EEPROM…</button
				>
			</div>
		</details>
	{/if}
</div>

<Modal
	open={!!adopting}
	title="Adopt {adopting?.name}"
	subtitle="It will join this show and receive its settings and sequences automatically."
	size="sm"
	onclose={() => (adopting = null)}
>
	<label class="field"
		><span class="label">Give it a friendly name</span><input
			class="input"
			placeholder="e.g. Back Yard"
			bind:value={adoptName}
			data-autofocus
			onfocus={(e) => (e.currentTarget as HTMLInputElement).select()}
			onkeydown={(e) => e.key === 'Enter' && adopt()}
		/><span class="hint">Where it lives, so you can tell your controllers apart.</span></label
	>
	<div class="notice success small" style="margin-top:14px">
		<CircleCheck size={16} /><span
			>After adopting, wire props to its ports from the Props page. There’s nothing to set up on the
			controller itself.</span
		>
	</div>
	{#snippet footer()}
		<button class="btn ghost" onclick={() => (adopting = null)}>Cancel</button>
		<button class="btn primary" onclick={adopt} disabled={adoptBusy || !adoptName.trim()}
			>{adoptBusy ? 'Adopting…' : 'Adopt controller'}</button
		>
	{/snippet}
</Modal>

<Modal
	bind:open={joinOpen}
	title="Join another show"
	subtitle="Make this controller a follower of another show leader."
	size="sm"
>
	<p class="small">
		For the next 15 minutes, the other show leader can adopt this controller from its
		<strong>Controllers</strong> page. When it does, this controller's own show is replaced by the other one (a
		copy is kept).
	</p>
	<label class="field"
		><span class="label">Other leader's address (optional)</span><input
			class="input"
			placeholder="e.g. 192.168.1.20"
			bind:value={joinAddr}
		/><span class="hint">Only that controller may adopt this one. Leave empty to allow any show leader.</span
		></label
	>
	{#snippet footer()}
		<button class="btn ghost" onclick={() => (joinOpen = false)}>Cancel</button>
		<button class="btn primary" onclick={joinShow} disabled={joinBusy}>Allow for 15 minutes</button>
	{/snippet}
</Modal>

<Modal bind:open={rxModal} title={rxDraft.id ? `Edit ${rxDraft.name}` : 'Add a receiver'} size="sm">
	<form id="rxform" class="col" style="gap:14px" onsubmit={saveReceiver}>
		<label class="field"
			><span class="label">Name</span><input
				class="input"
				placeholder="e.g. Front Yard"
				bind:value={rxDraft.name}
				required
			/></label
		>
		<label class="field"
			><span class="label">Type</span>
			<select
				class="select"
				bind:value={rxDraft.kind}
				onchange={() => (rxDraft.fuseAmps = RECEIVERS[rxDraft.kind as ReceiverKind].fuse)}
			>
				{#each Object.entries(RECEIVERS).filter(([k]) => k !== 'direct') as [k, v] (k)}<option value={k}
						>{v.name}</option
					>{/each}
			</select>
		</label>
		<div class="form-grid">
			<label class="field"
				><span class="label">Controller</span>
				<select class="select" bind:value={rxDraft.nodeId}
					>{#each show?.nodes.filter((x) => x.board === 'difftx' || x.board === 'difftxlarge') ?? [] as n (n.id)}<option
							value={n.id}>{n.name}</option
						>{/each}</select
				>
			</label>
			<label class="field"
				><span class="label">Jack</span>
				<select class="select" bind:value={rxDraft.jack}>
					{#each Array(show?.nodes.find((x) => x.id === rxDraft.nodeId)?.board === 'difftxlarge' ? 15 : 1) as _, j (j)}<option
							value={j + 1}>J{j + 1}</option
						>{/each}
				</select>
			</label>
		</div>
		<label class="field"
			><span class="label">Where is it?</span><input
				class="input"
				placeholder="e.g. Behind the hedge"
				bind:value={rxDraft.location}
			/></label
		>
		<label class="field"
			><span class="label">Fuse per port</span>
			<div class="input-group">
				<input class="input" type="number" min="1" step="0.5" bind:value={rxDraft.fuseAmps} /><span
					class="suffix">A</span
				>
			</div>
			<span class="hint">Used to warn you when a port could draw too much power.</span></label
		>
	</form>
	{#snippet footer()}
		{#if rxDraft.id}<button class="btn danger" onclick={() => deleteReceiver(rxDraft as Receiver)}
				><Trash2 size={14} /></button
			><span class="grow"></span>{/if}
		<button class="btn ghost" onclick={() => (rxModal = false)}>Cancel</button>
		<button class="btn primary" type="submit" form="rxform">{rxDraft.id ? 'Save' : 'Add receiver'}</button>
	{/snippet}
</Modal>

<Modal open={!!renameNode} title="Rename controller" size="sm" onclose={() => (renameNode = null)}>
	<form id="rename" onsubmit={doRename}>
		<input class="input" bind:value={renameValue} aria-label="Controller name" />
	</form>
	{#snippet footer()}
		<button class="btn ghost" onclick={() => (renameNode = null)}>Cancel</button>
		<button class="btn primary" type="submit" form="rename" disabled={!renameValue.trim()}>Save</button>
	{/snippet}
</Modal>

<Modal
	open={!!addTo}
	title="Add a prop to {addTo && show
		? portName(show.nodes.find((x) => x.id === addTo!.nodeId)?.board ?? 'difftx', addTo.output)
		: 'this port'}"
	subtitle="It goes on the end of the chain. Drag it into place afterwards if it sits earlier on the cable."
	size="md"
	onclose={() => (addTo = null)}
>
	<div class="input-group" style="margin-bottom:10px">
		<span class="prefix"><Search size={16} /></span><input
			class="input"
			placeholder="Search props"
			bind:value={addQ}
			aria-label="Search props"
			data-autofocus
		/>
	</div>
	<div class="pickprops">
		{#each addChoices as { p, left } (p.id)}
			<button
				class="pickprop"
				onclick={async () => {
					const t = addTo;
					addTo = null;
					if (t && show) await wirePropToPort(show, p.id, t.nodeId, t.output);
				}}
			>
				<span class="grow ellipsis"><strong>{p.name}</strong></span>
				<span class="faint small num"
					>{left > 0
						? left === p.pixelCount
							? `Not wired · ${p.pixelCount} px`
							: `${left} px not wired`
						: 'Already wired'}</span
				>
				<Plus size={15} />
			</button>
		{:else}
			<div class="faint small" style="padding:12px">
				{show?.props.length ? 'No props match.' : 'No props yet — import your xLights layout first.'}
			</div>
		{/each}
	</div>
</Modal>

<Modal bind:open={eepromOpen} title="Write board EEPROM" size="sm">
	<div class="col" style="gap:14px">
		<label class="field"
			><span class="label">Board</span>
			<select class="select" bind:value={eepromBoard}
				>{#each ['difftx', 'difftxlarge', 'diffsmart'] as b (b)}<option value={b}
						>{BOARDS[b as BoardKind].name}</option
					>{/each}</select
			>
		</label>
		<label class="field"
			><span class="label">Revision</span><input class="input" bind:value={eepromRev} maxlength="4" /></label
		>
		<div class="notice warn small">
			<TriangleAlert size={16} class="ico" /><span
				>Writes to the leader’s board. The board must have its write-protect jumper removed.</span
			>
		</div>
	</div>
	{#snippet footer()}
		<button class="btn ghost" onclick={() => (eepromOpen = false)}>Cancel</button>
		<button class="btn danger" onclick={writeEeprom}>Write EEPROM</button>
	{/snippet}
</Modal>

<style>
	.ic {
		display: flex;
	}
	.spin {
		animation: spin 0.9s linear infinite;
	}
	@keyframes spin {
		to {
			transform: rotate(360deg);
		}
	}
	.found {
		border-color: var(--accent-line);
		background: linear-gradient(180deg, var(--accent-soft), transparent 70%), var(--surface);
		margin-bottom: 24px;
		overflow: hidden;
	}
	.found-head {
		display: flex;
		gap: 14px;
		align-items: center;
		padding: 18px 20px 8px;
	}
	.radar {
		width: 40px;
		height: 40px;
		border-radius: 50%;
		display: grid;
		place-items: center;
		color: var(--accent-text);
		background: var(--accent-soft);
		animation: pulse 2s infinite;
	}
	.found-row {
		display: flex;
		align-items: center;
		gap: 16px;
		padding: 12px 20px;
		border-top: 1px solid var(--border);
	}
	.mini-board {
		width: 90px;
		flex: 0 0 auto;
	}
	.node {
		margin-bottom: 20px;
		overflow: hidden;
		scroll-margin-top: 20px;
	}
	.nhead {
		display: flex;
		align-items: center;
		gap: 14px;
		padding: 18px 20px;
		border-bottom: 1px solid var(--border);
		flex-wrap: wrap;
	}
	.nhead h2 {
		font-size: 17px;
	}
	.nact {
		display: flex;
		gap: 4px;
	}
	.nwarn {
		margin: 16px 20px 0;
	}
	.nbody {
		display: grid;
		grid-template-columns: minmax(0, 1.4fr) minmax(320px, 1fr);
		gap: 20px;
		padding: 20px;
	}
	.diagram {
		display: flex;
		flex-direction: column;
		gap: 12px;
		min-width: 0;
	}
	.legend {
		display: flex;
		gap: 14px;
		font-size: 12px;
		color: var(--text-3);
		align-items: center;
		flex-wrap: wrap;
	}
	.lg {
		display: inline-block;
		width: 9px;
		height: 9px;
		border-radius: 50%;
		background: #3a3d43;
		margin-right: 4px;
		vertical-align: middle;
	}
	.lg.on {
		background: #56f39a;
	}
	.lg.bad {
		background: #ff5a5a;
	}
	.jack {
		border: 1px solid var(--border);
		border-radius: 14px;
		background: var(--surface-2);
		overflow: hidden;
		align-self: start;
	}
	.jhead {
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 14px 16px;
		border-bottom: 1px solid var(--border);
	}
	.rxname {
		display: flex;
		align-items: center;
		gap: 6px;
		font-weight: 600;
		margin-top: 2px;
	}
	.port {
		border-bottom: 1px solid var(--border);
	}
	.port:last-child {
		border-bottom: 0;
	}
	.port.off {
		opacity: 0.55;
	}
	.prow {
		display: flex;
		align-items: center;
		gap: 10px;
		width: 100%;
		min-height: 52px;
		padding: 8px 14px 8px 16px;
		text-align: left;
	}
	.prow:hover {
		background: var(--surface-hover);
	}
	.pnum {
		font-weight: 600;
		font-size: 12.5px;
		width: 52px;
		flex: 0 0 auto;
	}
	.prow .num {
		white-space: nowrap;
	}
	.pprops {
		font-size: 13px;
	}
	.co {
		width: 60px;
		text-align: right;
	}
	.bad {
		color: var(--red);
	}
	.prow :global(.chev) {
		color: var(--text-3);
		transition: transform 160ms;
	}
	.prow :global(.chev.open) {
		transform: rotate(180deg);
	}
	.prow :global(.warn-ic) {
		color: var(--accent-text);
	}
	.oedit {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: 12px;
		padding: 4px 16px 16px;
	}
	.ochain-wrap {
		grid-column: span 2;
		display: flex;
		flex-direction: column;
		gap: 8px;
		padding-bottom: 4px;
	}
	.ochain {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 4px;
	}
	.ochain li {
		display: flex;
		align-items: center;
		gap: 6px;
		min-height: 44px;
		padding: 2px 4px 2px 2px;
		border-radius: 10px;
		background: var(--surface);
		border: 1px solid var(--border);
		font-size: 13px;
	}
	.on-n {
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
	.on-name {
		font-weight: 550;
	}
	.on-name:hover {
		color: var(--accent-text);
	}
	.add-prop {
		align-self: flex-start;
	}
	.pickprops {
		display: flex;
		flex-direction: column;
		max-height: 50dvh;
		overflow: auto;
		border: 1px solid var(--border);
		border-radius: 12px;
	}
	.pickprop {
		display: flex;
		align-items: center;
		gap: 10px;
		min-height: 48px;
		padding: 0 14px;
		border-bottom: 1px solid var(--border);
		text-align: left;
		font-size: 13.5px;
	}
	.pickprop:last-child {
		border-bottom: 0;
	}
	.pickprop:hover {
		background: var(--surface-hover);
	}
	.sensors {
		display: flex;
		flex-wrap: wrap;
		gap: 8px;
		padding: 0 20px 18px;
	}
	.sensor {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 8px 12px;
		border-radius: 10px;
		background: var(--surface-2);
		color: var(--text-3);
	}
	.sensor strong {
		color: var(--text);
		font-weight: 600;
		font-size: 13px;
	}
	.rxrow {
		width: 100%;
		text-align: left;
	}
	.adv {
		margin-top: 24px;
	}
	.adv summary {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 16px 20px;
		cursor: pointer;
		font-weight: 560;
		color: var(--text-2);
		list-style: none;
	}
	.adv .card-body {
		padding-top: 0;
	}
	@media (max-width: 1100px) {
		.nbody {
			grid-template-columns: 1fr;
		}
	}
	@media (max-width: 760px) {
		.nbody {
			padding: 14px;
		}
		.nhead {
			padding: 14px;
			align-items: flex-start;
		}
		.nhead > .grow {
			flex: 1 1 0;
		}
		.nact {
			width: 100%;
			padding-left: 54px;
		}
		.nwarn {
			margin: 12px 14px 0;
		}
		.co {
			display: none;
		}
		.found-row {
			flex-wrap: wrap;
		}
	}
</style>
