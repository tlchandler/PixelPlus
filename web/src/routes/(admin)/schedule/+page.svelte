<script lang="ts">
	import { api } from '$lib/api/client';
	import type { Schedule, ScheduleEntry, ScheduleOccurrence, Weekday } from '$lib/api/types';
	import { WEEKDAYS } from '$lib/api/types';
	import { app } from '$lib/stores/app.svelte';
	import { toasts, confirm } from '$lib/stores/toasts.svelte';
	import { expandSchedule, entrySummary, nextShow } from '$lib/util/schedule';
	import { describeTimeSpec, fmtDate, fmtTime, zonedParts, tzOffsetMs } from '$lib/util/time';
	import { fmtCountdown } from '$lib/util/format';
	import { searchCities, timezones } from '$lib/util/cities';
	import { newId } from '$lib/util/id';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import Segmented from '$lib/components/ui/Segmented.svelte';
	import Switch from '$lib/components/ui/Switch.svelte';
	import Modal from '$lib/components/ui/Modal.svelte';
	import Skeleton from '$lib/components/ui/Skeleton.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import TimeSpecPicker from '$lib/components/schedule/TimeSpecPicker.svelte';
	import {
		CalendarDays,
		List,
		Rows3,
		ChevronRight,
		Plus,
		Sunset,
		Star,
		Pencil,
		Trash2,
		MapPin,
		Moon,
		Volume1,
		CalendarClock,
		Sparkles,
		LocateFixed
	} from '@lucide/svelte';

	const show = $derived(app.show);
	const sched = $derived(show?.schedule);
	const tz = $derived(sched?.location.timezone ?? 'UTC');
	// Phones open on the day list: the week grid needs a wide screen.
	let view = $state<'days' | 'week' | 'list'>(
		typeof window !== 'undefined' && window.matchMedia?.('(max-width: 760px)').matches ? 'days' : 'week'
	);
	let preview = $state<ScheduleOccurrence[] | null>(null);
	let editing = $state<ScheduleEntry | null>(null);
	let locOpen = $state(false);
	let now = $state(new Date());

	$effect(() => {
		void show?.version;
		api
			.schedulePreview(14)
			.then((p) => (preview = p))
			.catch(() => (preview = []));
	});
	$effect(() => {
		const t = setInterval(() => (now = new Date()), 30000);
		return () => clearInterval(t);
	});

	async function save(next: Schedule, msg?: string) {
		const before = structuredClone($state.snapshot(sched) as Schedule);
		app.updateShow((s) => (s.schedule = next));
		try {
			await api.saveSchedule(next);
			await app.reloadShow();
			if (msg) toasts.success(msg, { label: 'Undo', run: () => save(before) });
		} catch (e) {
			toasts.error('Could not save schedule', (e as Error).message);
			app.reloadShow();
		}
	}
	function patch(fn: (s: Schedule) => void, msg?: string) {
		if (!sched) return;
		const s = structuredClone($state.snapshot(sched) as Schedule);
		fn(s);
		save(s, msg);
	}

	function newEntry() {
		editing = {
			id: newId(),
			name: 'Show night',
			enabled: true,
			playlistId: show?.playlists[0]?.id ?? '',
			days: [...WEEKDAYS],
			start: { kind: 'sunset', offsetMin: 15 },
			end: { kind: 'clock', time: '22:00' },
			// The Christmas season: roughly Thanksgiving through Twelfth Night.
			dateRange: { start: '11-26', end: '01-06' },
			priority: 0,
			endBehavior: 'finishSong'
		};
	}
	/** One-night specials: the date chips people reach for. */
	const specialDates = [
		{ label: 'Christmas Eve', md: '12-24' },
		{ label: 'Christmas', md: '12-25' },
		{ label: 'New Year’s Eve', md: '12-31' }
	];
	function setSpecial(on: boolean) {
		if (!editing) return;
		editing.priority = on ? Math.max(10, editing.priority) : 0;
		if (on) {
			// A special is usually one night: default to Christmas Eve, every weekday.
			const single = editing.dateRange && editing.dateRange.start === editing.dateRange.end;
			if (!single) editing.dateRange = { start: '12-24', end: '12-24' };
			editing.days = [...WEEKDAYS];
			if (editing.name === 'Show night') editing.name = 'Christmas Eve';
		} else if (editing.dateRange && editing.dateRange.start === editing.dateRange.end) {
			editing.dateRange = { start: '11-26', end: '01-06' };
		}
	}
	function setSingleDate(md: string) {
		if (!editing) return;
		editing.dateRange = { start: md, end: md };
		const chip = specialDates.find((d) => d.md === md);
		if (chip && (editing.name === 'Show night' || specialDates.some((d) => d.label === editing!.name)))
			editing.name = chip.label;
	}
	function saveEntry() {
		if (!editing || !sched) return;
		const e = $state.snapshot(editing) as ScheduleEntry;
		const exists = sched.entries.some((x) => x.id === e.id);
		patch(
			(s) => {
				if (exists) s.entries = s.entries.map((x) => (x.id === e.id ? e : x));
				else s.entries.push(e);
			},
			exists ? `Saved “${e.name}”` : `Added “${e.name}”`
		);
		editing = null;
	}
	async function deleteEntry(e: ScheduleEntry) {
		if (!(await confirm({ title: `Delete “${e.name}”?`, confirmLabel: 'Delete', danger: true }))) return;
		patch((s) => (s.entries = s.entries.filter((x) => x.id !== e.id)), `Deleted “${e.name}”`);
		editing = null;
	}

	// ---- week grid
	const week = $derived.by(() => {
		if (!sched) return [];
		const occ = expandSchedule(sched, now, 7);
		const start = zonedParts(now, tz);
		return Array.from({ length: 7 }, (_, i) => {
			const d = new Date(Date.UTC(start.y, start.m - 1, start.d + i, 12));
			const key = d.toISOString().slice(0, 10);
			return { key, date: d, items: occ.filter((o) => o.date === key) };
		});
	});
	function rawHour(iso: string) {
		const d = new Date(iso);
		const shifted = new Date(d.getTime() + tzOffsetMs(d, tz));
		return shifted.getUTCHours() + shifted.getUTCMinutes() / 60;
	}
	// Evening shows: a 2 pm → 2 am grid (after-midnight hours continue the night). A show
	// starting in the morning (e.g. Christmas morning) switches to a whole-day grid.
	const nightGrid = $derived(week.every((d) => d.items.every((o) => rawHour(o.start) >= 12)));
	// Start at 4 PM (or earlier if a show does) so evening blocks are big enough to read.
	const H0 = $derived(
		nightGrid ? Math.min(16, ...week.flatMap((d) => d.items.map((o) => Math.floor(rawHour(o.start))))) : 0
	);
	const H1 = $derived(nightGrid ? 26 : 24);
	function localHour(iso: string) {
		let h = rawHour(iso);
		if (nightGrid && h < 6) h += 24;
		return h;
	}
	function blockStyle(o: { start: string; end: string }) {
		const a = Math.max(H0, localHour(o.start));
		let b = localHour(o.end);
		if (b <= a) b = nightGrid ? b + 24 : H1;
		b = Math.min(H1, b);
		return `top:${((a - H0) / (H1 - H0)) * 100}%;height:${Math.max(4, ((b - a) / (H1 - H0)) * 100)}%`;
	}
	const colors = ['#f5a524', '#5b9dff', '#a88bfa', '#3fcf8e', '#f2555a', '#f5d547'];
	function entryColor(id: string) {
		const i = sched?.entries.findIndex((e) => e.id === id) ?? 0;
		return colors[Math.max(0, i) % colors.length];
	}
	const upcoming = $derived(sched ? nextShow(sched, now) : undefined);
	const nowPct = $derived.by(() => {
		const h = localHour(now.toISOString());
		return h >= H0 && h <= H1 ? ((h - H0) / (H1 - H0)) * 100 : null;
	});

	const previewByDay = $derived.by(() => {
		const m = new Map<string, ScheduleOccurrence[]>();
		for (const o of preview ?? []) m.set(o.date, [...(m.get(o.date) ?? []), o]);
		return [...m.entries()];
	});

	// ---- location
	let cityQ = $state('');
	const cityHits = $derived(searchCities(cityQ));
	let locDraft = $state({ lat: 0, lon: 0, timezone: 'UTC', label: '' });
	function openLoc() {
		if (!sched) return;
		locDraft = { ...sched.location, label: sched.location.label ?? '' };
		cityQ = '';
		locOpen = true;
	}
	function geolocate() {
		navigator.geolocation?.getCurrentPosition(
			(p) => {
				locDraft.lat = +p.coords.latitude.toFixed(4);
				locDraft.lon = +p.coords.longitude.toFixed(4);
				locDraft.timezone = Intl.DateTimeFormat().resolvedOptions().timeZone;
				locDraft.label = 'My location';
			},
			() => toasts.error('Couldn’t get your location', 'Search for a nearby city instead.')
		);
	}
	const tzList = timezones();

	const dayLabel: Record<Weekday, string> = {
		mon: 'Mon',
		tue: 'Tue',
		wed: 'Wed',
		thu: 'Thu',
		fri: 'Fri',
		sat: 'Sat',
		sun: 'Sun'
	};
	function setDays(d: Weekday[]) {
		if (editing) editing.days = d;
	}
	const months = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];
</script>

<div class="page">
	<PageHeader
		title="Schedule"
		subtitle="When the show runs. Times can follow sunset, so the show starts at dusk all season."
	>
		{#snippet actions()}
			{#if sched}
				<label class="master"
					><Switch
						checked={sched.enabled}
						label="Schedule on"
						onchange={(v) =>
							patch(
								(s) => (s.enabled = v),
								v ? 'Schedule turned on' : 'Schedule paused — nothing will start automatically'
							)}
					/> <span>{sched.enabled ? 'Schedule on' : 'Schedule paused'}</span></label
				>
			{/if}
			<button class="btn primary" onclick={newEntry} disabled={!show?.playlists.length}
				><Plus size={16} /> Add show time</button
			>
		{/snippet}
	</PageHeader>

	{#if !sched || !show}
		<div class="card card-pad"><Skeleton count={8} h={30} /></div>
	{:else}
		<div class="banner card" class:off={!sched.enabled}>
			<span class="icon-tile accent big"><CalendarClock size={24} /></span>
			<div class="grow">
				{#if !sched.enabled}
					<div class="eyebrow">Schedule paused</div>
					<h2>Shows won’t start on their own</h2>
					<p class="muted small">Turn the schedule back on, or start the show by hand from the player.</p>
				{:else if upcoming && new Date(upcoming.start) <= now}
					<div class="eyebrow">On now</div>
					<h2>{upcoming.name} · until {fmtTime(new Date(upcoming.end), tz)}</h2>
					<p class="muted small">
						Playing “{show.playlists.find((p) => p.id === upcoming.playlistId)?.name}”
					</p>
				{:else if upcoming}
					<div class="eyebrow">
						Next show · in {fmtCountdown(new Date(upcoming.start).getTime() - now.getTime())}
					</div>
					<h2>
						{fmtDate(new Date(upcoming.start), tz, { weekday: 'long', month: 'short', day: 'numeric' })} · {fmtTime(
							new Date(upcoming.start),
							tz
						)} – {fmtTime(new Date(upcoming.end), tz)}
					</h2>
					<p class="muted small">
						{upcoming.name} · “{show.playlists.find((p) => p.id === upcoming.playlistId)?.name}”
					</p>
				{:else}
					<div class="eyebrow">Nothing coming up</div>
					<h2>No show in the next 30 days</h2>
					<p class="muted small">Check the dates on your show times.</p>
				{/if}
			</div>
			<button class="loc" onclick={openLoc}
				><MapPin size={14} />
				{sched.location.label ?? `${sched.location.lat.toFixed(2)}, ${sched.location.lon.toFixed(2)}`}<span
					class="faint"
				>
					· {tz.replace(/_/g, ' ')}</span
				></button
			>
		</div>

		<div class="toolbar" style="margin-top:20px">
			<Segmented
				bind:value={view}
				label="View"
				options={[
					{ value: 'days', label: 'Days', icon: Rows3 },
					{ value: 'week', label: 'Week', icon: CalendarDays },
					{ value: 'list', label: 'Show times', icon: List }
				]}
			/>
		</div>

		<div class="cols">
			<div class="main">
				{#if !sched.entries.length}
					<div class="card">
						<EmptyState
							icon={Sunset}
							title="No show times yet"
							message={show.playlists.length
								? 'Add when your show should run — for example every night from 15 minutes after sunset until 10 PM.'
								: 'A show time plays one of your playlists. Build a playlist first, then come back to schedule it.'}
						>
							{#if show.playlists.length}
								<button class="btn primary" onclick={newEntry}><Plus size={16} /> Add show time</button>
							{:else}
								<a class="btn primary" href="/playlists"><Plus size={16} /> Build a playlist</a>
							{/if}
						</EmptyState>
					</div>
				{:else if view === 'days'}
					<div class="card days" aria-label="This week">
						{#each week as d, di (d.key)}
							{@const live = d.items.filter((o) => !o.overridden)}
							{@const replaced = d.items.filter((o) => o.overridden)}
							{@const first = live[0]}
							{@const e = first ? sched.entries.find((x) => x.id === first.entryId) : undefined}
							<button
								class="drow"
								class:today={di === 0}
								class:none={!live.length}
								onclick={() =>
									e ? (editing = structuredClone($state.snapshot(e) as ScheduleEntry)) : newEntry()}
							>
								<span class="dd">
									<span class="ddw"
										>{di === 0
											? 'Today'
											: fmtDate(d.date, 'UTC', { weekday: 'short', month: undefined, day: undefined })}</span
									>
									<span class="ddn num">{d.date.getUTCDate()}</span>
								</span>
								<span class="grow dbody">
									{#if live.length}
										{#each live as o (o.entryId + o.start)}
											<span class="dline">
												<span class="edot" style:background={entryColor(o.entryId)}></span>
												<strong class="num"
													>{fmtTime(new Date(o.start), tz)} – {fmtTime(new Date(o.end), tz)}</strong
												>
												<span class="muted ellipsis">{o.name}</span>
											</span>
										{/each}
										{#if replaced.length}
											<span class="faint tiny"
												>{live[0].name} replaces {replaced.map((r) => r.name).join(', ')} tonight</span
											>
										{/if}
									{:else}
										<span class="faint">No show</span>
									{/if}
								</span>
								<ChevronRight size={16} class="faint" />
							</button>
						{/each}
					</div>
				{:else if view === 'week'}
					<div class="card weekcard">
						<div class="wk">
							<div class="hours">
								{#each Array(H1 - H0 + 1) as _, i (i)}
									{@const h = (H0 + i) % 24}
									{#if H1 - H0 <= 14 || i % 2 === 0}
										<span style:top="{(i / (H1 - H0)) * 100}%"
											>{h === 0 ? '12a' : h < 12 ? `${h}a` : h === 12 ? '12p' : `${h - 12}p`}</span
										>
									{/if}
								{/each}
							</div>
							{#each week as d, di (d.key)}
								<div class="day" class:today={di === 0}>
									<div class="dh">
										<span class="dw"
											>{fmtDate(d.date, 'UTC', { weekday: 'short', month: undefined, day: undefined })}</span
										><span class="dn num">{d.date.getUTCDate()}</span>
									</div>
									<div class="dcol">
										{#each Array(H1 - H0) as _, i (i)}<span class="hl" style:top="{(i / (H1 - H0)) * 100}%"
											></span>{/each}
										{#if di === 0 && nowPct != null}<span class="now" style:top="{nowPct}%"></span>{/if}
										{#each d.items as o (o.entryId + o.start)}
											{@const e = sched.entries.find((x) => x.id === o.entryId)}
											<button
												class="blk"
												class:over={o.overridden}
												style="{blockStyle(o)};--c:{entryColor(o.entryId)}"
												onclick={() => e && (editing = structuredClone($state.snapshot(e) as ScheduleEntry))}
												title="{o.name}: {fmtTime(new Date(o.start), tz)}–{fmtTime(new Date(o.end), tz)}"
											>
												{#if !o.overridden}
													<span class="bn ellipsis"
														>{#if (e?.priority ?? 0) > 0}<Star size={10} fill="currentColor" />{/if}
														{o.name}</span
													>
													<span class="bt num">{fmtTime(new Date(o.start), tz)}</span>
												{:else}
													<span class="sr-only">{o.name} (replaced)</span>
												{/if}
											</button>
										{/each}
									</div>
								</div>
							{/each}
						</div>
					</div>
					<p class="faint tiny" style="margin-top:8px">
						Dashed outlines are regular show times replaced by a special night.
					</p>
				{:else}
					<div class="card list">
						{#each [...sched.entries].sort((a, b) => b.priority - a.priority) as e (e.id)}
							{@const pl = show.playlists.find((p) => p.id === e.playlistId)}
							<div class="list-row entry" class:disabled={!e.enabled}>
								<span class="edot" style:background={entryColor(e.id)}></span>
								<button
									class="grow etext"
									onclick={() => (editing = structuredClone($state.snapshot(e) as ScheduleEntry))}
								>
									<div class="row wrap" style="gap:8px">
										<strong>{e.name}</strong>
										{#if e.priority > 0}<span class="badge accent"
												><Star size={11} fill="currentColor" /> Special night</span
											>{/if}
										{#if e.dateRange}<span class="badge outline"
												>{months[+e.dateRange.start.split('-')[0] - 1]}
												{+e.dateRange.start.split('-')[1]} – {months[+e.dateRange.end.split('-')[0] - 1]}
												{+e.dateRange.end.split('-')[1]}</span
											>{/if}
									</div>
									<div class="muted small">
										{entrySummary(e)} · {describeTimeSpec(e.start)} → {describeTimeSpec(e.end)} · {pl?.name ??
											'Missing playlist'}
									</div>
								</button>
								<Switch
									checked={e.enabled}
									label="Enable {e.name}"
									onchange={(v) =>
										patch((s) => {
											const t = s.entries.find((x) => x.id === e.id);
											if (t) t.enabled = v;
										})}
								/>
								<button
									class="btn ghost icon sm"
									onclick={() => (editing = structuredClone($state.snapshot(e) as ScheduleEntry))}
									aria-label="Edit {e.name}"><Pencil size={14} /></button
								>
							</div>
						{/each}
					</div>
				{/if}

				<div class="section-title"><h2>Next 14 days</h2></div>
				<div class="card">
					{#if !preview}
						<div class="card-body"><Skeleton count={5} /></div>
					{:else if !preview.length}
						<div class="card-body faint small">No shows in the next two weeks.</div>
					{:else}
						{#each previewByDay as [date, occ] (date)}
							{@const d = new Date(occ[0].start)}
							<div class="pday">
								<div class="pd">
									<span class="num"
										>{fmtDate(d, tz, { weekday: 'short', month: 'short', day: 'numeric' })}</span
									>
								</div>
								<div class="grow col" style="gap:4px">
									{#each occ as o (o.entryId + o.start)}
										<div class="po">
											<span class="edot" style:background={entryColor(o.entryId)}></span><span class="num"
												>{fmtTime(new Date(o.start), tz)} – {fmtTime(new Date(o.end), tz)}</span
											><span class="muted ellipsis"
												>{o.name} · {show.playlists.find((p) => p.id === o.playlistId)?.name}</span
											>
										</div>
									{/each}
								</div>
							</div>
						{/each}
					{/if}
				</div>
			</div>

			<aside class="side">
				<section class="card card-pad sidecard">
					<div class="row">
						<Moon size={16} />
						<h3>Idle look</h3>
					</div>
					<p class="faint small">Shown during show hours when nothing is playing.</p>
					<select
						class="select"
						value={sched.idleEffectId ?? ''}
						onchange={(e) =>
							patch(
								(s) => (s.idleEffectId = (e.target as HTMLSelectElement).value || undefined),
								'Idle look updated'
							)}
						aria-label="Idle look"
					>
						<option value="">Lights off</option>
						{#each show.effects as fx (fx.id)}<option value={fx.id}>{fx.name}</option>{/each}
					</select>
					<p class="faint small" style="margin-top:6px">Outside show hours</p>
					<select
						class="select"
						value={sched.offEffectId ?? ''}
						onchange={(e) =>
							patch((s) => (s.offEffectId = (e.target as HTMLSelectElement).value || undefined), 'Updated')}
						aria-label="Look outside show hours"
					>
						<option value="">Dark</option>
						{#each show.effects as fx (fx.id)}<option value={fx.id}>{fx.name}</option>{/each}
					</select>
				</section>
				<section class="card card-pad sidecard">
					<div class="row">
						<Volume1 size={16} />
						<h3 class="grow">Volume curfew</h3>
						<Switch
							size="sm"
							checked={!!sched.volumeCurfew}
							label="Volume curfew"
							onchange={(v) =>
								patch(
									(s) =>
										(s.volumeCurfew = v ? { time: { kind: 'clock', time: '21:00' }, volume: 40 } : undefined),
									v ? 'Volume curfew on' : 'Volume curfew off'
								)}
						/>
					</div>
					<p class="faint small">Be a good neighbor: lower the volume late in the evening.</p>
					{#if sched.volumeCurfew}
						{@const vc = sched.volumeCurfew}
						<label class="field"
							><span class="label">From</span><input
								class="input"
								type="time"
								value={vc.time.kind === 'clock' ? vc.time.time : '21:00'}
								onchange={(e) =>
									patch(
										(s) =>
											s.volumeCurfew &&
											(s.volumeCurfew.time = { kind: 'clock', time: (e.target as HTMLInputElement).value }),
										'Curfew updated'
									)}
							/></label
						>
						<label class="field"
							><span class="label">Volume · {vc.volume}%</span>
							<input
								type="range"
								class="range"
								min="0"
								max="100"
								step="5"
								value={vc.volume}
								style:--pct="{vc.volume}%"
								onchange={(e) =>
									patch(
										(s) =>
											s.volumeCurfew &&
											(s.volumeCurfew.volume = Number((e.target as HTMLInputElement).value)),
										'Curfew updated'
									)}
							/>
						</label>
					{/if}
				</section>
				<section class="card card-pad sidecard">
					<div class="row">
						<Sparkles size={16} />
						<h3>Tips</h3>
					</div>
					<ul class="tips">
						<li>
							Use a <strong>special night</strong> (higher priority) for Christmas Eve — it wins over your regular
							nights.
						</li>
						<li>Date ranges can wrap the new year, e.g. Nov 25 – Jan 6.</li>
						<li>“Finish the song” lets the current song end before stopping.</li>
					</ul>
				</section>
			</aside>
		</div>
	{/if}
</div>

<Modal
	open={!!editing}
	title={sched?.entries.some((e) => e.id === editing?.id) ? 'Edit show time' : 'New show time'}
	size="lg"
	onclose={() => (editing = null)}
>
	{#if editing && sched && show}
		<div class="form-grid">
			<label class="field"
				><span class="label">Name</span><input class="input" bind:value={editing.name} /></label
			>
			<label class="field"
				><span class="label">Playlist</span>
				<select class="select" bind:value={editing.playlistId}
					>{#each show.playlists as p (p.id)}<option value={p.id}>{p.name}</option>{/each}</select
				>
			</label>
			<div class="field span-2">
				<span class="label">Kind of night</span>
				<Segmented
					value={editing.priority > 0 ? 'special' : 'regular'}
					label="Priority"
					onchange={(v) => setSpecial(v === 'special')}
					options={[
						{ value: 'regular', label: 'Regular' },
						{ value: 'special', label: 'Special night', icon: Star }
					]}
				/>
				<span class="hint"
					>A special night (like Christmas Eve) takes over from your regular show that evening.</span
				>
			</div>
			<div class="field span-2">
				<span class="label">Days</span>
				<div class="row wrap">
					{#each WEEKDAYS as d (d)}
						<button
							type="button"
							class="chip daychip"
							aria-pressed={editing.days.includes(d)}
							onclick={() =>
								editing &&
								(editing.days = editing.days.includes(d)
									? editing.days.filter((x) => x !== d)
									: [...editing.days, d])}>{dayLabel[d]}</button
						>
					{/each}
					<span class="sep"></span>
					<button type="button" class="btn ghost sm" onclick={() => setDays([...WEEKDAYS])}>Every day</button>
					<button type="button" class="btn ghost sm" onclick={() => setDays(['fri', 'sat'])}>Fri & Sat</button
					>
					<button
						type="button"
						class="btn ghost sm"
						onclick={() => setDays(['sun', 'mon', 'tue', 'wed', 'thu'])}>School nights</button
					>
				</div>
			</div>
			<div class="field">
				<span class="label">Starts</span><TimeSpecPicker
					bind:value={editing.start}
					location={sched.location}
					label="Start"
				/>
			</div>
			<div class="field">
				<span class="label">Ends</span><TimeSpecPicker
					bind:value={editing.end}
					location={sched.location}
					label="End"
				/>
			</div>
			{#if editing.priority > 0}
				<div class="field span-2">
					<span class="label">Which night?</span>
					<div class="row wrap">
						{#each specialDates as sd (sd.md)}
							<button
								type="button"
								class="chip"
								aria-pressed={editing.dateRange?.start === sd.md && editing.dateRange?.end === sd.md}
								onclick={() => setSingleDate(sd.md)}>{sd.label}</button
							>
						{/each}
						<span class="sep"></span>
						<select
							class="select sm month"
							value={(editing.dateRange?.start ?? '12-24').split('-')[0]}
							onchange={(e) =>
								setSingleDate(
									`${(e.target as HTMLSelectElement).value}-${(editing?.dateRange?.start ?? '12-24').split('-')[1]}`
								)}
							aria-label="Month"
							>{#each months as m, i (m)}<option value={String(i + 1).padStart(2, '0')}>{m}</option
								>{/each}</select
						>
						<input
							class="input sm dayn"
							type="number"
							min="1"
							max="31"
							value={+(editing.dateRange?.start ?? '12-24').split('-')[1]}
							onchange={(e) =>
								setSingleDate(
									`${(editing?.dateRange?.start ?? '12-24').split('-')[0]}-${String((e.target as HTMLInputElement).value).padStart(2, '0')}`
								)}
							aria-label="Day"
						/>
					</div>
					<span class="hint"
						>A special night replaces your regular show time for the whole evening, every year on this date.</span
					>
				</div>
			{:else}
				<div class="field span-2">
					<span class="label">Dates</span>
					<div class="row wrap">
						<Switch
							checked={!!editing.dateRange}
							label="Limit to dates"
							onchange={(v) =>
								editing && (editing.dateRange = v ? { start: '11-25', end: '01-06' } : undefined)}
						/>
						{#if editing.dateRange}
							{@const dr = editing.dateRange}
							<span class="small muted">From</span>
							<select
								class="select sm month"
								style="width:auto"
								value={dr.start.split('-')[0]}
								onchange={(e) =>
									(dr.start = `${(e.target as HTMLSelectElement).value}-${dr.start.split('-')[1]}`)}
								aria-label="Start month"
								>{#each months as m, i (m)}<option value={String(i + 1).padStart(2, '0')}>{m}</option
									>{/each}</select
							>
							<input
								class="input sm"
								style="width:64px"
								type="number"
								min="1"
								max="31"
								value={+dr.start.split('-')[1]}
								onchange={(e) =>
									(dr.start = `${dr.start.split('-')[0]}-${String((e.target as HTMLInputElement).value).padStart(2, '0')}`)}
								aria-label="Start day"
							/>
							<span class="small muted">to</span>
							<select
								class="select sm month"
								style="width:auto"
								value={dr.end.split('-')[0]}
								onchange={(e) =>
									(dr.end = `${(e.target as HTMLSelectElement).value}-${dr.end.split('-')[1]}`)}
								aria-label="End month"
								>{#each months as m, i (m)}<option value={String(i + 1).padStart(2, '0')}>{m}</option
									>{/each}</select
							>
							<input
								class="input sm"
								style="width:64px"
								type="number"
								min="1"
								max="31"
								value={+dr.end.split('-')[1]}
								onchange={(e) =>
									(dr.end = `${dr.end.split('-')[0]}-${String((e.target as HTMLInputElement).value).padStart(2, '0')}`)}
								aria-label="End day"
							/>
						{:else}
							<span class="small faint">Runs all year on the chosen days</span>
						{/if}
					</div>
				</div>
			{/if}
			<div class="field">
				<span class="label">When it ends</span>
				<select class="select" bind:value={editing.endBehavior}>
					<option value="finishSong">Finish the current song</option>
					<option value="fadeOut">Fade out</option>
					<option value="stopNow">Stop right away</option>
				</select>
			</div>
		</div>
	{/if}
	{#snippet footer()}
		{#if editing && sched?.entries.some((e) => e.id === editing?.id)}
			<button class="btn danger" onclick={() => editing && deleteEntry(editing)}
				><Trash2 size={14} /> Delete</button
			><span class="grow"></span>
		{/if}
		<button class="btn ghost" onclick={() => (editing = null)}>Cancel</button>
		<button class="btn primary" onclick={saveEntry} disabled={!editing?.days.length || !editing?.playlistId}
			>Save show time</button
		>
	{/snippet}
</Modal>

<Modal
	bind:open={locOpen}
	title="Show location"
	subtitle="Used to work out sunset and sunrise times"
	size="md"
>
	<div class="col" style="gap:14px">
		<div class="row">
			<div class="input-group grow">
				<span class="prefix"><MapPin size={16} /></span><input
					class="input"
					placeholder="Search for your town or city"
					bind:value={cityQ}
					aria-label="Search city"
				/>
			</div>
			<button class="btn" onclick={geolocate}><LocateFixed size={16} /> Use my location</button>
		</div>
		{#if cityHits.length}
			<div class="hits">
				{#each cityHits as c (c.name + c.region)}
					<button
						class="hit"
						onclick={() => {
							locDraft = {
								lat: c.lat,
								lon: c.lon,
								timezone: c.tz,
								label: `${c.name}, ${c.region.split(',')[0]}`
							};
							cityQ = '';
						}}
					>
						<strong>{c.name}</strong> <span class="faint small">{c.region}</span>
					</button>
				{/each}
			</div>
		{/if}
		<div class="form-grid">
			<label class="field"
				><span class="label">Latitude</span><input
					class="input"
					type="number"
					step="0.0001"
					bind:value={locDraft.lat}
				/></label
			>
			<label class="field"
				><span class="label">Longitude</span><input
					class="input"
					type="number"
					step="0.0001"
					bind:value={locDraft.lon}
				/></label
			>
			<label class="field"
				><span class="label">Time zone</span><select class="select" bind:value={locDraft.timezone}
					>{#each tzList.includes(locDraft.timezone) ? tzList : [locDraft.timezone, ...tzList] as z (z)}<option
							value={z}>{z.replace(/_/g, ' ')}</option
						>{/each}</select
				></label
			>
			<label class="field"
				><span class="label">Label</span><input class="input" bind:value={locDraft.label} /></label
			>
		</div>
	</div>
	{#snippet footer()}
		<button class="btn ghost" onclick={() => (locOpen = false)}>Cancel</button>
		<button
			class="btn primary"
			onclick={() => {
				patch(
					(s) => (s.location = { ...$state.snapshot(locDraft), label: locDraft.label || undefined }),
					'Location updated'
				);
				locOpen = false;
			}}>Save location</button
		>
	{/snippet}
</Modal>

<style>
	.master {
		display: flex;
		align-items: center;
		gap: 10px;
		font-size: 13px;
		font-weight: 550;
		padding: 0 8px;
		cursor: pointer;
	}
	.banner {
		display: flex;
		align-items: center;
		gap: 18px;
		padding: 20px 24px;
		background: linear-gradient(110deg, var(--accent-soft), transparent 55%), var(--surface);
		border-color: var(--accent-line);
		flex-wrap: wrap;
	}
	.banner.off {
		background: var(--surface);
		border-color: var(--border);
	}
	.icon-tile.big {
		width: 52px;
		height: 52px;
		border-radius: 14px;
	}
	.banner h2 {
		font-size: 19px;
		margin: 2px 0;
	}
	.loc {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		font-size: 12.5px;
		font-weight: 550;
		padding: 8px 12px;
		border-radius: 99px;
		background: var(--surface-2);
		border: 1px solid var(--border-2);
	}
	.loc:hover {
		border-color: var(--accent-line);
	}
	@media (pointer: coarse) {
		.loc {
			min-height: 44px;
		}
	}
	.cols {
		display: grid;
		grid-template-columns: minmax(0, 1fr) 300px;
		gap: 20px;
		align-items: start;
	}
	.weekcard {
		padding: 16px 16px 16px 8px;
		overflow-x: auto;
	}
	.wk {
		display: grid;
		grid-template-columns: 36px repeat(7, minmax(84px, 1fr));
		gap: 6px;
		min-width: 640px;
	}
	.hours {
		position: relative;
		margin-top: 48px;
		height: 440px;
	}
	.hours span {
		position: absolute;
		right: 4px;
		transform: translateY(-50%);
		font-size: 10.5px;
		color: var(--text-3);
	}
	.dh {
		height: 42px;
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		margin-bottom: 6px;
		border-radius: 10px;
	}
	.dw {
		font-size: 11px;
		color: var(--text-3);
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: 0.06em;
	}
	.dn {
		font-size: 16px;
		font-weight: 650;
	}
	.today .dh {
		background: var(--accent-soft);
		color: var(--accent-text);
	}
	.today .dw {
		color: var(--accent-text);
	}
	.dcol {
		position: relative;
		height: 440px;
		border-radius: 10px;
		background: var(--surface-2);
		overflow: hidden;
	}
	.hl {
		position: absolute;
		left: 0;
		right: 0;
		height: 1px;
		background: var(--border);
	}
	.now {
		position: absolute;
		left: 0;
		right: 0;
		height: 2px;
		background: var(--red);
		z-index: 2;
	}
	.now::before {
		content: '';
		position: absolute;
		left: -3px;
		top: -3px;
		width: 8px;
		height: 8px;
		border-radius: 50%;
		background: var(--red);
	}
	.blk {
		position: absolute;
		left: 4px;
		right: 4px;
		border-radius: 8px;
		padding: 6px 7px;
		background: color-mix(in srgb, var(--c) 22%, var(--surface));
		border-left: 3px solid var(--c);
		text-align: left;
		display: flex;
		flex-direction: column;
		gap: 1px;
		overflow: hidden;
		transition:
			transform 150ms var(--ease),
			box-shadow 150ms;
	}
	.blk:hover {
		transform: scale(1.02);
		box-shadow: var(--shadow-2);
		z-index: 3;
	}
	.blk.over {
		opacity: 0.4;
		background: transparent;
		border: 1px dashed var(--c);
		border-left-width: 3px;
	}
	.bn {
		font-size: 11.5px;
		font-weight: 620;
		display: flex;
		align-items: center;
		gap: 3px;
	}
	.bt {
		font-size: 10.5px;
		color: var(--text-2);
	}
	.entry.disabled {
		opacity: 0.5;
	}
	.etext {
		text-align: left;
		min-width: 0;
	}
	.edot {
		width: 10px;
		height: 10px;
		border-radius: 3px;
		flex: 0 0 auto;
	}
	.pday {
		display: flex;
		gap: 16px;
		padding: 12px 20px;
		border-bottom: 1px solid var(--border);
	}
	.pday:last-child {
		border-bottom: 0;
	}
	.pd {
		width: 110px;
		font-weight: 600;
		font-size: 13px;
		flex: 0 0 auto;
	}
	.po {
		display: flex;
		align-items: center;
		gap: 10px;
		font-size: 13px;
		min-width: 0;
	}
	.side {
		display: flex;
		flex-direction: column;
		gap: 14px;
	}
	.sidecard {
		display: flex;
		flex-direction: column;
		gap: 8px;
		padding: 18px;
	}
	.sidecard h3 {
		font-size: 14px;
	}
	.tips {
		margin: 0;
		padding-left: 18px;
		display: flex;
		flex-direction: column;
		gap: 6px;
		font-size: 12.5px;
		color: var(--text-2);
	}
	.tips strong {
		color: var(--text);
	}
	.daychip {
		min-width: 52px;
		justify-content: center;
	}
	.sep {
		width: 1px;
		height: 20px;
		background: var(--border-2);
	}
	.hits {
		display: flex;
		flex-direction: column;
		border: 1px solid var(--border);
		border-radius: 12px;
		overflow: hidden;
	}
	.hit {
		text-align: left;
		padding: 10px 14px;
		border-bottom: 1px solid var(--border);
	}
	.hit:last-child {
		border-bottom: 0;
	}
	.hit:hover {
		background: var(--accent-soft);
	}
	.days {
		display: flex;
		flex-direction: column;
		overflow: hidden;
	}
	.drow {
		display: flex;
		align-items: center;
		gap: 14px;
		min-height: 60px;
		padding: 10px 16px;
		border-bottom: 1px solid var(--border);
		text-align: left;
		transition: background var(--dur) var(--ease);
	}
	.drow:last-child {
		border-bottom: 0;
	}
	.drow:hover {
		background: var(--surface-hover);
	}
	.dd {
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		width: 48px;
		height: 44px;
		border-radius: 10px;
		background: var(--surface-2);
		flex: 0 0 auto;
	}
	.drow.today .dd {
		background: var(--accent-soft);
		color: var(--accent-text);
	}
	.ddw {
		font-size: 10.5px;
		font-weight: 650;
		letter-spacing: 0.05em;
		text-transform: uppercase;
		color: var(--text-3);
	}
	.drow.today .ddw {
		color: var(--accent-text);
	}
	.ddn {
		font-size: 16px;
		font-weight: 650;
		line-height: 1.1;
	}
	.dbody {
		display: flex;
		flex-direction: column;
		gap: 3px;
		min-width: 0;
	}
	.dline {
		display: flex;
		align-items: center;
		gap: 8px;
		font-size: 13.5px;
		min-width: 0;
	}
	.dline strong {
		font-weight: 600;
		white-space: nowrap;
	}
	.drow.none .dd {
		opacity: 0.7;
	}
	.month {
		width: auto;
		min-width: 86px;
	}
	.dayn {
		width: 72px;
	}
	@media (max-width: 1100px) {
		.cols {
			grid-template-columns: minmax(0, 1fr);
		}
	}
	@media (max-width: 760px) {
		.banner {
			padding: 16px;
			gap: 12px;
		}
		.banner .icon-tile.big {
			display: none;
		}
		.banner h2 {
			font-size: 17px;
		}
		.dline {
			flex-wrap: wrap;
			row-gap: 0;
		}
		.dline .muted {
			width: 100%;
			padding-left: 18px;
			font-size: 12.5px;
		}
		.drow {
			padding: 10px 12px;
			gap: 12px;
		}
		.pd {
			width: 84px;
		}
		.pday {
			padding: 12px 16px;
		}
	}
</style>
