<script lang="ts">
	import { api } from '$lib/api/client';
	import type { Prop, PropKind } from '$lib/api/types';
	import { PROP_KINDS } from '$lib/api/types';
	import { app } from '$lib/stores/app.svelte';
	import { toasts, confirm } from '$lib/stores/toasts.svelte';
	import { KIND_META } from '$lib/util/kinds';
	import { wiringChain, receiverFor } from '$lib/util/boards';
	import { propPower } from '$lib/util/power';
	import { fallbackLayout } from '$lib/util/geometry';
	import { sortable, moveItem } from '$lib/actions/sortable';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import Segmented from '$lib/components/ui/Segmented.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import Modal from '$lib/components/ui/Modal.svelte';
	import Skeleton from '$lib/components/ui/Skeleton.svelte';
	import PropPreview from '$lib/components/viz/PropPreview.svelte';
	import PropDrawer from '$lib/components/props/PropDrawer.svelte';
	import ImportModal from '$lib/components/props/ImportModal.svelte';
	import {
		Search,
		LayoutGrid,
		List,
		Plus,
		FileUp,
		Shapes,
		Cable,
		FlaskConical,
		Trash2,
		X,
		FolderPlus,
		GripVertical,
		TriangleAlert,
		Pencil,
		ChevronRight
	} from '@lucide/svelte';
	import { fly } from 'svelte/transition';

	const show = $derived(app.show);
	let q = $state('');
	let group = $state<string>('all');
	let nodeF = $state('');
	let rxF = $state('');
	let view = $state<'grid' | 'list'>('grid');
	let selected = $state<Set<string>>(new Set());
	let drawerOpen = $state(false);
	let drawerTab = $state('overview');
	let activeId = $state<string | null>(null);
	let importOpen = $state(false);
	let addOpen = $state(false);
	let groupOpen = $state(false);
	let bulkOpen = $state(false);
	/** Touch screens: checkboxes appear only after tapping "Select" (no checkbox on every card). */
	let selecting = $state(false);
	$effect(() => {
		// Dashboard / checklist links: /props?import=1 opens the xLights import straight away.
		if (new URLSearchParams(location.search).get('import') === '1') {
			importOpen = true;
			history.replaceState(history.state, '', location.pathname);
		}
	});

	try {
		const v = localStorage.getItem('pp-props-view');
		if (v === 'list' || v === 'grid') view = v;
	} catch {
		/* ignore */
	}
	$effect(() => {
		try {
			localStorage.setItem('pp-props-view', view);
		} catch {
			/* ignore */
		}
	});
	$effect(() => {
		const h = location.hash.slice(1);
		if (h && show?.props.some((p) => p.id === h)) openProp(h);
	});

	const filtered = $derived.by(() => {
		if (!show) return [];
		const needle = q.trim().toLowerCase();
		return show.props.filter((p) => {
			if (
				needle &&
				!p.name.toLowerCase().includes(needle) &&
				!KIND_META[p.kind].label.toLowerCase().includes(needle)
			)
				return false;
			if (group === 'unwired' && p.segments.length) return false;
			if (group !== 'all' && group !== 'unwired' && !p.groupIds.includes(group)) return false;
			if (nodeF && !p.segments.some((s) => s.nodeId === nodeF)) return false;
			if (rxF && !p.segments.some((s) => receiverFor(show, s.nodeId, s.output)?.id === rxF)) return false;
			return true;
		});
	});
	const unwiredCount = $derived(show?.props.filter((p) => !p.segments.length).length ?? 0);
	const filtering = $derived(!!q || group !== 'all' || !!nodeF || !!rxF);

	function openProp(id: string, tab = 'overview') {
		activeId = id;
		drawerTab = tab;
		drawerOpen = true;
	}
	function toggle(id: string) {
		const s = new Set(selected);
		if (s.has(id)) s.delete(id);
		else s.add(id);
		selected = s;
	}
	function selectAll() {
		selected = selected.size === filtered.length ? new Set() : new Set(filtered.map((p) => p.id));
	}

	/** "Port 3 · Driveway": the part you need when standing next to the prop (phones). */
	function portWhere(p: Prop): string {
		if (!show || !p.segments.length) return '';
		const steps = wiringChain(show, p.segments[0]);
		const port = steps.find((s) => s.kind === 'port')?.label ?? '';
		const where =
			steps.find((s) => s.kind === 'receiver')?.label.replace(/ receiver$/, '') ??
			steps.find((s) => s.kind === 'node')?.label ??
			'';
		const more = p.segments.length > 1 ? ` +${p.segments.length - 1}` : '';
		return `${port} · ${where}${more}`;
	}

	function shortChain(p: Prop): string {
		if (!show || !p.segments.length) return '';
		const steps = wiringChain(show, p.segments[0]);
		const chain = steps
			.filter((s) => s.kind !== 'pixels')
			.map((s) => s.label.replace(/ receiver$/, ''))
			.join(' › ');
		return p.segments.length > 1 ? `${chain} +${p.segments.length - 1}` : chain;
	}

	// ---- add prop
	let newName = $state('');
	let newKind = $state<PropKind>('arch');
	let newPixels = $state(50);
	async function addProp(e: SubmitEvent) {
		e.preventDefault();
		if (!show || !newName.trim()) return;
		const idx = show.props.length;
		const created = await app.mutate(
			() =>
				api.props.create({
					name: newName.trim(),
					kind: newKind,
					pixelCount: newPixels,
					channelStart: 0,
					channelsPerPixel: 3,
					segments: [],
					groupIds: [],
					layout: fallbackLayout(idx)
				}),
			{ success: `Added ${newName.trim()}` }
		);
		addOpen = false;
		newName = '';
		if (created) openProp(created.id, 'wiring');
	}

	// ---- groups
	let gName = $state('');
	let gColor = $state('#5b9dff');
	async function addGroup(e: SubmitEvent) {
		e.preventDefault();
		if (!gName.trim()) return;
		const ids = [...selected];
		const g = await app.mutate(() => api.groups.create({ name: gName.trim(), color: gColor, propIds: ids }), {
			success: `Created group ${gName.trim()}`
		});
		if (g && ids.length)
			await app.mutate(() =>
				api.props.bulk(
					ids.map((id) => ({
						op: 'update' as const,
						id,
						patch: { groupIds: [...(show!.props.find((p) => p.id === id)?.groupIds ?? []), g.id] }
					}))
				)
			);
		gName = '';
		groupOpen = false;
	}

	// ---- bulk
	let bulkKind = $state<PropKind | ''>('');
	let bulkMa = $state<number | ''>('');
	let bulkAddGroup = $state('');
	let bulkRemoveGroup = $state('');
	async function applyBulk(e: SubmitEvent) {
		e.preventDefault();
		if (!show) return;
		const before = show.props
			.filter((p) => selected.has(p.id))
			.map((p) => structuredClone($state.snapshot(p) as Prop));
		const ops = before.map((p) => {
			const patch: Partial<Prop> = {};
			if (bulkKind) patch.kind = bulkKind;
			if (bulkMa !== '') patch.maxMilliampsPerPixel = Number(bulkMa);
			let groups = [...p.groupIds];
			if (bulkAddGroup && !groups.includes(bulkAddGroup)) groups.push(bulkAddGroup);
			if (bulkRemoveGroup) groups = groups.filter((g) => g !== bulkRemoveGroup);
			patch.groupIds = groups;
			return { op: 'update' as const, id: p.id, patch };
		});
		await app.mutate(async () => {
			await api.props.bulk(ops);
			for (const gid of [bulkAddGroup, bulkRemoveGroup].filter(Boolean)) {
				const g = show!.propGroups.find((x) => x.id === gid)!;
				const set = new Set(g.propIds);
				for (const p of before) {
					if (gid === bulkAddGroup) set.add(p.id);
					else set.delete(p.id);
				}
				await api.groups.update(g.id, { ...g, propIds: [...set] });
			}
		});
		toasts.success(`Updated ${before.length} props`, {
			label: 'Undo',
			run: () =>
				app.mutate(() => api.props.bulk(before.map((p) => ({ op: 'update' as const, id: p.id, patch: p }))))
		});
		bulkOpen = false;
		bulkKind = '';
		bulkMa = '';
		bulkAddGroup = bulkRemoveGroup = '';
	}
	async function bulkDelete() {
		if (!show) return;
		const doomed = show.props
			.filter((p) => selected.has(p.id))
			.map((p) => structuredClone($state.snapshot(p) as Prop));
		if (
			!(await confirm({
				title: `Delete ${doomed.length} props?`,
				message: 'Their wiring is removed too. Sequences are not affected.',
				confirmLabel: 'Delete',
				danger: true
			}))
		)
			return;
		await app.mutate(() => api.props.bulk(doomed.map((p) => ({ op: 'delete' as const, id: p.id }))));
		selected = new Set();
		toasts.success(`Deleted ${doomed.length} props`, {
			label: 'Undo',
			run: async () => {
				for (const p of doomed) await api.props.create(p);
				await app.reloadShow();
			}
		});
	}
	async function bulkTest() {
		await api.testStart({ mode: 'rgbCycle', target: { propIds: [...selected] } }).catch(() => {});
		toasts.push({
			kind: 'info',
			message: `Testing ${selected.size} props`,
			action: { label: 'Stop', run: () => api.testStop().then(() => {}) }
		});
	}

	async function reorder(from: number, to: number) {
		if (!show) return;
		const ids = moveItem(
			show.props.map((p) => p.id),
			from,
			to
		);
		app.updateShow((s) => {
			const m = new Map(s.props.map((p) => [p.id, p]));
			s.props = ids.map((id) => m.get(id)!);
		});
		try {
			await api.props.reorder(ids);
		} catch (e) {
			toasts.error('Could not reorder', (e as Error).message);
			app.reloadShow();
		}
	}
</script>

<div class="page">
	<PageHeader
		title="Props"
		subtitle={show
			? `${show.props.length} props · ${show.props.reduce((n, p) => n + p.pixelCount, 0).toLocaleString()} pixels`
			: 'Everything in your display'}
	>
		{#snippet actions()}
			<button class="btn" onclick={() => (importOpen = true)}><FileUp size={16} /> Import from xLights</button
			>
			<button class="btn primary" onclick={() => (addOpen = true)}><Plus size={16} /> Add prop</button>
		{/snippet}
	</PageHeader>

	<div class="toolbar">
		<div class="input-group search">
			<span class="prefix"><Search size={16} /></span>
			<input class="input" placeholder="Search props" aria-label="Search props" bind:value={q} data-search />
			<span class="suffix kbd-hint"><span class="kbd">/</span></span>
		</div>
		<select class="select filter" bind:value={nodeF} aria-label="Filter by controller">
			<option value="">All controllers</option>
			{#each show?.nodes ?? [] as n (n.id)}<option value={n.id}>{n.name}</option>{/each}
		</select>
		<select class="select filter rx" bind:value={rxF} aria-label="Filter by receiver">
			<option value="">All receivers</option>
			{#each show?.receivers ?? [] as r (r.id)}<option value={r.id}>{r.name}</option>{/each}
		</select>
		<span class="grow"></span>
		{#if show?.props.length}
			<button
				class="btn select-btn"
				class:primary={selecting}
				aria-pressed={selecting}
				onclick={() => {
					selecting = !selecting;
					if (!selecting) selected = new Set();
				}}>{selecting ? 'Done' : 'Select'}</button
			>
		{/if}
		<Segmented
			bind:value={view}
			label="View"
			options={[
				{ value: 'grid', label: 'Grid', icon: LayoutGrid },
				{ value: 'list', label: 'List', icon: List }
			]}
		/>
	</div>

	<div class="chips">
		<button class="chip" aria-pressed={group === 'all'} onclick={() => (group = 'all')}>All</button>
		{#each show?.propGroups ?? [] as g (g.id)}
			<button
				class="chip"
				aria-pressed={group === g.id}
				onclick={() => (group = group === g.id ? 'all' : g.id)}
			>
				<span class="gdot" style:background={g.color ?? 'var(--text-3)'}></span>{g.name}<span
					class="faint num">{g.propIds.length}</span
				>
			</button>
		{/each}
		{#if unwiredCount}
			<button
				class="chip warnchip"
				aria-pressed={group === 'unwired'}
				onclick={() => (group = group === 'unwired' ? 'all' : 'unwired')}
			>
				<TriangleAlert size={13} /> Not wired <span class="num">{unwiredCount}</span>
			</button>
		{/if}
		<button class="chip ghostchip" onclick={() => (groupOpen = true)}
			><FolderPlus size={14} /> New group</button
		>
	</div>

	{#if !show}
		<div class="pgrid">
			{#each Array(8) as _, i (i)}<div class="card">
					<div class="skeleton" style="height:120px;border-radius:14px 14px 0 0"></div>
					<div class="card-pad"><Skeleton count={2} /></div>
				</div>{/each}
		</div>
	{:else if !show.props.length}
		<div class="card">
			<EmptyState
				icon={Shapes}
				title="No props yet"
				message="Import your xLights layout to bring in every prop at once, or add them one at a time."
			>
				<button class="btn primary" onclick={() => (importOpen = true)}
					><FileUp size={16} /> Import from xLights</button
				>
				<button class="btn" onclick={() => (addOpen = true)}><Plus size={16} /> Add a prop</button>
			</EmptyState>
		</div>
	{:else if !filtered.length}
		<div class="card">
			<EmptyState
				icon={Search}
				title="No props match"
				message="Try a different search or clear the filters."
				compact
			>
				<button
					class="btn"
					onclick={() => {
						q = '';
						group = 'all';
						nodeF = '';
						rxF = '';
					}}>Clear filters</button
				>
			</EmptyState>
		</div>
	{:else if view === 'grid'}
		<div class="pgrid">
			{#each filtered as p (p.id)}
				{@const K = KIND_META[p.kind]}
				{@const sel = selected.has(p.id)}
				<article class="card pcard interactive" class:sel>
					<button
						class="pv"
						onclick={() => (selecting ? toggle(p.id) : openProp(p.id))}
						aria-label={selecting ? `Select ${p.name}` : `Open ${p.name}`}
					>
						<PropPreview prop={p} height={116} />
					</button>
					<label class="pick" class:show={selected.size > 0 || selecting}>
						<input
							type="checkbox"
							class="check"
							checked={sel}
							onchange={() => toggle(p.id)}
							aria-label="Select {p.name}"
						/>
					</label>
					<button
						class="meta"
						onclick={() =>
							selecting ? toggle(p.id) : openProp(p.id, p.segments.length ? 'overview' : 'wiring')}
					>
						<div class="row">
							<K.icon size={15} class="kicon" />
							<span class="name ellipsis grow">{p.name}</span>
							<span class="faint small num">{p.pixelCount}</span>
						</div>
						{#if p.segments.length}
							<div class="wire ellipsis full">{shortChain(p)}</div>
							<div class="wire ellipsis compact">{portWhere(p)}</div>
						{:else}
							<div class="wire unwired"><Cable size={12} /> Not wired — tap to connect</div>
						{/if}
					</button>
				</article>
			{/each}
		</div>
	{:else}
		<div class="card">
			<div class="lhead">
				<input
					type="checkbox"
					class="check"
					checked={selected.size === filtered.length && filtered.length > 0}
					onchange={selectAll}
					aria-label="Select all"
				/>
				<span class="grow">Prop</span>
				<span class="c-type">Type</span>
				<span class="c-px">Pixels</span>
				<span class="c-wire">Wired to</span>
				<span class="c-pw">Full white</span>
				<span style="width:32px"></span>
			</div>
			<div use:sortable={{ onsort: reorder, disabled: filtering }}>
				{#each filtered as p, i (p.id)}
					{@const K = KIND_META[p.kind]}
					<div class="lrow" data-sort-index={i} class:sel={selected.has(p.id)}>
						{#if !filtering}<button class="drag-handle" aria-label="Reorder {p.name}"
								><GripVertical size={15} /></button
							>{/if}
						<input
							type="checkbox"
							class="check"
							checked={selected.has(p.id)}
							onchange={() => toggle(p.id)}
							aria-label="Select {p.name}"
						/>
						<button class="grow lname" onclick={() => openProp(p.id)}>
							<span
								class="swatch"
								style:background={p.color ??
									show.propGroups.find((g) => p.groupIds.includes(g.id))?.color ??
									'var(--text-3)'}
							></span>
							<span class="ellipsis">{p.name}</span>
						</button>
						<span class="c-type muted small"><K.icon size={14} /> {K.label}</span>
						<span class="c-px num small">{p.pixelCount.toLocaleString()}</span>
						<span class="c-wire small ellipsis {p.segments.length ? 'muted' : 'warn-t'}"
							>{p.segments.length ? shortChain(p) : 'Not wired'}</span
						>
						<span class="c-pw num small faint">{propPower(p).peak.toFixed(1)} A</span>
						<button class="btn ghost icon sm" onclick={() => openProp(p.id)} aria-label="Edit {p.name}"
							><Pencil size={14} /></button
						>
					</div>
				{/each}
			</div>
		</div>
		{#if filtering}<p class="faint tiny" style="margin-top:8px">
				Clear filters to drag props into a new order.
			</p>{/if}
	{/if}
</div>

{#if selected.size}
	<div class="bulk" transition:fly={{ y: 30, duration: 200 }} role="toolbar" aria-label="Bulk actions">
		<span class="count num">{selected.size}</span><span class="small">selected</span>
		<span class="sep"></span>
		<button class="btn sm ghost" onclick={() => (bulkOpen = true)}><Pencil size={14} /> Edit</button>
		<button class="btn sm ghost" onclick={() => (groupOpen = true)}><FolderPlus size={14} /> Group</button>
		<button class="btn sm ghost" onclick={bulkTest}><FlaskConical size={14} /> Test</button>
		<button class="btn sm ghost del" onclick={bulkDelete}><Trash2 size={14} /> Delete</button>
		<button
			class="btn sm ghost icon"
			onclick={() => {
				selected = new Set();
				selecting = false;
			}}
			aria-label="Clear selection"><X size={15} /></button
		>
	</div>
{/if}

<PropDrawer bind:open={drawerOpen} propId={activeId} bind:tab={drawerTab} />
<ImportModal bind:open={importOpen} />

<Modal bind:open={addOpen} title="Add a prop" size="sm">
	<form id="addprop" class="col" style="gap:14px" onsubmit={addProp}>
		<label class="field"
			><span class="label">Name</span><input
				class="input"
				placeholder="e.g. Left Arch"
				bind:value={newName}
				required
			/></label
		>
		<div class="field">
			<span class="label">Type</span>
			<div class="kinds">
				{#each PROP_KINDS as k (k)}
					{@const K = KIND_META[k]}
					<button
						type="button"
						class="kind"
						class:on={newKind === k}
						onclick={() => (newKind = k)}
						aria-pressed={newKind === k}><K.icon size={18} /><span>{K.label}</span></button
					>
				{/each}
			</div>
		</div>
		<label class="field"
			><span class="label">Pixels</span><input
				class="input"
				type="number"
				min="1"
				bind:value={newPixels}
			/></label
		>
	</form>
	{#snippet footer()}
		<button class="btn ghost" onclick={() => (addOpen = false)}>Cancel</button>
		<button class="btn primary" type="submit" form="addprop" disabled={!newName.trim()}
			>Add and wire it <ChevronRight size={16} /></button
		>
	{/snippet}
</Modal>

<Modal
	bind:open={groupOpen}
	title="New group"
	subtitle={selected.size
		? `With the ${selected.size} selected props`
		: 'Groups make it easy to target effects and tests'}
	size="sm"
>
	<form id="addgroup" class="col" style="gap:14px" onsubmit={addGroup}>
		<label class="field"
			><span class="label">Name</span><input
				class="input"
				placeholder="e.g. Driveway"
				bind:value={gName}
				required
			/></label
		>
		<label class="field"><span class="label">Color</span><input type="color" bind:value={gColor} /></label>
	</form>
	{#snippet footer()}
		<button class="btn ghost" onclick={() => (groupOpen = false)}>Cancel</button>
		<button class="btn primary" type="submit" form="addgroup" disabled={!gName.trim()}>Create group</button>
	{/snippet}
</Modal>

<Modal
	bind:open={bulkOpen}
	title="Edit {selected.size} props"
	subtitle="Only the fields you change are applied"
	size="sm"
>
	<form id="bulk" class="col" style="gap:14px" onsubmit={applyBulk}>
		<label class="field"
			><span class="label">Type</span>
			<select class="select" bind:value={bulkKind}
				><option value="">Keep as is</option>{#each PROP_KINDS as k (k)}<option value={k}
						>{KIND_META[k].label}</option
					>{/each}</select
			>
		</label>
		<label class="field"
			><span class="label">Add to group</span>
			<select class="select" bind:value={bulkAddGroup}
				><option value="">—</option>{#each show?.propGroups ?? [] as g (g.id)}<option value={g.id}
						>{g.name}</option
					>{/each}</select
			>
		</label>
		<label class="field"
			><span class="label">Remove from group</span>
			<select class="select" bind:value={bulkRemoveGroup}
				><option value="">—</option>{#each show?.propGroups ?? [] as g (g.id)}<option value={g.id}
						>{g.name}</option
					>{/each}</select
			>
		</label>
		<label class="field"
			><span class="label">Max current per pixel (mA)</span><input
				class="input"
				type="number"
				placeholder="Keep as is"
				bind:value={bulkMa}
			/></label
		>
	</form>
	{#snippet footer()}
		<button class="btn ghost" onclick={() => (bulkOpen = false)}>Cancel</button>
		<button class="btn primary" type="submit" form="bulk">Apply</button>
	{/snippet}
</Modal>

<style>
	.search {
		flex: 1 1 260px;
		max-width: 360px;
	}
	.kbd-hint {
		pointer-events: none;
	}
	.filter {
		width: auto;
		min-width: 150px;
	}
	.chips {
		display: flex;
		gap: 8px;
		flex-wrap: wrap;
		margin-bottom: 20px;
	}
	.gdot {
		width: 8px;
		height: 8px;
		border-radius: 50%;
	}
	.warnchip {
		color: var(--accent-text);
	}
	.ghostchip {
		border-style: dashed;
	}
	.pgrid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(220px, 1fr));
		gap: 14px;
	}
	.pcard {
		position: relative;
		overflow: hidden;
		display: flex;
		flex-direction: column;
	}
	.pcard.sel {
		border-color: var(--accent);
		box-shadow: 0 0 0 1px var(--accent);
	}
	.pv {
		display: block;
		width: 100%;
		border-bottom: 1px solid var(--border);
	}
	.pick {
		position: absolute;
		top: 10px;
		left: 10px;
		opacity: 0;
		transition: opacity 150ms;
		display: grid;
		place-items: center;
		width: 30px;
		height: 30px;
		border-radius: 8px;
		background: rgba(0, 0, 0, 0.5);
		backdrop-filter: blur(6px);
	}
	.pcard:hover .pick,
	.pick.show,
	.pick:focus-within {
		opacity: 1;
	}
	.select-btn,
	.wire.compact {
		display: none;
	}
	@media (pointer: coarse) {
		.select-btn {
			display: inline-flex;
		}
		.pcard:hover .pick {
			opacity: 0;
		}
		.pick.show {
			opacity: 1;
			width: 44px;
			height: 44px;
			top: 6px;
			left: 6px;
			border-radius: 12px;
		}
		.pick:not(.show) {
			pointer-events: none;
		}
		.pick .check {
			width: 24px;
			height: 24px;
		}
	}
	@media (max-width: 640px) {
		.wire.full {
			display: none;
		}
		.wire.compact {
			display: block;
		}
	}
	.meta {
		display: flex;
		flex-direction: column;
		gap: 4px;
		padding: 12px 14px 14px;
		text-align: left;
		width: 100%;
	}
	.meta :global(.kicon) {
		color: var(--text-3);
		flex: 0 0 auto;
	}
	.name {
		font-weight: 580;
	}
	.wire {
		font-size: 12px;
		color: var(--text-3);
	}
	.wire.unwired {
		color: var(--accent-text);
		display: flex;
		align-items: center;
		gap: 5px;
	}
	.lhead,
	.lrow {
		display: flex;
		align-items: center;
		gap: 12px;
		padding: 0 16px;
	}
	.lhead {
		height: 44px;
		font-size: 12px;
		color: var(--text-3);
		font-weight: 550;
		border-bottom: 1px solid var(--border);
	}
	.lrow {
		min-height: 52px;
		border-bottom: 1px solid var(--border);
		background: var(--surface);
		padding-left: 8px;
	}
	.lrow:last-child {
		border-bottom: 0;
	}
	.lrow:hover {
		background: var(--surface-hover);
	}
	.lrow.sel {
		background: var(--accent-soft);
	}
	.lname {
		display: flex;
		align-items: center;
		gap: 10px;
		text-align: left;
		font-weight: 550;
		min-width: 0;
		height: 44px;
	}
	.c-type {
		width: 140px;
		display: flex;
		align-items: center;
		gap: 6px;
	}
	.c-px {
		width: 70px;
		text-align: right;
	}
	.c-wire {
		width: 260px;
	}
	.c-pw {
		width: 80px;
		text-align: right;
	}
	.warn-t {
		color: var(--accent-text);
	}
	@media (max-width: 1100px) {
		.c-wire,
		.c-pw {
			display: none;
		}
	}
	@media (max-width: 640px) {
		.c-type {
			display: none;
		}
		.pgrid {
			grid-template-columns: repeat(2, minmax(0, 1fr));
			gap: 10px;
		}
		.filter {
			flex: 1 1 0;
			min-width: 0;
		}
		/* One compact row on phones: controller filter · Select · grid/list icons. */
		.filter.rx {
			display: none;
		}
		.toolbar :global(.seg button) {
			min-width: 48px;
			justify-content: center;
		}
		.toolbar :global(.seg button span) {
			position: absolute;
			width: 1px;
			height: 1px;
			overflow: hidden;
			clip: rect(0, 0, 0, 0);
		}
		.chips {
			flex-wrap: nowrap;
			overflow-x: auto;
			margin: 0 -16px 16px;
			padding: 0 16px 2px;
			scrollbar-width: none;
		}
		.chips .chip {
			flex: 0 0 auto;
		}
		.toolbar > .grow {
			display: none;
		}
		.search {
			max-width: none;
			flex-basis: 100%;
		}
		.kbd-hint {
			display: none;
		}
	}
	.bulk {
		position: fixed;
		left: calc(var(--sidebar-w) + (100vw - var(--sidebar-w)) / 2);
		transform: translateX(-50%);
		bottom: calc(var(--transport-h) + 16px);
		display: flex;
		align-items: center;
		gap: 4px;
		padding: 6px 6px 6px 14px;
		border-radius: 14px;
		background: var(--surface-3);
		border: 1px solid var(--border-2);
		box-shadow: var(--shadow-3);
		z-index: 45;
		white-space: nowrap;
	}
	.count {
		font-weight: 700;
		color: var(--accent-text);
		margin-right: 2px;
	}
	.sep {
		width: 1px;
		height: 20px;
		background: var(--border-2);
		margin: 0 6px;
	}
	.del:hover {
		color: var(--red);
	}
	@media (max-width: 760px) {
		.bulk {
			left: 8px;
			right: 8px;
			transform: none;
			bottom: calc(var(--tabbar-h) + 76px + env(safe-area-inset-bottom));
			overflow-x: auto;
		}
		.bulk :global(.btn span) {
			display: none;
		}
	}
	.kinds {
		display: grid;
		grid-template-columns: repeat(4, 1fr);
		gap: 6px;
	}
	.kind {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 4px;
		padding: 10px 4px;
		border-radius: 10px;
		border: 1px solid var(--border-2);
		font-size: 11px;
		color: var(--text-2);
		text-align: center;
	}
	.kind.on {
		border-color: var(--accent);
		background: var(--accent-soft);
		color: var(--accent-text);
	}
</style>
