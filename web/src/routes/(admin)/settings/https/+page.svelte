<!--
	Settings → Secure connection (F1, ARCHITECTURE §12.1). WS1.
	HTTPS for phones: on/off, the certificate fingerprint (also on the controller's screen while
	this page is open), a QR code to trust a phone, addresses, extra names, renew / reset.
-->
<script lang="ts">
	import { onDestroy, onMount } from 'svelte';
	import {
		ArrowLeft,
		ShieldCheck,
		Copy,
		RefreshCw,
		RotateCcw,
		ExternalLink,
		TriangleAlert,
		Check,
		Plus,
		X,
		Info,
		QrCode as QrIcon
	} from '@lucide/svelte';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import Switch from '$lib/components/ui/Switch.svelte';
	import QrCode from '$lib/components/viz/QrCode.svelte';
	import { api } from '$lib/api/client';
	import type { TlsStatus } from '$lib/api/types';
	import { app } from '$lib/stores/app.svelte';
	import { confirm, toasts } from '$lib/stores/toasts.svelte';
	import { tlsApi } from '$lib/sensing/api';

	let st = $state<TlsStatus | null>(null);
	let loadError = $state<string | null>(null);
	let busy = $state(false);
	let newName = $state('');
	let poll: ReturnType<typeof setInterval> | undefined;

	const settings = $derived(app.show?.settings.https ?? { enabled: true, extraNames: [] });
	const extra = $derived(settings.extraNames ?? []);

	async function load() {
		try {
			st = await tlsApi.status();
			loadError = null;
		} catch (e) {
			loadError = (e as Error).message;
		}
	}

	onMount(() => {
		void load();
		// Keeps the fingerprint on the controller's screen and follows re-issues.
		poll = setInterval(load, 15_000);
	});
	onDestroy(() => clearInterval(poll));

	async function save(patch: { enabled?: boolean; extraNames?: string[] }) {
		busy = true;
		try {
			await api.saveSettings({ https: { ...patch } });
			await app.reloadShow();
			// The certificate follows within a moment.
			setTimeout(load, 1200);
		} catch (e) {
			toasts.error("Couldn't save", (e as Error).message);
		} finally {
			busy = false;
		}
	}

	function addName() {
		const n = newName.trim().toLowerCase().replace(/\.$/, '');
		if (!n) return;
		if (!/^[a-z0-9-]+(\.[a-z0-9-]+)*$/.test(n) && !/^[0-9a-f:.]+$/.test(n)) {
			toasts.warn('Use a plain host name like lights.home.arpa');
			return;
		}
		if (!extra.includes(n)) void save({ extraNames: [...extra, n] });
		newName = '';
	}

	async function renew() {
		busy = true;
		try {
			st = await tlsApi.rotate(false);
			toasts.success('New certificate issued — phones keep working');
		} catch (e) {
			toasts.error("Couldn't renew the certificate", (e as Error).message);
		} finally {
			busy = false;
		}
	}

	async function reset() {
		const ok = await confirm({
			title: 'Reset the secure connection?',
			message:
				'This makes a brand-new certificate authority. Every phone that trusted this show has to install the new certificate again (open /trust on each phone).',
			confirmLabel: 'Reset',
			danger: true
		});
		if (!ok) return;
		busy = true;
		try {
			st = await tlsApi.rotate(true);
			toasts.success('Secure connection reset — trust your phones again');
		} catch (e) {
			toasts.error("Couldn't reset", (e as Error).message);
		} finally {
			busy = false;
		}
	}

	async function copyFp() {
		try {
			await navigator.clipboard.writeText(st?.caFingerprint ?? '');
			toasts.success('Fingerprint copied');
		} catch {
			toasts.warn("Couldn't copy on this browser");
		}
	}

	/** Plain-http trust page address for phones on the LAN (QR code). */
	const trustUrl = $derived.by(() => {
		const lan = st?.urls.lan ?? [];
		const ipUrl = lan.find((u) => /^https:\/\/\d/.test(u));
		if (ipUrl) {
			const host = ipUrl.replace(/^https:\/\//, '').replace(/:\d+$/, '');
			const httpPort = location.protocol === 'http:' && location.port ? `:${location.port}` : '';
			return `http://${host}${httpPort}/trust`;
		}
		return `${location.protocol}//${location.host}/trust`;
	});

	const date = (s?: string | null) =>
		s ? new Date(s).toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' }) : '—';
</script>

<svelte:head><title>Secure connection · Settings · PixelPlus</title></svelte:head>

<div class="page https">
	<a class="btn ghost sm back" href="/settings"><ArrowLeft size={16} /> Settings</a>
	<PageHeader
		title="Secure connection"
		subtitle="HTTPS lets phones use their camera and microphone with PixelPlus — for syncing lights to sound and mapping your yard."
	/>

	{#if loadError && !st}
		<div class="notice danger">
			<TriangleAlert size={18} />
			<div>{loadError}</div>
		</div>
	{:else if !st}
		<div class="card skeleton sk"></div>
	{:else if st.role === 'follower'}
		<div class="notice info">
			<Info size={18} />
			<div>
				This controller follows your show leader. Phones use the leader's secure connection; manage it there.
			</div>
		</div>
	{:else}
		<section class="card">
			<div class="card-head">
				<ShieldCheck size={18} />
				<h2 class="grow">Secure connection (HTTPS)</h2>
				<Switch
					label="Secure connection"
					checked={settings.enabled}
					disabled={busy}
					onchange={(v) => save({ enabled: v })}
				/>
			</div>
			<div class="card-body col status">
				{#if !settings.enabled}
					<p class="muted">
						Off. Camera and microphone tools will only work through Tailscale or a Cloudflare address.
					</p>
				{:else if st.listening}
					<p class="row good">
						<Check size={16} /> Running on port {st.port}. {st.secureNow ? 'This page is using it.' : ''}
					</p>
				{:else if st.error}
					<p class="row bad"><TriangleAlert size={16} /> Not running: {st.error}</p>
				{:else}
					<p class="muted">Starting…</p>
				{/if}
				<p class="faint small">
					Plain <span class="mono">http://</span> keeps working as before; other controllers and bookmarks aren't
					affected.
				</p>
			</div>
		</section>

		{#if st.caFingerprint}
			<section class="card">
				<div class="card-head">
					<QrIcon size={18} />
					<h2 class="grow">Trust a phone</h2>
				</div>
				<div class="card-body trustbox">
					<div class="qr"><QrCode text={trustUrl} size={148} /></div>
					<div class="col grow">
						<p>
							Scan with the phone's camera, or open <a class="mono" href="/trust"
								>{trustUrl.replace(/^https?:\/\//, '')}</a
							>
							on it, and follow the steps (about a minute, once per phone).
						</p>
						<div class="fpbox">
							<span class="faint small">Fingerprint — the phone shows the same code</span>
							<div class="row fprow">
								<span class="mono fp"
									><strong>{st.caFingerprint.slice(0, 17)}</strong>{st.caFingerprint.slice(17)}</span
								>
								<button class="btn icon sm ghost" onclick={copyFp} aria-label="Copy fingerprint"
									><Copy size={15} /></button
								>
							</div>
							<span class="faint small"
								>The controller's screen shows the first part while this page is open.</span
							>
						</div>
					</div>
				</div>
			</section>

			<section class="card">
				<div class="card-head"><h2 class="grow">Secure addresses</h2></div>
				<div class="card-body col">
					<ul class="urls">
						{#each st.urls.lan as u (u)}
							<li><a class="mono" href={u + '/calibrate'}>{u.replace(/^https:\/\//, '')}</a></li>
						{/each}
						{#if st.urls.tailscale}
							<li>
								<a class="mono" href={st.urls.tailscale}>{st.urls.tailscale.replace(/^https:\/\//, '')}</a>
								<span class="badge blue">Tailscale</span>
							</li>
						{/if}
						{#if st.urls.tunnel}
							<li>
								<a class="mono" href={st.urls.tunnel}>{st.urls.tunnel.replace(/^https:\/\//, '')}</a>
								<span class="badge purple">Cloudflare</span>
							</li>
						{/if}
					</ul>
					<p class="faint small">
						The certificate renews itself when the controller's address or name changes, and before it expires
						({date(st.leafNotAfter)}). Phones don't need to do anything.
					</p>
				</div>
			</section>

			<section class="card">
				<div class="card-head"><h2 class="grow">Other names</h2></div>
				<div class="card-body col">
					<p class="muted small">
						If your router gives the controller another name (like <span class="mono">lights.home.arpa</span
						>), add it so the certificate covers it. Only home-network names work:
						<span class="mono">.local</span>,
						<span class="mono">.lan</span>, <span class="mono">.home.arpa</span>,
						<span class="mono">.internal</span>.
					</p>
					{#if extra.length}
						<div class="row wrap chips">
							{#each extra as n (n)}
								<span class="chip" class:bad-chip={st.rejectedNames?.includes(n)}>
									<span class="mono">{n}</span>
									<button
										class="x"
										aria-label="Remove {n}"
										disabled={busy}
										onclick={() => save({ extraNames: extra.filter((x) => x !== n) })}><X size={13} /></button
									>
								</span>
							{/each}
						</div>
					{/if}
					{#if st.rejectedNames?.length}
						<p class="row bad small">
							<TriangleAlert size={14} />
							{st.rejectedNames.join(', ')}
							{st.rejectedNames.length === 1
								? "isn't a home-network name, so it can't be covered."
								: "aren't home-network names, so they can't be covered."}
							For a public domain use a Cloudflare or Tailscale address.
						</p>
					{/if}
					<form
						class="row add"
						onsubmit={(e) => {
							e.preventDefault();
							addName();
						}}
					>
						<input
							class="input grow"
							placeholder="lights.home.arpa"
							bind:value={newName}
							aria-label="Another name"
							autocapitalize="off"
						/>
						<button class="btn" type="submit" disabled={busy || !newName.trim()}
							><Plus size={16} /> Add</button
						>
					</form>
				</div>
			</section>

			<section class="card">
				<div class="card-head"><h2 class="grow">Certificate</h2></div>
				<div class="card-body col">
					<dl class="small">
						<dt>Authority</dt>
						<dd>{st.caSubject}</dd>
						<dt>Created</dt>
						<dd>{date(st.caCreatedAt)} · valid until {date(st.caNotAfter)}</dd>
						<dt>Covers</dt>
						<dd class="mono names">{st.leafNames.join(', ')}</dd>
						<dt>Renewed</dt>
						<dd>{date(st.leafIssuedAt)}</dd>
					</dl>
					<div class="row wrap actions">
						<button class="btn" onclick={renew} disabled={busy}><RefreshCw size={16} /> Renew now</button>
						<button class="btn danger" onclick={reset} disabled={busy}
							><RotateCcw size={16} /> Reset secure connection…</button
						>
					</div>
					<p class="faint small">
						Moving to a new controller? The <strong>controller transfer</strong> file carries this
						certificate, so phones keep trusting the replacement.
						<a
							href="https://developer.mozilla.org/docs/Web/Security/Secure_Contexts"
							target="_blank"
							rel="noopener">Why HTTPS? <ExternalLink size={12} /></a
						>
					</p>
				</div>
			</section>
		{/if}
	{/if}
</div>

<style>
	.https {
		max-width: 820px;
	}
	.back {
		margin-bottom: 8px;
	}
	.sk {
		height: 180px;
	}
	.col {
		display: flex;
		flex-direction: column;
		gap: 10px;
	}
	.status p {
		margin: 0;
	}
	.row {
		display: flex;
		align-items: center;
		gap: 8px;
	}
	.good {
		color: var(--green);
	}
	.bad {
		color: var(--red);
	}
	.trustbox {
		display: flex;
		gap: 20px;
		align-items: flex-start;
	}
	.qr {
		flex: 0 0 auto;
		padding: 6px;
		background: #fff;
		border-radius: var(--r-2);
	}
	.fpbox {
		display: flex;
		flex-direction: column;
		gap: 4px;
		padding: 10px 12px;
		border-radius: var(--r-2);
		background: var(--surface-2);
		border: 1px solid var(--border-2);
	}
	.fprow {
		align-items: flex-start;
	}
	.fp {
		overflow-wrap: anywhere;
		font-size: 12.5px;
		line-height: 1.6;
		flex: 1 1 auto;
		min-width: 0;
	}
	.fp strong {
		font-size: 14px;
	}
	.urls {
		list-style: none;
		margin: 0;
		padding: 0;
		display: grid;
		gap: 6px;
	}
	.urls li {
		display: flex;
		align-items: center;
		gap: 8px;
		min-height: 32px;
		overflow-wrap: anywhere;
	}
	.chips {
		gap: 6px;
	}
	.chip {
		display: inline-flex;
		align-items: center;
		gap: 4px;
	}
	.bad-chip {
		border-color: color-mix(in srgb, var(--red) 45%, transparent);
	}
	.x {
		background: none;
		border: 0;
		color: var(--text-3);
		cursor: pointer;
		display: grid;
		place-items: center;
		width: 24px;
		height: 24px;
		border-radius: 50%;
	}
	.x:hover {
		color: var(--text);
		background: var(--surface-3);
	}
	.add {
		gap: 8px;
	}
	dl {
		display: grid;
		grid-template-columns: auto 1fr;
		gap: 6px 16px;
		margin: 0;
	}
	dt {
		color: var(--text-3);
	}
	dd {
		margin: 0;
		min-width: 0;
	}
	.names {
		overflow-wrap: anywhere;
	}
	.actions {
		gap: 8px;
	}
	@media (max-width: 760px) {
		.trustbox {
			flex-direction: column;
			align-items: center;
		}
	}
</style>
