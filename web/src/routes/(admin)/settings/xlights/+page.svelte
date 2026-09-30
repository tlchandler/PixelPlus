<!--
	Settings → xLights (F16, ARCHITECTURE §12.14). WS6.
	Let xLights "FPP Connect" upload sequences and songs here: on/off, the upload
	password, step-by-step xLights setup, a drop folder alternative, and the log of
	uploads.
-->
<script lang="ts">
	import { onDestroy, onMount } from 'svelte';
	import {
		ArrowLeft,
		Upload,
		KeyRound,
		ListChecks,
		FolderInput,
		CircleCheck,
		CircleX,
		Copy,
		TriangleAlert,
		Info,
		Trash2
	} from '@lucide/svelte';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import Switch from '$lib/components/ui/Switch.svelte';
	import { api } from '$lib/api/client';
	import { app } from '$lib/stores/app.svelte';
	import { toasts } from '$lib/stores/toasts.svelte';
	import { fmtBytes, fmtRelative } from '$lib/util/format';
	import { xlightsApi, type XlightsStatus } from '$lib/insight/api';

	let st = $state<XlightsStatus | null>(null);
	let busy = $state(false);
	let pw = $state('');
	let folder = $state('');
	let timer: ReturnType<typeof setInterval> | undefined;

	const settings = $derived(app.show?.settings.xlights ?? { fppConnect: false, addToPlaylists: true });

	async function load() {
		try {
			st = await xlightsApi.status();
			if (!folder) folder = st.watch.folder ?? '';
		} catch (e) {
			toasts.error("Couldn't load the xLights status", (e as Error).message);
		}
	}
	onMount(() => {
		void load();
		timer = setInterval(load, 10_000);
	});
	onDestroy(() => clearInterval(timer));

	async function save(patch: Record<string, unknown>) {
		busy = true;
		try {
			await api.saveSettings({ xlights: patch });
			await app.reloadShow();
			await load();
		} catch (e) {
			toasts.error("Couldn't save", (e as Error).message);
		} finally {
			busy = false;
		}
	}

	async function enable(v: boolean) {
		if (v && st?.adminPasswordSet && !st.passwordSet && pw.length < 6) {
			toasts.warn('This show has a sign-in password: set an upload password for xLights below first.');
			return;
		}
		await save({ fppConnect: v });
	}

	async function setPassword(clear = false) {
		busy = true;
		try {
			await xlightsApi.setPassword(clear ? '' : pw);
			pw = '';
			toasts.success(clear ? 'Upload password removed' : 'Upload password saved');
			await app.reloadShow();
			await load();
		} catch (e) {
			toasts.error("Couldn't save the password", (e as Error).message);
		} finally {
			busy = false;
		}
	}

	async function copy(text: string) {
		try {
			await navigator.clipboard.writeText(text);
			toasts.success('Copied');
		} catch {
			toasts.warn("Couldn't copy on this browser");
		}
	}

	const address = $derived(st?.addresses.find((a) => /^\d+\.\d+\.\d+\.\d+$/.test(a)) ?? location.hostname);
</script>

<svelte:head><title>xLights · Settings · PixelPlus</title></svelte:head>

<div class="page xl">
	<a class="btn ghost sm back" href="/settings"><ArrowLeft size={16} /> Settings</a>
	<PageHeader
		title="xLights"
		subtitle="Send sequences and songs straight from xLights with FPP Connect — no copying files by hand."
	/>

	<section class="card">
		<div class="card-head">
			<Upload size={18} />
			<h2 class="grow">Let xLights upload here</h2>
			<Switch
				label="Let xLights upload here"
				checked={settings.fppConnect}
				disabled={busy}
				onchange={enable}
			/>
		</div>
		<div class="card-body col">
			{#if st}
				{#if !settings.fppConnect}
					<p class="muted small">Off: xLights can't see or reach PixelPlus.</p>
				{:else if st.ready}
					<p class="row good small">
						<CircleCheck size={16} /> Ready. xLights on this network can upload to {address}.
					</p>
				{:else}
					<p class="row bad small"><TriangleAlert size={16} /> {st.reason}</p>
				{/if}
			{/if}
			<div class="setting">
				<div class="text grow">
					<div class="title">Add uploaded sequences to playlists</div>
					<div class="desc">
						When you pick a playlist in FPP Connect, the sequences are added to the PixelPlus playlist of that
						name (nothing is removed).
					</div>
				</div>
				<Switch
					label="Add to playlists"
					checked={settings.addToPlaylists}
					disabled={busy}
					onchange={(v) => save({ addToPlaylists: v })}
				/>
			</div>
		</div>
	</section>

	<section class="card">
		<div class="card-head">
			<KeyRound size={18} />
			<h2 class="grow">Upload password</h2>
			{#if st?.passwordSet}<span class="badge green">Set</span>{:else}<span class="badge">Not set</span>{/if}
		</div>
		<div class="card-body col">
			<p class="muted small">
				xLights sends this with every upload (any user name). It only allows uploads — it can't sign in to
				PixelPlus.{st?.adminPasswordSet ? ' Required because this show has a sign-in password.' : ''}
			</p>
			<form
				class="row wrap"
				onsubmit={(e) => {
					e.preventDefault();
					void setPassword();
				}}
			>
				<input
					class="input grow"
					type="password"
					autocomplete="new-password"
					placeholder={st?.passwordSet ? 'New upload password' : 'Upload password (6+ characters)'}
					bind:value={pw}
					aria-label="Upload password"
				/>
				<button class="btn" type="submit" disabled={busy || pw.length < 6}>Save password</button>
				{#if st?.passwordSet}
					<button class="btn ghost" type="button" onclick={() => setPassword(true)} disabled={busy}
						>Remove</button
					>
				{/if}
			</form>
		</div>
	</section>

	<section class="card">
		<div class="card-head">
			<ListChecks size={18} />
			<h2 class="grow">Set up xLights (once)</h2>
		</div>
		<div class="card-body">
			<ol class="steps">
				<li>In xLights, open <b>Tools → FPP Connect</b>.</li>
				<li>
					Click <b>Add FPP</b> and enter
					<button class="mono chipbtn" onclick={() => copy(address)} title="Copy"
						>{address} <Copy size={12} /></button
					>
					{#if st?.hostname}(or <span class="mono">{st.hostname}.local</span>){/if}. PixelPlus appears in the
					list as <i>Falcon Player compatible</i>.
				</li>
				<li>
					If xLights asks for a password: user name <span class="mono">pixelplus</span> (anything works) and the
					upload password above.
				</li>
				<li>
					In the PixelPlus row, set <b>FSEQ type</b> to <b>V2 zstd</b>, tick <b>Media</b> to send the songs,
					and leave <b>Upload outputs</b> and <b>Models</b> off — PixelPlus keeps its own wiring.
				</li>
				<li>Optional: pick a <b>Playlist</b> name; the sequences are added to that playlist here.</li>
				<li>Tick your sequences and click <b>Upload</b>. Unchanged files are skipped next time.</li>
			</ol>
			<div class="notice info">
				<Info size={18} />
				<div class="small">
					Re-uploading a sequence with the same file name replaces it in place: playlists and schedules keep
					working. Tested with xLights 2025/2026 FPP Connect; videos and effect sequences (.eseq) are not
					supported.
				</div>
			</div>
		</div>
	</section>

	<section class="card">
		<div class="card-head">
			<FolderInput size={18} />
			<h2 class="grow">Drop folder (alternative)</h2>
		</div>
		<div class="card-body col">
			<p class="muted small">
				If you'd rather copy files: PixelPlus imports <span class="mono">.fseq</span> and audio files placed
				in this folder (for example a network share) once they finish copying, then moves them to
				<span class="mono">imported/</span> (or <span class="mono">failed/</span>).
			</p>
			<form
				class="row wrap"
				onsubmit={(e) => {
					e.preventDefault();
					void save({ watchFolder: folder.trim() || null });
				}}
			>
				<input
					class="input grow mono"
					placeholder={st?.watch.suggested ?? '/var/lib/pixelplus/xlights-drop'}
					bind:value={folder}
					aria-label="Drop folder"
				/>
				<button class="btn" type="submit" disabled={busy}
					>{folder.trim() ? 'Watch this folder' : 'Turn off'}</button
				>
				{#if !folder && st}
					<button class="btn ghost" type="button" onclick={() => (folder = st?.watch.suggested ?? '')}
						>Use suggested</button
					>
				{/if}
			</form>
			{#if st?.watch.folder}
				<p class="small {st.watch.error ? 'bad' : 'faint'}">
					{st.watch.error ??
						(st.watch.exists
							? `Watching; last checked ${st.watch.lastScan ? fmtRelative(st.watch.lastScan) : 'soon'}.`
							: 'The folder doesn’t exist yet.')}
				</p>
			{/if}
		</div>
	</section>

	<section class="card">
		<div class="card-head">
			<Upload size={18} />
			<h2 class="grow">Recent uploads</h2>
			{#if st?.uploads.length}
				<button
					class="btn sm ghost icon"
					aria-label="Clear the list"
					onclick={async () => {
						await xlightsApi.clearLog().catch(() => {});
						await load();
					}}><Trash2 size={14} /></button
				>
			{/if}
		</div>
		<div class="list">
			{#each st?.uploads ?? [] as u (u.at + u.name)}
				<div class="list-row">
					<span class={u.ok ? 'good' : 'bad'}
						>{#if u.ok}<CircleCheck size={16} />{:else}<CircleX size={16} />{/if}</span
					>
					<div class="grow" style="min-width:0">
						<div class="small ellipsis">{u.name}</div>
						<div class="faint tiny ellipsis">{u.message}</div>
					</div>
					<span class="faint tiny num"
						>{fmtBytes(u.bytes)} · {u.source === 'folder' ? 'folder' : 'xLights'} · {fmtRelative(u.at)}</span
					>
				</div>
			{:else}
				<div class="list-row faint small">Nothing uploaded yet.</div>
			{/each}
		</div>
	</section>
</div>

<style>
	.xl {
		max-width: 820px;
		display: flex;
		flex-direction: column;
		gap: 16px;
	}
	.back {
		align-self: flex-start;
		margin-bottom: -8px;
	}
	.col {
		gap: 12px;
	}
	.good {
		color: var(--green);
	}
	.bad {
		color: var(--red);
	}
	.steps {
		margin: 0 0 12px;
		padding-left: 20px;
		display: flex;
		flex-direction: column;
		gap: 8px;
		line-height: 1.5;
	}
	.chipbtn {
		display: inline-flex;
		align-items: center;
		gap: 4px;
		padding: 1px 8px;
		border-radius: var(--r-1);
		border: 1px solid var(--border-2);
		background: var(--surface-2);
		color: var(--text);
		font-size: 13px;
	}
</style>
