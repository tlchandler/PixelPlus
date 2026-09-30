<script lang="ts">
	import type { PowerEstimate, Prop, Show, TestMode } from '$lib/api/types';
	import { PROP_KINDS } from '$lib/api/types';
	import { api } from '$lib/api/client';
	import { app } from '$lib/stores/app.svelte';
	import { toasts, confirm } from '$lib/stores/toasts.svelte';
	import { KIND_META } from '$lib/util/kinds';
	import { propPower } from '$lib/util/power';
	import { fmtAmps } from '$lib/util/format';
	import { receiverFor, portOf } from '$lib/util/boards';
	import Drawer from '$lib/components/ui/Drawer.svelte';
	import Segmented from '$lib/components/ui/Segmented.svelte';
	import PropPreview from '$lib/components/viz/PropPreview.svelte';
	import WiringEditor from './WiringEditor.svelte';
	import FaultFinder from './FaultFinder.svelte';
	import { Cable, Info, Zap, FlaskConical, Search, Trash2, Square, Copy } from '@lucide/svelte';

	let { open = $bindable(false), propId, tab = $bindable('overview') }: { open?: boolean; propId: string | null; tab?: string } = $props();

	const show = $derived(app.show as Show);
	const original = $derived(show?.props.find((p) => p.id === propId) ?? null);
	let draft = $state<Prop | null>(null);
	let loadedFor: string | null = null;
	let loadedVersion = -1;
	let saving = $state(false);
	let faultOpen = $state(false);
	let power = $state<PowerEstimate | null>(null);
	let testing = $state<string | null>(null);

	$effect(() => {
		// (re)load draft when switching prop, or when the show changed and we have no local edits
		if (!original) return;
		if (loadedFor !== original.id || (!dirty && loadedVersion !== show.version)) {
			draft = structuredClone($state.snapshot(original) as Prop);
			loadedFor = original.id;
			loadedVersion = show.version;
		}
	});

	$effect(() => {
		if (open && tab === 'power' && !power) api.power().then((p) => (power = p)).catch(() => {});
	});

	const dirty = $derived(!!draft && !!original && JSON.stringify(draft) !== JSON.stringify(original));

	async function save() {
		if (!draft || !original) return;
		saving = true;
		const before = structuredClone($state.snapshot(original) as Prop);
		const d = $state.snapshot(draft) as Prop;
		// keep group membership in sync
		try {
			await api.props.update(d.id, d);
			for (const g of show.propGroups) {
				const want = d.groupIds.includes(g.id);
				const has = g.propIds.includes(d.id);
				if (want !== has)
					await api.groups.update(g.id, { ...g, propIds: want ? [...g.propIds, d.id] : g.propIds.filter((x) => x !== d.id) });
			}
			await app.reloadShow();
			loadedVersion = -1;
			toasts.success(`Saved ${d.name}`, {
				label: 'Undo',
				run: async () => {
					await api.props.update(before.id, before);
					await app.reloadShow();
					loadedVersion = -1;
				}
			});
		} catch (e) {
			toasts.error('Could not save prop', (e as Error).message);
		} finally {
			saving = false;
		}
	}

	async function remove() {
		if (!original) return;
		const p = structuredClone($state.snapshot(original) as Prop);
		if (!(await confirm({ title: `Delete ${p.name}?`, message: 'The prop and its wiring are removed. Sequences are not affected.', confirmLabel: 'Delete prop', danger: true }))) return;
		await app.mutate(() => api.props.remove(p.id));
		open = false;
		toasts.success(`Deleted ${p.name}`, { label: 'Undo', run: () => app.mutate(() => api.props.create(p)) });
	}

	async function duplicate() {
		if (!original) return;
		const p = structuredClone($state.snapshot(original) as Prop);
		const copy = { ...p, id: undefined as unknown as string, name: `${p.name} copy`, segments: [], layout: p.layout ? { ...p.layout, x: p.layout.x + 20, y: p.layout.y + 20 } : undefined };
		await app.mutate(() => api.props.create(copy), { success: `Created ${copy.name}` });
	}

	async function test(mode: TestMode, color?: string, key = mode + (color ?? '')) {
		if (!original) return;
		try {
			await api.testStart({ mode, color, target: { propIds: [original.id] } });
			testing = key;
		} catch (e) {
			toasts.error('Test failed', (e as Error).message);
		}
	}
	async function stopTest() {
		await api.testStop().catch(() => {});
		testing = null;
	}
	$effect(() => {
		if (!open && testing) stopTest();
	});

	function toggleGroup(id: string) {
		if (!draft) return;
		draft.groupIds = draft.groupIds.includes(id) ? draft.groupIds.filter((g) => g !== id) : [...draft.groupIds, id];
	}

	const tabs = [
		{ value: 'overview', label: 'Overview', icon: Info },
		{ value: 'wiring', label: 'Wiring', icon: Cable },
		{ value: 'power', label: 'Power', icon: Zap },
		{ value: 'test', label: 'Test', icon: FlaskConical }
	];
	const pw = $derived(draft ? propPower(draft) : { peak: 0, typical: 0 });
	const tests: { mode: TestMode; label: string; color?: string; swatch?: string }[] = [
		{ mode: 'solid', label: 'White', color: '#ffffff', swatch: '#ffffff' },
		{ mode: 'solid', label: 'Red', color: '#ff0000', swatch: '#ff3b3b' },
		{ mode: 'solid', label: 'Green', color: '#00ff00', swatch: '#3fcf5e' },
		{ mode: 'solid', label: 'Blue', color: '#0000ff', swatch: '#3b6bff' },
		{ mode: 'rgbCycle', label: 'RGB cycle' },
		{ mode: 'chase', label: 'Chase', color: '#ffffff' },
		{ mode: 'countPixels', label: 'Count pixels' },
		{ mode: 'walk', label: 'Walk one pixel' }
	];
</script>

<Drawer bind:open width={620} title={draft?.name}>
	{#snippet header()}
		{#if draft}
			{@const K = KIND_META[draft.kind]}
			<div class="row">
				<span class="icon-tile accent"><K.icon size={20} /></span>
				<div class="grow">
					<h2 class="ellipsis">{draft.name}</h2>
					<div class="faint small">{K.label} · {draft.pixelCount.toLocaleString()} pixels{draft.xlightsModel ? ` · xLights “${draft.xlightsModel}”` : ''}</div>
				</div>
			</div>
		{/if}
	{/snippet}

	{#if draft && original}
		<div class="preview card"><PropPreview prop={original} height={150} /></div>
		<div class="tabs"><Segmented bind:value={tab} options={tabs} label="Prop sections" /></div>

		{#if tab === 'overview'}
			<div class="form-grid">
				<label class="field span-2"><span class="label">Name</span><input class="input" bind:value={draft.name} /></label>
				<label class="field">
					<span class="label">Type</span>
					<select class="select" bind:value={draft.kind}>
						{#each PROP_KINDS as k (k)}<option value={k}>{KIND_META[k].label}</option>{/each}
					</select>
				</label>
				<label class="field"><span class="label">Pixels</span><input class="input" type="number" min="1" bind:value={draft.pixelCount} /></label>
				<div class="field span-2">
					<span class="label">Groups</span>
					<div class="row wrap">
						{#each show.propGroups as g (g.id)}
							<button type="button" class="chip" aria-pressed={draft.groupIds.includes(g.id)} onclick={() => toggleGroup(g.id)}>
								<span class="swatch" style:background={g.color ?? 'var(--text-3)'} style="width:10px;height:10px;border-radius:3px"></span>{g.name}
							</button>
						{/each}
						{#if !show.propGroups.length}<span class="faint small">No groups yet — create them from the Props page.</span>{/if}
					</div>
				</div>
				<label class="field">
					<span class="label">Accent color</span>
					<div class="row"><input type="color" value={draft.color ?? '#f5a524'} oninput={(e) => draft && (draft.color = (e.target as HTMLInputElement).value)} /><span class="faint small">Used in lists and the layout</span></div>
				</label>
				<label class="field">
					<span class="label">Max current per pixel</span>
					<div class="input-group"><input class="input" type="number" min="1" max="200" value={draft.maxMilliampsPerPixel ?? 60} oninput={(e) => draft && (draft.maxMilliampsPerPixel = Number((e.target as HTMLInputElement).value))} /><span class="suffix">mA</span></div>
				</label>
				<label class="field span-2"><span class="label">Notes</span><textarea class="textarea" rows="2" placeholder="e.g. Replace pixel 12 after the season" bind:value={draft.notes}></textarea></label>
			</div>
			<div class="row danger-zone">
				<button class="btn ghost sm" onclick={duplicate}><Copy size={14} /> Duplicate</button>
				<span class="grow"></span>
				<button class="btn danger sm" onclick={remove}><Trash2 size={14} /> Delete prop</button>
			</div>
		{:else if tab === 'wiring'}
			<WiringEditor {show} bind:prop={draft} />
		{:else if tab === 'power'}
			<div class="power">
				<div class="grid grid-2">
					<div class="card card-pad pstat"><span class="faint small">Full white (worst case)</span><span class="big num">{fmtAmps(pw.peak)}</span><span class="faint tiny">{draft.pixelCount} px × {draft.maxMilliampsPerPixel ?? 60} mA</span></div>
					<div class="card card-pad pstat"><span class="faint small">Typical during a show</span><span class="big num">{fmtAmps(pw.typical)}</span><span class="faint tiny">about a third of full white</span></div>
				</div>
				{#each draft.segments as seg (seg.nodeId + seg.output + seg.propOffset)}
					{@const rx = receiverFor(show, seg.nodeId, seg.output)}
					{@const port = power?.perReceiverPort.find((x) => x.receiverId === rx?.id && x.port === portOf(seg.output))}
					{#if rx && port}
						{@const fuse = port.fuseAmps ?? rx.fuseAmps ?? 6}
						{@const pct = Math.min(100, (port.peakAmps / fuse) * 100)}
						<div class="fuse">
							<div class="row between small"><span>{rx.name} receiver · Port {portOf(seg.output)}</span><span class="num">{port.peakAmps.toFixed(1)} A of {fuse} A fuse</span></div>
							<div class="progress"><span style:width="{pct}%" style:background={pct > 100 || port.peakAmps > fuse ? 'var(--red)' : pct > 80 ? 'var(--accent)' : 'var(--green)'}></span></div>
							<span class="faint tiny">Everything chained on this port at full white. {port.peakAmps > fuse ? 'Over the fuse rating — add power injection or lower brightness.' : 'Within the fuse rating.'}</span>
						</div>
					{/if}
				{/each}
				{#if power?.warnings.length}
					<div class="notice warn small"><Zap size={16} /><div>{power.warnings[0]}</div></div>
				{/if}
			</div>
		{:else if tab === 'test'}
			<p class="muted small" style="margin-bottom:12px">Lights only this prop so you can check it from the street. The running show is paused while testing.</p>
			<div class="tests">
				{#each tests as t (t.label)}
					{@const key = t.mode + (t.color ?? '')}
					<button class="test" class:on={testing === key} onclick={() => (testing === key ? stopTest() : test(t.mode, t.color, key))}>
						{#if t.swatch}<span class="sw" style:background={t.swatch}></span>{:else}<span class="sw grad {t.mode}"></span>{/if}
						{t.label}
					</button>
				{/each}
			</div>
			{#if testing}
				<button class="btn block" style="margin-top:12px" onclick={stopTest}><Square size={14} /> Stop test</button>
			{/if}
			<div class="ff card">
				<span class="icon-tile accent"><Search size={20} /></span>
				<div class="grow"><strong>Find a faulty pixel</strong><div class="faint small">A few yes/no questions pinpoint the first bad pixel.</div></div>
				<button class="btn" onclick={() => (faultOpen = true)}>Start</button>
			</div>
		{/if}
	{/if}

	{#snippet footer()}
		<span class="small faint grow" style="align-self:center">{dirty ? 'Unsaved changes' : 'All changes saved'}</span>
		<button class="btn ghost" disabled={!dirty} onclick={() => original && (draft = structuredClone($state.snapshot(original) as Prop))}>Discard</button>
		<button class="btn primary" disabled={!dirty || saving} onclick={save}>{saving ? 'Saving…' : 'Save changes'}</button>
	{/snippet}
</Drawer>

<FaultFinder bind:open={faultOpen} prop={original} />

<style>
	.preview {
		overflow: hidden;
		margin-bottom: 16px;
		border-radius: 14px;
	}
	.tabs {
		margin-bottom: 20px;
	}
	.danger-zone {
		margin-top: 24px;
		padding-top: 16px;
		border-top: 1px solid var(--border);
	}
	.power {
		display: flex;
		flex-direction: column;
		gap: 16px;
	}
	.pstat {
		display: flex;
		flex-direction: column;
		gap: 2px;
		padding: 16px;
	}
	.big {
		font-size: 26px;
		font-weight: 650;
		letter-spacing: -0.03em;
	}
	.fuse {
		display: flex;
		flex-direction: column;
		gap: 6px;
	}
	.tests {
		display: grid;
		grid-template-columns: repeat(4, 1fr);
		gap: 8px;
	}
	@media (max-width: 520px) {
		.tests {
			grid-template-columns: repeat(2, 1fr);
		}
	}
	.test {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 8px;
		padding: 14px 8px;
		border-radius: 12px;
		border: 1px solid var(--border-2);
		background: var(--surface-2);
		font-size: 12.5px;
		font-weight: 540;
		transition: all 150ms var(--ease);
	}
	.test:hover {
		border-color: var(--border-3);
	}
	.test.on {
		border-color: var(--accent);
		background: var(--accent-soft);
	}
	.sw {
		width: 28px;
		height: 28px;
		border-radius: 50%;
		box-shadow: 0 0 12px rgba(255, 255, 255, 0.15);
	}
	.grad {
		background: conic-gradient(#ff3b3b, #3fcf5e, #3b6bff, #ff3b3b);
	}
	.grad.chase {
		background: repeating-linear-gradient(90deg, #fff 0 5px, #333 5px 10px);
	}
	.grad.countPixels {
		background: repeating-linear-gradient(90deg, #3b6bff 0 4px, #3fcf5e 4px 6px, #3b6bff 6px 10px, #ff3b3b 10px 12px);
	}
	.grad.walk {
		background: radial-gradient(circle, #fff 20%, #222 22%);
	}
	.ff {
		display: flex;
		align-items: center;
		gap: 12px;
		padding: 14px;
		margin-top: 20px;
		background: var(--surface-2);
	}
</style>
