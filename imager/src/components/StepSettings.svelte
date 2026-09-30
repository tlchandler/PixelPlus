<script lang="ts">
	import type { Defaults, FieldError, ImagerSettings } from '../lib/types';
	import { errorFor, normalizeHostname } from '../lib/validate';
	import { countryOptions } from '../lib/countries';

	let {
		settings = $bindable(),
		defaults,
		errors
	}: { settings: ImagerSettings; defaults: Defaults; errors: FieldError[] } = $props();

	let showPw = $state(false);
	let advanced = $state(settings.ssh || !!settings.uiPassword);
	let touched = $state<Record<string, boolean>>({});
	const countries = countryOptions();
	const intlZones: string[] =
		(Intl as unknown as { supportedValuesOf?: (k: string) => string[] }).supportedValuesOf?.('timeZone') ?? [];
	const zones = $derived.by(() => {
		const list = defaults.timezones.length ? defaults.timezones : intlZones;
		const tz = settings.timezone || 'UTC';
		return list.includes(tz) ? list : [tz, ...list];
	});

	const err = (f: keyof ImagerSettings) => (touched[f] ? errorFor(errors, f) : undefined);
	const touch = (f: string) => () => (touched[f] = true);
	const host = $derived(normalizeHostname(settings.hostname) || 'pixelplus');
</script>

<section>
	<h2>Settings</h2>
	<p class="hint">
		Saved on the card as <code>pixelplus.txt</code> and applied the first time the Pi starts. You can change all of this
		later in the PixelPlus app. Skip Wi-Fi if you use a network cable or want to pick the network from your phone.
	</p>

	<div class="card block">
		<h3>Wi-Fi</h3>
		<div class="grid2">
			<label class="field">
				Network name (SSID)
				<input
					bind:value={settings.wifiSsid}
					onblur={touch('wifiSsid')}
					placeholder="e.g. MyHome"
					autocomplete="off"
					spellcheck="false"
					aria-invalid={!!err('wifiSsid')}
				/>
				{#if err('wifiSsid')}<span class="err">{err('wifiSsid')}</span>{/if}
			</label>
			<label class="field">
				Password
				<div class="pw">
					<input
						type={showPw ? 'text' : 'password'}
						bind:value={settings.wifiPassword}
						onblur={touch('wifiPassword')}
						placeholder={settings.wifiSsid ? 'Wi-Fi password' : ''}
						autocomplete="off"
						spellcheck="false"
						aria-invalid={!!err('wifiPassword')}
					/>
					<button type="button" class="eye" onclick={() => (showPw = !showPw)} aria-pressed={showPw}>
						{showPw ? 'Hide' : 'Show'}
					</button>
				</div>
				{#if err('wifiPassword')}<span class="err">{err('wifiPassword')}</span>{/if}
			</label>
			<label class="field">
				Country
				<select bind:value={settings.wifiCountry} onblur={touch('wifiCountry')} aria-invalid={!!err('wifiCountry')}>
					<option value="">Choose…</option>
					{#each countries as c (c.code)}<option value={c.code}>{c.name}</option>{/each}
				</select>
				{#if err('wifiCountry')}<span class="err">{err('wifiCountry')}</span>{:else}<span class="hint"
						>Sets the legal Wi-Fi channels.</span
					>{/if}
			</label>
			<label class="check">
				<input type="checkbox" bind:checked={settings.wifiHidden} />
				Hidden network
			</label>
		</div>
		<p class="hint">
			No Wi-Fi here? If the Pi can't connect it opens its own network <strong>PixelPlus-XXXX</strong> (password
			<code>pixelplus</code>) - join it with your phone and pick your Wi-Fi.
		</p>
	</div>

	<div class="card block">
		<h3>This controller</h3>
		<div class="grid2">
			<label class="field">
				Name
				<input
					bind:value={settings.hostname}
					onblur={touch('hostname')}
					autocomplete="off"
					spellcheck="false"
					aria-invalid={!!err('hostname')}
				/>
				{#if err('hostname')}<span class="err">{err('hostname')}</span>{:else}<span class="hint"
						>Open it at <strong>http://{host}.local</strong></span
					>{/if}
			</label>
			<label class="field">
				Time zone
				<select bind:value={settings.timezone}>
					{#each zones as z (z)}<option value={z}>{z.replaceAll('_', ' ')}</option>{/each}
				</select>
				<span class="hint">Detected from this computer.</span>
			</label>
		</div>
		<div class="field" role="radiogroup" aria-label="Role">
			<span>Role</span>
			<div class="roles">
				<button
					type="button"
					class="option"
					role="radio"
					aria-checked={settings.role === 'leader'}
					onclick={() => (settings.role = 'leader')}
				>
					<div>
						<strong>Show leader</strong>
						<div class="hint">Your main controller: holds the show, plays the music. You need exactly one.</div>
					</div>
				</button>
				<button
					type="button"
					class="option"
					role="radio"
					aria-checked={settings.role === 'follower'}
					onclick={() => (settings.role = 'follower')}
				>
					<div>
						<strong>Follower</strong>
						<div class="hint">An extra controller, set up from the leader. Give it its own name!</div>
					</div>
				</button>
			</div>
		</div>
	</div>

	<details class="card block" bind:open={advanced}>
		<summary><h3>Security & remote login <span class="hint">(optional)</span></h3></summary>
		<div class="grid2">
			<label class="field">
				Password for the PixelPlus web page
				<input
					type="password"
					bind:value={settings.uiPassword}
					onblur={touch('uiPassword')}
					placeholder="None"
					autocomplete="new-password"
					aria-invalid={!!err('uiPassword')}
				/>
				{#if err('uiPassword')}<span class="err">{err('uiPassword')}</span>{:else}<span class="hint"
						>Leave empty on a trusted home network.</span
					>{/if}
			</label>
			<label class="check">
				<input type="checkbox" bind:checked={settings.ssh} onchange={touch('sshPassword')} />
				Enable SSH (for advanced users)
			</label>
			{#if settings.ssh}
				<label class="field">
					SSH password for user <code>pi</code>
					<input
						type="password"
						bind:value={settings.sshPassword}
						onblur={touch('sshPassword')}
						autocomplete="new-password"
						aria-invalid={!!err('sshPassword')}
					/>
					{#if err('sshPassword')}<span class="err">{err('sshPassword')}</span>{/if}
				</label>
				<label class="field">
					…or public key
					<input
						bind:value={settings.sshKey}
						onblur={touch('sshKey')}
						placeholder="ssh-ed25519 AAAA…"
						spellcheck="false"
						aria-invalid={!!err('sshKey')}
					/>
					{#if err('sshKey')}<span class="err">{err('sshKey')}</span>{/if}
				</label>
			{/if}
		</div>
	</details>
</section>

<style>
	h2 {
		margin: 0 0 4px;
		font-size: 20px;
	}
	h3 {
		margin: 0 0 12px;
		font-size: 14px;
		display: inline;
	}
	.block {
		padding: 16px;
		margin-top: 16px;
		display: grid;
		gap: 12px;
	}
	.pw {
		position: relative;
	}
	.pw input {
		padding-right: 64px;
	}
	.eye {
		position: absolute;
		right: 4px;
		top: 4px;
		height: 32px;
		padding: 0 10px;
		border: 0;
		border-radius: 8px;
		background: transparent;
		color: var(--text-2);
		cursor: pointer;
	}
	.check {
		display: flex;
		align-items: center;
		gap: 8px;
		align-self: end;
		min-height: 40px;
		color: var(--text-2);
	}
	.check input {
		width: 18px;
		min-height: 18px;
		accent-color: var(--accent);
	}
	.roles {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: 10px;
	}
	summary {
		cursor: pointer;
		list-style: none;
	}
	summary::-webkit-details-marker {
		display: none;
	}
	summary::before {
		content: '▸ ';
		color: var(--text-3);
	}
	details[open] summary::before {
		content: '▾ ';
	}
	code {
		font-size: 12px;
		padding: 1px 5px;
		border-radius: 5px;
		background: var(--surface-3);
	}
</style>
