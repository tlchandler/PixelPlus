<!--
	Gate for pages that use the phone's camera or microphone (sound sync, yard mapping, receiver
	wizard). Browsers only allow those on a secure (https) page; when this page isn't, explain why
	in plain words and offer the ways in that this controller has, best first: Tailscale, the
	controller's own https address (after trusting it once, or with Chrome's "Proceed"), then a
	Cloudflare address.

	Usage:
	  <SecureGate need="camera" purpose="map your yard">
	    …page that calls getUserMedia…
	  </SecureGate>
-->
<script lang="ts">
	import type { Snippet } from 'svelte';
	import { onMount } from 'svelte';
	import { ShieldCheck, Lock, ExternalLink, ChevronRight, Globe, Info } from '@lucide/svelte';
	import { tlsApi, type PublicTls } from '$lib/sensing/api';
	import type { TlsStatus } from '$lib/api/types';
	import { secureOptions, type SecureOption } from '$lib/sensing/secure';

	let {
		need = 'camera and microphone',
		purpose,
		children
	}: {
		/** What the page needs, as the user would say it. */
		need?: 'camera' | 'microphone' | 'camera and microphone';
		/** What it's for ("measure the sound delay"), completing "…to {purpose}". */
		purpose?: string;
		children: Snippet;
	} = $props();

	const secure = typeof window !== 'undefined' && window.isSecureContext;
	const hasMedia = typeof navigator !== 'undefined' && !!navigator.mediaDevices?.getUserMedia;

	let loading = $state(!secure);
	let options = $state<SecureOption[]>([]);
	let problem = $state<'off' | 'follower' | 'none' | null>(null);
	let showWhy = $state(false);

	onMount(async () => {
		if (secure) return;
		let st: TlsStatus | null = null;
		let pub: PublicTls | null = null;
		try {
			st = await tlsApi.status();
		} catch {
			pub = await tlsApi.publicStatus().catch(() => null);
		}
		const r = secureOptions(st, pub, window.location);
		options = r.options;
		problem = r.problem;
		loading = false;
	});
</script>

{#if secure && hasMedia}
	{@render children()}
{:else if secure}
	<section class="card gate" aria-live="polite">
		<div class="halo"><Info size={26} strokeWidth={1.6} /></div>
		<h2>This browser can't use the {need}</h2>
		<p class="muted">Open this page in Chrome on your phone (or Safari on an iPhone) and try again.</p>
	</section>
{:else}
	<section class="card gate" aria-live="polite">
		<div class="halo"><Lock size={26} strokeWidth={1.6} /></div>
		<h2>Needs a secure connection</h2>
		<p class="muted lead">
			Phones only let web pages use the {need}{purpose ? ` to ${purpose}` : ''} on a secure (<span
				class="mono">https</span
			>) page. Open the secure version of this page:
		</p>

		{#if loading}
			<div class="skeleton opt-skel"></div>
		{:else}
			{#if options.length}
				<ul class="opts">
					{#each options as o, i (o.url)}
						<li>
							<a class="opt interactive" class:first={i === 0} href={o.url} rel="noopener">
								<span class="ico">
									{#if o.kind === 'lan'}<ShieldCheck size={18} />{:else}<Globe size={18} />{/if}
								</span>
								<span class="grow txt">
									<span class="title">{o.title}</span>
									<span class="faint small">{o.detail}</span>
								</span>
								<ChevronRight size={18} />
							</a>
						</li>
					{/each}
				</ul>
			{/if}
			{#if options.some((o) => o.kind === 'lan')}
				<div class="notice info small tip">
					<ShieldCheck size={16} />
					<div>
						<strong>First time on this phone?</strong> Chrome will warn that the connection isn't private.
						<a href={options.find((o) => o.kind === 'lan')?.trustUrl ?? '/trust'}>Make this phone trusted</a>
						once (about a minute), or tap <strong>Advanced → Proceed</strong> each time.
					</div>
				</div>
			{/if}
			{#if problem === 'off'}
				<div class="notice warn small">
					<Info size={16} />
					<div>
						The secure connection is turned off on this controller. Turn it on in
						<a href="/settings/https">Settings → Secure connection</a>.
					</div>
				</div>
			{:else if problem === 'follower'}
				<div class="notice warn small">
					<Info size={16} />
					<div>This controller follows your show leader. Open this page on the leader instead.</div>
				</div>
			{:else if problem === 'none'}
				<div class="notice warn small">
					<Info size={16} />
					<div>
						The secure connection isn't ready yet. Wait a minute and reload, or check
						<a href="/settings/https">Settings → Secure connection</a>.
					</div>
				</div>
			{/if}
		{/if}

		<button class="btn ghost sm why" onclick={() => (showWhy = !showWhy)} aria-expanded={showWhy}>
			{showWhy ? 'Hide details' : 'Why is this needed?'}
		</button>
		{#if showWhy}
			<p class="faint small why-text">
				Browsers protect your camera and microphone by only offering them to pages whose address they can
				verify. Your controller makes its own certificate for your home network, so nothing leaves your house.
				The picture and sound are analysed on this phone and never uploaded — only the result is saved.
				<a
					href="https://developer.mozilla.org/docs/Web/Security/Secure_Contexts"
					target="_blank"
					rel="noopener">Learn more <ExternalLink size={12} /></a
				>
			</p>
		{/if}
	</section>
{/if}

<style>
	.gate {
		max-width: 560px;
		margin: 0 auto;
		padding: var(--s-6) var(--s-5);
		display: flex;
		flex-direction: column;
		align-items: stretch;
		gap: 12px;
		text-align: left;
	}
	.halo {
		width: 56px;
		height: 56px;
		border-radius: 50%;
		display: grid;
		place-items: center;
		background: var(--accent-soft);
		color: var(--accent-text);
		margin-bottom: 4px;
	}
	h2 {
		font-size: 19px;
	}
	.lead {
		font-size: 14.5px;
		line-height: 1.55;
	}
	.opts {
		list-style: none;
		margin: 4px 0 0;
		padding: 0;
		display: grid;
		gap: 8px;
	}
	.opt {
		display: flex;
		align-items: center;
		gap: 12px;
		min-height: 60px;
		padding: 10px 14px;
		border-radius: var(--r-2);
		border: 1px solid var(--border-2);
		background: var(--surface-2);
		color: var(--text);
		text-decoration: none;
	}
	.opt.first {
		border-color: var(--accent-line);
		background: var(--accent-soft);
	}
	.ico {
		width: 34px;
		height: 34px;
		border-radius: 50%;
		display: grid;
		place-items: center;
		background: var(--surface-3);
		color: var(--accent-text);
		flex: 0 0 auto;
	}
	.txt {
		display: flex;
		flex-direction: column;
		gap: 2px;
		min-width: 0;
	}
	.txt .small {
		overflow-wrap: anywhere;
	}
	.title {
		font-weight: 600;
	}
	.tip {
		margin-top: 4px;
	}
	.why {
		align-self: flex-start;
		margin-top: 4px;
	}
	.why-text {
		line-height: 1.55;
	}
	.opt-skel {
		height: 60px;
		border-radius: var(--r-2);
	}
	@media (max-width: 760px) {
		.gate {
			padding: var(--s-5) var(--s-4);
		}
	}
</style>
