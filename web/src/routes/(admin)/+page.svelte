<script lang="ts">
	import GeometryBanner from '$lib/components/ui/GeometryBanner.svelte';
	import { api } from '$lib/api/client';
	import type { HealthReport, SensorHistory, SongRequest } from '$lib/api/types';
	import { app } from '$lib/stores/app.svelte';
	import { toasts } from '$lib/stores/toasts.svelte';
	import { playerAct } from '$lib/player';
	import { BOARDS, needsPort3Warning } from '$lib/util/boards';
	import { fmtCountdown, fmtDuration, plural } from '$lib/util/format';
	import { fmtTime, fmtDate } from '$lib/util/time';
	import { nextShow } from '$lib/util/schedule';
	import LayoutCanvas from '$lib/components/viz/LayoutCanvas.svelte';
	import Skeleton from '$lib/components/ui/Skeleton.svelte';
	import {
		Play,
		Pause,
		Square,
		Power,
		FlaskConical,
		CalendarClock,
		Thermometer,
		Zap,
		Gauge,
		Activity,
		CircleCheck,
		TriangleAlert,
		CircleAlert,
		RefreshCw,
		Cpu,
		ChevronRight,
		Music,
		Maximize2,
		Hand,
		X
	} from '@lucide/svelte';

	const show = $derived(app.show);
	const st = $derived(app.status);
	let health = $state<HealthReport | null>(null);
	let history = $state<SensorHistory | null>(null);
	let requests = $state<SongRequest[]>([]);
	let runningHealth = $state(false);
	let now = $state(Date.now());

	$effect(() => {
		api
			.health()
			.then((h) => (health = h))
			.catch(() => {});
		api
			.sensorHistory(60)
			.then((h) => (history = h))
			.catch(() => {});
		api
			.requests()
			.then((r) => (requests = r))
			.catch(() => {});
		const t = setInterval(() => {
			now = Date.now();
			api
				.requests()
				.then((r) => (requests = r))
				.catch(() => {});
		}, 15000);
		return () => clearInterval(t);
	});

	const greeting = $derived.by(() => {
		const h = new Date(now).getHours();
		return h < 5 ? 'Good evening' : h < 12 ? 'Good morning' : h < 18 ? 'Good afternoon' : 'Good evening';
	});

	const upcoming = $derived(show ? nextShow(show.schedule, new Date(now)) : undefined);
	const tz = $derived(show?.schedule.location.timezone);
	const pct = $derived(st?.durationMs ? (st.posMs / st.durationMs) * 100 : 0);

	async function runHealth() {
		runningHealth = true;
		try {
			health = await api.runHealth();
			toasts.success(health.ok ? 'All systems ready for showtime' : 'Health check found problems');
		} catch (e) {
			toasts.error('Health check failed', (e as Error).message);
		} finally {
			runningHealth = false;
		}
	}

	function playShow() {
		const id = upcoming?.playlistId ?? show?.playlists[0]?.id;
		playerAct(() => api.play({ playlistId: id }));
	}

	async function testAll() {
		await playerAct(() => api.testStart({ mode: 'rgbCycle', target: { all: true } }));
		toasts.push({
			kind: 'info',
			message: 'Testing every prop: red, green, blue',
			action: { label: 'Stop test', run: () => playerAct(api.testStop) }
		});
	}

	function spark(id: string): string {
		const s = history?.series[id];
		if (!s || s.length < 2) return '';
		const vs = s.map((p) => p[1]);
		const lo = Math.min(...vs),
			hi = Math.max(...vs);
		const r = hi - lo || 1;
		return s
			.map((p, i) => `${i ? 'L' : 'M'}${(i / (s.length - 1)) * 100} ${28 - ((p[1] - lo) / r) * 24}`)
			.join(' ');
	}

	const sensorIcon = { temperature: Thermometer, voltage: Zap, current: Activity, power: Gauge };
	const mainSensors = $derived(
		app.sensors.filter((s) => ['cpu', 'volts', 'amps', 'watts'].includes(s.id)).length
			? app.sensors.filter((s) => ['cpu', 'volts', 'amps', 'watts'].includes(s.id))
			: app.sensors.slice(0, 4)
	);
	const issues = $derived(health?.checks.filter((c) => c.status !== 'ok') ?? []);
	const warnLogs = $derived(app.logs.filter((l) => l.level === 'warn' || l.level === 'error').slice(0, 3));
	const totalPixels = $derived(show?.props.reduce((n, p) => n + p.pixelCount, 0) ?? 0);
</script>

<div class="page">
	<header class="hello">
		<div class="grow">
			<div class="eyebrow">
				{new Intl.DateTimeFormat(undefined, { weekday: 'long', month: 'long', day: 'numeric' }).format(now)}
			</div>
			<h1>{greeting}{show ? `, ${show.name}` : ''}</h1>
			{#if show}
				<p class="muted">
					{plural(show.props.length, 'prop')} · {totalPixels.toLocaleString()} pixels · {plural(
						show.nodes.length,
						'controller'
					)} · {plural(show.sequences.length, 'sequence')}
				</p>
			{/if}
		</div>
		<a class="btn" href="/layout"><Maximize2 size={16} /> Full layout</a>
	</header>

	<GeometryBanner />

	<section class="hero card">
		<div class="stage">
			{#if show}
				<LayoutCanvas props={show.props} height="100%" />
			{:else}
				<div class="skeleton" style="height:100%"></div>
			{/if}
			<div class="stage-badge">
				{#if st?.state === 'playing'}<span class="live"><span class="dot live"></span> LIVE</span>
				{:else if st?.state === 'paused'}<span class="paused">PAUSED</span>
				{:else if st?.state === 'testing'}<span class="testing">TESTING</span>
				{:else if st?.blackout}<span class="paused">BLACKOUT</span>
				{:else}<span class="idle">IDLE</span>{/if}
			</div>
		</div>
		<div class="np">
			<div class="eyebrow">Now playing</div>
			{#if st?.item}
				<h2 class="song ellipsis">{st.item.name}</h2>
				<p class="muted small ellipsis">
					{#if st.playlist}{st.playlist.name} · item {st.playlist.index + 1} of {st.playlist
							.count}{:else if st.state === 'effect'}Live effect{:else}Single item{/if}
				</p>
				{#if st.durationMs}
					<div class="progress big"><span style:width="{pct}%"></span></div>
					<div class="row between tiny faint num">
						<span>{fmtDuration(st.posMs)}</span><span>-{fmtDuration(st.durationMs - st.posMs)}</span>
					</div>
				{/if}
				{#if st.nextItem}
					<div class="next">
						<Music size={14} /> <span class="faint">Up next</span>
						<span class="ellipsis">{st.nextItem.name}</span>
					</div>
				{/if}
			{:else}
				<h2 class="song">The show is resting</h2>
				<p class="muted small">
					{#if upcoming}Starts automatically {fmtDate(new Date(upcoming.start), tz)} at {fmtTime(
							new Date(upcoming.start),
							tz
						)}.{:else}Nothing is scheduled. Press play to start any time.{/if}
				</p>
			{/if}
			<div class="actions">
				{#if st?.state === 'playing'}
					<button class="btn lg" onclick={() => playerAct(api.pause)}><Pause size={18} /> Pause</button>
				{:else if st?.state === 'paused'}
					<button class="btn primary lg" onclick={() => playerAct(api.resume)}
						><Play size={18} /> Resume</button
					>
				{:else}
					<button class="btn primary lg" onclick={playShow}
						><Play size={18} fill="currentColor" /> Play show now</button
					>
				{/if}
				<button
					class="btn lg"
					disabled={!st || st.state === 'idle'}
					onclick={() => playerAct(() => api.stop(true))}><Square size={16} /> Stop</button
				>
			</div>
			<div class="quick">
				<button
					class="qa"
					class:on={st?.blackout}
					onclick={() => playerAct(() => api.blackout(!st?.blackout))}
				>
					<Power size={16} />
					{st?.blackout ? 'Lights off' : 'Blackout'}
				</button>
				{#if st?.state === 'testing'}
					<button class="qa on" onclick={() => playerAct(api.testStop)}><X size={16} /> Stop test</button>
				{:else}
					<button class="qa" onclick={testAll}><FlaskConical size={16} /> Test all props</button>
				{/if}
			</div>
		</div>
	</section>

	<div class="grid grid-3 row2">
		<section class="card next-card">
			<div class="card-body">
				<div class="row">
					<span class="icon-tile accent"><CalendarClock size={20} /></span>
					<div class="grow">
						<div class="eyebrow">{st?.scheduleEntry ? 'Scheduled show' : 'Next show'}</div>
						{#if st?.scheduleEntry}
							<div class="big-num">Until {fmtTime(new Date(st.scheduleEntry.endsAt), tz)}</div>
							<div class="muted small">{st.scheduleEntry.name}</div>
						{:else if upcoming}
							<div class="big-num">in {fmtCountdown(new Date(upcoming.start).getTime() - now)}</div>
							<div class="muted small">
								{upcoming.name} · {fmtDate(new Date(upcoming.start), tz)}
								{fmtTime(new Date(upcoming.start), tz)}–{fmtTime(new Date(upcoming.end), tz)}
							</div>
						{:else if show}
							<div class="big-num">Not scheduled</div>
							<div class="muted small">Add show times on the schedule page.</div>
						{:else}
							<Skeleton h={22} w="60%" />
						{/if}
					</div>
				</div>
				<a class="link" href="/schedule">Open schedule <ChevronRight size={14} /></a>
			</div>
		</section>

		<section class="card health-card">
			<div class="card-body">
				<div class="row">
					<span
						class="icon-tile {health
							? health.ok && !issues.length
								? 'green'
								: issues.some((i) => i.status === 'fail')
									? 'red'
									: 'accent'
							: ''}"
					>
						{#if !health}<Activity size={20} />{:else if health.ok && !issues.length}<CircleCheck
								size={20}
							/>{:else}<TriangleAlert size={20} />{/if}
					</span>
					<div class="grow">
						<div class="eyebrow">Pre-show check</div>
						{#if health}
							<div class="big-num">
								{issues.length ? plural(issues.length, 'thing') + ' to look at' : 'Ready for showtime'}
							</div>
							<div class="muted small">
								{health.checks.length - issues.length} of {health.checks.length} checks passed
							</div>
						{:else}
							<Skeleton h={22} w="70%" />
						{/if}
					</div>
					<button
						class="btn ghost icon sm"
						onclick={runHealth}
						disabled={runningHealth}
						aria-label="Run health check"
						title="Run check now"
					>
						<span class:spin={runningHealth}><RefreshCw size={16} /></span>
					</button>
				</div>
				{#if issues.length}
					<ul class="issues">
						{#each issues.slice(0, 3) as c (c.id)}
							<li class={c.status}>
								{#if c.status === 'fail'}<CircleAlert size={14} />{:else}<TriangleAlert size={14} />{/if}
								<span><strong>{c.label}:</strong> {c.detail}</span>
							</li>
						{/each}
					</ul>
				{/if}
			</div>
		</section>

		<section class="card req-card">
			<div class="card-body">
				<div class="row">
					<span class="icon-tile purple"><Hand size={20} /></span>
					<div class="grow">
						<div class="eyebrow">Song requests</div>
						{#if show?.settings.requests.enabled}
							<div class="big-num">
								{requests.length ? plural(requests.length, 'request') + ' waiting' : 'No requests yet'}
							</div>
							<div class="muted small">
								{requests[0]
									? `Next: ${requests[0].name}${requests[0].requestedBy ? ` for ${requests[0].requestedBy}` : ''}`
									: 'Visitors can scan the QR code to pick a song'}
							</div>
						{:else}
							<div class="big-num">Off</div>
							<div class="muted small">Let visitors pick songs from their phones.</div>
						{/if}
					</div>
				</div>
				<a class="link" href="/settings#requests">Request settings <ChevronRight size={14} /></a>
			</div>
		</section>
	</div>

	<div class="section-title">
		<h2>Controllers</h2>
		<span class="grow"></span><a class="btn ghost sm" href="/controllers">Manage <ChevronRight size={14} /></a
		>
	</div>
	<div class="grid grid-3">
		{#if !show}
			{#each [0, 1] as i (i)}<div class="card card-pad"><Skeleton count={3} /></div>{/each}
		{:else}
			{#each show.nodes as n (n.id)}
				{@const live = app.nodes.find((x) => x.id === n.id)}
				{@const temp = app.sensors.find((s) => s.nodeId === n.id && s.kind === 'temperature')}
				<a class="card node interactive" href="/controllers#{n.id}">
					<div class="row">
						<span class="icon-tile {live?.online === false ? 'red' : 'green'}"><Cpu size={20} /></span>
						<div class="grow">
							<div class="row">
								<strong class="ellipsis">{n.name}</strong><span
									class="badge {n.role === 'leader' ? 'accent' : 'outline'}"
									>{n.role === 'leader' ? 'Leader' : 'Follower'}</span
								>
							</div>
							<div class="faint small ellipsis">
								{BOARDS[n.board].name}{n.boardRev ? ` · rev ${n.boardRev}` : ''}
							</div>
						</div>
					</div>
					<div class="node-stats">
						<div>
							<span class="faint tiny">Status</span>
							<span
								class="stat {live?.online === false ? 'bad' : live?.syncState === 'syncing' ? 'warn' : 'ok'}"
							>
								<span class="dot"></span>{live
									? live.online
										? live.syncState === 'syncing'
											? `Syncing ${live.files.total - live.files.pending}/${live.files.total}`
											: n.role === 'leader'
												? 'Running'
												: 'In sync'
										: 'Offline'
									: '—'}
							</span>
						</div>
						<div>
							<span class="faint tiny">Sync</span><span class="num"
								>{n.role === 'leader'
									? 'Clock source'
									: live
										? `±${live.syncOffsetMs.toFixed(1)} ms`
										: '—'}</span
							>
						</div>
						<div>
							<span class="faint tiny">Temp</span><span class="num"
								>{temp ? `${temp.value.toFixed(0)} °C` : '—'}</span
							>
						</div>
					</div>
					{#if needsPort3Warning(n)}
						<div class="mini-warn"><TriangleAlert size={13} /> Rev D: port 3 needs the 4/5-swapped lead</div>
					{/if}
				</a>
			{/each}
		{/if}
	</div>

	<div class="section-title">
		<h2>Power & temperature</h2>
		<span class="faint small">Main Controller · live</span>
	</div>
	<div class="grid grid-4">
		{#if !app.sensors.length}
			{#each [0, 1, 2, 3] as i (i)}<div class="card card-pad"><Skeleton count={2} /></div>{/each}
		{/if}
		{#each mainSensors as s (s.id)}
			{@const Icon = sensorIcon[s.kind]}
			{@const bad = s.crit != null && (s.kind === 'voltage' ? s.value <= s.crit : s.value >= s.crit)}
			{@const warn = s.warn != null && (s.kind === 'voltage' ? s.value <= s.warn : s.value >= s.warn)}
			<div class="card sensor" class:warn class:bad>
				<div class="row between">
					<span class="faint small">{s.label}</span><Icon size={16} class="sico" />
				</div>
				<div class="sv num">
					{s.value.toFixed(s.kind === 'voltage' || s.kind === 'current' ? 1 : 0)}<span class="unit"
						>{s.unit}</span
					>
				</div>
				<svg viewBox="0 0 100 30" preserveAspectRatio="none" class="spark" aria-hidden="true">
					<path
						d={spark(s.id)}
						fill="none"
						stroke="currentColor"
						stroke-width="1.5"
						vector-effect="non-scaling-stroke"
					/>
				</svg>
			</div>
		{/each}
	</div>

	{#if warnLogs.length}
		<div class="section-title"><h2>Recent alerts</h2></div>
		<div class="card list">
			{#each warnLogs as l (l.time + l.message)}
				<div class="list-row">
					<span
						class="icon-tile {l.level === 'error' ? 'red' : 'accent'}"
						style="width:32px;height:32px;border-radius:9px"><TriangleAlert size={16} /></span
					>
					<div class="grow small">{l.message}</div>
					<span class="faint tiny num">{fmtTime(new Date(l.time))}</span>
				</div>
			{/each}
		</div>
	{/if}
</div>

<style>
	.hello {
		display: flex;
		align-items: flex-end;
		gap: 16px;
		margin-bottom: 24px;
	}
	.hello h1 {
		font-size: 28px;
		margin: 4px 0 4px;
	}
	.hero {
		display: grid;
		grid-template-columns: minmax(0, 1.75fr) minmax(300px, 1fr);
		overflow: hidden;
		min-height: 340px;
	}
	.stage {
		position: relative;
		min-height: 340px;
		background: #060608;
	}
	.stage :global(canvas) {
		position: absolute;
		inset: 0;
		height: 100% !important;
	}
	.stage-badge {
		position: absolute;
		top: 14px;
		left: 14px;
		font-size: 11px;
		font-weight: 700;
		letter-spacing: 0.08em;
	}
	.stage-badge span {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		padding: 4px 9px;
		border-radius: 99px;
		background: rgba(0, 0, 0, 0.55);
		backdrop-filter: blur(8px);
		color: #ddd;
	}
	.stage-badge .live {
		color: #ff6b6b;
	}
	.stage-badge .testing {
		color: #7fd3ff;
	}
	.np {
		padding: 28px;
		display: flex;
		flex-direction: column;
		gap: 8px;
		border-left: 1px solid var(--border);
		background: linear-gradient(180deg, var(--surface), var(--surface-2));
	}
	.song {
		font-size: 22px;
		letter-spacing: -0.02em;
		margin-top: 4px;
	}
	.progress.big {
		height: 6px;
		margin-top: 14px;
	}
	.next {
		display: flex;
		align-items: center;
		gap: 6px;
		font-size: 13px;
		margin-top: 6px;
		color: var(--text-2);
		min-width: 0;
	}
	.actions {
		display: flex;
		gap: 8px;
		margin-top: auto;
		padding-top: 18px;
		flex-wrap: wrap;
	}
	.actions .btn {
		flex: 1;
	}
	.quick {
		display: flex;
		gap: 8px;
	}
	.qa {
		flex: 1;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		gap: 6px;
		height: 38px;
		border-radius: 10px;
		font-size: 12.5px;
		font-weight: 560;
		color: var(--text-2);
		background: var(--surface-3);
		transition: all 150ms var(--ease);
	}
	.qa:hover {
		color: var(--text);
	}
	.qa.on {
		background: var(--red);
		color: #fff;
	}
	.row2 {
		margin-top: 16px;
	}
	.big-num {
		font-size: 17px;
		font-weight: 620;
		letter-spacing: -0.015em;
		margin: 2px 0;
	}
	.link {
		display: inline-flex;
		align-items: center;
		gap: 2px;
		margin-top: 14px;
		font-size: 12.5px;
		font-weight: 560;
		color: var(--accent);
	}
	.issues {
		list-style: none;
		margin: 14px 0 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 6px;
	}
	.issues li {
		display: flex;
		gap: 8px;
		font-size: 12.5px;
		color: var(--text-2);
		line-height: 1.4;
	}
	.issues li :global(svg) {
		flex: 0 0 auto;
		margin-top: 2px;
		color: var(--accent);
	}
	.issues li.fail :global(svg) {
		color: var(--red);
	}
	.issues strong {
		color: var(--text);
		font-weight: 560;
	}
	.spin {
		display: flex;
		animation: spin 0.9s linear infinite;
	}
	@keyframes spin {
		to {
			transform: rotate(360deg);
		}
	}
	.node {
		display: flex;
		flex-direction: column;
		gap: 16px;
		padding: 18px;
	}
	.node-stats {
		display: grid;
		grid-template-columns: 1.3fr 1fr 0.7fr;
		gap: 8px;
		font-size: 13px;
	}
	.node-stats > div {
		display: flex;
		flex-direction: column;
		gap: 2px;
		min-width: 0;
	}
	.stat {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		white-space: nowrap;
	}
	.stat.ok .dot {
		color: var(--green);
		background: var(--green);
	}
	.stat.warn .dot {
		background: var(--accent);
	}
	.stat.bad .dot {
		background: var(--red);
	}
	.mini-warn {
		display: flex;
		align-items: center;
		gap: 6px;
		font-size: 12px;
		color: var(--accent);
		margin-top: -4px;
	}
	.sensor {
		padding: 16px 18px 12px;
		display: flex;
		flex-direction: column;
		gap: 4px;
		color: var(--text-3);
	}
	.sensor :global(.sico) {
		color: var(--text-3);
	}
	.sv {
		font-size: 28px;
		font-weight: 620;
		letter-spacing: -0.03em;
		color: var(--text);
	}
	.unit {
		font-size: 14px;
		font-weight: 500;
		color: var(--text-3);
		margin-left: 3px;
	}
	.spark {
		width: 100%;
		height: 30px;
		color: var(--accent);
		opacity: 0.8;
	}
	.sensor.warn .sv {
		color: var(--accent);
	}
	.sensor.bad .sv {
		color: var(--red);
	}
	@media (max-width: 1000px) {
		.hero {
			grid-template-columns: 1fr;
		}
		.stage {
			min-height: 240px;
		}
		.np {
			border-left: 0;
			border-top: 1px solid var(--border);
		}
	}
	@media (max-width: 760px) {
		.hello {
			margin-bottom: 16px;
		}
		.hello h1 {
			font-size: 22px;
		}
		.hello .btn {
			display: none;
		}
		.np {
			padding: 18px;
		}
		.stage {
			min-height: 200px;
		}
		.grid-4 {
			grid-template-columns: repeat(2, minmax(0, 1fr));
		}
		.sv {
			font-size: 24px;
		}
	}
</style>
