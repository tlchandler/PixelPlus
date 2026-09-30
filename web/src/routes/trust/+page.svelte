<!--
	"Make this phone trusted" (F1, ARCHITECTURE §12.1). WS1. Public (no sign-in), reachable over
	plain http: phones download the show's certificate authority here once, so the controller's
	https pages (camera and microphone tools) open without warnings.
-->
<script lang="ts">
	import { onMount } from 'svelte';
	import { page } from '$app/state';
	import {
		ShieldCheck,
		Download,
		ArrowRight,
		Check,
		Info,
		Smartphone,
		TriangleAlert,
		Lock
	} from '@lucide/svelte';
	import Segmented from '$lib/components/ui/Segmented.svelte';
	import { initBackend } from '$lib/api/mode';
	import { tlsApi, type PublicTls } from '$lib/sensing/api';
	import { safeNext } from '$lib/sensing/secure';

	let status = $state<PublicTls | null>(null);
	let loading = $state(true);
	let failed = $state(false);
	/** Phone trust (HTTPS) is turned off on this controller (Settings → Features). */
	let off = $state(false);
	let platform = $state<'android' | 'ios'>('android');
	let downloaded = $state(false);

	const next = $derived(safeNext(page.url.searchParams.get('next'), '/calibrate'));
	const secureHere =
		typeof window !== 'undefined' && window.isSecureContext && location.protocol === 'https:';

	onMount(async () => {
		if (/iPhone|iPad|iPod/.test(navigator.userAgent)) platform = 'ios';
		try {
			// Public page: the app shell doesn't boot here, so pick the backend (demo or real).
			await initBackend();
			status = await tlsApi.publicStatus();
		} catch (e) {
			if ((e as { code?: string })?.code === 'feature_disabled') off = true;
			else failed = true;
		} finally {
			loading = false;
		}
	});

	/** The controller's https address for `next`, keeping the address the phone already uses. */
	const secureUrl = $derived.by(() => {
		if (!status) return null;
		const here = location.hostname.replace(/^\[|\]$/g, '').toLowerCase();
		const port = status.port === 443 ? '' : `:${status.port}`;
		if (status.leafNames.includes(here)) {
			const host = here.includes(':') ? `[${here}]` : here;
			return `https://${host}${port}${next}`;
		}
		return status.urls[0] ? status.urls[0].replace(/\/$/, '') + next : null;
	});

	/** Groups of the fingerprint; the first six bytes are what the controller's screen shows. */
	const fpShort = $derived(status?.caFingerprint?.slice(0, 17) ?? '');
	const fpRest = $derived(status?.caFingerprint?.slice(18) ?? '');
</script>

<svelte:head><title>Trust this phone · PixelPlus</title></svelte:head>

<main class="trust">
	<header class="head">
		<div class="logo"><ShieldCheck size={26} strokeWidth={1.7} /></div>
		<h1>Make this phone trusted</h1>
		<p class="muted">
			A one-time setup, about a minute. Afterwards your light show's camera and microphone tools open securely
			on this phone.
		</p>
	</header>

	{#if loading}
		<div class="card skeleton sk"></div>
	{:else if secureHere}
		<section class="card pad done">
			<div class="ok"><Check size={26} strokeWidth={2} /></div>
			<h2>This phone already trusts your show</h2>
			<p class="muted">You're on a secure connection. Nothing more to do.</p>
			<a class="btn primary lg" href={next}>Continue <ArrowRight size={18} /></a>
		</section>
	{:else if off}
		<div class="notice warn">
			<Info size={18} />
			<div>
				Secure phone connections are turned off on this controller. Turn on <strong
					>Phone trust (HTTPS)</strong
				>
				in Settings → Features, then open this page again.
			</div>
		</div>
	{:else if failed || !status}
		<div class="notice warn">
			<TriangleAlert size={18} />
			<div>
				Can't reach your controller right now. Check that this phone is on the same Wi-Fi, then reload.
			</div>
		</div>
	{:else if status.role === 'follower'}
		<div class="notice warn">
			<Info size={18} />
			<div>This controller follows your show leader. Open this page on the leader's address instead.</div>
		</div>
	{:else if !status.caFingerprint}
		<div class="notice warn">
			<Info size={18} />
			<div>
				The secure connection isn't set up yet. Turn it on in <strong>Settings → Secure connection</strong> on your
				controller, then come back.
			</div>
		</div>
	{:else}
		{#if !status.available}
			<div class="notice warn spaced">
				<Info size={18} />
				<div>
					The secure connection is turned off at the moment. You can still install the certificate now; turn
					it on in
					<strong>Settings → Secure connection</strong>.
				</div>
			</div>
		{/if}

		<section class="card pad">
			<h2 class="h"><Lock size={18} /> Check the code</h2>
			<p class="muted small">
				It should match <strong>Settings → Secure connection</strong> on your controller, or its little screen,
				which shows it while this page is open.
			</p>
			<div class="fp" aria-label="Certificate fingerprint">
				<div class="fp-short mono">{fpShort}</div>
				<div class="fp-rest mono faint">{fpRest}</div>
			</div>
		</section>

		<div class="platform">
			<Segmented
				label="Phone type"
				bind:value={platform}
				options={[
					{ value: 'android', label: 'Android', icon: Smartphone },
					{ value: 'ios', label: 'iPhone / iPad', icon: Smartphone }
				]}
			/>
		</div>

		{#if platform === 'android'}
			<ol class="steps">
				<li class="card pad step">
					<span class="n">1</span>
					<div class="grow">
						<h3>Download the certificate</h3>
						<a
							class="btn primary block"
							href={tlsApi.caUrl}
							download="PixelPlus-CA.crt"
							onclick={() => (downloaded = true)}
						>
							<Download size={18} /> Download certificate
						</a>
						{#if downloaded}<p class="faint small ok-line">
								<Check size={14} /> Saved to Downloads as PixelPlus-CA.crt
							</p>{/if}
					</div>
				</li>
				<li class="card pad step">
					<span class="n">2</span>
					<div class="grow">
						<h3>Install it</h3>
						<p>
							Open <strong>Settings</strong>, search for <strong>certificate</strong>, and choose
							<strong>CA certificate</strong> (under <em>Install a certificate</em>). Tap
							<strong>Install anyway</strong>, then pick <strong>PixelPlus-CA.crt</strong> from Downloads.
						</p>
						<details>
							<summary class="small">Where exactly?</summary>
							<ul class="small paths">
								<li>
									<strong>Pixel and most phones:</strong> Settings → Security &amp; privacy → More security settings
									→ Encryption &amp; credentials → Install a certificate → CA certificate
								</li>
								<li>
									<strong>Samsung:</strong> Settings → Security and privacy (or Biometrics and security) → Other
									security settings → Install from device storage → CA certificate
								</li>
							</ul>
						</details>
						<p class="faint small">
							Your phone needs a screen lock (PIN, pattern or fingerprint) to install certificates.
						</p>
					</div>
				</li>
				<li class="card pad step">
					<span class="n">3</span>
					<div class="grow">
						<h3>Open the secure page</h3>
						{#if secureUrl}
							<a class="btn primary block" href={secureUrl}>Open secure page <ArrowRight size={18} /></a>
							<p class="faint small addr mono">{secureUrl.replace(/^https:\/\//, '').replace(next, '')}</p>
						{:else}
							<p class="muted small">
								Open your controller's address with <span class="mono">https://</span> in front.
							</p>
						{/if}
					</div>
				</li>
			</ol>
			<p class="faint small tip">
				<strong>Firefox?</strong> It also needs: Settings → About Firefox (tap the logo five times) → Secret settings
				→ Use third-party CA certificates.
			</p>
		{:else}
			<ol class="steps">
				<li class="card pad step">
					<span class="n">1</span>
					<div class="grow">
						<h3>Download the profile</h3>
						<a class="btn primary block" href={tlsApi.mobileconfigUrl}
							><Download size={18} /> Download profile</a
						>
						<p class="faint small">Tap <strong>Allow</strong> when Safari asks.</p>
					</div>
				</li>
				<li class="card pad step">
					<span class="n">2</span>
					<div class="grow">
						<h3>Install it</h3>
						<p>Open <strong>Settings → Profile Downloaded → Install</strong> and enter your passcode.</p>
					</div>
				</li>
				<li class="card pad step">
					<span class="n">3</span>
					<div class="grow">
						<h3>Turn on full trust</h3>
						<p>
							<strong>Settings → General → About → Certificate Trust Settings</strong>, then switch on
							<strong>{status.caSubject ?? 'PixelPlus Local CA'}</strong>.
						</p>
					</div>
				</li>
				<li class="card pad step">
					<span class="n">4</span>
					<div class="grow">
						<h3>Open the secure page</h3>
						{#if secureUrl}
							<a class="btn primary block" href={secureUrl}>Open secure page <ArrowRight size={18} /></a>
						{:else}
							<p class="muted small">
								Open your controller's address with <span class="mono">https://</span> in front.
							</p>
						{/if}
					</div>
				</li>
			</ol>
		{/if}

		<section class="card pad hurry">
			<h2 class="h">In a hurry?</h2>
			<p class="muted small">
				Skip the install: open the secure page and, when the browser warns that the connection isn't private,
				tap
				<strong>Advanced → Proceed</strong>. You'll see that warning on each visit.
			</p>
			{#if secureUrl}<a class="btn" href={secureUrl}>Open secure page anyway</a>{/if}
		</section>

		<p class="faint small why">
			<strong>Is this safe?</strong> The certificate was made by your own controller and can only vouch for
			addresses on your home network (like <span class="mono">192.168.x.x</span> or
			<span class="mono">.local</span> names) — never for other web sites. You can remove it any time under Encryption
			&amp; credentials → User credentials.
		</p>
	{/if}
</main>

<style>
	.trust {
		max-width: 560px;
		margin: 0 auto;
		padding: 24px 16px 48px;
		min-height: 100vh;
		background: var(--bg);
		display: flex;
		flex-direction: column;
		gap: 14px;
	}
	.head {
		text-align: center;
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 8px;
		margin-bottom: 4px;
	}
	.logo,
	.ok {
		width: 56px;
		height: 56px;
		border-radius: 50%;
		display: grid;
		place-items: center;
		background: var(--accent-soft);
		color: var(--accent-text);
	}
	.ok {
		background: var(--green-soft);
		color: var(--green);
	}
	h1 {
		font-size: 24px;
	}
	h2 {
		font-size: 17px;
	}
	.h {
		display: flex;
		align-items: center;
		gap: 8px;
	}
	h3 {
		font-size: 15px;
		margin-bottom: 8px;
	}
	.pad {
		padding: 16px;
	}
	.done {
		display: flex;
		flex-direction: column;
		align-items: center;
		text-align: center;
		gap: 10px;
	}
	.sk {
		height: 200px;
	}
	.spaced {
		margin-bottom: 2px;
	}
	.fp {
		margin-top: 10px;
		padding: 12px;
		border-radius: var(--r-2);
		background: var(--surface-2);
		border: 1px solid var(--border-2);
		overflow-wrap: anywhere;
	}
	.fp-short {
		font-size: 20px;
		font-weight: 650;
		letter-spacing: 0.02em;
	}
	.fp-rest {
		font-size: 12px;
		margin-top: 4px;
		line-height: 1.6;
	}
	.platform {
		display: flex;
		justify-content: center;
	}
	.steps {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 10px;
	}
	.step {
		display: flex;
		gap: 12px;
		align-items: flex-start;
	}
	.step p {
		line-height: 1.55;
		margin: 0 0 6px;
	}
	.n {
		width: 28px;
		height: 28px;
		border-radius: 50%;
		display: grid;
		place-items: center;
		font-weight: 700;
		font-size: 14px;
		background: var(--accent);
		color: var(--accent-fg);
		flex: 0 0 auto;
	}
	.grow {
		min-width: 0;
	}
	.ok-line {
		display: flex;
		align-items: center;
		gap: 6px;
		margin-top: 6px;
		color: var(--green);
	}
	details summary {
		cursor: pointer;
		min-height: var(--touch);
		display: flex;
		align-items: center;
		color: var(--accent-text);
	}
	.paths {
		margin: 0 0 8px;
		padding-left: 18px;
		display: grid;
		gap: 6px;
		line-height: 1.5;
	}
	.addr {
		margin-top: 6px;
		overflow-wrap: anywhere;
	}
	.tip,
	.why {
		line-height: 1.55;
		padding: 0 4px;
	}
	.hurry {
		display: flex;
		flex-direction: column;
		gap: 8px;
		align-items: flex-start;
	}
</style>
