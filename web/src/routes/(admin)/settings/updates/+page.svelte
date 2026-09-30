<!--
	Settings → Updates (F15, ARCHITECTURE §12.13) and the controller transfer file (F10). WS5.
	Signed over-the-air updates for the whole show: channel, automatic installs inside a window
	(never near a show), "Update everything" with live progress (followers first, then the
	leader; any failure puts every controller back), history and "Roll back".
-->
<script lang="ts">
	import { onDestroy, onMount } from 'svelte';
	import {
		ArrowLeft,
		Check,
		CircleAlert,
		Clock,
		Download,
		History,
		Info,
		PackageCheck,
		RefreshCw,
		RotateCcw,
		ShieldCheck,
		TriangleAlert,
		HardDriveDownload
	} from '@lucide/svelte';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import Segmented from '$lib/components/ui/Segmented.svelte';
	import TransferExport from '$lib/components/controllers/TransferExport.svelte';
	import { fleetApi, nodePhaseLabel, runPhaseLabel } from '$lib/api/fleet';
	import type {
		AutoUpdate,
		UpdateChannel,
		UpdateInfo,
		UpdateRun,
		UpdateSettings,
		Weekday
	} from '$lib/api/types';
	import { app } from '$lib/stores/app.svelte';
	import { confirm, toasts } from '$lib/stores/toasts.svelte';

	let info = $state<UpdateInfo | null>(null);
	let loadError = $state<string | null>(null);
	let checking = $state(false);
	let starting = $state(false);
	let saving = $state(false);
	let run = $state<UpdateRun | null>(null);
	let unsub: (() => void) | undefined;
	let poll: ReturnType<typeof setInterval> | undefined;

	const DEFAULTS: UpdateSettings = {
		channel: 'stable',
		auto: 'notify',
		window: { from: '10:00', to: '14:00', days: [] },
		avoidShowHours: 2
	};
	let form = $state<UpdateSettings>(structuredClone(DEFAULTS));
	let dirty = $state(false);
	$effect(() => {
		const s = app.show?.settings.updates;
		if (s && !dirty) form = structuredClone($state.snapshot(s) as UpdateSettings);
	});

	const isFollower = $derived(app.system?.role === 'follower');
	const docker = $derived(!!app.system?.docker);
	const running = $derived(!!run && !['done', 'rolledBack', 'failed'].includes(run.phase));

	async function load(refresh = false) {
		try {
			info = await fleetApi.updates(refresh);
			if (info.run) run = info.run;
			loadError = null;
		} catch (e) {
			loadError = (e as Error).message;
		}
	}

	onMount(() => {
		void load();
		unsub = app.onMessage('updateJob', (j) => {
			run = j;
			if (['done', 'rolledBack', 'failed'].includes(j.phase)) void load();
		});
		// The leader restarts during its own update: keep polling so the page recovers.
		poll = setInterval(() => running && load(), 5000);
	});
	onDestroy(() => {
		unsub?.();
		clearInterval(poll);
	});

	async function check() {
		checking = true;
		await load(true);
		checking = false;
		if (info && !info.available && !loadError) toasts.success(`PixelPlus ${info.current} is the latest`);
	}

	async function start() {
		if (!info) return;
		const n = info.nodes?.length ?? 1;
		const ok = await confirm({
			title: `Update to PixelPlus ${info.latest}?`,
			message:
				n > 1
					? `All ${n} controllers are updated: the followers first, then this leader. The lights pause for about a minute. If any controller has a problem, every one goes back to ${info.current} by itself.`
					: `The lights pause for about a minute. If the new version has a problem, PixelPlus goes back to ${info.current} by itself.`,
			confirmLabel: 'Update now'
		});
		if (!ok) return;
		starting = true;
		try {
			const r = await fleetApi.startUpdate({ version: info.latest, scope: 'cluster' });
			if (r.run) run = r.run;
			else toasts.info(r.message);
		} catch (e) {
			toasts.error("The update didn't start", (e as Error).message);
		} finally {
			starting = false;
		}
	}

	async function rollback() {
		if (!info?.previous) return;
		const ok = await confirm({
			title: `Go back to PixelPlus ${info.previous}?`,
			message: 'Every controller reinstalls the version it had before the last update.',
			confirmLabel: `Roll back to ${info.previous}`,
			danger: true
		});
		if (!ok) return;
		try {
			run = (await fleetApi.rollback('cluster')).run;
		} catch (e) {
			toasts.error("Couldn't roll back", (e as Error).message);
		}
	}

	async function save() {
		saving = true;
		try {
			await fleetApi.updateSettings($state.snapshot(form) as UpdateSettings);
			dirty = false;
			await app.reloadShow();
			toasts.success('Update settings saved');
			void load();
		} catch (e) {
			toasts.error("Couldn't save", (e as Error).message);
		} finally {
			saving = false;
		}
	}

	const touch = () => (dirty = true);
	const DAYS: { v: Weekday; l: string }[] = [
		{ v: 'mon', l: 'Mon' },
		{ v: 'tue', l: 'Tue' },
		{ v: 'wed', l: 'Wed' },
		{ v: 'thu', l: 'Thu' },
		{ v: 'fri', l: 'Fri' },
		{ v: 'sat', l: 'Sat' },
		{ v: 'sun', l: 'Sun' }
	];
	function toggleDay(d: Weekday) {
		const days = form.window.days.includes(d)
			? form.window.days.filter((x) => x !== d)
			: [...form.window.days, d];
		form.window.days = days;
		touch();
	}
	const when = (s?: string) =>
		s
			? new Date(s).toLocaleString(undefined, {
					month: 'short',
					day: 'numeric',
					hour: 'numeric',
					minute: '2-digit'
				})
			: '';
	const phaseClass = (p: string) =>
		p === 'healthy' || p === 'done'
			? 'green'
			: p === 'failed'
				? 'red'
				: p === 'rolledBack' || p === 'rollingBack'
					? 'purple'
					: 'blue';
</script>

<svelte:head><title>Updates · Settings · PixelPlus</title></svelte:head>

<div class="page upd">
	<a class="btn ghost sm back" href="/settings"><ArrowLeft size={16} /> Settings</a>
	<PageHeader
		title="Updates"
		subtitle="Signed updates for every controller of the show, installed when no show is on — and undone by themselves if anything goes wrong."
	/>

	{#if isFollower}
		<div class="notice info">
			<Info size={18} />
			<div>
				This controller is updated together with its show leader. Open Settings → Updates on the leader.
			</div>
		</div>
	{:else if loadError && !info}
		<div class="notice danger">
			<TriangleAlert size={18} />
			<div>{loadError}</div>
		</div>
	{:else if !info}
		<div class="card skeleton sk"></div>
	{:else}
		<!-- Status and the one button -->
		<section class="card">
			<div class="card-head">
				<PackageCheck size={18} />
				<h2 class="grow">PixelPlus {info.current}</h2>
				<button class="btn ghost sm" onclick={check} disabled={checking || running}>
					<RefreshCw size={15} />
					{checking ? 'Checking…' : 'Check now'}
				</button>
			</div>
			<div class="card-body col">
				{#if docker}
					<p class="muted">
						PixelPlus runs in Docker here: update by pulling the new image (<span class="mono"
							>docker compose pull &amp;&amp; docker compose up -d</span
						>). Followers on Pis are updated from their own pages.
					</p>
				{:else if info.available}
					<div class="row avail">
						<Download size={18} />
						<div class="grow">
							<strong>PixelPlus {info.latest} is available</strong>
							<span class="badge {info.channel === 'beta' ? 'purple' : 'blue'}"
								>{info.channel ?? 'stable'}</span
							>
						</div>
					</div>
					{#if info.notes}<pre class="notes">{info.notes}</pre>{/if}
					{#if info.problems?.length}
						<div class="notice warn small">
							<Clock size={16} />
							<div>
								<strong>Not right now:</strong>
								<ul>
									{#each info.problems as p (p)}<li>{p}</li>{/each}
								</ul>
							</div>
						</div>
					{/if}
					{#if info.canApply}
						<div class="row">
							<button
								class="btn primary"
								onclick={start}
								disabled={starting || running || !!info.problems?.length}
							>
								<Download size={16} />
								{starting ? 'Starting…' : (info.nodes?.length ?? 1) > 1 ? 'Update everything' : 'Update now'}
							</button>
							<span class="faint small"
								><ShieldCheck size={13} /> Signed by PixelPlus; checked on every controller before installing.</span
							>
						</div>
					{:else if info.message}
						<p class="muted">{info.message}</p>
					{/if}
				{:else}
					<p class="row good"><Check size={16} /> Up to date.</p>
					{#if info.message}<p class="muted small">{info.message}</p>{/if}
				{/if}
			</div>
		</section>

		{#if run}
			<section class="card" aria-live="polite">
				<div class="card-head">
					{#if run.phase === 'failed'}<CircleAlert size={18} />{:else}<RefreshCw
							size={18}
							class={running ? 'spin' : ''}
						/>{/if}
					<h2 class="grow">
						{run.kind === 'rollback' ? 'Going back to' : 'Updating to'}
						{run.version}: {runPhaseLabel[run.phase]}
					</h2>
					<span class="faint small">{when(run.startedAt)}</span>
				</div>
				<div class="card-body col">
					{#if run.message}<p class="small {run.phase === 'done' ? 'good' : 'muted'}">{run.message}</p>{/if}
					<ul class="nodes">
						{#each run.nodes as n (n.id)}
							<li>
								<span class="grow"
									><strong>{n.name}</strong>{#if n.isSelf}<span class="faint small">
											· this leader</span
										>{/if}</span
								>
								<span class="faint small mono">{n.from} → {run.version}</span>
								<span class="badge {phaseClass(n.phase)}">{nodePhaseLabel[n.phase]}</span>
								{#if n.message}<span class="msg small muted">{n.message}</span>{/if}
							</li>
						{/each}
					</ul>
					{#if run.phase === 'committingLeader'}
						<p class="faint small">This controller restarts now; the page reconnects by itself.</p>
					{/if}
				</div>
			</section>
		{/if}

		<!-- Channel and automatic updates -->
		<section class="card">
			<div class="card-head">
				<Clock size={18} />
				<h2 class="grow">When to update</h2>
			</div>
			<div class="card-body col">
				<div class="grid2">
					<div class="field">
						<span class="label">Channel</span>
						<Segmented
							label="Update channel"
							bind:value={form.channel}
							onchange={touch}
							options={[
								{ value: 'stable' as UpdateChannel, label: 'Stable' },
								{ value: 'beta' as UpdateChannel, label: 'Beta' }
							]}
						/>
						{#if form.channel === 'beta'}<span class="hint warnc"
								>Beta releases get new features first and may have rough edges. Not recommended during the
								season.</span
							>{/if}
					</div>
					<div class="field">
						<span class="label">Automatic updates</span>
						<Segmented
							label="Automatic updates"
							bind:value={form.auto}
							onchange={touch}
							options={[
								{ value: 'off' as AutoUpdate, label: 'Off' },
								{ value: 'notify' as AutoUpdate, label: 'Tell me' },
								{ value: 'install' as AutoUpdate, label: 'Install' }
							]}
						/>
					</div>
				</div>
				{#if form.auto === 'install'}
					<div class="grid3">
						<label class="field">
							<span class="label">Install between</span>
							<input class="input" type="time" bind:value={form.window.from} oninput={touch} />
						</label>
						<label class="field">
							<span class="label">and</span>
							<input class="input" type="time" bind:value={form.window.to} oninput={touch} />
						</label>
						<label class="field">
							<span class="label">Never within (hours of a show)</span>
							<input
								class="input"
								type="number"
								min="1"
								max="24"
								bind:value={form.avoidShowHours}
								oninput={touch}
							/>
						</label>
					</div>
					<div class="field">
						<span class="label">On</span>
						<div class="row wrap">
							{#each DAYS as d (d.v)}
								<button
									type="button"
									class="chip"
									aria-pressed={form.window.days.length === 0 || form.window.days.includes(d.v)}
									onclick={() => toggleDay(d.v)}>{d.l}</button
								>
							{/each}
							<span class="faint small">{form.window.days.length === 0 ? 'every day' : ''}</span>
						</div>
					</div>
					<p class="faint small">
						Never while a show plays, inside a show window or within {form.avoidShowHours} h before one, and only
						when every controller is online.
					</p>
				{/if}
				<div class="row">
					<button class="btn primary" onclick={save} disabled={!dirty || saving}
						>{saving ? 'Saving…' : 'Save'}</button
					>
				</div>
			</div>
		</section>

		{#if info.nodes && info.nodes.length > 1}
			<section class="card">
				<div class="card-head"><h2 class="grow">Controllers</h2></div>
				<div class="card-body">
					<ul class="nodes">
						{#each info.nodes as n (n.id)}
							<li>
								<span class="grow"><strong>{n.name ?? n.id}</strong></span>
								<span class="mono small">{n.version || '—'}</span>
								{#if n.online === false}<span class="badge outline">offline</span>
								{:else if n.version && n.version !== info.current}<span class="badge red"
										>different version</span
									>
								{:else if !n.canApply}<span class="badge outline">can't update itself</span>{/if}
							</li>
						{/each}
					</ul>
				</div>
			</section>
		{/if}

		{#if info.history?.length || info.previous}
			<section class="card">
				<div class="card-head">
					<History size={18} />
					<h2 class="grow">History</h2>
					{#if info.previous && !docker}
						<button class="btn sm danger" onclick={rollback} disabled={running}
							><RotateCcw size={15} /> Roll back to {info.previous}</button
						>
					{/if}
				</div>
				<div class="card-body">
					<ul class="hist">
						{#each info.history ?? [] as h (h.at + h.to)}
							<li>
								{#if h.ok}<Check size={15} class="good" />{:else}<CircleAlert size={15} class="bad" />{/if}
								<span class="mono">{h.from} → {h.to}</span>
								<span class="faint small grow">{h.message ?? ''}</span>
								<span class="faint small">{when(h.at)}</span>
							</li>
						{/each}
					</ul>
				</div>
			</section>
		{/if}
	{/if}

	{#if !isFollower}
		<section class="card">
			<div class="card-head">
				<HardDriveDownload size={18} />
				<h2 class="grow">Controller transfer file</h2>
			</div>
			<div class="card-body"><TransferExport /></div>
		</section>
	{/if}
</div>

<style>
	.upd {
		max-width: 820px;
	}
	.back {
		margin-bottom: 8px;
	}
	.sk {
		height: 160px;
	}
	.col {
		display: flex;
		flex-direction: column;
		gap: 12px;
	}
	.col p {
		margin: 0;
	}
	.row {
		display: flex;
		align-items: center;
		gap: 10px;
	}
	.wrap {
		flex-wrap: wrap;
		gap: 6px;
	}
	.grow {
		flex: 1 1 auto;
		min-width: 0;
	}
	.good,
	:global(svg.good) {
		color: var(--green);
	}
	:global(svg.bad) {
		color: var(--red);
	}
	.warnc {
		color: var(--amber, var(--text-2));
	}
	.notes {
		margin: 0;
		white-space: pre-wrap;
		font: inherit;
		font-size: 13.5px;
		color: var(--text-2);
		background: var(--surface-2);
		border: 1px solid var(--border-2);
		border-radius: var(--r-2);
		padding: 10px 12px;
		max-height: 220px;
		overflow: auto;
	}
	.notice ul {
		margin: 4px 0 0;
		padding-left: 18px;
	}
	.grid2 {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: 14px;
	}
	.grid3 {
		display: grid;
		grid-template-columns: 1fr 1fr 1fr;
		gap: 12px;
	}
	.nodes,
	.hist {
		list-style: none;
		margin: 0;
		padding: 0;
		display: grid;
		gap: 4px;
	}
	.nodes li,
	.hist li {
		display: flex;
		align-items: center;
		flex-wrap: wrap;
		gap: 8px;
		min-height: 34px;
		border-bottom: 1px solid var(--border-2);
		padding: 4px 0;
	}
	.nodes li:last-child,
	.hist li:last-child {
		border-bottom: 0;
	}
	.msg {
		flex-basis: 100%;
	}
	:global(.spin) {
		animation: spin 1.2s linear infinite;
	}
	@keyframes spin {
		to {
			transform: rotate(360deg);
		}
	}
	@media (prefers-reduced-motion: reduce) {
		:global(.spin) {
			animation: none;
		}
	}
	@media (max-width: 640px) {
		.grid2,
		.grid3 {
			grid-template-columns: 1fr;
		}
	}
</style>
