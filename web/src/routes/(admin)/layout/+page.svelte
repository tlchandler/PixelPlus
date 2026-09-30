<script lang="ts">
	import { api } from '$lib/api/client';
	import type { PropLayout } from '$lib/api/types';
	import { app } from '$lib/stores/app.svelte';
	import { toasts } from '$lib/stores/toasts.svelte';
	import { KIND_META } from '$lib/util/kinds';
	import LayoutCanvas from '$lib/components/viz/LayoutCanvas.svelte';
	import Segmented from '$lib/components/ui/Segmented.svelte';
	import Switch from '$lib/components/ui/Switch.svelte';
	import {
		Eye,
		Move,
		ZoomIn,
		ZoomOut,
		Scan,
		Search,
		Tag,
		PanelRightClose,
		PanelRightOpen
	} from '@lucide/svelte';

	const show = $derived(app.show);
	let mode = $state<'live' | 'edit'>('live');
	let labels = $state(false);
	let selected = $state<string | null>(null);
	let q = $state('');
	let panel = $state(true);
	let canvasRef: LayoutCanvas | undefined = $state();

	const sel = $derived(show?.props.find((p) => p.id === selected) ?? null);
	const list = $derived(
		show?.props.filter((p) => !q || p.name.toLowerCase().includes(q.toLowerCase())) ?? []
	);

	async function saveLayout(id: string, patch: Partial<PropLayout>) {
		const p = show?.props.find((x) => x.id === id);
		if (!p) return;
		const before = p.layout ? { ...p.layout } : undefined;
		const layout = { ...(p.layout ?? { x: 0, y: 0, w: 80, h: 60, rotation: 0 }), ...patch };
		app.updateShow((s) => {
			const t = s.props.find((x) => x.id === id);
			if (t) t.layout = layout;
		});
		try {
			await api.props.update(id, { layout });
			toasts.push({
				kind: 'success',
				message: `Moved ${p.name}`,
				timeout: 3000,
				action: before
					? { label: 'Undo', run: () => app.mutate(() => api.props.update(id, { layout: before })) }
					: undefined
			});
		} catch (e) {
			toasts.error('Could not save position', (e as Error).message);
			app.reloadShow();
		}
	}
</script>

<div class="wrap">
	<div class="bar">
		<h1>Layout</h1>
		<Segmented
			bind:value={mode}
			label="Mode"
			options={[
				{ value: 'live', label: 'Live', icon: Eye },
				{ value: 'edit', label: 'Arrange', icon: Move }
			]}
		/>
		<label class="lbl"
			><Switch bind:checked={labels} label="Show names" size="sm" />
			<Tag size={14} /> <span class="hide-sm">Names</span></label
		>
		<span class="grow"></span>
		<div class="zoom">
			<button class="btn ghost icon sm" onclick={() => canvasRef?.zoom(1 / 1.3)} aria-label="Zoom out"
				><ZoomOut size={16} /></button
			>
			<button class="btn ghost icon sm" onclick={() => canvasRef?.fit()} aria-label="Fit to screen"
				><Scan size={16} /></button
			>
			<button class="btn ghost icon sm" onclick={() => canvasRef?.zoom(1.3)} aria-label="Zoom in"
				><ZoomIn size={16} /></button
			>
		</div>
		<button
			class="btn ghost icon sm hide-sm"
			onclick={() => (panel = !panel)}
			aria-label={panel ? 'Hide prop list' : 'Show prop list'}
		>
			{#if panel}<PanelRightClose size={16} />{:else}<PanelRightOpen size={16} />{/if}
		</button>
	</div>

	<div class="body">
		<div class="stage">
			{#if show}
				<LayoutCanvas
					bind:this={canvasRef}
					props={show.props}
					edit={mode === 'edit'}
					{labels}
					bind:selected
					onmove={(id, pos) => saveLayout(id, pos)}
				/>
			{/if}
			{#if mode === 'edit'}
				<div class="hint">
					Drag props to arrange them. Hold <span class="kbd">Shift</span> for fine moves. Scroll or pinch to zoom.
				</div>
			{/if}
		</div>

		{#if panel}
			<aside class="side">
				{#if sel}
					{@const K = KIND_META[sel.kind]}
					<div class="selcard">
						<div class="row">
							<span class="icon-tile accent"><K.icon size={18} /></span>
							<div class="grow">
								<strong class="ellipsis">{sel.name}</strong>
								<div class="faint small">{sel.pixelCount} pixels</div>
							</div>
						</div>
						{#if mode === 'edit' && sel.layout}
							<div class="xy">
								{#each [['x', 'X'], ['y', 'Y'], ['w', 'Width'], ['h', 'Height'], ['rotation', 'Rotate°']] as [k, label] (k)}
									<label class="field">
										<span class="label">{label}</span>
										<input
											class="input sm"
											type="number"
											value={Math.round((sel.layout as any)[k])}
											onchange={(e) =>
												saveLayout(sel.id, { [k]: Number((e.target as HTMLInputElement).value) })}
										/>
									</label>
								{/each}
							</div>
						{/if}
						<a class="btn sm block" href="/props#{sel.id}">Open prop</a>
					</div>
				{/if}
				<div class="input-group">
					<span class="prefix"><Search size={15} /></span><input
						class="input sm"
						placeholder="Find a prop"
						bind:value={q}
						data-search
						aria-label="Find a prop"
					/>
				</div>
				<div class="plist">
					{#each list as p (p.id)}
						{@const K = KIND_META[p.kind]}
						<button
							class="pitem"
							class:on={selected === p.id}
							onclick={() => (selected = selected === p.id ? null : p.id)}
						>
							<K.icon size={15} /><span class="grow ellipsis">{p.name}</span><span class="faint tiny num"
								>{p.pixelCount}</span
							>
						</button>
					{/each}
				</div>
			</aside>
		{/if}
	</div>
</div>

<style>
	.wrap {
		height: calc(100dvh - var(--transport-h));
		display: flex;
		flex-direction: column;
		animation: page-in 260ms var(--ease);
	}
	.bar {
		display: flex;
		align-items: center;
		gap: 14px;
		padding: 14px 20px;
		border-bottom: 1px solid var(--border);
		flex-wrap: wrap;
	}
	.bar h1 {
		font-size: 20px;
		margin-right: 6px;
	}
	.lbl {
		display: flex;
		align-items: center;
		gap: 6px;
		color: var(--text-2);
		font-size: 13px;
		cursor: pointer;
	}
	.zoom {
		display: flex;
		gap: 2px;
		padding: 2px;
		border-radius: 10px;
		border: 1px solid var(--border);
	}
	.body {
		flex: 1;
		display: flex;
		min-height: 0;
	}
	.stage {
		flex: 1;
		position: relative;
		min-width: 0;
	}
	.stage :global(canvas) {
		position: absolute;
		inset: 0;
		height: 100% !important;
	}
	.hint {
		position: absolute;
		left: 50%;
		bottom: 16px;
		transform: translateX(-50%);
		padding: 8px 14px;
		border-radius: 99px;
		background: rgba(20, 21, 26, 0.85);
		backdrop-filter: blur(8px);
		color: #cfcfd6;
		font-size: 12.5px;
		white-space: nowrap;
	}
	.side {
		width: 280px;
		border-left: 1px solid var(--border);
		display: flex;
		flex-direction: column;
		gap: 12px;
		padding: 14px;
		background: var(--sidebar);
		min-height: 0;
	}
	.selcard {
		display: flex;
		flex-direction: column;
		gap: 12px;
		padding: 14px;
		border-radius: 14px;
		background: var(--surface-2);
		border: 1px solid var(--accent-line);
	}
	.xy {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: 8px;
	}
	.plist {
		flex: 1;
		overflow: auto;
		display: flex;
		flex-direction: column;
		gap: 2px;
		margin: 0 -6px;
	}
	.pitem {
		display: flex;
		align-items: center;
		gap: 10px;
		height: 36px;
		padding: 0 10px;
		border-radius: 8px;
		font-size: 13px;
		color: var(--text-2);
		text-align: left;
	}
	.pitem:hover {
		background: var(--surface-2);
		color: var(--text);
	}
	.pitem.on {
		background: var(--accent-soft);
		color: var(--accent);
	}
	@media (max-width: 760px) {
		.wrap {
			height: calc(100dvh - var(--tabbar-h) - 76px - env(safe-area-inset-bottom));
		}
		.side,
		.hide-sm {
			display: none;
		}
		.bar {
			padding: 10px 12px;
			gap: 10px;
		}
		.hint {
			white-space: normal;
			width: calc(100% - 32px);
			text-align: center;
			border-radius: 12px;
		}
	}
</style>
