<!--
	Settings → Remote access (F14, ARCHITECTURE §12.12). WS5.
	Reach the show without port forwarding: Tailscale (admin pages on your own devices over
	HTTPS; optional public song-request page through Funnel) and Cloudflare Tunnel (a
	temporary public link, or a named tunnel with your own host names). Public pages only by
	default: tunnels point at PixelPlus's public-only port; the admin pages need an explicit
	opt-in and a password.
-->
<script lang="ts">
	import { onDestroy, onMount } from 'svelte';
	import {
		ArrowLeft,
		Check,
		Cloud,
		Copy,
		ExternalLink,
		Globe,
		Info,
		KeyRound,
		Link2,
		Lock,
		Network,
		Power,
		ShieldAlert,
		ShieldCheck,
		TriangleAlert,
		Wifi
	} from '@lucide/svelte';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import Switch from '$lib/components/ui/Switch.svelte';
	import QrCode from '$lib/components/viz/QrCode.svelte';
	import { fleetApi } from '$lib/api/fleet';
	import type { RemoteStatus } from '$lib/api/types';
	import { app } from '$lib/stores/app.svelte';
	import { confirm, toasts } from '$lib/stores/toasts.svelte';

	let st = $state<RemoteStatus | null>(null);
	let loadError = $state<string | null>(null);
	let busy = $state<string | null>(null);
	let authKey = $state('');
	let useKey = $state(false);
	let token = $state('');
	let publicHost = $state('');
	let adminHost = $state('');
	let hostsLoaded = false;
	let tests = $state<Record<string, { ok: boolean; text: string }>>({});
	let poll: ReturnType<typeof setInterval> | undefined;

	async function load(fresh = false) {
		try {
			st = await fleetApi.remoteStatus(fresh);
			loadError = null;
			if (!hostsLoaded && st) {
				publicHost = st.cloudflare.publicHost ?? '';
				adminHost = st.cloudflare.adminHost ?? '';
				hostsLoaded = true;
			}
		} catch (e) {
			loadError = (e as Error).message;
		}
	}
	onMount(() => {
		void load(true);
		// Follows the Tailscale login (done on another device) and tunnel start-up.
		poll = setInterval(() => load(), 4000);
	});
	onDestroy(() => clearInterval(poll));

	const isFollower = $derived(app.system?.role === 'follower');
	const ts = $derived(st?.tailscale);
	const cf = $derived(st?.cloudflare);
	const tsRunning = $derived(ts?.state === 'Running');
	const tsAdminUrl = $derived(ts?.dnsName ? `https://${ts.dnsName}` : null);
	const tsPublicUrl = $derived(ts?.dnsName ? `https://${ts.dnsName}:8443/request` : null);
	const quickUrl = $derived(cf?.mode === 'quick' ? (cf.urls[0] ?? null) : null);
	const cantManage = $derived(!!st && !st.canManage);

	async function act(key: string, f: () => Promise<unknown>, ok?: string) {
		busy = key;
		try {
			await f();
			if (ok) toasts.success(ok);
			await app.reloadShow();
		} catch (e) {
			toasts.error("That didn't work", (e as Error).message);
		} finally {
			busy = null;
			await load(true);
		}
	}

	const tsInstall = () => act('ts', () => fleetApi.tailscale('install'));
	const tsUp = () =>
		act('ts', () =>
			fleetApi.tailscale('up', useKey && authKey.trim() ? { authKey: authKey.trim() } : {})
		).then(() => (authKey = ''));
	async function tsServe(on: boolean) {
		if (on) {
			const ok = await confirm({
				title: 'Manage PixelPlus over Tailscale?',
				message:
					'The admin pages become reachable at your tailnet address — only from devices signed in to your Tailscale account. Anyone with such a device still needs the PixelPlus password.',
				confirmLabel: 'Turn on'
			});
			if (!ok) return load();
		}
		await act('ts-serve', () => fleetApi.tailscale('serve', { on }));
	}
	async function tsFunnel(on: boolean) {
		if (on) {
			const ok = await confirm({
				title: 'Publish the song request page?',
				message:
					'Anyone on the internet can open the song request page and play the games (with the usual limits). The admin pages stay private.',
				confirmLabel: 'Publish'
			});
			if (!ok) return load();
		}
		await act('ts-funnel', () => fleetApi.tailscale('funnel', { on }));
	}
	async function tsDown() {
		const ok = await confirm({
			title: 'Disconnect from Tailscale?',
			message: 'The tailnet address and the public page through Funnel stop working.',
			confirmLabel: 'Disconnect',
			danger: true
		});
		if (ok) await act('ts', () => fleetApi.tailscale('down'));
	}

	const cfInstall = () => act('cf', () => fleetApi.cloudflare('install'));
	async function cfQuick(on: boolean) {
		await act('cf-quick', () => fleetApi.cloudflare('quick', { on }));
	}
	async function cfToken() {
		if (adminHost.trim()) {
			const ok = await confirm({
				title: 'Put the admin pages on the internet?',
				message: `Anyone who finds ${adminHost.trim()} sees the PixelPlus sign-in page. Protect it with Cloudflare Access (steps below) and a strong password.`,
				confirmLabel: 'I understand',
				danger: true
			});
			if (!ok) return;
		}
		await act(
			'cf-token',
			() =>
				fleetApi.cloudflare('token', {
					token: token.trim(),
					publicHost: publicHost.trim() || undefined,
					adminHost: adminHost.trim() || undefined
				}),
			'Cloudflare tunnel started'
		);
		token = '';
	}
	const cfHosts = () =>
		act(
			'cf-hosts',
			() =>
				fleetApi.cloudflare('hosts', {
					publicHost: publicHost.trim() || undefined,
					adminHost: adminHost.trim() || undefined
				}),
			'Host names saved'
		);
	async function cfStop() {
		const ok = await confirm({
			title: 'Stop the Cloudflare tunnel?',
			message: 'The public addresses stop working, and the tunnel token is deleted from this controller.',
			confirmLabel: 'Stop',
			danger: true
		});
		if (ok) await act('cf', () => fleetApi.cloudflare('stop'));
	}

	async function test(url: string) {
		tests[url] = { ok: false, text: 'Testing…' };
		try {
			const r = await fleetApi.remoteTest(url);
			tests[url] = r.ok
				? { ok: true, text: `Reachable (${r.ms} ms)` }
				: { ok: false, text: r.error ?? 'Not reachable' };
		} catch (e) {
			tests[url] = { ok: false, text: (e as Error).message };
		}
	}

	async function copy(t: string) {
		try {
			await navigator.clipboard.writeText(t);
			toasts.success('Copied');
		} catch {
			toasts.warn("Couldn't copy on this browser");
		}
	}
</script>

<svelte:head><title>Remote access · Settings · PixelPlus</title></svelte:head>

{#snippet urlRow(url: string, label: string, testable = true)}
	<li>
		<a class="mono" href={url} target="_blank" rel="noopener">{url.replace(/^https:\/\//, '')}</a>
		<span class="badge outline">{label}</span>
		<button class="btn icon sm ghost" aria-label="Copy address" onclick={() => copy(url)}
			><Copy size={14} /></button
		>
		{#if testable}
			<button class="btn sm ghost" onclick={() => test(url)}>Test</button>
			{#if tests[url]}<span class="small {tests[url].ok ? 'good' : 'bad'}">{tests[url].text}</span>{/if}
		{/if}
	</li>
{/snippet}

<div class="page remote">
	<a class="btn ghost sm back" href="/settings"><ArrowLeft size={16} /> Settings</a>
	<PageHeader
		title="Remote access"
		subtitle="Let visitors request songs from the street, and manage your show from anywhere — without opening ports on your router."
	/>

	{#if isFollower}
		<div class="notice info">
			<Info size={18} />
			<div>Set up remote access on your show leader.</div>
		</div>
	{:else if loadError && !st}
		<div class="notice danger">
			<TriangleAlert size={18} />
			<div>{loadError}</div>
		</div>
	{:else if !st}
		<div class="card skeleton sk"></div>
	{:else}
		<div class="notice info small">
			<ShieldCheck size={16} />
			<div>
				<strong>Public pages only, by default.</strong> Tunnels point at PixelPlus's public-only port (<span
					class="mono">127.0.0.1:{st.publicPort ?? 8081}</span
				>): the song request page and the games. The admin pages stay on your home network unless you turn
				them on below — and that needs a password.
			</div>
		</div>
		{#if st.message}
			<div class="notice warn small">
				<Info size={16} />
				<div>{st.message}</div>
			</div>
		{/if}
		{#if !st.passwordSet}
			<div class="notice warn small">
				<Lock size={16} />
				<div>
					No password is set. Public pages work, but the admin pages can't be reached remotely until you
					<a href="/settings#security">set a password</a>.
				</div>
			</div>
		{/if}

		<!-- Tailscale -->
		<section class="card">
			<div class="card-head">
				<Network size={18} />
				<h2 class="grow">Tailscale</h2>
				<span class="badge {tsRunning ? 'green' : 'outline'}"
					>{!ts?.installed ? 'not installed' : tsRunning ? 'connected' : ts?.state}</span
				>
			</div>
			<div class="card-body col">
				<p class="muted small">
					The safest way to manage the show from anywhere: a private network between your own devices, with a
					real HTTPS address (also good for the phone camera and microphone tools).
				</p>
				<ol class="wizard">
					<li class:done={ts?.installed}>
						<span class="n">1</span>
						<div class="grow">
							<strong>Install Tailscale</strong>
							{#if !ts?.installed}
								<div class="row">
									<button class="btn sm" onclick={tsInstall} disabled={!!busy || cantManage}>Install</button>
									<span class="faint small">From Tailscale's signed package repository (about a minute).</span
									>
								</div>
							{/if}
						</div>
					</li>
					<li class:done={tsRunning} class:off={!ts?.installed}>
						<span class="n">2</span>
						<div class="grow col">
							<strong>Connect to your tailnet</strong>
							{#if ts?.installed && !tsRunning}
								{#if ts.loginUrl}
									<div class="login">
										<div class="qr"><QrCode text={ts.loginUrl} size={120} /></div>
										<div class="col">
											<span class="small"
												>Open the link (or scan it with your phone) and sign in to Tailscale:</span
											>
											<a class="btn primary sm" href={ts.loginUrl} target="_blank" rel="noopener"
												><ExternalLink size={14} /> Open login link</a
											>
											<span class="faint small">This page notices when you're done.</span>
										</div>
									</div>
								{:else}
									<label class="row small"
										><input type="checkbox" bind:checked={useKey} /> I have an auth key</label
									>
									{#if useKey}
										<input
											class="input mono"
											type="password"
											autocomplete="off"
											placeholder="tskey-auth-…"
											bind:value={authKey}
										/>
									{/if}
									<div class="row">
										<button
											class="btn sm"
											onclick={tsUp}
											disabled={!!busy || cantManage || (useKey && !authKey.trim())}
											>{busy === 'ts' ? 'Connecting…' : 'Connect'}</button
										>
									</div>
								{/if}
							{:else if tsRunning}
								<span class="small mono">{ts?.dnsName}</span>
							{/if}
						</div>
					</li>
					<li class:done={ts?.serve} class:off={!tsRunning}>
						<span class="n">3</span>
						<div class="grow col">
							<div class="row">
								<strong class="grow">Admin pages on your tailnet</strong>
								<Switch
									label="Admin pages over Tailscale"
									checked={!!ts?.serve}
									disabled={!tsRunning || !!busy || !st.passwordSet}
									onchange={tsServe}
								/>
							</div>
							{#if tsRunning && !ts?.httpsOk}
								<span class="small warnc"
									><TriangleAlert size={13} /> Turn on <strong>MagicDNS</strong> and
									<strong>HTTPS certificates</strong>
									in the
									<a href="https://login.tailscale.com/admin/dns" target="_blank" rel="noopener"
										>Tailscale admin console</a
									> first.</span
								>
							{/if}
							{#if ts?.serve && tsAdminUrl}
								<ul class="urls">{@render urlRow(tsAdminUrl, 'your devices only', false)}</ul>
							{/if}
						</div>
					</li>
					<li class:done={ts?.funnel} class:off={!tsRunning}>
						<span class="n">4</span>
						<div class="grow col">
							<div class="row">
								<strong class="grow">Public song request page (Funnel)</strong>
								<Switch
									label="Public page through Tailscale Funnel"
									checked={!!ts?.funnel}
									disabled={!tsRunning || !!busy}
									onchange={tsFunnel}
								/>
							</div>
							<span class="faint small"
								>Optional. Allow Funnel for this device in the tailnet's access controls.</span
							>
							{#if ts?.funnel && tsPublicUrl}
								<ul class="urls">{@render urlRow(tsPublicUrl, 'public')}</ul>
							{/if}
						</div>
					</li>
				</ol>
				{#if tsRunning}
					<div class="row">
						<button class="btn sm ghost" onclick={tsDown} disabled={!!busy}
							><Power size={14} /> Disconnect</button
						>
					</div>
				{/if}
			</div>
		</section>

		<!-- Cloudflare -->
		<section class="card">
			<div class="card-head">
				<Cloud size={18} />
				<h2 class="grow">Cloudflare Tunnel</h2>
				<span class="badge {cf?.running ? 'green' : 'outline'}"
					>{!cf?.installed
						? 'not installed'
						: cf?.running
							? cf.mode === 'quick'
								? 'temporary link'
								: 'running'
							: 'off'}</span
				>
			</div>
			<div class="card-body col">
				<p class="muted small">
					A public address for the song request page — no Tailscale needed on visitors' phones.
				</p>
				{#if !cf?.installed}
					<div class="row">
						<button class="btn sm" onclick={cfInstall} disabled={!!busy || cantManage}
							>Install cloudflared</button
						>
						<span class="faint small">From Cloudflare's signed package repository.</span>
					</div>
				{:else}
					<div class="sub">
						<div class="row">
							<Link2 size={16} />
							<strong class="grow">Quick link (no account)</strong>
							<Switch
								label="Temporary public link"
								checked={cf.mode === 'quick' && cf.running}
								disabled={!!busy || cf.mode === 'token'}
								onchange={cfQuick}
							/>
						</div>
						<span class="faint small"
							>A random <span class="mono">*.trycloudflare.com</span> address for trying things out. It changes
							whenever the controller restarts.</span
						>
						{#if quickUrl}
							<ul class="urls">{@render urlRow(quickUrl + '/request', 'public, temporary')}</ul>
						{/if}
					</div>

					<div class="sub">
						<div class="row">
							<Globe size={16} />
							<strong class="grow">Your own address (Cloudflare account)</strong>
							{#if cf.tokenSet}<span class="badge green"><KeyRound size={12} /> token saved</span>{/if}
						</div>
						<details class="how small">
							<summary>What to set up in Cloudflare</summary>
							<ol>
								<li>
									In the <a href="https://one.dash.cloudflare.com/" target="_blank" rel="noopener"
										>Zero Trust dashboard</a
									>
									go to <strong>Networks → Tunnels → Create a tunnel</strong> (Cloudflared) and copy its token.
								</li>
								<li>
									Add a <strong>public hostname</strong>, e.g. <span class="mono">lights.example.com</span> →
									service <span class="mono">http://localhost:{st.publicPort ?? 8081}</span>. That is the song
									request page only.
								</li>
								<li>
									Optional admin hostname, e.g. <span class="mono">admin.example.com</span> →
									<span class="mono">http://localhost:80</span>. Then under
									<strong>Access → Applications</strong>
									add a self-hosted application for it with an <strong>email one-time PIN</strong> policy for your
									address.
								</li>
								<li>Paste the token and the host names here.</li>
							</ol>
						</details>
						<div class="grid2">
							<label class="field">
								<span class="label">Public host name</span>
								<input
									class="input mono"
									placeholder="lights.example.com"
									bind:value={publicHost}
									autocapitalize="off"
								/>
							</label>
							<label class="field">
								<span class="label"><ShieldAlert size={14} /> Admin host name (optional)</span>
								<input
									class="input mono"
									placeholder="admin.example.com"
									bind:value={adminHost}
									autocapitalize="off"
									disabled={!st.passwordSet}
								/>
								{#if !st.passwordSet}<span class="hint">Needs a password first.</span>{/if}
							</label>
						</div>
						{#if adminHost.trim()}
							<div class="notice warn small">
								<TriangleAlert size={16} />
								<div>
									The admin sign-in page will be on the internet. Put it behind Cloudflare Access (step 3
									above) and use a strong password. Sign-ins are rate limited.
								</div>
							</div>
						{/if}
						<label class="field">
							<span class="label"
								>Tunnel token {cf.tokenSet ? '(leave empty to keep the saved one)' : ''}</span
							>
							<input
								class="input mono"
								type="password"
								autocomplete="off"
								placeholder="eyJhIjoi…"
								bind:value={token}
							/>
						</label>
						<div class="row">
							{#if token.trim()}
								<button class="btn primary sm" onclick={cfToken} disabled={!!busy || cantManage}
									>{busy === 'cf-token'
										? 'Starting…'
										: cf.tokenSet
											? 'Replace token and restart'
											: 'Start tunnel'}</button
								>
							{:else}
								<button class="btn sm" onclick={cfHosts} disabled={!!busy}>Save host names</button>
							{/if}
							{#if cf.running || cf.tokenSet}
								<button class="btn sm ghost" onclick={cfStop} disabled={!!busy}
									><Power size={14} /> Stop</button
								>
							{/if}
						</div>
						{#if cf.mode === 'token' && cf.running}
							<ul class="urls">
								{#if cf.publicHost}{@render urlRow(`https://${cf.publicHost}/request`, 'public')}{/if}
								{#if cf.adminHost}{@render urlRow(`https://${cf.adminHost}`, 'admin')}{/if}
							</ul>
						{/if}
					</div>
				{/if}
			</div>
		</section>

		<section class="card">
			<div class="card-head">
				<Wifi size={18} />
				<h2 class="grow">What's reachable</h2>
			</div>
			<div class="card-body">
				<ul class="facts small">
					<li>
						<Check size={14} class="good" /> Song requests and games: rate limited per visitor and per hour.
					</li>
					<li>
						<Check size={14} class="good" /> Admin pages over Tailscale: only your own signed-in devices, plus the
						PixelPlus password.
					</li>
					<li>
						<TriangleAlert size={14} class="warnc" /> Admin pages through Cloudflare: the sign-in page is public
						— use Cloudflare Access and a strong password.
					</li>
					<li>
						<Lock size={14} /> Without a password PixelPlus refuses every admin request that comes through a tunnel.
					</li>
				</ul>
			</div>
		</section>
	{/if}
</div>

<style>
	.remote {
		max-width: 820px;
	}
	.back {
		margin-bottom: 8px;
	}
	.sk {
		height: 200px;
	}
	.col {
		display: flex;
		flex-direction: column;
		gap: 8px;
	}
	.col p {
		margin: 0;
	}
	.row {
		display: flex;
		align-items: center;
		gap: 8px;
		flex-wrap: wrap;
	}
	.grow {
		flex: 1 1 auto;
		min-width: 0;
	}
	.notice {
		margin-bottom: 12px;
	}
	.wizard {
		list-style: none;
		margin: 0;
		padding: 0;
		display: grid;
		gap: 12px;
	}
	.wizard li {
		display: flex;
		gap: 12px;
		align-items: flex-start;
	}
	.wizard li.off {
		opacity: 0.55;
	}
	.n {
		flex: 0 0 26px;
		height: 26px;
		border-radius: 50%;
		display: grid;
		place-items: center;
		font-size: 13px;
		font-weight: 600;
		background: var(--surface-3);
		color: var(--text-2);
	}
	.wizard li.done .n {
		background: var(--green);
		color: #fff;
	}
	.login {
		display: flex;
		gap: 14px;
		align-items: center;
	}
	.qr {
		padding: 6px;
		background: #fff;
		border-radius: var(--r-2);
	}
	.sub {
		display: flex;
		flex-direction: column;
		gap: 8px;
		padding: 12px;
		border: 1px solid var(--border-2);
		border-radius: var(--r-2);
		background: var(--surface-2);
	}
	.grid2 {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: 12px;
	}
	.urls {
		list-style: none;
		margin: 0;
		padding: 0;
		display: grid;
		gap: 4px;
	}
	.urls li {
		display: flex;
		align-items: center;
		flex-wrap: wrap;
		gap: 8px;
		overflow-wrap: anywhere;
	}
	.how summary {
		cursor: pointer;
		color: var(--text-2);
	}
	.how ol {
		margin: 8px 0 0;
		padding-left: 20px;
		display: grid;
		gap: 6px;
	}
	.facts {
		list-style: none;
		margin: 0;
		padding: 0;
		display: grid;
		gap: 8px;
	}
	.facts li {
		display: flex;
		gap: 8px;
		align-items: flex-start;
	}
	.good,
	:global(svg.good) {
		color: var(--green);
	}
	.bad {
		color: var(--red);
	}
	.warnc,
	:global(svg.warnc) {
		color: var(--amber, var(--text-2));
	}
	@media (max-width: 640px) {
		.grid2 {
			grid-template-columns: 1fr;
		}
		.login {
			flex-direction: column;
			align-items: flex-start;
		}
	}
</style>
