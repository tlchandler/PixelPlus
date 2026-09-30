<script lang="ts">
	import { untrack } from 'svelte';
	import { api } from '$lib/api/client';
	import type {
		NetworkConfig,
		NetwatchStatus,
		SshState,
		ShowSettings,
		Snapshot,
		SongRequest,
		Trigger,
		UpdateInfo,
		WifiNetwork
	} from '$lib/api/types';
	import { app } from '$lib/stores/app.svelte';
	import { theme, type ThemePref } from '$lib/stores/theme.svelte';
	import { toasts, confirm } from '$lib/stores/toasts.svelte';
	import { BOARDS } from '$lib/util/boards';
	import { fmtBytes, fmtRelative, fmtUptime } from '$lib/util/format';
	import { newId } from '$lib/util/id';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import Switch from '$lib/components/ui/Switch.svelte';
	import Segmented from '$lib/components/ui/Segmented.svelte';
	import Modal from '$lib/components/ui/Modal.svelte';
	import Skeleton from '$lib/components/ui/Skeleton.svelte';
	import SaveState from '$lib/components/ui/SaveState.svelte';
	import SignalBars from '$lib/components/ui/SignalBars.svelte';
	import SyncWizard from '$lib/components/ui/SyncWizard.svelte';
	import { requestLink, prettyUrl } from '$lib/util/visitors';
	import { parseLogs, dayLabel } from '$lib/util/logs';
	import { countryName, fmtTemp, tempUnitOf, tempValue, fToC } from '$lib/util/units';
	import QrCode from '$lib/components/viz/QrCode.svelte';
	import {
		Wifi,
		Volume2,
		AudioLines,
		Bell,
		House,
		Hand,
		Zap,
		ShieldCheck,
		History,
		Download,
		Cpu,
		ScrollText,
		RefreshCw,
		Lock,
		Send,
		Plus,
		Trash2,
		RotateCcw,
		Upload,
		Power,
		Camera,
		Check,
		Copy,
		Sun,
		Moon,
		Monitor,
		Radio,
		Terminal,
		TriangleAlert,
		X,
		WifiOff,
		SlidersHorizontal,
		ChevronRight,
		ChevronLeft,
		Printer,
		Globe,
		Info
	} from '@lucide/svelte';

	type Sec =
		| 'general'
		| 'network'
		| 'audio'
		| 'alerts'
		| 'mqtt'
		| 'requests'
		| 'triggers'
		| 'security'
		| 'snapshots'
		| 'updates'
		| 'hardware'
		| 'logs';
	/** `device`: settings for this controller only (the rest apply to the whole show). */
	const sections: { id: Sec; label: string; icon: typeof Wifi; desc: string; device?: boolean }[] = [
		{ id: 'general', label: 'General', icon: SlidersHorizontal, desc: 'Units and appearance' },
		{
			id: 'network',
			label: 'Network & Wi-Fi',
			icon: Wifi,
			desc: 'Wi-Fi and the controller’s name',
			device: true
		},
		{
			id: 'audio',
			label: 'Audio',
			icon: Volume2,
			desc: 'Speakers, volume, leveling, lights-to-sound timing'
		},
		{ id: 'alerts', label: 'Alerts', icon: Bell, desc: 'A message when something needs you' },
		{ id: 'mqtt', label: 'Home Assistant', icon: House, desc: 'Control the show from your smart home' },
		{ id: 'requests', label: 'Song requests', icon: Hand, desc: 'Visitors pick songs · radio · yard sign' },
		{ id: 'triggers', label: 'Triggers', icon: Zap, desc: 'Start things with a button or a link' },
		{ id: 'security', label: 'Security', icon: ShieldCheck, desc: 'Password and remote access' },
		{ id: 'snapshots', label: 'Backups', icon: History, desc: 'Go back to any earlier version' },
		{ id: 'updates', label: 'Updates', icon: Download, desc: 'New versions of PixelPlus', device: true },
		{ id: 'hardware', label: 'Hardware & about', icon: Cpu, desc: 'Board, restart, shut down', device: true },
		{ id: 'logs', label: 'Logs', icon: ScrollText, desc: 'What happened, for troubleshooting', device: true }
	];

	let sec = $state<Sec>('general');
	/** Phones show the section list first and drill into one section (with a back button). */
	let mobileOpen = $state(false);
	$effect(() => {
		const h = location.hash.slice(1) as Sec;
		if (sections.some((s) => s.id === h)) {
			sec = h;
			mobileOpen = true;
		}
	});
	function go(s: Sec) {
		sec = s;
		mobileOpen = true;
		history.replaceState(history.state, '', `#${s}`);
		window.scrollTo({ top: 0 });
	}
	function backToList() {
		mobileOpen = false;
		history.replaceState(history.state, '', location.pathname);
	}
	const secInfo = $derived(sections.find((x) => x.id === sec)!);

	const show = $derived(app.show);
	let s = $state<ShowSettings | null>(null);
	let dirtyKeys = new Set<keyof ShowSettings>();
	let timer: ReturnType<typeof setTimeout>;
	let saved = $state(true);
	$effect(() => {
		const src = show?.settings;
		if (src)
			untrack(() => {
				if (saved || !s) s = structuredClone($state.snapshot(src) as ShowSettings);
			});
	});
	function changed(key: keyof ShowSettings) {
		dirtyKeys.add(key);
		saved = false;
		clearTimeout(timer);
		timer = setTimeout(flush, 600);
	}
	async function flush() {
		if (!s) return;
		const patch: Record<string, unknown> = {};
		for (const k of dirtyKeys) patch[k] = $state.snapshot(s[k]);
		dirtyKeys = new Set();
		try {
			await api.saveSettings(patch);
			await app.reloadShow();
			saved = true;
		} catch (e) {
			toasts.error('Could not save settings', (e as Error).message);
		}
	}

	// ---- network
	let net = $state<NetworkConfig | null>(null);
	let netOrig = $state('');
	/** The Wi-Fi network the controller is set to join (before any edits here). */
	const savedSsid = $derived(netOrig ? ((JSON.parse(netOrig) as NetworkConfig).wifi?.ssid ?? '') : '');
	/** Picked a different network than the saved one: needs its password, then Apply. */
	const switching = $derived(!!net && !!net.wifi.ssid && net.wifi.ssid !== savedSsid);
	let scan = $state<WifiNetwork[] | null>(null);
	let scanning = $state(false);
	let psk = $state('');
	const pickedSecure = $derived(scan?.find((n) => n.ssid === net?.wifi.ssid)?.secure ?? true);
	$effect(() => {
		if (sec === 'network' && !net)
			api
				.network()
				.then((n) => {
					net = n;
					netOrig = JSON.stringify(n);
				})
				.catch(() => {});
	});
	async function doScan() {
		scanning = true;
		scan = await api.scanWifi().catch(() => []);
		scanning = false;
	}
	async function saveNet() {
		if (!net) return;
		if (
			!(await confirm({
				title: 'Apply network changes?',
				message:
					'The controller may drop off the network for a moment. If the new settings don’t work it falls back to its own Wi-Fi hotspot so you can fix them.',
				confirmLabel: 'Apply'
			}))
		)
			return;
		try {
			const n = $state.snapshot(net) as NetworkConfig;
			if (psk) n.wifi.psk = psk;
			await api.saveNetwork(n);
			netOrig = JSON.stringify(net);
			psk = '';
			toasts.success('Network settings applied');
		} catch (e) {
			toasts.error('Could not apply network settings', (e as Error).message);
		}
	}
	const netDirty = $derived(!!net && (JSON.stringify(net) !== netOrig || !!psk));
	const nw = $derived<NetwatchStatus | null>(net?.netwatch ?? null);
	// Refresh the hotspot status while this section is open (it changes on its own).
	$effect(() => {
		if (sec !== 'network') return;
		const t = setInterval(async () => {
			const n = await api.network().catch(() => null);
			if (n && net) net.netwatch = n.netwatch ?? null;
		}, 10_000);
		return () => clearInterval(t);
	});
	function ago(unix?: number | null) {
		return unix ? fmtRelative(new Date(unix * 1000).toISOString()) : '';
	}

	// ---- audio
	let devices = $state<{ id: string; name: string }[]>([]);
	$effect(() => {
		if (sec === 'audio' && !devices.length)
			api
				.audioDevices()
				.then((d) => (devices = d))
				.catch(() => {});
	});

	// ---- lights-to-sound timing
	let syncOpen = $state(false);
	async function saveDelay(ms: number) {
		if (!s) return;
		s.audio.outputDelayMs = ms;
		try {
			await api.saveSettings({ audio: { outputDelayMs: ms } });
			await app.reloadShow();
		} catch (e) {
			toasts.error('Could not save the sound delay', (e as Error).message);
		}
	}
	const fmtDelay = (v: number) => (v === 0 ? 'none' : `${v > 0 ? '+' : '−'}${Math.abs(v)} ms`);

	// ---- alerts / mqtt tests
	let testing = $state<string | null>(null);
	async function test(kind: 'email' | 'ntfy' | 'mqtt') {
		if (!saved) await flush();
		testing = kind;
		try {
			const r = kind === 'mqtt' ? await api.testMqtt() : await api.testAlert(kind);
			if (r.ok) toasts.success(r.message);
			else toasts.warn(r.message);
		} catch (e) {
			toasts.error('Test failed', (e as Error).message);
		} finally {
			testing = null;
		}
	}

	// ---- requests
	let queue = $state<SongRequest[]>([]);
	$effect(() => {
		if (sec === 'requests')
			api
				.requests()
				.then((q) => (queue = q))
				.catch(() => {});
	});
	const reqLink = $derived(requestLink(show, location.origin));
	const requestUrl = $derived(reqLink.url);

	// ---- security
	let pwOpen = $state(false);
	let pwCur = $state('');
	let pwNew = $state('');
	let pwNew2 = $state('');
	async function setPassword(remove = false) {
		try {
			await api.setPassword(pwCur || undefined, remove ? null : pwNew);
			toasts.success(remove ? 'Password removed' : 'Password saved');
			pwOpen = false;
			pwCur = pwNew = pwNew2 = '';
			await app.loadSystem();
		} catch (e) {
			toasts.error('Could not change the password', (e as Error).message);
		}
	}

	// ---- snapshots
	let snaps = $state<Snapshot[] | null>(null);
	let snapLabel = $state('');
	let importInput: HTMLInputElement | undefined = $state();
	async function loadSnaps() {
		snaps = await api.snapshots().catch(() => []);
	}
	$effect(() => {
		if (sec === 'snapshots' && !snaps) loadSnaps();
	});
	async function takeSnap() {
		const sn = await api
			.createSnapshot(snapLabel.trim() || 'Backup')
			.catch((e) => toasts.error('Backup failed', e.message));
		if (sn) {
			toasts.success('Backup saved');
			snapLabel = '';
			loadSnaps();
		}
	}
	async function restore(sn: Snapshot) {
		if (
			!(await confirm({
				title: `Restore “${sn.label}”?`,
				message: `Your show goes back to how it was ${fmtRelative(sn.createdAt)}. Your current show is backed up first, so you can undo this.`,
				confirmLabel: 'Restore'
			}))
		)
			return;
		await api.createSnapshot('Before restore').catch(() => {});
		await app.mutate(() => api.restoreSnapshot(sn.id));
		loadSnaps();
	}
	async function delSnap(sn: Snapshot) {
		if (!(await confirm({ title: `Delete “${sn.label}”?`, confirmLabel: 'Delete', danger: true }))) return;
		await api.deleteSnapshot(sn.id).catch(() => {});
		loadSnaps();
	}
	async function importSnap(e: Event) {
		const f = (e.target as HTMLInputElement).files?.[0];
		(e.target as HTMLInputElement).value = '';
		if (!f) return;
		await api
			.importSnapshot(f)
			.then(() => toasts.success('Backup imported — restore it from the list'))
			.catch((err) => toasts.error('Import failed', err.message));
		loadSnaps();
	}

	// ---- updates
	let upd = $state<UpdateInfo | null>(null);
	let updating = $state(false);
	$effect(() => {
		if (sec === 'updates' && !upd)
			api
				.checkUpdate()
				.then((u) => (upd = u))
				.catch(() => {});
	});
	async function applyUpdate() {
		if (
			!(await confirm({
				title: `Update to ${upd?.latest}?`,
				message:
					'The show stops for about a minute while PixelPlus restarts. Followers update automatically afterwards.',
				confirmLabel: 'Update now'
			}))
		)
			return;
		updating = true;
		await api
			.applyUpdate()
			.catch((e) => toasts.error('Update failed', e.message))
			.finally(() => (updating = false));
	}
	const updJob = $derived(app.helpers['update'] ?? upd?.job ?? null);

	// ---- ssh
	let ssh = $state<SshState | null>(null);
	$effect(() => {
		if (sec === 'security' && !ssh)
			api
				.ssh()
				.then((v) => (ssh = v))
				.catch(() => {});
	});
	const sshJob = $derived(
		[app.helpers['ssh-on'], app.helpers['ssh-off']]
			.filter((j) => !!j)
			.sort((a, b) => b.updatedAt - a.updatedAt)[0]
	);
	let sshSeen = '';
	$effect(() => {
		const key = sshJob ? `${sshJob.verb}:${sshJob.state}:${sshJob.updatedAt}` : '';
		if (key && key !== sshSeen && sshJob.state !== 'running') {
			sshSeen = key;
			api
				.ssh()
				.then((v) => (ssh = v))
				.catch(() => {});
		}
	});
	async function setSsh(on: boolean) {
		if (
			on &&
			!(await confirm({
				title: 'Turn SSH on?',
				message:
					'SSH lets someone with the controller’s login password (or key) run commands on it. Only turn it on if you need it.',
				confirmLabel: 'Turn on'
			}))
		)
			return;
		try {
			await api.setSsh(on);
		} catch (e) {
			toasts.error('Could not change SSH', (e as Error).message);
		}
	}

	// ---- system
	async function power(kind: 'reboot' | 'shutdown' | 'restart') {
		const label = kind === 'reboot' ? 'Reboot' : kind === 'shutdown' ? 'Shut down' : 'Restart PixelPlus';
		if (
			!(await confirm({
				title: `${label}?`,
				message:
					kind === 'shutdown'
						? 'You’ll need to unplug and replug the power to start it again.'
						: 'The show stops for about a minute.',
				confirmLabel: label,
				danger: kind === 'shutdown'
			}))
		)
			return;
		await (
			kind === 'reboot' ? api.reboot() : kind === 'shutdown' ? api.shutdown() : api.restartService()
		).catch((e) => toasts.error(`${label} failed`, e.message));
	}

	// ---- logs
	let logs = $state<string | null>(null);
	let logFilter = $state<'all' | 'warn'>('all');
	async function loadLogs() {
		logs = await api.logs(500).catch(() => 'Could not load logs');
	}
	$effect(() => {
		if (sec === 'logs' && logs == null) loadLogs();
	});
	let logQ = $state('');
	const fmtClock = (d: Date) =>
		new Intl.DateTimeFormat(undefined, { hour: 'numeric', minute: '2-digit', second: '2-digit' }).format(d);
	const parsedLogs = $derived(parseLogs(logs ?? ''));
	/** Newest first, grouped by local day ("Today", "Yesterday", …). */
	const logGroups = $derived.by(() => {
		const needle = logQ.trim().toLowerCase();
		const rows = parsedLogs.filter(
			(l) =>
				(logFilter === 'all' || l.level === 'warn' || l.level === 'error') &&
				(!needle || l.message.toLowerCase().includes(needle))
		);
		const groups: { day: string; rows: typeof rows }[] = [];
		for (const r of rows) {
			const day = r.time ? dayLabel(r.time) : 'Earlier';
			if (groups[groups.length - 1]?.day !== day) groups.push({ day, rows: [] });
			groups[groups.length - 1].rows.push(r);
		}
		return groups;
	});
	const logProblems = $derived(parsedLogs.filter((l) => l.level === 'warn' || l.level === 'error').length);
	function copyLogs() {
		navigator.clipboard?.writeText(logs ?? '');
		toasts.success('Logs copied — paste them into your support message');
	}

	// ---- triggers
	function addTrigger() {
		if (!s) return;
		s.triggers = [
			...s.triggers,
			{
				id: newId(),
				name: 'New trigger',
				kind: 'gpio',
				gpio: 17,
				action: { type: 'playPlaylist', ref: show?.playlists[0]?.id }
			}
		];
		changed('triggers');
	}
	function removeTrigger(t: Trigger) {
		if (!s) return;
		s.triggers = s.triggers.filter((x) => x.id !== t.id);
		changed('triggers');
	}

	const sys = $derived(app.system);
	const tunit = $derived(s?.units?.temperature ?? tempUnitOf(show));
	/** The heat alert, shown in the chosen unit but always stored in °C. */
	const alertTemp = $derived(s ? Math.round(tempValue(s.alerts.rules.tempC, tunit)) : 0);
	function setAlertTemp(v: number) {
		if (!s || !Number.isFinite(v)) return;
		s.alerts.rules.tempC = tunit === 'f' ? Math.round(fToC(v) * 10) / 10 : v;
		changed('alerts');
	}
</script>

<div class="page">
	<div class="head" class:mobile-sr={mobileOpen}>
		<PageHeader
			title="Settings"
			subtitle="Changes save automatically and apply to the whole show, followers included — except those marked “This controller”."
		/>
	</div>

	<div class="layout">
		<nav class="snav" class:mobile-hidden={mobileOpen} aria-label="Settings sections">
			{#each sections as x (x.id)}
				<button
					class="si"
					class:on={sec === x.id}
					onclick={() => go(x.id)}
					aria-current={sec === x.id ? 'page' : undefined}
				>
					<span class="si-ic"><x.icon size={16} /></span>
					<span class="si-txt"
						><span class="si-label">{x.label}</span><span class="si-desc">{x.desc}</span></span
					>
					<ChevronRight size={16} class="si-chev" />
				</button>
			{/each}
		</nav>

		<div class="content" class:mobile-hidden={!mobileOpen}>
			<div class="secbar">
				<button class="btn ghost back" onclick={backToList}><ChevronLeft size={18} /> Settings</button>
				<span class="grow"></span>
				{#if sec === 'network' && netDirty}
					<span class="small notapplied">Not applied yet</span>
				{:else if !secInfo.device || sec === 'hardware'}
					<SaveState state={saved ? 'saved' : 'saving'} />
				{/if}
			</div>
			{#if secInfo.device}
				<div class="devnote">
					<Cpu size={14} /> This controller only · <strong>{sys?.name || sys?.hostname}</strong>
				</div>
			{/if}
			{#if !s || !show}
				<div class="card card-pad"><Skeleton count={8} h={28} /></div>
			{:else if sec === 'general'}
				<section class="card">
					<div class="card-head"><h2 class="grow">General</h2></div>
					<div class="card-body">
						<div class="setting stack">
							<div class="text">
								<div class="title">Temperature</div>
								<div class="desc">For controller temperatures and heat alerts, everywhere in PixelPlus.</div>
							</div>
							<div class="control">
								<Segmented
									value={tunit}
									label="Temperature unit"
									onchange={(v) => {
										if (s) {
											s.units = { temperature: v };
											changed('units');
										}
									}}
									options={[
										{ value: 'f', label: '°F Fahrenheit' },
										{ value: 'c', label: '°C Celsius' }
									]}
								/>
							</div>
						</div>
						<div class="setting stack">
							<div class="text">
								<div class="title">Appearance</div>
								<div class="desc">
									Just for this browser. Automatic follows your phone or computer (dark at night).
								</div>
							</div>
							<div class="control">
								<Segmented
									value={theme.preference}
									label="Appearance"
									onchange={(v) => theme.set(v as ThemePref)}
									options={[
										{ value: 'system', label: 'Automatic', icon: Monitor },
										{ value: 'dark', label: 'Dark', icon: Moon },
										{ value: 'light', label: 'Light', icon: Sun }
									]}
								/>
							</div>
						</div>
					</div>
				</section>
			{:else if sec === 'network'}
				<section class="card">
					<div class="card-head"><h2 class="grow">Network & Wi-Fi</h2></div>
					<div class="card-body">
						{#if !net}<Skeleton count={4} h={36} />{:else}
							{#if net.managed === false}
								<div class="nw-note muted small">
									{sys?.docker
										? 'PixelPlus runs in Docker here: Wi-Fi, the computer’s name and wired settings belong to the host. Change them there.'
										: 'This computer’s network isn’t managed by PixelPlus. Change it in the computer’s own settings.'}
								</div>
							{/if}
							{#if nw}
								<div class="nw {nw.state}">
									<span
										class="icon-tile {nw.state === 'hotspot'
											? 'accent'
											: nw.state === 'online'
												? 'green'
												: ''}"><Radio size={18} /></span
									>
									<div class="grow">
										{#if nw.state === 'hotspot'}
											<strong>Setup hotspot is on{nw.hotspotSsid ? `: ${nw.hotspotSsid}` : ''}</strong>
											<div class="small muted">
												{#if nw.hotspotSecured && nw.hotspotPassword}Password:
													<span class="mono">{nw.hotspotPassword}</span>.{:else if nw.hotspotSecured}Password
													protected.{:else}Open network, no password.{/if}
												Join it with a phone and open
												<span class="mono">{nw.portalUrl ?? 'http://10.42.0.1/'}</span> to pick a Wi-Fi network.
												It turns off by itself once the controller is back online.
											</div>
										{:else if nw.state === 'connecting'}
											<strong>Joining a Wi-Fi network…</strong>
											<div class="small muted">
												The setup hotspot is paused while the controller tries the network picked on the setup
												page.
											</div>
										{:else if nw.state === 'online'}
											<strong>Online</strong>
											<div class="small muted">
												{#if nw.lastJoined}Joined “{nw.lastJoined.ssid}” from the setup page {ago(
														nw.lastJoined.at
													)}{nw.lastJoined.ips?.length ? ` (${nw.lastJoined.ips.join(', ')})` : ''}.{:else}If
													the network is lost for 10 minutes, the setup hotspot turns on so you can fix it
													from a phone{nw.hotspotPassword ? ' (password ' : ''}{#if nw.hotspotPassword}<span
															class="mono">{nw.hotspotPassword}</span
														>){/if}.{/if}
											</div>
										{:else}
											<strong>Checking the network…</strong>
											<div class="small muted">
												No connection yet. The setup hotspot starts if none comes up within a minute or two.
											</div>
										{/if}
										{#if nw.lastError}
											<div class="small nw-err"><TriangleAlert size={13} /> {nw.lastError}</div>
										{/if}
									</div>
								</div>
							{/if}
							{#if net.managed === false}
								<!-- Not ours to change: show what the computer uses, read-only. -->
								<dl class="nw-facts">
									<dt>Name on the network</dt>
									<dd class="mono">{net.hostname || sys?.hostname || '—'}</dd>
									<dt>Addresses</dt>
									<dd class="mono">{sys?.ips?.length ? sys.ips.join(', ') : '—'}</dd>
									<dt>Wi-Fi</dt>
									<dd>
										{#if sys?.wifi?.ssid}{sys.wifi.ssid} · <SignalBars
												dbm={sys.wifi.signal}
												showLabel
											/>{:else}Not connected{/if}
									</dd>
								</dl>
							{:else}
								<div class="form-grid">
									<label class="field"
										><span class="label">Controller name on the network</span>
										<div class="input-group">
											<input class="input" bind:value={net.hostname} /><span class="suffix">.local</span>
										</div>
										<span class="hint">Open the app at http://{net.hostname}.local</span></label
									>
									<label class="field"
										><span class="label">Wi-Fi country</span><select
											class="select"
											bind:value={net.wifi.country}
											>{#each ['US', 'CA', 'GB', 'IE', 'AU', 'NZ', 'DE', 'FR', 'NL', 'SE', 'NO', 'MX'] as c (c)}<option
													value={c}>{countryName(c)}</option
												>{/each}</select
										></label
									>
								</div>
								<div class="wifi">
									<div class="row">
										<h3 class="grow">Wi-Fi network</h3>
										<button class="btn sm" onclick={doScan} disabled={scanning}
											><RefreshCw size={14} class={scanning ? 'spin' : ''} />
											{scanning ? 'Scanning…' : 'Scan'}</button
										>
									</div>
									<div class="current">
										{#if sys?.wifi?.ssid}
											<Wifi size={16} /> Connected to <strong>{sys.wifi.ssid}</strong>
											<SignalBars dbm={sys.wifi.signal} showLabel />
										{:else}
											<WifiOff size={16} class="off" /> Not connected to Wi-Fi — scan and pick a network.
										{/if}
									</div>
									{#if switching}
										<div class="switchto">
											Switch to <strong>{net.wifi.ssid}</strong> — {pickedSecure
												? 'type its password below, then'
												: 'then'} press <em>Apply network settings</em>.
											<button class="linkish" onclick={() => net && (net.wifi.ssid = savedSsid)}
												>Keep {savedSsid || 'the current network'}</button
											>
										</div>
									{/if}
									{#if scan}
										<div class="nets">
											{#each scan as n (n.ssid)}
												<button
													class="netrow"
													class:on={net.wifi.ssid === n.ssid}
													onclick={() => net && (net.wifi.ssid = n.ssid)}
												>
													<SignalBars dbm={n.signal} /><span class="grow">{n.ssid}</span
													>{#if n.ssid === sys?.wifi?.ssid}<span class="faint tiny">Connected</span
														>{/if}{#if n.secure}<Lock
															size={13}
															class="faint"
														/>{/if}{#if net.wifi.ssid === n.ssid}<Check size={15} />{/if}
												</button>
											{/each}
										</div>
									{/if}
									<label class="field" style="margin-top:12px"
										><span class="label"
											>{net.wifi.ssid ? `Password for ${net.wifi.ssid}` : 'Wi-Fi password'}</span
										><input
											class="input"
											type="password"
											placeholder={switching
												? pickedSecure
													? `Password for ${net.wifi.ssid} (required)`
													: 'This network has no password'
												: 'Leave empty to keep the current password'}
											disabled={switching && !pickedSecure}
											bind:value={psk}
											autocomplete="new-password"
										/></label
									>
								</div>
								<details class="adv">
									<summary>Wired network (Ethernet)</summary>
									<div class="form-grid" style="margin-top:12px">
										<div class="field span-2">
											<span class="label">Addressing</span><Segmented
												value={net.ethernet.dhcp ? 'dhcp' : 'static'}
												label="Addressing"
												onchange={(v) => net && (net.ethernet.dhcp = v === 'dhcp')}
												options={[
													{ value: 'dhcp', label: 'Automatic (DHCP)' },
													{ value: 'static', label: 'Fixed address' }
												]}
											/>
										</div>
										{#if !net.ethernet.dhcp}
											<label class="field"
												><span class="label">Address</span><input
													class="input mono"
													placeholder="192.168.1.40/24"
													bind:value={net.ethernet.address}
												/></label
											>
											<label class="field"
												><span class="label">Gateway</span><input
													class="input mono"
													placeholder="192.168.1.1"
													bind:value={net.ethernet.gateway}
												/></label
											>
											<label class="field"
												><span class="label">DNS</span><input
													class="input mono"
													placeholder="1.1.1.1"
													bind:value={net.ethernet.dns}
												/></label
											>
										{/if}
									</div>
								</details>
								<div class="row" style="margin-top:18px;justify-content:flex-end">
									<button
										class="btn primary"
										disabled={!netDirty || (switching && pickedSecure && !psk)}
										onclick={saveNet}>Apply network settings</button
									>
								</div>
							{/if}
						{/if}
					</div>
				</section>
			{:else if sec === 'audio'}
				<section class="card">
					<div class="card-head"><h2 class="grow">Audio</h2></div>
					<div class="card-body">
						<div class="setting stack">
							<div class="text">
								<div class="title">Output</div>
								<div class="desc">Where the show’s music plays (e.g. to your FM transmitter).</div>
							</div>
							<div class="control">
								<select
									class="select"
									style="width:260px"
									bind:value={s.audio.device}
									onchange={() => changed('audio')}
									>{#each devices.length ? devices : [{ id: s.audio.device, name: s.audio.device }] as d (d.id)}<option
											value={d.id}>{d.name}</option
										>{/each}</select
								>
							</div>
						</div>
						<div class="setting stack">
							<div class="text">
								<div class="title">Sync lights to sound</div>
								<div class="desc">
									When the audience hears the music late (FM radio, a TV or Bluetooth speaker, or just far
									away), delay the lights to match. Sound delay now: <strong
										>{fmtDelay(s.audio.outputDelayMs ?? 0)}</strong
									>.
								</div>
							</div>
							<div class="control">
								<button class="btn" onclick={() => (syncOpen = true)}
									><AudioLines size={16} /> Sync lights to sound…</button
								>
							</div>
						</div>
						<div class="setting">
							<div class="text">
								<div class="title">
									Strings change together <span class="badge accent">Experimental</span>
								</div>
								<div class="desc">
									Long and short strings on a controller show each new frame at the same moment instead of up
									to 49 ms apart. Try it on your pixels before a show: a few pixel types may not like it.
								</div>
							</div>
							<div class="control">
								<Switch
									checked={s.output?.latchAlign ?? false}
									label="Strings change together (experimental)"
									onchange={(v: boolean) => {
										if (s) {
											s.output = { latchAlign: v };
											changed('output');
										}
									}}
								/>
							</div>
						</div>
						<div class="setting stack">
							<div class="text"><div class="title">Volume</div></div>
							<div class="control" style="width:260px">
								<input
									type="range"
									class="range"
									min="0"
									max="100"
									bind:value={s.audio.volume}
									oninput={() => changed('audio')}
									style:--pct="{s.audio.volume}%"
									aria-label="Volume"
								/><span class="num small" style="width:40px">{s.audio.volume}%</span>
							</div>
						</div>
						<div class="setting">
							<div class="text">
								<div class="title">Volume leveling</div>
								<div class="desc">
									Plays every song at the same loudness, so nobody reaches for the volume knob.
								</div>
							</div>
							<div class="control">
								<Switch
									bind:checked={s.audio.normalize}
									label="Volume leveling"
									onchange={() => changed('audio')}
								/>
							</div>
						</div>
						{#if s.audio.normalize}
							<div class="setting stack">
								<div class="text">
									<div class="title">How loud</div>
									<div class="desc">Normal matches music apps. Pick Quiet for a sleepy street.</div>
								</div>
								<div class="control">
									<Segmented
										value={s.audio.targetLufs}
										label="How loud"
										size="sm"
										onchange={(v) => {
											if (s) {
												s.audio.targetLufs = v;
												changed('audio');
											}
										}}
										options={[
											{ value: -18, label: 'Quiet' },
											{ value: -16, label: 'Relaxed' },
											{ value: -14, label: 'Normal' },
											{ value: -12, label: 'Loud' }
										]}
									/>
								</div>
							</div>
						{/if}
						<div class="setting stack">
							<div class="text">
								<div class="title">Where DJ voices are made</div>
								<div class="desc">
									Automatic uses this controller when it’s fast enough (Raspberry Pi 4 or 5) and your browser
									otherwise.
								</div>
							</div>
							<div class="control">
								<Segmented
									bind:value={s.tts.mode}
									label="Where DJ voices are made"
									size="sm"
									onchange={() => changed('tts')}
									options={[
										{ value: 'auto', label: 'Automatic' },
										{ value: 'device', label: 'Controller' },
										{ value: 'browser', label: 'Browser' }
									]}
								/>
							</div>
						</div>
					</div>
				</section>
			{:else if sec === 'alerts'}
				<section class="card">
					<div class="card-head"><h2 class="grow">Alerts</h2></div>
					<div class="card-body">
						<p class="muted small" style="margin-bottom:8px">
							Get a message when something needs attention — even when you’re not home.
						</p>
						<div class="setting stack">
							<div class="text">
								<div class="title">Too hot</div>
								<div class="desc">Alert when any controller gets hotter than this.</div>
							</div>
							<div class="control">
								<div class="input-group" style="width:120px">
									<input
										class="input num"
										type="number"
										value={alertTemp}
										oninput={(e) => setAlertTemp(Number((e.target as HTMLInputElement).value))}
										aria-label="Alert temperature"
									/><span class="suffix">{tunit === 'f' ? '°F' : '°C'}</span>
								</div>
							</div>
						</div>
						<div class="setting stack">
							<div class="text">
								<div class="title">Low 12 V supply</div>
								<div class="desc">Alert when the power supply voltage drops below this.</div>
							</div>
							<div class="control">
								<div class="input-group" style="width:120px">
									<input
										class="input num"
										type="number"
										step="0.1"
										bind:value={s.alerts.rules.voltageMin}
										oninput={() => changed('alerts')}
									/><span class="suffix">V</span>
								</div>
							</div>
						</div>
						<div class="setting">
							<div class="text"><div class="title">A follower goes offline</div></div>
							<div class="control">
								<Switch
									bind:checked={s.alerts.rules.followerOffline}
									label="Follower offline"
									onchange={() => changed('alerts')}
								/>
							</div>
						</div>
						<div class="setting">
							<div class="text"><div class="title">The show fails to start</div></div>
							<div class="control">
								<Switch
									bind:checked={s.alerts.rules.showFailure}
									label="Show failure"
									onchange={() => changed('alerts')}
								/>
							</div>
						</div>
					</div>
				</section>
				<section class="card" style="margin-top:16px">
					<div class="card-head">
						<Send size={16} />
						<h2 class="grow">Phone notifications (ntfy)</h2>
						<Switch
							checked={!!s.alerts.ntfy}
							label="Phone notifications"
							onchange={(v) => {
								if (s) {
									s.alerts.ntfy = v
										? { server: 'https://ntfy.sh', topic: `pixelplus-${newId()}` }
										: undefined;
									changed('alerts');
								}
							}}
						/>
					</div>
					{#if s.alerts.ntfy}
						<div class="card-body">
							<div class="form-grid">
								<label class="field"
									><span class="label">Server</span><input
										class="input"
										bind:value={s.alerts.ntfy.server}
										oninput={() => changed('alerts')}
									/></label
								>
								<label class="field"
									><span class="label">Topic</span><input
										class="input mono"
										bind:value={s.alerts.ntfy.topic}
										oninput={() => changed('alerts')}
									/><span class="hint">Subscribe to this topic in the free ntfy app.</span></label
								>
							</div>
							<button
								class="btn sm"
								style="margin-top:14px"
								onclick={() => test('ntfy')}
								disabled={testing === 'ntfy'}
								><Send size={14} /> {testing === 'ntfy' ? 'Sending…' : 'Send a test notification'}</button
							>
						</div>
					{/if}
				</section>
				<section class="card" style="margin-top:16px">
					<div class="card-head">
						<Bell size={16} />
						<h2 class="grow">Email</h2>
						<Switch
							checked={!!s.alerts.email}
							label="Email alerts"
							onchange={(v) => {
								if (s) {
									s.alerts.email = v
										? { smtpHost: '', smtpPort: 587, username: '', password: '', from: '', to: '', tls: true }
										: undefined;
									changed('alerts');
								}
							}}
						/>
					</div>
					{#if s.alerts.email}
						<div class="card-body">
							<div class="form-grid">
								<label class="field"
									><span class="label">Send to</span><input
										class="input"
										type="email"
										bind:value={s.alerts.email.to}
										oninput={() => changed('alerts')}
									/></label
								>
								<label class="field"
									><span class="label">From</span><input
										class="input"
										type="email"
										bind:value={s.alerts.email.from}
										oninput={() => changed('alerts')}
									/></label
								>
								<label class="field"
									><span class="label">Mail server (SMTP)</span><input
										class="input"
										placeholder="smtp.gmail.com"
										bind:value={s.alerts.email.smtpHost}
										oninput={() => changed('alerts')}
									/></label
								>
								<label class="field"
									><span class="label">Port</span><input
										class="input num"
										type="number"
										bind:value={s.alerts.email.smtpPort}
										oninput={() => changed('alerts')}
									/></label
								>
								<label class="field"
									><span class="label">Username</span><input
										class="input"
										bind:value={s.alerts.email.username}
										oninput={() => changed('alerts')}
									/></label
								>
								<label class="field"
									><span class="label">Password</span><input
										class="input"
										type="password"
										bind:value={s.alerts.email.password}
										oninput={() => changed('alerts')}
										autocomplete="new-password"
									/></label
								>
								<div class="field">
									<span class="label">Secure connection (TLS)</span>
									<div class="row" style="height:40px">
										<Switch
											bind:checked={s.alerts.email.tls}
											label="TLS"
											onchange={() => changed('alerts')}
										/>
									</div>
								</div>
							</div>
							<button
								class="btn sm"
								style="margin-top:14px"
								onclick={() => test('email')}
								disabled={testing === 'email'}
								><Send size={14} /> {testing === 'email' ? 'Sending…' : 'Send a test email'}</button
							>
						</div>
					{/if}
				</section>
			{:else if sec === 'mqtt'}
				<section class="card">
					<div class="card-head">
						<House size={16} />
						<h2 class="grow">Home Assistant & MQTT</h2>
						<Switch bind:checked={s.mqtt.enabled} label="MQTT" onchange={() => changed('mqtt')} />
					</div>
					<div class="card-body">
						<p class="muted small" style="margin-bottom:14px">
							Control the show from Home Assistant: play, stop, brightness, blackout and live status as
							entities.
						</p>
						<div class="form-grid" class:disabled={!s.mqtt.enabled}>
							<label class="field"
								><span class="label">Broker</span><input
									class="input"
									bind:value={s.mqtt.host}
									oninput={() => changed('mqtt')}
								/></label
							>
							<label class="field"
								><span class="label">Port</span><input
									class="input num"
									type="number"
									bind:value={s.mqtt.port}
									oninput={() => changed('mqtt')}
								/></label
							>
							<label class="field"
								><span class="label">Username</span><input
									class="input"
									bind:value={s.mqtt.username}
									oninput={() => changed('mqtt')}
								/></label
							>
							<label class="field"
								><span class="label">Password</span><input
									class="input"
									type="password"
									bind:value={s.mqtt.password}
									oninput={() => changed('mqtt')}
									autocomplete="new-password"
								/></label
							>
							<label class="field"
								><span class="label">Topic prefix</span><input
									class="input mono"
									bind:value={s.mqtt.baseTopic}
									oninput={() => changed('mqtt')}
								/></label
							>
							<div class="field">
								<span class="label">Home Assistant auto-discovery</span>
								<div class="row" style="height:40px">
									<Switch
										bind:checked={s.mqtt.homeAssistantDiscovery}
										label="Discovery"
										onchange={() => changed('mqtt')}
									/><span class="small muted">Entities appear automatically</span>
								</div>
							</div>
						</div>
						<button
							class="btn sm"
							style="margin-top:16px"
							onclick={() => test('mqtt')}
							disabled={testing === 'mqtt'}
							><RefreshCw size={14} /> {testing === 'mqtt' ? 'Connecting…' : 'Test connection'}</button
						>
					</div>
				</section>
			{:else if sec === 'requests'}
				<section class="card">
					<div class="card-head">
						<Hand size={16} />
						<h2 class="grow">Song requests</h2>
						<Switch
							bind:checked={s.requests.enabled}
							label="Song requests"
							onchange={() => changed('requests')}
						/>
					</div>
					<div class="card-body">
						<div class="reqgrid">
							<div class="col" style="gap:14px">
								<label class="field"
									><span class="label">Page title</span><input
										class="input"
										bind:value={s.requests.title}
										oninput={() => changed('requests')}
									/></label
								>
								<label class="field"
									><span class="label">Message</span><textarea
										class="textarea"
										rows="2"
										bind:value={s.requests.message}
										oninput={() => changed('requests')}></textarea></label
								>
								<div class="form-grid">
									<label class="field"
										><span class="label">Songs to choose from</span><select
											class="select"
											bind:value={s.requests.playlistId}
											onchange={() => changed('requests')}
											><option value={undefined}>All sequences</option
											>{#each show.playlists as p (p.id)}<option value={p.id}>{p.name}</option>{/each}</select
										></label
									>
									<label class="field"
										><span class="label">Most requests waiting</span><input
											class="input num"
											type="number"
											min="1"
											max="50"
											bind:value={s.requests.maxQueue}
											oninput={() => changed('requests')}
										/></label
									>
								</div>
								<div class="form-grid">
									<label class="field"
										><span class="label"><Radio size={13} /> FM radio station</span><input
											class="input"
											placeholder="e.g. 88.3 FM"
											value={s.requests.radioFrequency ?? ''}
											oninput={(e) => {
												if (s) {
													s.requests.radioFrequency = (e.target as HTMLInputElement).value;
													changed('requests');
												}
											}}
										/><span class="hint"
											>Shown as “Tune your radio to…” on the request page and yard sign.</span
										></label
									>
									<label class="field"
										><span class="label"><Globe size={13} /> Internet address (optional)</span><input
											class="input"
											placeholder="e.g. requests.yourlights.com"
											value={s.requests.publicUrl ?? ''}
											inputmode="url"
											autocapitalize="off"
											spellcheck="false"
											oninput={(e) => {
												if (s) {
													s.requests.publicUrl = (e.target as HTMLInputElement).value.trim();
													changed('requests');
												}
											}}
										/><span class="hint">So visitors on the street can open the page on their own data.</span
										></label
									>
								</div>
							</div>
							<div class="qrbox">
								<QrCode text={requestUrl} size={150} />
								<a
									class="small"
									href={reqLink.isPublic ? requestUrl : '/request'}
									target="_blank"
									rel="noopener">{prettyUrl(requestUrl)}</a
								>
								<div class="row wrap" style="justify-content:center">
									<button
										class="btn sm"
										onclick={() => {
											navigator.clipboard?.writeText(requestUrl);
											toasts.success('Link copied');
										}}><Copy size={13} /> Copy link</button
									>
									<a class="btn sm" href="/yard-sign"><Printer size={13} /> Yard sign</a>
								</div>
							</div>
						</div>
						{#if !reqLink.isPublic}
							<div class="notice info small lanwarn">
								<Info size={16} />
								<div>
									<strong>This QR code only works on your home Wi-Fi.</strong> It points at the controller’s
									address on your network, which phones on the street can’t reach. Add an internet address
									above to share it with visitors.
									<details class="howto">
										<summary>How do I get an internet address?</summary>
										<ol>
											<li>
												Set up a free <strong>Cloudflare Tunnel</strong> (or a similar service) on a computer at
												home, or on this controller.
											</li>
											<li>
												Point it at <span class="mono">{location.origin}/request</span> — only the request page,
												not the rest of PixelPlus.
											</li>
											<li>Paste the address it gives you into “Internet address” above.</li>
										</ol>
										<p>Keep a password on PixelPlus (Security) so only you can change the show.</p>
									</details>
								</div>
							</div>
						{/if}
						<h3 class="eyebrow" style="margin:22px 0 8px">Waiting to play · {queue.length}</h3>
						<div class="list qlist">
							{#each queue as q (q.id)}
								<div class="list-row">
									<span class="grow"
										><strong class="small">{q.name}</strong><span class="faint tiny"
											>{q.requestedBy ? ` · for ${q.requestedBy}` : ''} · {fmtRelative(q.requestedAt)}</span
										></span
									>
									<button
										class="btn ghost icon sm"
										onclick={async () => {
											await api.removeRequest(q.id);
											queue = queue.filter((x) => x.id !== q.id);
										}}
										aria-label="Remove request"><X size={14} /></button
									>
								</div>
							{:else}
								<div class="faint small" style="padding:12px 0">No requests right now.</div>
							{/each}
						</div>
					</div>
				</section>
			{:else if sec === 'triggers'}
				<section class="card">
					<div class="card-head">
						<Zap size={16} />
						<h2 class="grow">Triggers</h2>
						<button class="btn sm" onclick={addTrigger}><Plus size={14} /> Add trigger</button>
					</div>
					<div class="card-body">
						<p class="muted small" style="margin-bottom:14px">
							Start things with a push button wired to the controller, or from another app (like Home
							Assistant) by opening a link.
						</p>
						{#each s.triggers as t (t.id)}
							<div class="trig">
								<input
									class="input"
									bind:value={t.name}
									oninput={() => changed('triggers')}
									aria-label="Trigger name"
								/>
								<div class="row wrap">
									<select
										class="select sm"
										style="width:auto"
										bind:value={t.kind}
										onchange={() => changed('triggers')}
										aria-label="Trigger kind"
										><option value="gpio">Button wired to the controller</option><option value="http"
											>Link (web request)</option
										></select
									>
									{#if t.kind === 'gpio'}<span
											class="small muted"
											title="The Raspberry Pi GPIO pin the button is wired to">on pin</span
										><input
											class="input sm num"
											style="width:70px"
											type="number"
											min="2"
											max="27"
											bind:value={t.gpio}
											oninput={() => changed('triggers')}
											aria-label="GPIO pin"
										/>{/if}
									<span class="small muted">→</span>
									<select
										class="select sm"
										style="width:auto"
										bind:value={t.action.type}
										onchange={() => changed('triggers')}
										aria-label="Action"
									>
										<option value="playPlaylist">Play playlist</option><option value="playSequence"
											>Play sequence</option
										><option value="effect">Show a look</option><option value="stop">Stop the show</option>
									</select>
									{#if t.action.type !== 'stop'}
										<select
											class="select sm"
											style="width:auto;max-width:220px"
											bind:value={t.action.ref}
											onchange={() => changed('triggers')}
											aria-label="Target"
										>
											{#each t.action.type === 'playPlaylist' ? show.playlists : t.action.type === 'playSequence' ? show.sequences : show.effects as o (o.id)}<option
													value={o.id}>{o.name}</option
												>{/each}
										</select>
									{/if}
									<span class="grow"></span>
									<button
										class="btn ghost icon sm"
										onclick={() => removeTrigger(t)}
										aria-label="Remove trigger"><Trash2 size={14} /></button
									>
								</div>
								{#if t.kind === 'http'}<code class="mono faint tiny"
										>POST {location.origin}/api/v1/triggers/{t.id}</code
									>{/if}
							</div>
						{:else}
							<div class="faint small">No triggers yet.</div>
						{/each}
					</div>
				</section>
			{:else if sec === 'security'}
				<section class="card">
					<div class="card-head">
						<ShieldCheck size={16} />
						<h2 class="grow">Security</h2>
					</div>
					<div class="card-body">
						<div class="setting">
							<div class="text">
								<div class="title">Password</div>
								<div class="desc">
									{sys?.passwordSet
										? 'Anyone opening this page must sign in.'
										: 'Anyone on your network can open this page. The song request page is always public.'}
								</div>
							</div>
							<div class="control">
								<span class="badge {sys?.passwordSet ? 'green' : ''}">{sys?.passwordSet ? 'On' : 'Off'}</span
								><button class="btn sm" onclick={() => (pwOpen = true)}
									>{sys?.passwordSet ? 'Change' : 'Set a password'}</button
								>
							</div>
						</div>
						<div class="setting">
							<div class="text">
								<div class="title"><Terminal size={14} /> SSH (remote command line)</div>
								<div class="desc">
									{#if !ssh}Checking…{:else if ssh.enabled === null}Not available here: PixelPlus doesn’t
										manage this computer’s SSH server.{:else if sshJob?.state === 'running'}{sshJob.message}{:else}For
										advanced troubleshooting. Log in as the controller’s user (set with Raspberry Pi Imager or
										<span class="mono">ssh_password=</span> in pixelplus.txt).{/if}
								</div>
							</div>
							<div class="control">
								<Switch
									checked={!!ssh?.enabled}
									disabled={!ssh?.canChange || sshJob?.state === 'running'}
									label="SSH"
									onchange={(v: boolean) => setSsh(v)}
								/>
							</div>
						</div>
						{#if s}
							<div class="setting">
								<div class="text">
									<div class="title">Other names for this controller</div>
									<div class="desc">
										PixelPlus only answers to its IP address and <span class="mono"
											>{sys?.hostname ?? 'pixelplus'}.local</span
										>. If you open it through a tunnel or your own domain, add that name here (comma
										separated, <span class="mono">*.example.com</span> allowed).
									</div>
								</div>
								<div class="control">
									<input
										class="input"
										placeholder="lights.example.com"
										aria-label="Other names for this controller"
										value={(s.security.allowedHosts ?? []).join(', ')}
										onchange={(e) => {
											s!.security.allowedHosts = (e.currentTarget as HTMLInputElement).value
												.split(/[\s,]+/)
												.map((x) => x.trim())
												.filter(Boolean);
											changed('security');
										}}
									/>
								</div>
							</div>
						{/if}
						<div class="setting">
							<div class="text">
								<div class="title">Sign out</div>
								<div class="desc">Ends your session in this browser.</div>
							</div>
							<div class="control">
								<button
									class="btn sm"
									disabled={!sys?.passwordSet}
									onclick={async () => {
										await api.logout().catch(() => {});
										location.reload();
									}}>Sign out</button
								>
							</div>
						</div>
					</div>
				</section>
			{:else if sec === 'snapshots'}
				<section class="card">
					<div class="card-head">
						<History size={16} />
						<h2 class="grow">Backups</h2>
						<button class="btn sm" onclick={() => importInput?.click()}
							><Upload size={14} /> Import a backup file</button
						>
						<input
							bind:this={importInput}
							type="file"
							class="sr-only"
							accept=".zst,.tar.zst,.ppbackup"
							onchange={importSnap}
						/>
					</div>
					<div class="card-body">
						<p class="muted small">
							PixelPlus backs up your whole show every night and before big changes. Go back to any of them,
							or download one to keep somewhere safe.
						</p>
						<div class="row" style="margin:16px 0 8px">
							<input
								class="input"
								placeholder="Name this backup (optional)"
								bind:value={snapLabel}
								aria-label="Backup name"
							/>
							<button class="btn primary" onclick={takeSnap}><Camera size={15} /> Back up now</button>
						</div>
					</div>
					<div class="list">
						{#if !snaps}<div class="card-body"><Skeleton count={3} h={40} /></div>{/if}
						{#each snaps ?? [] as sn, i (sn.id)}
							<div class="list-row snap">
								<span class="tl" class:first={i === 0}></span>
								<div class="grow">
									<strong class="small">{sn.label}</strong>
									<div class="faint tiny">
										{new Date(sn.createdAt).toLocaleString(undefined, {
											dateStyle: 'medium',
											timeStyle: 'short'
										})} · {fmtRelative(sn.createdAt)} · {fmtBytes(sn.sizeBytes)}{sn.auto
											? ' · automatic'
											: ''}
									</div>
								</div>
								<button class="btn sm" onclick={() => restore(sn)}><RotateCcw size={14} /> Restore</button>
								<a
									class="btn ghost icon sm"
									href={api.snapshotDownloadUrl(sn.id)}
									download
									aria-label="Download {sn.label}"><Download size={14} /></a
								>
								<button class="btn ghost icon sm" onclick={() => delSnap(sn)} aria-label="Delete {sn.label}"
									><Trash2 size={14} /></button
								>
							</div>
						{/each}
					</div>
				</section>
			{:else if sec === 'updates'}
				<section class="card">
					<div class="card-head">
						<Download size={16} />
						<h2 class="grow">Updates</h2>
						<button
							class="btn ghost sm"
							onclick={() => {
								upd = null;
								api.checkUpdate().then((u) => (upd = u));
							}}><RefreshCw size={14} /> Check again</button
						>
					</div>
					<div class="card-body">
						{#if !upd}<Skeleton count={3} />{:else if upd.available}
							<div class="upd">
								<span class="icon-tile accent"><Download size={20} /></span>
								<div class="grow">
									<strong>PixelPlus {upd.latest} is available</strong>
									<div class="faint small">
										You have {upd.current}{upd.channel ? ` · ${upd.channel} channel` : ''}
									</div>
								</div>
								{#if upd.canApply !== false}
									<button
										class="btn primary"
										onclick={applyUpdate}
										disabled={updating || updJob?.state === 'running'}
										>{updating || updJob?.state === 'running' ? 'Updating…' : 'Update now'}</button
									>
								{/if}
							</div>
							{#if updJob}
								<div class="small upd-job {updJob.state}">
									{#if updJob.state === 'running'}<RefreshCw
											size={13}
											class="spin"
										/>{:else if updJob.state === 'ok'}<Check size={13} />{:else}<TriangleAlert
											size={13}
										/>{/if}
									{updJob.message}
								</div>
							{/if}
							{#if upd.message && upd.canApply === false}<div class="small muted">{upd.message}</div>{/if}
							{#if upd.notes}<pre class="notes">{upd.notes}</pre>{/if}
						{:else}
							<!-- A message without an update means the check couldn't run (no repository,
							     offline, Docker): don't claim "up to date" then. -->
							<div class="upd">
								{#if upd.message}
									<span class="icon-tile"><Download size={20} /></span>
									<div class="grow">
										<strong>PixelPlus {upd.current}</strong>
										<div class="faint small">Updates can’t be checked from here</div>
									</div>
								{:else}
									<span class="icon-tile green"><Check size={20} /></span>
									<div class="grow">
										<strong>You’re up to date</strong>
										<div class="faint small">PixelPlus {upd.current}</div>
									</div>
								{/if}
							</div>
							{#if upd.message}<div class="small muted" style="margin-top:10px">{upd.message}</div>{/if}
						{/if}
					</div>
				</section>
			{:else if sec === 'hardware'}
				<section class="card">
					<div class="card-head">
						<Cpu size={16} />
						<h2 class="grow">This controller</h2>
					</div>
					<div class="card-body">
						<dl class="about">
							<div>
								<dt>Board</dt>
								<dd>
									{sys?.board ? BOARDS[sys.board].name : '—'}{sys?.boardRev ? ` · rev ${sys.boardRev}` : ''}
								</dd>
							</div>
							<div>
								<dt>Raspberry Pi</dt>
								<dd>{sys?.piModel ?? '—'}</dd>
							</div>
							<div>
								<dt>Software</dt>
								<dd>PixelPlus {sys?.version}</dd>
							</div>
							<div>
								<dt>Address</dt>
								<dd class="mono">{sys?.ips.join(', ')}</dd>
							</div>
							<div>
								<dt>Up for</dt>
								<dd>{sys ? fmtUptime(sys.uptimeS) : '—'}</dd>
							</div>
							<div>
								<dt>CPU · memory</dt>
								<dd>
									{sys?.cpuPct != null ? `${Math.round(sys.cpuPct)}%` : '—'} · {sys?.memPct != null
										? `${Math.round(sys.memPct)}%`
										: '—'}
								</dd>
							</div>
							<div>
								<dt>Free space</dt>
								<dd>{sys ? fmtBytes(sys.diskFreeMb * 1024 * 1024) : '—'}</dd>
							</div>
							<div>
								<dt>Temperature</dt>
								<dd>{fmtTemp(sys?.tempC, tunit)}</dd>
							</div>
						</dl>
						<div class="setting" style="margin-top:12px">
							<div class="text">
								<div class="title">Status screen</div>
								<div class="desc">Shows the song and status on the little screen on the transmitter.</div>
							</div>
							<div class="control">
								<Switch
									bind:checked={s.oled.enabled}
									label="Status screen"
									onchange={() => changed('oled')}
								/>
							</div>
						</div>

						<div class="row wrap" style="margin-top:16px">
							<button class="btn" onclick={() => power('restart')}
								><RefreshCw size={14} /> Restart PixelPlus</button
							>
							<button class="btn" onclick={() => power('reboot')}><Monitor size={14} /> Reboot</button>
							<button class="btn danger" onclick={() => power('shutdown')}
								><Power size={14} /> Shut down</button
							>
						</div>
						<p class="faint tiny" style="margin-top:16px">
							Board identity (EEPROM) can be rewritten under Controllers → Advanced.
						</p>
					</div>
				</section>
			{:else if sec === 'logs'}
				<section class="card">
					<div class="card-head">
						<ScrollText size={16} />
						<h2 class="grow">Logs</h2>
						<button class="btn ghost icon sm" onclick={loadLogs} aria-label="Refresh logs" title="Refresh"
							><RefreshCw size={14} /></button
						>
						<button class="btn sm" onclick={copyLogs} disabled={!logs}
							><Copy size={13} /> Copy for support</button
						>
					</div>
					<div class="logbar">
						<Segmented
							bind:value={logFilter}
							size="sm"
							label="Show"
							options={[
								{ value: 'all', label: 'Everything' },
								{ value: 'warn', label: logProblems ? `Problems · ${logProblems}` : 'Problems' }
							]}
						/>
						<input
							class="input sm grow"
							placeholder="Search logs"
							bind:value={logQ}
							aria-label="Search logs"
						/>
					</div>
					<div class="loglist">
						{#if logs == null}<div class="card-body"><Skeleton count={8} h={18} /></div>{/if}
						{#each logGroups as g (g.day)}
							<div class="logday">{g.day}</div>
							{#each g.rows as l, i (i + l.raw)}
								<div class="logrow {l.level}">
									<span class="lt num">{l.time ? fmtClock(l.time) : ''}</span>
									<span class="lvl-badge {l.level}"
										>{l.level === 'error'
											? 'Error'
											: l.level === 'warn'
												? 'Warning'
												: l.level === 'debug'
													? 'Detail'
													: 'Info'}</span
									>
									<span class="lm">{l.message}</span>
								</div>
							{/each}
						{:else}
							{#if logs != null}
								<div class="card-body faint small">
									{logFilter === 'warn' ? 'No problems logged. Nice.' : 'Nothing matches.'}
								</div>
							{/if}
						{/each}
					</div>
				</section>
			{/if}
		</div>
	</div>
</div>

{#if s}
	<SyncWizard bind:open={syncOpen} delayMs={s.audio.outputDelayMs ?? 0} onsave={saveDelay} />
{/if}

<Modal bind:open={pwOpen} title={sys?.passwordSet ? 'Change password' : 'Set a password'} size="sm">
	<div class="col" style="gap:12px">
		{#if sys?.passwordSet}<label class="field"
				><span class="label">Current password</span><input
					class="input"
					type="password"
					bind:value={pwCur}
					autocomplete="current-password"
				/></label
			>{/if}
		<label class="field"
			><span class="label">New password</span><input
				class="input"
				type="password"
				bind:value={pwNew}
				autocomplete="new-password"
			/>{#if pwNew && pwNew.length < 6}<span class="hint" style="color:var(--red)"
					>Use at least 6 characters</span
				>{/if}</label
		>
		<label class="field"
			><span class="label">Repeat it</span><input
				class="input"
				type="password"
				bind:value={pwNew2}
				autocomplete="new-password"
			/>{#if pwNew2 && pwNew !== pwNew2}<span class="hint" style="color:var(--red)"
					>Passwords don’t match</span
				>{/if}</label
		>
	</div>
	{#snippet footer()}
		{#if sys?.passwordSet}<button class="btn ghost" onclick={() => setPassword(true)}>Remove password</button
			><span class="grow"></span>{/if}
		<button class="btn ghost" onclick={() => (pwOpen = false)}>Cancel</button>
		<button class="btn primary" disabled={pwNew.length < 6 || pwNew !== pwNew2} onclick={() => setPassword()}
			>Save password</button
		>
	{/snippet}
</Modal>

<style>
	.savestate {
		display: inline-flex;
		align-items: center;
		gap: 4px;
		align-self: center;
	}
	.layout {
		display: grid;
		grid-template-columns: 220px minmax(0, 1fr);
		gap: 24px;
		align-items: start;
	}
	.snav {
		display: flex;
		flex-direction: column;
		gap: 2px;
		position: sticky;
		top: 16px;
	}
	.si {
		display: flex;
		align-items: center;
		gap: 10px;
		height: 38px;
		padding: 0 12px;
		border-radius: 9px;
		font-size: 13.5px;
		font-weight: 520;
		color: var(--text-2);
		text-align: left;
	}
	.si:hover {
		background: var(--surface);
		color: var(--text);
	}
	.si.on {
		background: var(--surface);
		color: var(--text);
		box-shadow: inset 0 0 0 1px var(--border-2);
	}
	.si.on .si-ic {
		color: var(--accent-text);
	}
	.si-ic {
		display: grid;
		place-items: center;
		flex: 0 0 auto;
	}
	.si-txt {
		display: flex;
		flex-direction: column;
		min-width: 0;
		flex: 1 1 auto;
	}
	.si-desc,
	.si :global(.si-chev),
	.secbar .back {
		display: none;
	}
	.content {
		max-width: 820px;
		min-width: 0;
	}
	.secbar {
		display: flex;
		align-items: center;
		gap: 8px;
		min-height: 28px;
		margin: -4px 0 8px;
	}
	.notapplied {
		color: var(--accent-text);
		font-weight: 560;
	}
	.devnote {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		margin-bottom: 10px;
		padding: 4px 10px;
		border-radius: 99px;
		background: var(--blue-soft);
		color: var(--blue);
		font-size: 12px;
		font-weight: 560;
	}
	.devnote strong {
		font-weight: 650;
	}
	:global(.spin) {
		animation: spin 1s linear infinite;
	}
	@keyframes spin {
		to {
			transform: rotate(360deg);
		}
	}
	.nw {
		display: flex;
		gap: 12px;
		align-items: flex-start;
		padding: 12px 14px;
		margin-bottom: 16px;
		border: 1px solid var(--border-2);
		border-radius: 12px;
		background: var(--surface-2);
	}
	.nw.hotspot {
		border-color: var(--accent-line);
		background: var(--accent-soft);
	}
	.nw .grow {
		min-width: 0;
	}
	.nw-err {
		color: var(--red);
		margin-top: 6px;
		display: flex;
		gap: 6px;
		align-items: center;
	}
	.nw-note {
		margin-bottom: 12px;
	}
	.nw-facts {
		display: grid;
		grid-template-columns: max-content 1fr;
		gap: 10px 20px;
		margin: 4px 0 0;
		font-size: 14px;
	}
	.nw-facts dt {
		color: var(--text-3);
	}
	.nw-facts dd {
		margin: 0;
		overflow-wrap: anywhere;
	}
	.upd-job {
		display: flex;
		gap: 6px;
		align-items: center;
		margin-top: 10px;
	}
	.upd-job.failed {
		color: var(--red);
	}
	.upd-job.ok {
		color: var(--green);
	}
	.wifi {
		margin-top: 20px;
		padding: 16px;
		border-radius: 14px;
		background: var(--surface-2);
	}
	.current {
		display: flex;
		align-items: center;
		gap: 8px;
		margin-top: 10px;
		font-size: 13.5px;
	}
	.current :global(svg) {
		color: var(--green);
	}
	.nets {
		margin-top: 12px;
		display: flex;
		flex-direction: column;
		border-radius: 10px;
		border: 1px solid var(--border);
		background: var(--surface);
		overflow: hidden;
	}
	.netrow {
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 0 14px;
		height: 44px;
		border-bottom: 1px solid var(--border);
		text-align: left;
		font-size: 13.5px;
	}
	.netrow:last-child {
		border-bottom: 0;
	}
	.netrow:hover {
		background: var(--surface-hover);
	}
	.netrow.on {
		color: var(--accent-text);
	}
	.adv {
		margin-top: 18px;
	}
	.adv summary {
		cursor: pointer;
		font-weight: 560;
		font-size: 13px;
		color: var(--text-2);
	}
	.disabled {
		opacity: 0.5;
	}
	.reqgrid {
		display: grid;
		grid-template-columns: minmax(0, 1fr) 210px;
		gap: 20px;
	}
	.lanwarn {
		margin-top: 16px;
	}
	.howto {
		margin-top: 8px;
	}
	.howto summary {
		cursor: pointer;
		font-weight: 600;
		color: var(--text);
	}
	.howto ol {
		margin: 8px 0;
		padding-left: 20px;
		display: flex;
		flex-direction: column;
		gap: 4px;
	}
	.field .label :global(svg) {
		display: inline-block;
		vertical-align: -2px;
		margin-right: 2px;
	}
	.qrbox {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 8px;
		padding: 16px;
		border-radius: 14px;
		background: var(--surface-2);
		text-align: center;
	}
	.qrbox a:not(.btn) {
		color: var(--accent-text);
		word-break: break-all;
	}
	.qlist .list-row {
		padding-left: 0;
		padding-right: 0;
		min-height: 44px;
	}
	.trig {
		display: flex;
		flex-direction: column;
		gap: 8px;
		padding: 14px;
		border-radius: 12px;
		background: var(--surface-2);
		margin-bottom: 10px;
	}
	.snap {
		position: relative;
		padding-left: 44px;
	}
	.tl {
		position: absolute;
		left: 22px;
		top: 0;
		bottom: 0;
		width: 2px;
		background: var(--border-2);
	}
	.tl::after {
		content: '';
		position: absolute;
		left: -4px;
		top: 50%;
		width: 10px;
		height: 10px;
		margin-top: -5px;
		border-radius: 50%;
		background: var(--surface-3);
		border: 2px solid var(--border-3);
	}
	.tl.first::after {
		background: var(--accent);
		border-color: var(--accent);
	}
	.upd {
		display: flex;
		align-items: center;
		gap: 14px;
	}
	.notes {
		margin: 16px 0 0;
		padding: 14px;
		border-radius: 12px;
		background: var(--surface-2);
		font-family: var(--font);
		font-size: 13px;
		white-space: pre-wrap;
		color: var(--text-2);
	}
	.about {
		display: grid;
		grid-template-columns: repeat(2, 1fr);
		gap: 14px 24px;
		margin: 0;
	}
	.about dt {
		font-size: 12px;
		color: var(--text-3);
	}
	.about dd {
		margin: 2px 0 0;
		font-weight: 550;
		font-size: 13.5px;
	}
	.logbar {
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 12px 20px;
		border-bottom: 1px solid var(--border);
		flex-wrap: wrap;
	}
	.logbar .input {
		width: auto;
		min-width: 160px;
	}
	.loglist {
		max-height: 62vh;
		overflow: auto;
		border-radius: 0 0 var(--r-3) var(--r-3);
	}
	.logday {
		position: sticky;
		top: 0;
		z-index: 1;
		padding: 8px 20px;
		font-size: 11px;
		font-weight: 650;
		letter-spacing: 0.06em;
		text-transform: uppercase;
		color: var(--text-3);
		background: var(--surface-2);
		border-bottom: 1px solid var(--border);
	}
	.logrow {
		display: grid;
		grid-template-columns: 92px 74px minmax(0, 1fr);
		align-items: baseline;
		gap: 10px;
		padding: 8px 20px;
		border-bottom: 1px solid var(--border);
		font-size: 13px;
	}
	.lt {
		color: var(--text-3);
		font-size: 12px;
	}
	.lm {
		overflow-wrap: anywhere;
	}
	.lvl-badge {
		justify-self: start;
		font-size: 11px;
		font-weight: 620;
		padding: 1px 8px;
		border-radius: 99px;
		background: var(--surface-3);
		color: var(--text-2);
	}
	.lvl-badge.warn {
		background: var(--accent-soft);
		color: var(--accent-text);
	}
	.lvl-badge.error {
		background: var(--red-soft);
		color: var(--red);
	}
	.lvl-badge.debug {
		color: var(--text-3);
	}
	@media (max-width: 900px) {
		.layout {
			grid-template-columns: minmax(0, 1fr);
		}
		/* Phones and small tablets: an iOS-style list of sections that drills into one. */
		.mobile-hidden {
			display: none !important;
		}
		/* Drilled into a section: the page title stays for screen readers only. */
		.mobile-sr {
			position: absolute;
			width: 1px;
			height: 1px;
			overflow: hidden;
			clip: rect(0, 0, 0, 0);
			white-space: nowrap;
		}
		.snav {
			position: static;
			gap: 0;
			border-radius: var(--r-3);
			background: var(--surface);
			border: 1px solid var(--border);
			overflow: hidden;
		}
		.si,
		.si.on {
			height: auto;
			min-height: 60px;
			padding: 10px 14px;
			gap: 14px;
			border-radius: 0;
			background: none;
			box-shadow: none;
			color: var(--text);
			border-bottom: 1px solid var(--border);
		}
		.si:last-child {
			border-bottom: 0;
		}
		.si-ic {
			width: 34px;
			height: 34px;
			border-radius: 9px;
			background: var(--surface-3);
			color: var(--text-2);
		}
		.si.on .si-ic {
			color: var(--text-2);
		}
		.si-label {
			font-weight: 580;
			font-size: 14.5px;
		}
		.si-desc {
			display: block;
			font-size: 12.5px;
			color: var(--text-3);
			white-space: nowrap;
			overflow: hidden;
			text-overflow: ellipsis;
		}
		.si :global(.si-chev) {
			display: block;
			color: var(--text-3);
			flex: 0 0 auto;
		}
		.secbar .back {
			display: inline-flex;
			margin-left: -10px;
			color: var(--accent-text);
			font-size: 15px;
		}
		.secbar {
			margin: -6px 0 10px;
		}
		.logrow {
			grid-template-columns: max-content max-content minmax(0, 1fr);
			padding: 8px 14px;
		}
		.logrow .lm {
			grid-column: 1 / -1;
		}
		.logday,
		.logbar {
			padding-left: 14px;
			padding-right: 14px;
		}
		.reqgrid {
			grid-template-columns: 1fr;
		}
		.about {
			grid-template-columns: 1fr;
		}
	}
</style>
