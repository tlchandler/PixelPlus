<script lang="ts">
	import { app } from '$lib/stores/app.svelte';
	import QrCode from '$lib/components/viz/QrCode.svelte';
	import { requestLink, fmStation, prettyUrl } from '$lib/util/visitors';
	import { ArrowLeft, Printer, Radio, Smartphone, TriangleAlert, Sparkles } from '@lucide/svelte';

	const show = $derived(app.show);
	const link = $derived(requestLink(show, location.origin));
	const station = $derived(fmStation(show?.settings.requests.radioFrequency));

	function saved(key: string, fallback: string) {
		try {
			return localStorage.getItem(key) ?? fallback;
		} catch {
			return fallback;
		}
	}
	let headline = $state(saved('pp-sign-headline', 'Pick the next song!'));
	let footer = $state(
		saved('pp-sign-footer', 'Please stay in your car and keep the driveway clear. Merry Christmas!')
	);
	function remember() {
		try {
			localStorage.setItem('pp-sign-headline', headline);
			localStorage.setItem('pp-sign-footer', footer);
		} catch {
			/* ignore */
		}
	}
</script>

<svelte:head><title>Yard sign · {show?.name ?? 'PixelPlus'}</title></svelte:head>

<div class="page signpage">
	<div class="tools">
		<a class="btn ghost" href="/settings#requests"><ArrowLeft size={16} /> Song requests</a>
		<span class="grow"></span>
		<button class="btn primary" onclick={() => window.print()}><Printer size={16} /> Print sign</button>
	</div>
	<h1 class="sr-only">Yard sign</h1>

	{#if !link.isPublic}
		<div class="notice warn small screen-only">
			<TriangleAlert size={16} class="ico" />
			<div>
				<strong>This QR code only works on your home Wi-Fi.</strong> Visitors on the street can’t reach it yet.
				<a href="/settings#requests">Add an internet address</a> first, then print.
			</div>
		</div>
	{/if}
	{#if !station}
		<div class="notice info small screen-only">
			<Radio size={16} />
			<div>
				Add your FM station in <a href="/settings#requests">Song requests</a> and it appears on the sign.
			</div>
		</div>
	{/if}

	<div class="edit screen-only">
		<label class="field grow"
			><span class="label">Headline</span><input
				class="input"
				bind:value={headline}
				oninput={remember}
				maxlength="40"
			/></label
		>
		<label class="field grow"
			><span class="label">Small print</span><input
				class="input"
				bind:value={footer}
				oninput={remember}
				maxlength="90"
			/></label
		>
	</div>

	<div class="paper-wrap">
		<article class="paper" aria-label="Sign preview">
			<div class="ribbon"></div>
			<p class="show">{show?.name ?? 'Our light show'}</p>
			<h2 class="headline">{headline}</h2>
			<div class="qr">
				<QrCode text={link.url} size={420} fg="#111" />
			</div>
			<p class="scan"><Smartphone size={26} /> Scan with your phone camera</p>
			<p class="url">{prettyUrl(link.url)}</p>
			{#if station}
				<div class="radio">
					<Radio size={40} />
					<div>
						<span class="tune">Tune your radio to</span>
						<strong class="freq">{station}</strong>
					</div>
				</div>
			{/if}
			<p class="small-print"><Sparkles size={16} /> {footer}</p>
		</article>
	</div>
</div>

<style>
	.tools {
		display: flex;
		align-items: center;
		gap: 8px;
		margin-bottom: 16px;
	}
	.signpage .notice {
		margin-bottom: 12px;
		max-width: 720px;
		margin-left: auto;
		margin-right: auto;
	}
	.notice a {
		color: var(--accent-text);
		font-weight: 600;
		text-decoration: underline;
	}
	.edit {
		display: flex;
		gap: 12px;
		max-width: 720px;
		margin: 0 auto 20px;
	}
	.paper-wrap {
		display: flex;
		justify-content: center;
	}
	/* US Letter portrait; prints edge to edge with @page margins. */
	.paper {
		position: relative;
		width: min(720px, 100%);
		aspect-ratio: 8.5 / 11;
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: space-between;
		padding: 7% 8% 6%;
		background: #fffdf8;
		color: #141414;
		border-radius: 6px;
		box-shadow:
			0 30px 80px rgba(0, 0, 0, 0.35),
			0 2px 8px rgba(0, 0, 0, 0.2);
		text-align: center;
		overflow: hidden;
		font-family: var(--font);
	}
	.ribbon {
		position: absolute;
		inset: 0 0 auto 0;
		height: 14px;
		background: repeating-linear-gradient(-45deg, #c21f2b 0 22px, #fffdf8 22px 44px, #1c7a45 44px 66px, #fffdf8 66px 88px);
	}
	.show {
		margin-top: 8px;
		font-family: Georgia, 'Times New Roman', serif;
		font-style: italic;
		font-size: clamp(18px, 4.2vw, 34px);
		color: #8c1720;
	}
	.headline {
		font-size: clamp(26px, 7vw, 60px);
		font-weight: 800;
		letter-spacing: -0.03em;
		line-height: 1.02;
		color: #111;
	}
	.qr {
		width: 62%;
		padding: 2.5%;
		border: 3px solid #111;
		border-radius: 18px;
		background: #fff;
	}
	.qr :global(svg) {
		width: 100%;
		height: auto;
	}
	.scan {
		display: inline-flex;
		align-items: center;
		gap: 10px;
		font-size: clamp(14px, 3vw, 26px);
		font-weight: 650;
	}
	.url {
		font-size: clamp(12px, 2.4vw, 20px);
		color: #444;
		font-family: var(--mono);
		word-break: break-all;
	}
	.radio {
		display: flex;
		align-items: center;
		gap: 16px;
		padding: 10px 26px;
		border-radius: 999px;
		background: #1c7a45;
		color: #fff;
		text-align: left;
	}
	.tune {
		display: block;
		font-size: clamp(12px, 2.2vw, 18px);
		font-weight: 600;
		opacity: 0.9;
	}
	.freq {
		display: block;
		font-size: clamp(22px, 5.4vw, 46px);
		font-weight: 800;
		letter-spacing: -0.02em;
		line-height: 1;
	}
	.small-print {
		display: inline-flex;
		align-items: center;
		gap: 8px;
		font-size: clamp(11px, 2vw, 16px);
		color: #555;
	}
	@media (max-width: 640px) {
		.edit {
			flex-direction: column;
		}
		.paper {
			padding: 8% 7% 6%;
		}
	}
	@media print {
		@page {
			size: letter portrait;
			margin: 0.4in;
		}
		:global(.desk),
		:global(.mob),
		:global(.transport),
		:global(.toaster),
		:global(.banner),
		:global(.lights-off),
		.tools,
		.screen-only {
			display: none !important;
		}
		:global(html),
		:global(body) {
			background: #fff !important;
		}
		:global(.main) {
			margin: 0 !important;
			padding: 0 !important;
		}
		.signpage {
			padding: 0 !important;
			max-width: none;
			animation: none;
		}
		.paper {
			width: 100%;
			height: 10.1in;
			aspect-ratio: auto;
			box-shadow: none;
			border-radius: 0;
			-webkit-print-color-adjust: exact;
			print-color-adjust: exact;
		}
		.headline {
			font-size: 58px;
		}
		.show {
			font-size: 32px;
		}
		.freq {
			font-size: 46px;
		}
	}
</style>
