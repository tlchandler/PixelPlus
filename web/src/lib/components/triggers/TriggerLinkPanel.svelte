<!--
	"Connect Home Assistant & other devices" for an HTTP trigger (ARCHITECTURE §12.18): makes a
	secret link (token shown once, with copy buttons, a Home Assistant rest_command, a curl
	example and a QR code), rotates / revokes it, and the two switches that widen where it
	works (from the internet, plain GET). Token fields come from the saved show (`show`): the
	server owns them. The switches edit `trigger` and call `onchange` (the page autosaves).
-->
<script lang="ts">
	import {
		Link2,
		Copy,
		RefreshCw,
		Ban,
		ChevronDown,
		KeyRound,
		House,
		Globe,
		TriangleAlert,
		Check,
		QrCode as QrIcon,
		Info
	} from '@lucide/svelte';
	import type { Show, Trigger, TriggerLinks, TriggerTokenResult } from '$lib/api/types';
	import { sensorsApi } from '$lib/insight/api';
	import { app } from '$lib/stores/app.svelte';
	import { confirm, toasts } from '$lib/stores/toasts.svelte';
	import { isEnabled } from '$lib/features';
	import { fmtRelative } from '$lib/util/format';
	import Switch from '$lib/components/ui/Switch.svelte';
	import Segmented from '$lib/components/ui/Segmented.svelte';
	import QrCode from '$lib/components/viz/QrCode.svelte';
	import {
		TOKEN_PLACEHOLDER,
		curlCommand,
		fallbackAddress,
		homeAssistantAction,
		homeAssistantYaml,
		hookUrl,
		usableAddresses,
		withToken
	} from './links';

	let { trigger: t, show, onchange }: { trigger: Trigger; show: Show; onchange: () => void } = $props();

	/** The trigger as the controller has it (token fields are the server's). */
	const saved = $derived(show.settings.triggers.find((x) => x.id === t.id && x.kind === 'http'));
	const hasLink = $derived(!!saved?.tokenHint);

	let open = $state(false);
	let fresh = $state<TriggerTokenResult | null>(null);
	let links = $state<TriggerLinks | null>(null);
	let addr = $state('');
	let tab = $state<'ha' | 'curl' | 'url'>('ha');
	let busy = $state(false);
	let showQr = $state(false);

	const addresses = $derived.by(() => {
		const list = usableAddresses(links?.addresses ?? [], !!t.allowInternet);
		return list.length ? list : [fallbackAddress(typeof location === 'undefined' ? '' : location.origin)];
	});
	const internetAvailable = $derived((links?.addresses ?? []).some((a) => a.kind === 'internet'));
	const current = $derived(addresses.find((a) => a.base === addr) ?? addresses[0]);
	$effect(() => {
		// Keep the address picker on a listed address (the list changes with "from the internet").
		if (!addresses.some((a) => a.base === addr)) addr = addresses[0].base;
	});
	const url = $derived(hookUrl(current.base, t.id));
	const https = $derived(current.base.startsWith('https://') && current.kind !== 'internet');
	const token = $derived(fresh?.token ?? TOKEN_PLACEHOLDER);
	const snippet = $derived(
		tab === 'ha'
			? homeAssistantYaml(t.name, url, token, { https })
			: tab === 'curl'
				? curlCommand(url, token, { https })
				: withToken(url, token)
	);
	const lastUse = $derived(links?.links[t.id]);
	const mqttButton = $derived(
		isEnabled('mqtt') && !!show.settings.mqtt?.enabled && !!show.settings.mqtt?.homeAssistantDiscovery
	);

	async function loadLinks() {
		try {
			links = await sensorsApi.triggerLinks();
		} catch {
			links = { addresses: [], links: {} };
		}
	}

	function toggle() {
		open = !open;
		if (open && !links) void loadLinks();
	}

	async function generate(rotate: boolean) {
		if (rotate) {
			const ok = await confirm({
				title: 'Make a new link?',
				message:
					'The current link stops working right away. Paste the new one into Home Assistant, your doorbell or anything else that uses it.',
				confirmLabel: 'Make a new link'
			});
			if (!ok) return;
		}
		busy = true;
		try {
			fresh = await sensorsApi.makeTriggerToken(t.id);
			open = true;
			showQr = false;
			await Promise.all([app.reloadShow(), loadLinks()]);
			toasts.success(rotate ? 'New link ready — the old one no longer works' : 'Link ready');
		} catch (e) {
			toasts.error("Couldn't make the link", (e as Error).message);
		} finally {
			busy = false;
		}
	}

	async function revoke() {
		const ok = await confirm({
			title: 'Turn off this link?',
			message: 'Anything that uses it (Home Assistant, a doorbell…) stops working until you make a new one.',
			confirmLabel: 'Turn off link',
			danger: true
		});
		if (!ok) return;
		busy = true;
		try {
			await sensorsApi.revokeTriggerToken(t.id);
			fresh = null;
			await Promise.all([app.reloadShow(), loadLinks()]);
			toasts.success('Link turned off');
		} catch (e) {
			toasts.error("Couldn't turn it off", (e as Error).message);
		} finally {
			busy = false;
		}
	}

	async function copy(text: string, what: string) {
		try {
			await navigator.clipboard.writeText(text);
			toasts.success(`${what} copied`);
		} catch {
			toasts.warn("Couldn't copy on this browser — select the text and copy it by hand");
		}
	}

	function setAllow(key: 'allowInternet' | 'allowGet', on: boolean) {
		if (on) t[key] = true;
		else delete t[key];
		onchange();
	}

	const madeOn = $derived(
		saved?.tokenCreatedAt
			? new Date(saved.tokenCreatedAt).toLocaleDateString(undefined, {
					month: 'short',
					day: 'numeric',
					year: 'numeric'
				})
			: ''
	);
</script>

<section class="lp" class:open data-testid="trigger-link-panel">
	<button class="head" onclick={toggle} aria-expanded={open}>
		<span class="ic" aria-hidden="true"><Link2 size={16} /></span>
		<span class="grow txt">
			<b>Connect Home Assistant &amp; other devices</b>
			<span class="small muted sub">
				{#if !saved}
					Saving the trigger…
				{:else if hasLink}
					Secret link on · ends in <span class="mono">…{saved.tokenHint}</span>
					{#if t.allowInternet}· works from the internet{:else}· home network only{/if}
				{:else}
					Run it from a doorbell, a Stream Deck or a smart-home system
				{/if}
			</span>
		</span>
		<ChevronDown size={16} class={open ? 'rot' : ''} />
	</button>

	{#if open}
		<div class="body">
			{#if mqttButton}
				<div class="notice info small">
					<Info size={16} />
					<div>
						<b>Using Home Assistant with MQTT?</b> This trigger is already a button there (“Trigger: {t.name}”)
						— you don't need a link for it.
					</div>
				</div>
			{/if}

			{#if !hasLink}
				<p class="small lead">
					Make a <b>secret link</b> that runs this trigger. Home Assistant, a video doorbell or a Stream Deck can
					call it without signing in. Anyone who has the link can run the trigger, so keep it private.
				</p>
				<div>
					<button class="btn primary" disabled={!saved || busy} onclick={() => generate(false)}>
						<KeyRound size={16} /> Make a secret link
					</button>
				</div>
			{:else}
				{#if fresh}
					<div class="once" role="status">
						<TriangleAlert size={16} />
						<span class="small"
							><b>Copy it now.</b> For your safety the token is shown only this once. Lost it? Make a new link.</span
						>
					</div>
				{/if}

				{#if addresses.length > 1}
					<label class="fld small">
						<span class="muted">Address</span>
						<select class="select sm" bind:value={addr} aria-label="Address to use">
							{#each addresses as a (a.base)}<option value={a.base}
									>{a.label}{a.kind === 'name' ? ' (recommended)' : ''}</option
								>{/each}
						</select>
					</label>
				{/if}

				<div class="kv">
					<span class="k small muted">Link</span>
					<code class="v mono" data-testid="link-url">{url}</code>
					<button class="btn icon sm ghost" aria-label="Copy link" onclick={() => copy(url, 'Link')}
						><Copy size={14} /></button
					>
					<span class="k small muted">Token</span>
					{#if fresh}
						<code class="v mono tok" data-testid="link-token">{fresh.token}</code>
						<button
							class="btn icon sm ghost"
							aria-label="Copy token"
							onclick={() => copy(fresh!.token, 'Token')}><Copy size={14} /></button
						>
					{:else}
						<span class="v small muted"
							>Hidden · ends in <span class="mono">…{saved?.tokenHint}</span>{#if madeOn}, made {madeOn}{/if}</span
						>
						<span></span>
					{/if}
				</div>

				<div class="snip">
					<Segmented
						size="sm"
						label="Example"
						bind:value={tab}
						options={[
							{ value: 'ha', label: 'Home Assistant' },
							{ value: 'curl', label: 'curl' },
							{ value: 'url', label: 'One link' }
						]}
					/>
					<div class="code">
						<pre class="mono" data-testid="link-snippet">{snippet}</pre>
						<button class="btn sm copy" onclick={() => copy(snippet, 'Example')}
							><Copy size={14} /> Copy</button
						>
					</div>
					<p class="tiny faint hint">
						{#if tab === 'ha'}
							Add this to <span class="mono">configuration.yaml</span>, restart Home Assistant, then use
							<span class="mono">{homeAssistantAction(t.name)}</span> in an automation.
						{:else if tab === 'curl'}
							Sends the token in the <span class="mono">Authorization</span> header — the safest way.
						{:else}
							For devices that can only call a web address. The token is part of the address, so it can end up
							in their logs.{#if !t.allowGet}
								It needs POST unless you allow simple GET links below.{/if}
						{/if}
						{#if !fresh}<b>Replace {TOKEN_PLACEHOLDER} with your token.</b>{/if}
					</p>
				</div>

				{#if fresh}
					<div class="qr-row">
						<button class="btn sm ghost" onclick={() => (showQr = !showQr)} aria-expanded={showQr}
							><QrIcon size={14} /> {showQr ? 'Hide' : 'Show'} QR code</button
						>
						{#if showQr}
							<div class="qr">
								<QrCode text={withToken(url, fresh.token)} size={168} />
								<span class="tiny faint">Scan to copy the whole link on a phone or tablet.</span>
							</div>
						{/if}
					</div>
				{/if}

				<div class="last small" data-testid="link-last-use">
					{#if lastUse}
						{#if lastUse.origin === 'internet'}<Globe size={14} />{:else}<House size={14} />{/if}
						<span>
							Last used {fmtRelative(lastUse.at)} from <span class="mono">{lastUse.from}</span>
							({lastUse.origin === 'internet' ? 'the internet' : 'home network'}) ·
							{#if lastUse.fired}<span class="good"><Check size={12} /> ran</span>{:else}<span class="muted"
									>didn't run: {lastUse.message}</span
								>{/if}
						</span>
					{:else}
						<span class="muted">Not used yet.</span>
					{/if}
				</div>

				<div class="opts">
					<div class="opt">
						<Switch
							size="sm"
							label="Allow from the internet"
							checked={!!t.allowInternet}
							onchange={(v) => setAllow('allowInternet', v)}
						/>
						<div class="small">
							<b>Allow from the internet</b>
							<div class="muted">
								Off: only devices on your home network can use it.
								{#if t.allowInternet}
									<span class="warn-text"
										>Anyone on the internet who gets hold of the link can run this trigger.</span
									>
									{#if !internetAvailable}
										It works once <a href="/settings/remote">Remote access</a> has a public address.
									{/if}
								{/if}
							</div>
						</div>
					</div>
					<div class="opt">
						<Switch
							size="sm"
							label="Allow simple GET links"
							checked={!!t.allowGet}
							onchange={(v) => setAllow('allowGet', v)}
						/>
						<div class="small">
							<b>Allow simple GET links</b>
							<div class="muted">
								For doorbells that can only open a web address.
								{#if t.allowGet}
									<span class="warn-text"
										>Chat apps and browsers sometimes open links on their own (to show a preview), which would
										set it off. Don't paste the link into messages.</span
									>
								{/if}
							</div>
						</div>
					</div>
				</div>

				<div class="row wrap acts">
					<button class="btn sm" disabled={busy} onclick={() => generate(true)}
						><RefreshCw size={14} /> Make a new link</button
					>
					<button class="btn sm danger" disabled={busy} onclick={revoke}
						><Ban size={14} /> Turn off link</button
					>
					{#if fresh}
						<button class="btn sm ghost" onclick={() => (fresh = null)}>Done — hide the token</button>
					{/if}
				</div>
			{/if}
		</div>
	{/if}
</section>

<style>
	.lp {
		border: 1px solid var(--border-2);
		border-radius: var(--r-2);
		background: var(--surface);
		min-width: 0;
	}
	.head {
		display: flex;
		align-items: center;
		gap: 10px;
		width: 100%;
		padding: 10px 12px;
		background: none;
		border: 0;
		color: inherit;
		text-align: left;
		cursor: pointer;
		min-height: 44px;
	}
	.head :global(.rot) {
		transform: rotate(180deg);
	}
	.ic {
		display: inline-flex;
		color: var(--accent-text);
	}
	.txt {
		display: flex;
		flex-direction: column;
		min-width: 0;
	}
	.sub {
		overflow-wrap: anywhere;
	}
	.body {
		display: flex;
		flex-direction: column;
		gap: 12px;
		padding: 0 12px 12px;
		min-width: 0;
	}
	.lead {
		margin: 0;
		max-width: 60ch;
	}
	.once {
		display: flex;
		gap: 8px;
		align-items: flex-start;
		padding: 8px 10px;
		border-radius: var(--r-2);
		background: var(--accent-soft);
		color: var(--accent-text);
	}
	.fld {
		display: flex;
		flex-direction: column;
		gap: 4px;
		max-width: 340px;
	}
	.kv {
		display: grid;
		grid-template-columns: auto minmax(0, 1fr) auto;
		gap: 6px 10px;
		align-items: center;
	}
	.kv .v {
		overflow-wrap: anywhere;
		word-break: break-all;
		padding: 6px 8px;
		border-radius: var(--r-1);
		background: var(--surface-2);
		font-size: 12.5px;
	}
	.kv span.v {
		background: none;
		padding-left: 0;
		word-break: normal;
	}
	.tok {
		color: var(--accent-text);
	}
	.snip {
		display: flex;
		flex-direction: column;
		gap: 8px;
		min-width: 0;
	}
	.code {
		position: relative;
		min-width: 0;
	}
	.code pre {
		margin: 0;
		padding: 12px 12px 44px;
		border-radius: var(--r-2);
		background: var(--surface-3);
		font-size: 12px;
		line-height: 1.5;
		white-space: pre;
		overflow-x: auto;
		max-width: 100%;
	}
	.code .copy {
		position: absolute;
		right: 8px;
		bottom: 8px;
	}
	.hint {
		margin: 0;
		overflow-wrap: anywhere;
	}
	.hint .mono {
		word-break: break-all;
	}
	.snip :global(.seg) {
		max-width: 100%;
		overflow-x: auto;
	}
	.qr-row {
		display: flex;
		flex-direction: column;
		gap: 8px;
		align-items: flex-start;
	}
	.qr {
		display: flex;
		align-items: center;
		gap: 12px;
		flex-wrap: wrap;
	}
	.last {
		display: flex;
		gap: 8px;
		align-items: flex-start;
		overflow-wrap: anywhere;
	}
	.last :global(svg) {
		flex: none;
		margin-top: 2px;
	}
	.good {
		color: var(--green);
		display: inline-flex;
		align-items: center;
		gap: 2px;
	}
	.opts {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(240px, 1fr));
		gap: 12px 16px;
	}
	.opt {
		display: flex;
		gap: 10px;
		align-items: flex-start;
	}
	.opt :global(.switch) {
		flex: none;
		margin-top: 2px;
	}
	.warn-text {
		color: var(--accent-text);
	}
	.acts {
		gap: 8px;
	}
	@media (max-width: 600px) {
		.kv {
			grid-template-columns: minmax(0, 1fr) auto;
		}
		.kv .k {
			grid-column: 1 / -1;
			margin-bottom: -4px;
		}
	}
</style>
