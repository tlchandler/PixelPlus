<!--
	Settings → Nightly report (F11, ARCHITECTURE §12.10). WS6.
	When the report is made (a morning time or right after the show), where it goes
	(email / push through the Alerts settings), only when there are problems, how long
	reports are kept; "Send a test report now".
-->
<script lang="ts">
	import { ArrowLeft, ClipboardList, Mail, Bell, Send, Info, ExternalLink } from '@lucide/svelte';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import Switch from '$lib/components/ui/Switch.svelte';
	import Segmented from '$lib/components/ui/Segmented.svelte';
	import SaveState from '$lib/components/ui/SaveState.svelte';
	import { api } from '$lib/api/client';
	import type { ReportSettings } from '$lib/api/types';
	import { app } from '$lib/stores/app.svelte';
	import { toasts } from '$lib/stores/toasts.svelte';
	import { reportsApi } from '$lib/insight/api';

	const DEFAULTS: ReportSettings = {
		enabled: true,
		time: '07:00',
		email: true,
		push: true,
		onlyWhenProblems: false,
		keepDays: 90
	};
	const current = $derived<ReportSettings>({ ...DEFAULTS, ...(app.show?.settings.reports ?? {}) });
	const alerts = $derived(app.show?.settings.alerts);
	const emailReady = $derived(!!alerts?.email?.smtpHost && !!alerts?.email?.to);
	const pushReady = $derived(!!alerts?.ntfy?.topic);

	let saving = $state<'saved' | 'saving'>('saved');
	let sending = $state(false);
	let when = $state<'morning' | 'after'>('morning');
	let morning = $state('07:00');
	$effect(() => {
		const t = current.time;
		when = t === 'afterShow' ? 'after' : 'morning';
		if (t !== 'afterShow') morning = t;
	});

	async function save(patch: Partial<ReportSettings>) {
		saving = 'saving';
		try {
			await api.saveSettings({ reports: { ...current, ...patch } });
			await app.reloadShow();
		} catch (e) {
			toasts.error("Couldn't save", (e as Error).message);
		} finally {
			saving = 'saved';
		}
	}

	async function sendTest() {
		sending = true;
		try {
			const r = await reportsApi.run(undefined, true);
			const lines = r.delivery ?? [];
			if (lines.some((l) => /failed|not set up/i.test(l))) toasts.warn(lines.join(' '));
			else toasts.success(lines.join(' ') || 'Report sent');
		} catch (e) {
			toasts.error("Couldn't send the report", (e as Error).message);
		} finally {
			sending = false;
		}
	}
</script>

<svelte:head><title>Nightly report · Settings · PixelPlus</title></svelte:head>

<div class="page rset">
	<a class="btn ghost sm back" href="/settings"><ArrowLeft size={16} /> Settings</a>
	<PageHeader
		title="Nightly report"
		subtitle="Every morning: shows, songs, requests, problems, temperatures and pixels to check — by email or push."
	>
		{#snippet actions()}<SaveState state={saving} />{/snippet}
	</PageHeader>

	<section class="card">
		<div class="card-head">
			<ClipboardList size={18} />
			<h2 class="grow">Make a report after each show night</h2>
			<Switch label="Nightly report" checked={current.enabled} onchange={(v) => save({ enabled: v })} />
		</div>
		<div class="card-body col">
			<div class="setting">
				<div class="text">
					<div class="title">When</div>
					<div class="desc">
						A show night runs from noon to noon, so a late show still counts as that evening.
					</div>
				</div>
				<Segmented
					label="When to make the report"
					size="sm"
					bind:value={when}
					options={[
						{ value: 'morning', label: 'Next morning' },
						{ value: 'after', label: 'Right after the show' }
					]}
					onchange={(v) => save({ time: v === 'after' ? 'afterShow' : morning })}
				/>
			</div>
			{#if when === 'morning'}
				<div class="setting">
					<div class="text"><div class="title">Time</div></div>
					<input
						class="input"
						style="width:130px"
						type="time"
						bind:value={morning}
						onchange={() => save({ time: morning || '07:00' })}
						aria-label="Report time"
					/>
				</div>
			{:else}
				<p class="faint small">15 minutes after the night's last show window ends.</p>
			{/if}
			<a class="btn ghost sm" href="/reports" style="align-self:flex-start"
				>See past reports <ExternalLink size={13} /></a
			>
		</div>
	</section>

	<section class="card">
		<div class="card-head">
			<Send size={18} />
			<h2 class="grow">Send it to me</h2>
		</div>
		<div class="card-body col">
			<div class="setting">
				<Mail size={18} />
				<div class="text grow">
					<div class="title">Email</div>
					<div class="desc">
						{emailReady ? `To ${alerts?.email?.to}` : 'Set up email in Settings → Alerts first.'}
					</div>
				</div>
				<Switch label="Email the report" checked={current.email} onchange={(v) => save({ email: v })} />
			</div>
			<div class="setting">
				<Bell size={18} />
				<div class="text grow">
					<div class="title">Push notification (ntfy)</div>
					<div class="desc">
						{pushReady
							? `Topic "${alerts?.ntfy?.topic}" — a short summary with a link, no addresses.`
							: 'Choose an ntfy topic in Settings → Alerts first.'}
					</div>
				</div>
				<Switch label="Push the report" checked={current.push} onchange={(v) => save({ push: v })} />
			</div>
			<div class="setting">
				<div class="text grow">
					<div class="title">Only when something needs attention</div>
					<div class="desc">Quiet nights are still saved on the Reports page.</div>
				</div>
				<Switch
					label="Only when there are problems"
					checked={current.onlyWhenProblems}
					onchange={(v) => save({ onlyWhenProblems: v })}
				/>
			</div>
			{#if !emailReady && !pushReady}
				<div class="notice info">
					<Info size={18} />
					<div>
						Reports are saved here either way. To receive them, set up email or ntfy in
						<a href="/settings#alerts">Settings → Alerts</a>.
					</div>
				</div>
			{/if}
			<div class="row wrap">
				<button class="btn" onclick={sendTest} disabled={sending || (!emailReady && !pushReady)}
					><Send size={16} /> Send last night's report now</button
				>
			</div>
		</div>
	</section>

	<section class="card">
		<div class="card-head"><h2 class="grow">Keep reports</h2></div>
		<div class="card-body">
			<div class="setting">
				<div class="text grow"><div class="title">Delete reports older than</div></div>
				<select
					class="select"
					style="width:auto"
					value={current.keepDays}
					onchange={(e) => save({ keepDays: Number((e.currentTarget as HTMLSelectElement).value) })}
					aria-label="Keep reports for"
				>
					{#each [30, 60, 90, 180, 365] as d (d)}<option value={d}>{d} days</option>{/each}
				</select>
			</div>
		</div>
	</section>
</div>

<style>
	.rset {
		max-width: 760px;
	}
	.back {
		margin-bottom: 8px;
	}
	.col {
		gap: 12px;
	}
</style>
