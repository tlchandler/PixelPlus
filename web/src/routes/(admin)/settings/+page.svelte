<script lang="ts">
	import { untrack } from 'svelte';
	import { api } from '$lib/api/client';
	import type { NetworkConfig, ShowSettings, Snapshot, SongRequest, Trigger, UpdateInfo, WifiNetwork } from '$lib/api/types';
	import { app } from '$lib/stores/app.svelte';
	import { theme } from '$lib/stores/theme.svelte';
	import { toasts, confirm } from '$lib/stores/toasts.svelte';
	import { BOARDS } from '$lib/util/boards';
	import { fmtBytes, fmtRelative, fmtUptime } from '$lib/util/format';
	import { newId } from '$lib/util/id';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import Switch from '$lib/components/ui/Switch.svelte';
	import Segmented from '$lib/components/ui/Segmented.svelte';
	import Modal from '$lib/components/ui/Modal.svelte';
	import Skeleton from '$lib/components/ui/Skeleton.svelte';
	import QrCode from '$lib/components/viz/QrCode.svelte';
	import {
		Wifi,
		Volume2,
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
		WifiHigh,
		WifiLow,
		WifiZero,
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
		X
	} from '@lucide/svelte';

	type Sec = 'network' | 'audio' | 'alerts' | 'mqtt' | 'requests' | 'triggers' | 'security' | 'snapshots' | 'updates' | 'hardware' | 'logs';
	const sections: { id: Sec; label: string; icon: typeof Wifi }[] = [
		{ id: 'network', label: 'Network & Wi-Fi', icon: Wifi },
		{ id: 'audio', label: 'Audio', icon: Volume2 },
		{ id: 'alerts', label: 'Alerts', icon: Bell },
		{ id: 'mqtt', label: 'Home Assistant', icon: House },
		{ id: 'requests', label: 'Song requests', icon: Hand },
		{ id: 'triggers', label: 'Triggers', icon: Zap },
		{ id: 'security', label: 'Security', icon: ShieldCheck },
		{ id: 'snapshots', label: 'Time machine', icon: History },
		{ id: 'updates', label: 'Updates', icon: Download },
		{ id: 'hardware', label: 'Hardware & about', icon: Cpu },
		{ id: 'logs', label: 'Logs', icon: ScrollText }
	];

	let sec = $state<Sec>('network');
	$effect(() => {
		const h = location.hash.slice(1) as Sec;
		if (sections.some((s) => s.id === h)) sec = h;
	});
	function go(s: Sec) {
		sec = s;
		history.replaceState(history.state, '', `#${s}`);
	}

	const show = $derived(app.show);
	let s = $state<ShowSettings | null>(null);
	let dirtyKeys = new Set<keyof ShowSettings>();
	let timer: ReturnType<typeof setTimeout>;
	let saved = $state(true);
	$effect(() => {
		const src = show?.settings;
		if (src) untrack(() => { if (saved || !s) s = structuredClone($state.snapshot(src) as ShowSettings); });
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
	let netOrig = '';
	let scan = $state<WifiNetwork[] | null>(null);
	let scanning = $state(false);
	let psk = $state('');
	$effect(() => {
		if (sec === 'network' && !net)
			api.network().then((n) => {
				net = n;
				netOrig = JSON.stringify(n);
			}).catch(() => {});
	});
	async function doScan() {
		scanning = true;
		scan = await api.scanWifi().catch(() => []);
		scanning = false;
	}
	async function saveNet() {
		if (!net) return;
		if (!(await confirm({ title: 'Apply network changes?', message: 'The controller may drop off the network for a moment. If the new settings don’t work it falls back to its own Wi-Fi hotspot so you can fix them.', confirmLabel: 'Apply' }))) return;
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
	function sigIcon(dbm: number) {
		return dbm > -60 ? WifiHigh : dbm > -75 ? Wifi : dbm > -85 ? WifiLow : WifiZero;
	}

	// ---- audio
	let devices = $state<{ id: string; name: string }[]>([]);
	$effect(() => {
		if (sec === 'audio' && !devices.length) api.audioDevices().then((d) => (devices = d)).catch(() => {});
	});

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
		if (sec === 'requests') api.requests().then((q) => (queue = q)).catch(() => {});
	});
	const requestUrl = $derived(`${location.origin}/request`);

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
		const sn = await api.createSnapshot(snapLabel.trim() || 'Manual snapshot').catch((e) => toasts.error('Snapshot failed', e.message));
		if (sn) {
			toasts.success('Snapshot saved');
			snapLabel = '';
			loadSnaps();
		}
	}
	async function restore(sn: Snapshot) {
		if (!(await confirm({ title: `Restore “${sn.label}”?`, message: `Your show goes back to how it was ${fmtRelative(sn.createdAt)}. A snapshot of the current show is taken first, so you can undo this.`, confirmLabel: 'Restore' }))) return;
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
		await api.importSnapshot(f).then(() => toasts.success('Backup imported — restore it from the list')).catch((err) => toasts.error('Import failed', err.message));
		loadSnaps();
	}

	// ---- updates
	let upd = $state<UpdateInfo | null>(null);
	let updating = $state(false);
	$effect(() => {
		if (sec === 'updates' && !upd) api.checkUpdate().then((u) => (upd = u)).catch(() => {});
	});
	async function applyUpdate() {
		if (!(await confirm({ title: `Update to ${upd?.latest}?`, message: 'The show stops for about a minute while PixelPlus restarts. Followers update automatically afterwards.', confirmLabel: 'Update now' }))) return;
		updating = true;
		await api.applyUpdate().then(() => toasts.success('Update installed')).catch((e) => toasts.error('Update failed', e.message));
		updating = false;
	}

	// ---- system
	async function power(kind: 'reboot' | 'shutdown' | 'restart') {
		const label = kind === 'reboot' ? 'Reboot' : kind === 'shutdown' ? 'Shut down' : 'Restart PixelPlus';
		if (!(await confirm({ title: `${label}?`, message: kind === 'shutdown' ? 'You’ll need to unplug and replug the power to start it again.' : 'The show stops for about a minute.', confirmLabel: label, danger: kind === 'shutdown' }))) return;
		await (kind === 'reboot' ? api.reboot() : kind === 'shutdown' ? api.shutdown() : api.restartService()).catch((e) => toasts.error(`${label} failed`, e.message));
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
	const logLines = $derived((logs ?? '').split('\n').filter((l) => l && (logFilter === 'all' || /WARN|ERROR/.test(l))));

	// ---- triggers
	function addTrigger() {
		if (!s) return;
		s.triggers = [...s.triggers, { id: newId(), name: 'New trigger', kind: 'gpio', gpio: 17, action: { type: 'playPlaylist', ref: show?.playlists[0]?.id } }];
		changed('triggers');
	}
	function removeTrigger(t: Trigger) {
		if (!s) return;
		s.triggers = s.triggers.filter((x) => x.id !== t.id);
		changed('triggers');
	}

	const sys = $derived(app.system);
</script>

<div class="page">
	<PageHeader title="Settings" subtitle="Everything here applies to the whole show, including followers.">
		{#snippet actions()}
			<span class="faint small savestate">{#if saved}<Check size={13} /> All changes saved{:else}Saving…{/if}</span>
		{/snippet}
	</PageHeader>

	<div class="layout">
		<nav class="snav" aria-label="Settings sections">
			{#each sections as x (x.id)}
				<button class="si" class:on={sec === x.id} onclick={() => go(x.id)} aria-current={sec === x.id ? 'page' : undefined}><x.icon size={16} /> {x.label}</button>
			{/each}
		</nav>

		<div class="content">
			{#if !s || !show}
				<div class="card card-pad"><Skeleton count={8} h={28} /></div>
			{:else if sec === 'network'}
				<section class="card">
					<div class="card-head"><h2 class="grow">Network & Wi-Fi</h2></div>
					<div class="card-body">
						{#if !net}<Skeleton count={4} h={36} />{:else}
							<div class="form-grid">
								<label class="field"><span class="label">Controller name on the network</span><div class="input-group"><input class="input" bind:value={net.hostname} /><span class="suffix">.local</span></div><span class="hint">Open the app at http://{net.hostname}.local</span></label>
								<label class="field"><span class="label">Wi-Fi country</span><select class="select" bind:value={net.wifi.country}>{#each ['US', 'CA', 'GB', 'IE', 'AU', 'NZ', 'DE', 'FR', 'NL', 'SE', 'NO', 'MX'] as c (c)}<option value={c}>{c}</option>{/each}</select></label>
							</div>
							<div class="wifi">
								<div class="row"><h3 class="grow">Wi-Fi network</h3><button class="btn sm" onclick={doScan} disabled={scanning}><RefreshCw size={14} class={scanning ? 'spin' : ''} /> {scanning ? 'Scanning…' : 'Scan'}</button></div>
								<div class="current"><Wifi size={16} /> Connected to <strong>{net.wifi.ssid || '—'}</strong>{#if sys?.wifi}<span class="faint small">· {sys.wifi.signal} dBm</span>{/if}</div>
								{#if scan}
									<div class="nets">
										{#each scan as n (n.ssid)}
											{@const Sig = sigIcon(n.signal)}
											<button class="netrow" class:on={net.wifi.ssid === n.ssid} onclick={() => net && (net.wifi.ssid = n.ssid)}>
												<Sig size={16} /><span class="grow">{n.ssid}</span>{#if n.secure}<Lock size={13} class="faint" />{/if}{#if net.wifi.ssid === n.ssid}<Check size={15} />{/if}
											</button>
										{/each}
									</div>
								{/if}
								<label class="field" style="margin-top:12px"><span class="label">Password for {net.wifi.ssid}</span><input class="input" type="password" placeholder="Leave empty to keep the current password" bind:value={psk} autocomplete="new-password" /></label>
							</div>
							<details class="adv">
								<summary>Wired network (Ethernet)</summary>
								<div class="form-grid" style="margin-top:12px">
									<div class="field span-2"><span class="label">Addressing</span><Segmented value={net.ethernet.dhcp ? 'dhcp' : 'static'} label="Addressing" onchange={(v) => net && (net.ethernet.dhcp = v === 'dhcp')} options={[{ value: 'dhcp', label: 'Automatic (DHCP)' }, { value: 'static', label: 'Fixed address' }]} /></div>
									{#if !net.ethernet.dhcp}
										<label class="field"><span class="label">Address</span><input class="input mono" placeholder="192.168.1.40/24" bind:value={net.ethernet.address} /></label>
										<label class="field"><span class="label">Gateway</span><input class="input mono" placeholder="192.168.1.1" bind:value={net.ethernet.gateway} /></label>
										<label class="field"><span class="label">DNS</span><input class="input mono" placeholder="1.1.1.1" bind:value={net.ethernet.dns} /></label>
									{/if}
								</div>
							</details>
							<div class="row" style="margin-top:18px;justify-content:flex-end"><button class="btn primary" disabled={!netDirty} onclick={saveNet}>Apply network settings</button></div>
						{/if}
					</div>
				</section>
			{:else if sec === 'audio'}
				<section class="card">
					<div class="card-head"><h2 class="grow">Audio</h2></div>
					<div class="card-body">
						<div class="setting stack"><div class="text"><div class="title">Output</div><div class="desc">Where the show’s music plays (e.g. to your FM transmitter).</div></div>
							<div class="control"><select class="select" style="width:260px" bind:value={s.audio.device} onchange={() => changed('audio')}>{#each devices.length ? devices : [{ id: s.audio.device, name: s.audio.device }] as d (d.id)}<option value={d.id}>{d.name}</option>{/each}</select></div></div>
						<div class="setting stack"><div class="text"><div class="title">Volume</div></div>
							<div class="control" style="width:260px"><input type="range" class="range" min="0" max="100" bind:value={s.audio.volume} oninput={() => changed('audio')} style:--pct="{s.audio.volume}%" aria-label="Volume" /><span class="num small" style="width:40px">{s.audio.volume}%</span></div></div>
						<div class="setting"><div class="text"><div class="title">Even out song volume</div><div class="desc">Plays every song at the same loudness, so nobody reaches for the volume knob.</div></div>
							<div class="control"><Switch bind:checked={s.audio.normalize} label="Normalize loudness" onchange={() => changed('audio')} /></div></div>
						{#if s.audio.normalize}
							<div class="setting stack"><div class="text"><div class="title">Target loudness</div><div class="desc">-14 LUFS is typical for streaming; -16 is a little quieter.</div></div>
								<div class="control"><Segmented value={s.audio.targetLufs} label="Target loudness" size="sm" onchange={(v) => { if (s) { s.audio.targetLufs = v; changed('audio'); } }} options={[{ value: -18, label: 'Quiet' }, { value: -16, label: '-16' }, { value: -14, label: '-14' }, { value: -12, label: 'Loud' }]} /></div></div>
						{/if}
						<div class="setting stack"><div class="text"><div class="title">DJ voice rendering</div><div class="desc">Auto uses the controller when it can (Pi 4/5, Docker) and your browser otherwise.</div></div>
							<div class="control"><Segmented bind:value={s.tts.mode} label="Voice rendering" size="sm" onchange={() => changed('tts')} options={[{ value: 'auto', label: 'Auto' }, { value: 'device', label: 'Controller' }, { value: 'browser', label: 'Browser' }]} /></div></div>
					</div>
				</section>
			{:else if sec === 'alerts'}
				<section class="card">
					<div class="card-head"><h2 class="grow">Alerts</h2></div>
					<div class="card-body">
						<p class="muted small" style="margin-bottom:8px">Get a message when something needs attention — even when you’re not home.</p>
						<div class="setting stack"><div class="text"><div class="title">Too hot</div><div class="desc">Alert when any controller gets hotter than this.</div></div>
							<div class="control"><div class="input-group" style="width:120px"><input class="input num" type="number" bind:value={s.alerts.rules.tempC} oninput={() => changed('alerts')} /><span class="suffix">°C</span></div></div></div>
						<div class="setting stack"><div class="text"><div class="title">Low 12 V supply</div><div class="desc">Alert when the power supply voltage drops below this.</div></div>
							<div class="control"><div class="input-group" style="width:120px"><input class="input num" type="number" step="0.1" bind:value={s.alerts.rules.voltageMin} oninput={() => changed('alerts')} /><span class="suffix">V</span></div></div></div>
						<div class="setting"><div class="text"><div class="title">A follower goes offline</div></div><div class="control"><Switch bind:checked={s.alerts.rules.followerOffline} label="Follower offline" onchange={() => changed('alerts')} /></div></div>
						<div class="setting"><div class="text"><div class="title">The show fails to start</div></div><div class="control"><Switch bind:checked={s.alerts.rules.showFailure} label="Show failure" onchange={() => changed('alerts')} /></div></div>
					</div>
				</section>
				<section class="card" style="margin-top:16px">
					<div class="card-head"><Send size={16} /><h2 class="grow">Phone notifications (ntfy)</h2>
						<Switch checked={!!s.alerts.ntfy} label="Phone notifications" onchange={(v) => { if (s) { s.alerts.ntfy = v ? { server: 'https://ntfy.sh', topic: `pixelplus-${newId()}` } : undefined; changed('alerts'); } }} /></div>
					{#if s.alerts.ntfy}
						<div class="card-body">
							<div class="form-grid">
								<label class="field"><span class="label">Server</span><input class="input" bind:value={s.alerts.ntfy.server} oninput={() => changed('alerts')} /></label>
								<label class="field"><span class="label">Topic</span><input class="input mono" bind:value={s.alerts.ntfy.topic} oninput={() => changed('alerts')} /><span class="hint">Subscribe to this topic in the free ntfy app.</span></label>
							</div>
							<button class="btn sm" style="margin-top:14px" onclick={() => test('ntfy')} disabled={testing === 'ntfy'}><Send size={14} /> {testing === 'ntfy' ? 'Sending…' : 'Send a test notification'}</button>
						</div>
					{/if}
				</section>
				<section class="card" style="margin-top:16px">
					<div class="card-head"><Bell size={16} /><h2 class="grow">Email</h2>
						<Switch checked={!!s.alerts.email} label="Email alerts" onchange={(v) => { if (s) { s.alerts.email = v ? { smtpHost: '', smtpPort: 587, username: '', password: '', from: '', to: '', tls: true } : undefined; changed('alerts'); } }} /></div>
					{#if s.alerts.email}
						<div class="card-body">
							<div class="form-grid">
								<label class="field"><span class="label">Send to</span><input class="input" type="email" bind:value={s.alerts.email.to} oninput={() => changed('alerts')} /></label>
								<label class="field"><span class="label">From</span><input class="input" type="email" bind:value={s.alerts.email.from} oninput={() => changed('alerts')} /></label>
								<label class="field"><span class="label">Mail server (SMTP)</span><input class="input" placeholder="smtp.gmail.com" bind:value={s.alerts.email.smtpHost} oninput={() => changed('alerts')} /></label>
								<label class="field"><span class="label">Port</span><input class="input num" type="number" bind:value={s.alerts.email.smtpPort} oninput={() => changed('alerts')} /></label>
								<label class="field"><span class="label">Username</span><input class="input" bind:value={s.alerts.email.username} oninput={() => changed('alerts')} /></label>
								<label class="field"><span class="label">Password</span><input class="input" type="password" bind:value={s.alerts.email.password} oninput={() => changed('alerts')} autocomplete="new-password" /></label>
								<div class="field"><span class="label">Secure connection (TLS)</span><div class="row" style="height:40px"><Switch bind:checked={s.alerts.email.tls} label="TLS" onchange={() => changed('alerts')} /></div></div>
							</div>
							<button class="btn sm" style="margin-top:14px" onclick={() => test('email')} disabled={testing === 'email'}><Send size={14} /> {testing === 'email' ? 'Sending…' : 'Send a test email'}</button>
						</div>
					{/if}
				</section>
			{:else if sec === 'mqtt'}
				<section class="card">
					<div class="card-head"><House size={16} /><h2 class="grow">Home Assistant & MQTT</h2><Switch bind:checked={s.mqtt.enabled} label="MQTT" onchange={() => changed('mqtt')} /></div>
					<div class="card-body">
						<p class="muted small" style="margin-bottom:14px">Control the show from Home Assistant: play, stop, brightness, blackout and live status as entities.</p>
						<div class="form-grid" class:disabled={!s.mqtt.enabled}>
							<label class="field"><span class="label">Broker</span><input class="input" bind:value={s.mqtt.host} oninput={() => changed('mqtt')} /></label>
							<label class="field"><span class="label">Port</span><input class="input num" type="number" bind:value={s.mqtt.port} oninput={() => changed('mqtt')} /></label>
							<label class="field"><span class="label">Username</span><input class="input" bind:value={s.mqtt.username} oninput={() => changed('mqtt')} /></label>
							<label class="field"><span class="label">Password</span><input class="input" type="password" bind:value={s.mqtt.password} oninput={() => changed('mqtt')} autocomplete="new-password" /></label>
							<label class="field"><span class="label">Topic prefix</span><input class="input mono" bind:value={s.mqtt.baseTopic} oninput={() => changed('mqtt')} /></label>
							<div class="field"><span class="label">Home Assistant auto-discovery</span><div class="row" style="height:40px"><Switch bind:checked={s.mqtt.homeAssistantDiscovery} label="Discovery" onchange={() => changed('mqtt')} /><span class="small muted">Entities appear automatically</span></div></div>
						</div>
						<button class="btn sm" style="margin-top:16px" onclick={() => test('mqtt')} disabled={testing === 'mqtt'}><RefreshCw size={14} /> {testing === 'mqtt' ? 'Connecting…' : 'Test connection'}</button>
					</div>
				</section>
			{:else if sec === 'requests'}
				<section class="card">
					<div class="card-head"><Hand size={16} /><h2 class="grow">Song requests</h2><Switch bind:checked={s.requests.enabled} label="Song requests" onchange={() => changed('requests')} /></div>
					<div class="card-body">
						<div class="reqgrid">
							<div class="col" style="gap:14px">
								<label class="field"><span class="label">Page title</span><input class="input" bind:value={s.requests.title} oninput={() => changed('requests')} /></label>
								<label class="field"><span class="label">Message</span><textarea class="textarea" rows="2" bind:value={s.requests.message} oninput={() => changed('requests')}></textarea></label>
								<div class="form-grid">
									<label class="field"><span class="label">Songs to choose from</span><select class="select" bind:value={s.requests.playlistId} onchange={() => changed('requests')}><option value={undefined}>All sequences</option>{#each show.playlists as p (p.id)}<option value={p.id}>{p.name}</option>{/each}</select></label>
									<label class="field"><span class="label">Most requests waiting</span><input class="input num" type="number" min="1" max="50" bind:value={s.requests.maxQueue} oninput={() => changed('requests')} /></label>
								</div>
							</div>
							<div class="qrbox">
								<QrCode text={requestUrl} size={150} />
								<a class="small" href="/request" target="_blank" rel="noopener">{requestUrl.replace(/^https?:\/\//, '')}</a>
								<button class="btn sm" onclick={() => { navigator.clipboard?.writeText(requestUrl); toasts.success('Link copied'); }}><Copy size={13} /> Copy link</button>
							</div>
						</div>
						<h3 class="eyebrow" style="margin:22px 0 8px">Waiting to play · {queue.length}</h3>
						<div class="list qlist">
							{#each queue as q (q.id)}
								<div class="list-row"><span class="grow"><strong class="small">{q.name}</strong><span class="faint tiny">{q.requestedBy ? ` · for ${q.requestedBy}` : ''} · {fmtRelative(q.requestedAt)}</span></span>
									<button class="btn ghost icon sm" onclick={async () => { await api.removeRequest(q.id); queue = queue.filter((x) => x.id !== q.id); }} aria-label="Remove request"><X size={14} /></button></div>
							{:else}
								<div class="faint small" style="padding:12px 0">No requests right now.</div>
							{/each}
						</div>
					</div>
				</section>
			{:else if sec === 'triggers'}
				<section class="card">
					<div class="card-head"><Zap size={16} /><h2 class="grow">Triggers</h2><button class="btn sm" onclick={addTrigger}><Plus size={14} /> Add trigger</button></div>
					<div class="card-body">
						<p class="muted small" style="margin-bottom:14px">Start things with a button wired to the Pi, or from another system over HTTP.</p>
						{#each s.triggers as t (t.id)}
							<div class="trig">
								<input class="input" bind:value={t.name} oninput={() => changed('triggers')} aria-label="Trigger name" />
								<div class="row wrap">
									<select class="select sm" style="width:auto" bind:value={t.kind} onchange={() => changed('triggers')} aria-label="Trigger kind"><option value="gpio">Button (GPIO)</option><option value="http">Web request</option></select>
									{#if t.kind === 'gpio'}<span class="small muted">pin</span><input class="input sm num" style="width:70px" type="number" min="2" max="27" bind:value={t.gpio} oninput={() => changed('triggers')} aria-label="GPIO pin" />{/if}
									<span class="small muted">→</span>
									<select class="select sm" style="width:auto" bind:value={t.action.type} onchange={() => changed('triggers')} aria-label="Action">
										<option value="playPlaylist">Play playlist</option><option value="playSequence">Play sequence</option><option value="effect">Show effect</option><option value="stop">Stop the show</option>
									</select>
									{#if t.action.type !== 'stop'}
										<select class="select sm" style="width:auto;max-width:220px" bind:value={t.action.ref} onchange={() => changed('triggers')} aria-label="Target">
											{#each t.action.type === 'playPlaylist' ? show.playlists : t.action.type === 'playSequence' ? show.sequences : show.effects as o (o.id)}<option value={o.id}>{o.name}</option>{/each}
										</select>
									{/if}
									<span class="grow"></span>
									<button class="btn ghost icon sm" onclick={() => removeTrigger(t)} aria-label="Remove trigger"><Trash2 size={14} /></button>
								</div>
								{#if t.kind === 'http'}<code class="mono faint tiny">POST {location.origin}/api/v1/triggers/{t.id}</code>{/if}
							</div>
						{:else}
							<div class="faint small">No triggers yet.</div>
						{/each}
					</div>
				</section>
			{:else if sec === 'security'}
				<section class="card">
					<div class="card-head"><ShieldCheck size={16} /><h2 class="grow">Security</h2></div>
					<div class="card-body">
						<div class="setting"><div class="text"><div class="title">Password</div><div class="desc">{sys?.passwordSet ? 'Anyone opening this page must sign in.' : 'Anyone on your network can open this page. The song request page is always public.'}</div></div>
							<div class="control"><span class="badge {sys?.passwordSet ? 'green' : ''}">{sys?.passwordSet ? 'On' : 'Off'}</span><button class="btn sm" onclick={() => (pwOpen = true)}>{sys?.passwordSet ? 'Change' : 'Set a password'}</button></div></div>
						<div class="setting"><div class="text"><div class="title">Sign out</div><div class="desc">Ends your session in this browser.</div></div>
							<div class="control"><button class="btn sm" disabled={!sys?.passwordSet} onclick={async () => { await api.logout().catch(() => {}); location.reload(); }}>Sign out</button></div></div>
					</div>
				</section>
			{:else if sec === 'snapshots'}
				<section class="card">
					<div class="card-head"><History size={16} /><h2 class="grow">Time machine</h2>
						<button class="btn sm" onclick={() => importInput?.click()}><Upload size={14} /> Import backup</button>
						<input bind:this={importInput} type="file" class="sr-only" accept=".zst,.tar.zst,.ppbackup" onchange={importSnap} />
					</div>
					<div class="card-body">
						<p class="muted small">PixelPlus saves a snapshot of your show every night and before big changes. Go back to any of them — or download one as a backup.</p>
						<div class="row" style="margin:16px 0 8px">
							<input class="input" placeholder="Name this snapshot (optional)" bind:value={snapLabel} aria-label="Snapshot name" />
							<button class="btn primary" onclick={takeSnap}><Camera size={15} /> Take snapshot</button>
						</div>
					</div>
					<div class="list">
						{#if !snaps}<div class="card-body"><Skeleton count={3} h={40} /></div>{/if}
						{#each snaps ?? [] as sn, i (sn.id)}
							<div class="list-row snap">
								<span class="tl" class:first={i === 0}></span>
								<div class="grow"><strong class="small">{sn.label}</strong><div class="faint tiny">{new Date(sn.createdAt).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' })} · {fmtRelative(sn.createdAt)} · {fmtBytes(sn.sizeBytes)}{sn.auto ? ' · automatic' : ''}</div></div>
								<button class="btn sm" onclick={() => restore(sn)}><RotateCcw size={14} /> Restore</button>
								<a class="btn ghost icon sm" href={api.snapshotDownloadUrl(sn.id)} download aria-label="Download {sn.label}"><Download size={14} /></a>
								<button class="btn ghost icon sm" onclick={() => delSnap(sn)} aria-label="Delete {sn.label}"><Trash2 size={14} /></button>
							</div>
						{/each}
					</div>
				</section>
			{:else if sec === 'updates'}
				<section class="card">
					<div class="card-head"><Download size={16} /><h2 class="grow">Updates</h2><button class="btn ghost sm" onclick={() => { upd = null; api.checkUpdate().then((u) => (upd = u)); }}><RefreshCw size={14} /> Check again</button></div>
					<div class="card-body">
						{#if !upd}<Skeleton count={3} />{:else if upd.available}
							<div class="upd">
								<span class="icon-tile accent"><Download size={20} /></span>
								<div class="grow"><strong>PixelPlus {upd.latest} is available</strong><div class="faint small">You have {upd.current}{upd.channel ? ` · ${upd.channel} channel` : ''}</div></div>
								<button class="btn primary" onclick={applyUpdate} disabled={updating}>{updating ? 'Updating…' : 'Update now'}</button>
							</div>
							{#if upd.notes}<pre class="notes">{upd.notes}</pre>{/if}
						{:else}
							<div class="upd"><span class="icon-tile green"><Check size={20} /></span><div class="grow"><strong>You’re up to date</strong><div class="faint small">PixelPlus {upd.current}</div></div></div>
						{/if}
					</div>
				</section>
			{:else if sec === 'hardware'}
				<section class="card">
					<div class="card-head"><Cpu size={16} /><h2 class="grow">This controller</h2></div>
					<div class="card-body">
						<dl class="about">
							<div><dt>Board</dt><dd>{sys?.board ? BOARDS[sys.board].name : '—'}{sys?.boardRev ? ` · rev ${sys.boardRev}` : ''}</dd></div>
							<div><dt>Raspberry Pi</dt><dd>{sys?.piModel ?? '—'}</dd></div>
							<div><dt>Software</dt><dd>PixelPlus {sys?.version}</dd></div>
							<div><dt>Address</dt><dd class="mono">{sys?.ips.join(', ')}</dd></div>
							<div><dt>Up for</dt><dd>{sys ? fmtUptime(sys.uptimeS) : '—'}</dd></div>
							<div><dt>CPU · memory</dt><dd>{sys?.cpuPct}% · {sys?.memPct}%</dd></div>
							<div><dt>Free space</dt><dd>{sys ? fmtBytes(sys.diskFreeMb * 1024 * 1024) : '—'}</dd></div>
							<div><dt>Temperature</dt><dd>{sys?.tempC?.toFixed(0) ?? '—'} °C</dd></div>
						</dl>
						<div class="setting" style="margin-top:12px"><div class="text"><div class="title">OLED status display</div><div class="desc">Shows the song and status on the little screen on the transmitter.</div></div>
							<div class="control"><Switch bind:checked={s.oled.enabled} label="OLED display" onchange={() => changed('oled')} /></div></div>
						<div class="setting stack"><div class="text"><div class="title">Appearance</div></div>
							<div class="control"><Segmented value={theme.current} label="Theme" size="sm" onchange={(v) => theme.set(v as 'dark' | 'light')} options={[{ value: 'dark', label: 'Dark', icon: Moon }, { value: 'light', label: 'Light', icon: Sun }]} /></div></div>
						<div class="row wrap" style="margin-top:16px">
							<button class="btn" onclick={() => power('restart')}><RefreshCw size={14} /> Restart PixelPlus</button>
							<button class="btn" onclick={() => power('reboot')}><Monitor size={14} /> Reboot</button>
							<button class="btn danger" onclick={() => power('shutdown')}><Power size={14} /> Shut down</button>
						</div>
						<p class="faint tiny" style="margin-top:16px">Board identity (EEPROM) can be rewritten under Controllers → Advanced.</p>
					</div>
				</section>
			{:else if sec === 'logs'}
				<section class="card">
					<div class="card-head"><ScrollText size={16} /><h2 class="grow">Logs</h2>
						<Segmented bind:value={logFilter} size="sm" label="Filter" options={[{ value: 'all', label: 'Everything' }, { value: 'warn', label: 'Problems' }]} />
						<button class="btn ghost icon sm" onclick={loadLogs} aria-label="Refresh logs"><RefreshCw size={14} /></button>
					</div>
					<div class="logs mono">
						{#if logs == null}<Skeleton count={10} h={14} />{/if}
						{#each logLines as l, i (i)}<div class:warn={/WARN/.test(l)} class:err={/ERROR/.test(l)}>{l}</div>{/each}
					</div>
				</section>
			{/if}
		</div>
	</div>
</div>

<Modal bind:open={pwOpen} title={sys?.passwordSet ? 'Change password' : 'Set a password'} size="sm">
	<div class="col" style="gap:12px">
		{#if sys?.passwordSet}<label class="field"><span class="label">Current password</span><input class="input" type="password" bind:value={pwCur} autocomplete="current-password" /></label>{/if}
		<label class="field"><span class="label">New password</span><input class="input" type="password" bind:value={pwNew} autocomplete="new-password" /></label>
		<label class="field"><span class="label">Repeat it</span><input class="input" type="password" bind:value={pwNew2} autocomplete="new-password" />{#if pwNew2 && pwNew !== pwNew2}<span class="hint" style="color:var(--red)">Passwords don’t match</span>{/if}</label>
	</div>
	{#snippet footer()}
		{#if sys?.passwordSet}<button class="btn ghost" onclick={() => setPassword(true)}>Remove password</button><span class="grow"></span>{/if}
		<button class="btn ghost" onclick={() => (pwOpen = false)}>Cancel</button>
		<button class="btn primary" disabled={!pwNew || pwNew !== pwNew2} onclick={() => setPassword()}>Save password</button>
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
	.si.on :global(svg) {
		color: var(--accent);
	}
	.content {
		max-width: 820px;
		min-width: 0;
	}
	:global(.spin) {
		animation: spin 1s linear infinite;
	}
	@keyframes spin {
		to {
			transform: rotate(360deg);
		}
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
		color: var(--accent);
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
		grid-template-columns: minmax(0, 1fr) 200px;
		gap: 20px;
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
	.qrbox a {
		color: var(--accent);
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
	.logs {
		max-height: 60vh;
		overflow: auto;
		padding: 14px 20px;
		font-size: 12px;
		line-height: 1.7;
		background: var(--canvas-bg);
		color: #b9bcc6;
		border-radius: 0 0 var(--r-3) var(--r-3);
		white-space: pre-wrap;
		word-break: break-word;
	}
	.logs .warn {
		color: #f5c35b;
	}
	.logs .err {
		color: #ff7b7b;
	}
	@media (max-width: 900px) {
		.layout {
			grid-template-columns: 1fr;
		}
		.snav {
			position: static;
			flex-direction: row;
			overflow-x: auto;
			margin: 0 -16px;
			padding: 0 16px 4px;
			scrollbar-width: none;
		}
		.si {
			flex: 0 0 auto;
			white-space: nowrap;
		}
		.reqgrid {
			grid-template-columns: 1fr;
		}
		.about {
			grid-template-columns: 1fr;
		}
	}
</style>
