<!--
	Reports (F11, ARCHITECTURE §12.10). WS6.
	Every show night as a report: history of nights (songs per night, status marks),
	and the selected night's numbers, problems, controllers (temperature and sync charts),
	lights, housekeeping. "Make it now" / "Send now", and a preview of the email.
-->
<script lang="ts">
	import { onMount } from 'svelte';
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import {
		ClipboardList,
		RefreshCw,
		Send,
		Mail,
		Settings2,
		CircleCheck,
		TriangleAlert,
		CircleX,
		Thermometer,
		Lightbulb,
		HardDrive,
		Music,
		Hand,
		Clock,
		CalendarDays
	} from '@lucide/svelte';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import Skeleton from '$lib/components/ui/Skeleton.svelte';
	import Modal from '$lib/components/ui/Modal.svelte';
	import { request } from '$lib/api/client';
	import { app } from '$lib/stores/app.svelte';
	import { toasts } from '$lib/stores/toasts.svelte';
	import { fmtTemp, tempUnitOf, tempValue, tempSymbol } from '$lib/util/units';
	import { reportsApi, type NightReportFull, type ReportSummaryFull } from '$lib/insight/api';
	import LineChart from '$lib/insight/LineChart.svelte';
	import NightBars from '$lib/insight/NightBars.svelte';

	let list = $state<ReportSummaryFull[] | null>(null);
	let report = $state<NightReportFull | null>(null);
	let loadingReport = $state(false);
	let missing = $state(false);
	let busy = $state<'' | 'run' | 'send'>('');
	let emailHtml = $state<string | null>(null);

	const tz = $derived(app.show?.schedule.location.timezone);
	const tunit = $derived(tempUnitOf(app.show));
	const selected = $derived(page.url.searchParams.get('date') ?? list?.[0]?.date ?? null);

	async function loadList() {
		try {
			list = await reportsApi.list(60);
		} catch (e) {
			list = [];
			toasts.error("Couldn't load the reports", (e as Error).message);
		}
	}

	$effect(() => {
		const d = selected;
		if (!d) {
			report = null;
			return;
		}
		loadingReport = true;
		missing = false;
		reportsApi
			.get(d)
			.then((r) => (report = r))
			.catch(() => {
				report = null;
				missing = true;
			})
			.finally(() => (loadingReport = false));
	});

	onMount(loadList);

	function pick(date: string) {
		goto(`/reports?date=${date}`, { noScroll: true, keepFocus: true });
	}

	async function run(send: boolean) {
		busy = send ? 'send' : 'run';
		try {
			const r = await reportsApi.run(selected && missing ? selected : (report?.date ?? undefined), send);
			report = r;
			missing = false;
			await loadList();
			if (send) {
				const failed = (r.delivery ?? []).some((l) => /failed|not set up/i.test(l));
				(failed ? toasts.warn : toasts.success).call(toasts, (r.delivery ?? ['Sent']).join(' '));
			} else toasts.success('Report updated');
			if (page.url.searchParams.get('date') !== r.date) pick(r.date);
		} catch (e) {
			toasts.error("Couldn't make the report", (e as Error).message);
		} finally {
			busy = '';
		}
	}

	async function previewEmail() {
		if (!report) return;
		try {
			emailHtml = await request<string>('GET', `/reports/${report.date}/email`, undefined, { raw: 'text' });
		} catch (e) {
			toasts.error("Couldn't load the email", (e as Error).message);
		}
	}

	const longDay = (d: string) =>
		new Date(d + 'T12:00:00').toLocaleDateString(undefined, {
			weekday: 'long',
			month: 'long',
			day: 'numeric'
		});
	const shortDay = (d: string) =>
		new Date(d + 'T12:00:00').toLocaleDateString(undefined, {
			weekday: 'short',
			month: 'short',
			day: 'numeric'
		});
	const time = (iso: string) =>
		new Date(iso).toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit', timeZone: tz });
	const hm = (min: number) =>
		min >= 60 ? `${Math.floor(min / 60)}h ${String(min % 60).padStart(2, '0')}m` : `${min} min`;
	const statusText = { ok: 'All good', warn: 'Mostly fine', fail: 'Needs attention' } as const;
	const statusClass = { ok: 'green', warn: 'accent', fail: 'red' } as const;
	const tempSeries = $derived(
		(report?.series?.tempC ?? []).map((s) => ({
			...s,
			points: s.points.map(([t, v]) => [t, tempValue(v, tunit)] as [number, number])
		}))
	);
</script>

<svelte:head><title>Reports · PixelPlus</title></svelte:head>

<div class="page reports">
	<PageHeader title="Reports" subtitle="How each show night went — made every morning and sent to you.">
		{#snippet actions()}
			<a class="btn ghost" href="/settings/reports"><Settings2 size={16} /> Report settings</a>
			<button class="btn" onclick={() => run(false)} disabled={!!busy}
				><RefreshCw size={16} class={busy === 'run' ? 'spin' : ''} /> Make it now</button
			>
		{/snippet}
	</PageHeader>

	{#if list === null}
		<div class="card card-pad"><Skeleton count={4} /></div>
	{:else if list.length === 0 && !report}
		<section class="card">
			<EmptyState
				icon={ClipboardList}
				title="No reports yet"
				message="After your first show night PixelPlus writes a report here every morning (and emails or pushes it if alerts are set up). You can also make one for last night now."
			>
				<button class="btn primary" onclick={() => run(false)} disabled={!!busy}
					>Make last night's report</button
				>
			</EmptyState>
		</section>
	{:else}
		{#if list.length > 1}
			<section class="card card-pad history">
				<div class="row between">
					<h2 class="small">Songs per night</h2>
					<span class="faint tiny">✓ good · ! warnings · ✕ problems</span>
				</div>
				<NightBars nights={list} selected={selected ?? undefined} onpick={pick} />
			</section>
		{/if}

		<div class="split">
			<nav class="card list nights" aria-label="Nights">
				{#each list as n (n.date)}
					<a
						class="list-row interactive night"
						class:active={n.date === selected}
						href="/reports?date={n.date}"
						data-sveltekit-noscroll
						aria-current={n.date === selected ? 'page' : undefined}
					>
						<span class="sicon {n.status}" aria-label={statusText[n.status]}>
							{#if n.status === 'ok'}<CircleCheck size={16} />{:else if n.status === 'warn'}<TriangleAlert
									size={16}
								/>{:else}<CircleX size={16} />{/if}
						</span>
						<div class="grow" style="min-width:0">
							<div class="small">{shortDay(n.date)}</div>
							<div class="faint tiny ellipsis">{n.headline}</div>
						</div>
					</a>
				{/each}
			</nav>

			<div class="detail">
				{#if loadingReport && !report}
					<div class="card card-pad"><Skeleton count={6} /></div>
				{:else if missing || !report}
					<section class="card">
						<EmptyState
							icon={CalendarDays}
							title="No report for {selected ? shortDay(selected) : 'that night'}"
							message="Make it now from the show journal."
						>
							<button class="btn primary" onclick={() => run(false)} disabled={!!busy}>Make it now</button>
						</EmptyState>
					</section>
				{:else}
					{@const r = report}
					<section class="card card-pad head">
						<div class="row wrap between">
							<div>
								<div class="eyebrow">{r.season ?? 'Show night'}</div>
								<h2 class="day">{longDay(r.date)}</h2>
							</div>
							<span class="badge {statusClass[r.status]}">
								{#if r.status === 'ok'}<CircleCheck size={13} />{:else if r.status === 'warn'}<TriangleAlert
										size={13}
									/>{:else}<CircleX size={13} />{/if}
								{statusText[r.status]}
							</span>
						</div>
						<p class="headline">{r.headline}.</p>
						<div class="tiles">
							<div class="tile">
								<Clock size={16} /><span class="v num">{r.shows.length}</span><span class="l"
									>shows · {hm(r.runtimeMin ?? 0)}</span
								>
							</div>
							<div class="tile">
								<Music size={16} /><span class="v num">{r.itemsPlayed}</span><span class="l">songs</span>
							</div>
							<div class="tile">
								<Hand size={16} /><span class="v num">{r.requests}</span><span class="l">requests</span>
							</div>
							<div class="tile">
								<TriangleAlert size={16} /><span class="v num"
									>{r.problems.reduce((a, p) => a + p.count, 0)}</span
								><span class="l">problems</span>
							</div>
						</div>
						<div class="row wrap acts">
							<button class="btn sm" onclick={() => run(true)} disabled={!!busy}
								><Send size={14} /> Send now</button
							>
							<button class="btn sm ghost" onclick={previewEmail}><Mail size={14} /> Email preview</button>
							{#if r.generatedAt}<span class="faint tiny"
									>Made {new Date(r.generatedAt).toLocaleString()}</span
								>{/if}
						</div>
						{#if r.delivery?.length}
							<ul class="faint tiny delivery">
								{#each r.delivery as d (d)}<li>{d}</li>{/each}
							</ul>
						{/if}
					</section>

					{#if r.problems.length}
						<section class="card">
							<div class="card-head">
								<TriangleAlert size={16} />
								<h2 class="grow">Problems</h2>
							</div>
							<div class="list">
								{#each r.problems as p (p.level + p.code)}
									<div class="list-row">
										<span class="sicon {p.level === 'error' ? 'fail' : 'warn'}"
											>{#if p.level === 'error'}<CircleX size={16} />{:else}<TriangleAlert
													size={16}
												/>{/if}</span
										>
										<div class="grow small">{p.message}</div>
										<span class="faint tiny num">×{p.count}</span>
									</div>
								{/each}
							</div>
						</section>
					{/if}

					<div class="grid-2 two">
						<section class="card">
							<div class="card-head">
								<Clock size={16} />
								<h2 class="grow">Shows</h2>
							</div>
							<div class="list">
								{#each r.shows as s (s.entryId + s.startedAt)}
									<div class="list-row">
										<div class="grow small">{s.name}</div>
										<span class="faint tiny num">{time(s.startedAt)} · {hm(s.runtimeMin)}</span>
									</div>
								{:else}
									<div class="list-row faint small">No show window ran.</div>
								{/each}
							</div>
						</section>
						<section class="card">
							<div class="card-head">
								<Hand size={16} />
								<h2 class="grow">Most requested</h2>
							</div>
							<div class="list">
								{#each r.topRequests as q (q.sequenceId)}
									<div class="list-row">
										<div class="grow small ellipsis">{q.name}</div>
										<span class="num small">{q.count}</span>
									</div>
								{:else}
									<div class="list-row faint small">No song requests.</div>
								{/each}
							</div>
						</section>
					</div>

					<section class="card">
						<div class="card-head">
							<Thermometer size={16} />
							<h2 class="grow">Controllers</h2>
						</div>
						<div class="table-wrap">
							<table class="table">
								<thead>
									<tr
										><th>Controller</th><th>Temperature</th><th>Lowest voltage</th><th
											>Sync (typical / worst)</th
										><th>Offline</th></tr
									>
								</thead>
								<tbody>
									{#each r.nodes as n (n.nodeId)}
										<tr>
											<td>{n.name}</td>
											<td class="num">
												{#if n.tempMinC != null && n.tempMaxC != null}
													{tempValue(n.tempMinC, tunit).toFixed(0)}–{fmtTemp(n.tempMaxC, tunit)}
												{:else}—{/if}
											</td>
											<td class="num">{n.voltsMin != null ? `${n.voltsMin.toFixed(1)} V` : '—'}</td>
											<td class="num">
												{n.syncP50Ms != null
													? `${n.syncP50Ms.toFixed(1)} / ${(n.syncP95Ms ?? 0).toFixed(1)} ms`
													: '—'}
											</td>
											<td class="num" class:bad={n.offlineMin > 10}
												>{n.offlineMin > 0 ? `${Math.round(n.offlineMin)} min` : '—'}</td
											>
										</tr>
									{/each}
								</tbody>
							</table>
						</div>
						{#if tempSeries.length || r.series?.syncMs.length}
							<div class="charts">
								{#each tempSeries as s (s.nodeId)}
									<LineChart
										points={s.points}
										unit={tempSymbol(tunit)}
										label="{s.name} · temperature"
										digits={0}
										{tz}
									/>
								{/each}
								{#each r.series?.syncMs ?? [] as s (s.nodeId)}
									<LineChart points={s.points} unit="ms" label="{s.name} · sync error" digits={1} {tz} />
								{/each}
							</div>
						{/if}
					</section>

					{#if r.suspectPixels.length || r.limiter.length}
						<section class="card">
							<div class="card-head">
								<Lightbulb size={16} />
								<h2 class="grow">Lights</h2>
							</div>
							<div class="list">
								{#each r.suspectPixels as s (s.propId)}
									<a class="list-row interactive" href="/props">
										<div class="grow small">
											{s.name ?? s.propId}: suspect pixel{s.pixels.length === 1 ? '' : 's'}
											<span class="num"
												>{s.pixels
													.slice(0, 12)
													.map((p) => p + 1)
													.join(', ')}</span
											>
										</div>
									</a>
								{/each}
								{#each r.limiter as l (l.nodeId + l.port)}
									<div class="list-row">
										<div class="grow small">
											Brightness limiter on {r.nodes.find((n) => n.nodeId === l.nodeId)?.name ?? l.nodeId} port
											{l.port + 1}
										</div>
										<span class="faint tiny num">{Math.round(l.seconds)} s</span>
									</div>
								{/each}
							</div>
						</section>
					{/if}

					<section class="card">
						<div class="card-head">
							<HardDrive size={16} />
							<h2 class="grow">Housekeeping</h2>
						</div>
						<div class="card-body kv small">
							<div>
								<span class="faint">Storage</span><span
									>{r.diskFreePct != null ? `${Math.round(r.diskFreePct)} % free` : '—'}</span
								>
							</div>
							<div>
								<span class="faint">Newest backup</span><span
									>{r.backupAgeDays == null
										? 'none yet'
										: r.backupAgeDays === 0
											? 'today'
											: r.backupAgeDays === 1
												? '1 day old'
												: `${r.backupAgeDays} days old`}</span
								>
							</div>
							<div>
								<span class="faint">Visitor games</span><span
									>{r.games ?? 0}{r.gameMinutes ? ` (${Math.round(r.gameMinutes)} min)` : ''}</span
								>
							</div>
							<div><span class="faint">Triggers & sensors</span><span>{r.triggers ?? 0}</span></div>
							<div><span class="faint">Restarts</span><span>{r.restarts ?? 0}</span></div>
							{#each r.updates as u (u)}<div><span class="faint">Updated</span><span>{u}</span></div>{/each}
						</div>
					</section>
				{/if}
			</div>
		</div>
	{/if}
</div>

<Modal open={emailHtml !== null} title="Email preview" onclose={() => (emailHtml = null)} size="lg">
	{#if emailHtml}
		<iframe class="mail" title="Email preview" sandbox="" srcdoc={emailHtml}></iframe>
	{/if}
</Modal>

<style>
	.history {
		margin-bottom: 16px;
		display: flex;
		flex-direction: column;
		gap: 10px;
	}
	.history h2 {
		margin: 0;
		font-weight: 600;
	}
	.split {
		display: grid;
		grid-template-columns: 260px minmax(0, 1fr);
		gap: 16px;
		align-items: start;
	}
	.nights {
		position: sticky;
		top: 16px;
		max-height: calc(100vh - 120px);
		overflow: auto;
	}
	.night.active {
		background: var(--accent-soft);
	}
	.sicon {
		display: inline-flex;
		color: var(--green);
	}
	.sicon.warn {
		color: var(--accent-text);
	}
	.sicon.fail {
		color: var(--red);
	}
	.detail {
		display: flex;
		flex-direction: column;
		gap: 16px;
		min-width: 0;
	}
	.day {
		margin: 2px 0 0;
		font-size: 22px;
	}
	.headline {
		margin: 12px 0;
		font-size: 16px;
	}
	.tiles {
		display: grid;
		grid-template-columns: repeat(4, minmax(0, 1fr));
		gap: 8px;
	}
	.tile {
		display: flex;
		flex-direction: column;
		gap: 2px;
		padding: 10px 12px;
		border-radius: var(--r-2);
		background: var(--surface-2);
		color: var(--text-3);
	}
	.tile .v {
		font-size: 24px;
		font-weight: 650;
		color: var(--text);
	}
	.tile .l {
		font-size: 12px;
	}
	.acts {
		margin-top: 12px;
		gap: 8px;
		align-items: center;
	}
	.delivery {
		margin: 8px 0 0;
		padding-left: 18px;
	}
	.two {
		gap: 16px;
	}
	.table-wrap {
		overflow-x: auto;
	}
	td.bad {
		color: var(--red);
	}
	.charts {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(240px, 1fr));
		gap: 20px;
		padding: 16px;
		border-top: 1px solid var(--border);
	}
	.kv {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(200px, 1fr));
		gap: 10px 20px;
	}
	.kv > div {
		display: flex;
		justify-content: space-between;
		gap: 8px;
	}
	.mail {
		width: 100%;
		height: min(70vh, 720px);
		border: 0;
		border-radius: var(--r-2);
		background: #f4f5f7;
	}
	:global(.spin) {
		animation: rspin 0.9s linear infinite;
	}
	@keyframes rspin {
		to {
			transform: rotate(360deg);
		}
	}
	@media (max-width: 900px) {
		.split {
			grid-template-columns: 1fr;
		}
		.nights {
			position: static;
			max-height: 220px;
		}
		.tiles {
			grid-template-columns: repeat(2, minmax(0, 1fr));
		}
	}
</style>
