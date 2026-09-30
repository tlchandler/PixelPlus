<script lang="ts">
	import { api } from '../lib/api';
	import type { Drive, ImageChoice, ImagerSettings, Progress } from '../lib/types';
	import { formatBytes, formatEta } from '../lib/validate';

	let {
		image,
		drive,
		settings,
		writing = $bindable(),
		finished = $bindable(),
		progress = $bindable(),
		onrestart
	}: {
		image: ImageChoice | null;
		drive: Drive | null;
		settings: ImagerSettings;
		writing: boolean;
		finished: boolean;
		progress: Progress | null;
		onrestart: () => void;
	} = $props();

	let confirming = $state(false);
	let error = $state<string | null>(null);
	let speed = $state(0);
	let lastSample: { t: number; bytes: number; phase: string } | null = null;

	const phaseLabel: Record<string, string> = {
		download: 'Downloading PixelPlus',
		prepare: 'Preparing the card',
		write: 'Writing',
		verify: 'Checking the card',
		customize: 'Saving your settings',
		done: 'Done',
		error: 'Something went wrong'
	};

	const pct = $derived(progress?.total ? Math.min(100, (progress.bytes / progress.total) * 100) : null);
	const eta = $derived(progress?.total && speed > 0 ? formatEta((progress.total - progress.bytes) / speed) : '');

	function onProgress(p: Progress) {
		progress = p;
		const now = performance.now();
		if (p.total && lastSample && lastSample.phase === p.phase && now - lastSample.t > 800) {
			const s = ((p.bytes - lastSample.bytes) / (now - lastSample.t)) * 1000;
			speed = speed ? speed * 0.7 + s * 0.3 : s;
			lastSample = { t: now, bytes: p.bytes, phase: p.phase };
		} else if (!lastSample || lastSample.phase !== p.phase) {
			lastSample = { t: now, bytes: p.bytes, phase: p.phase };
			speed = 0;
		}
		if (p.phase === 'error') error = p.message ?? 'Unknown error';
	}

	async function start() {
		if (!image || !drive) return;
		confirming = false;
		writing = true;
		error = null;
		progress = { phase: 'prepare', bytes: 0, total: null, message: 'Asking for permission to write the card…' };
		try {
			await api.write(
				{
					image: image.kind === 'release' ? image.image : null,
					localPath: image.kind === 'file' ? image.path : null,
					device: drive.device,
					settings
				},
				onProgress
			);
			if (!error) finished = true;
		} catch (e) {
			error = String(e).replace(/^Error: /, '');
		} finally {
			writing = false;
		}
	}

	const imageName = $derived(image?.kind === 'release' ? image.image.name : image?.kind === 'file' ? image.name : '');
</script>

<section>
	{#if finished}
		<div class="done card">
			<div class="big-check" aria-hidden="true">✓</div>
			<h2>Your SD card is ready</h2>
			<ol>
				<li>Put the card in your Raspberry Pi and power it on.</li>
				<li>Wait about <strong>2 minutes</strong> - the first start takes a little longer.</li>
				<li>Open <a href={`http://${settings.hostname || 'pixelplus'}.local`} target="_blank" rel="noreferrer"
						><strong>http://{settings.hostname || 'pixelplus'}.local</strong></a
					> in a browser on the same network.</li>
			</ol>
			<p class="hint">
				Can't reach it? If Wi-Fi didn't work, the Pi opens the Wi-Fi network <strong>PixelPlus-XXXX</strong>
				(password <code>pixelplus</code>) - join it with your phone to choose your network.
			</p>
			<button class="btn" onclick={onrestart}>Write another card</button>
		</div>
	{:else}
		<h2>Ready to write</h2>
		<div class="card summary">
			<div><span class="hint">PixelPlus</span><strong>{imageName}</strong></div>
			<div><span class="hint">SD card</span><strong>{drive?.name}</strong></div>
			<div>
				<span class="hint">Wi-Fi</span><strong
					>{settings.wifiSsid || 'Not set (Ethernet or phone setup)'}{settings.wifiSsid && settings.wifiCountry
						? ` · ${settings.wifiCountry}`
						: ''}</strong
				>
			</div>
			<div><span class="hint">Name</span><strong>{settings.hostname || 'pixelplus'}.local · {settings.role ?? 'choose later'}</strong></div>
		</div>

		{#if writing || progress}
			<div class="card progress" aria-live="polite">
				<div class="row between">
					<strong>{phaseLabel[progress?.phase ?? 'prepare']}</strong>
					<span class="hint">
						{#if pct !== null}{pct.toFixed(0)}% · {formatBytes(progress?.bytes)} of {formatBytes(progress?.total)}{/if}
						{#if speed > 0} · {formatBytes(speed)}/s{/if}
						{#if eta} · {eta}{/if}
					</span>
				</div>
				<div class="bar" class:indeterminate={pct === null && writing}>
					<div style:width={pct !== null ? `${pct}%` : '30%'}></div>
				</div>
				{#if progress?.message && progress.phase !== 'error'}<span class="hint">{progress.message}</span>{/if}
			</div>
		{/if}

		{#if error}
			<div class="card errbox" role="alert">
				<strong>Writing failed</strong>
				<span>{error}</span>
				<span class="hint">Nothing is lost: take the card out, put it back in and try again.</span>
			</div>
		{/if}

		<div class="actions">
			{#if writing}
				<button class="btn" onclick={() => api.cancel()}>Cancel</button>
			{:else if confirming}
				<span>Erase <strong>{drive?.name}</strong> and write PixelPlus?</span>
				<button class="btn" onclick={() => (confirming = false)}>No</button>
				<button class="btn danger" onclick={start}>Yes, erase and write</button>
			{:else}
				<button class="btn primary" onclick={() => (confirming = true)} disabled={!image || !drive}>
					{error ? 'Try again' : 'Write'}
				</button>
			{/if}
		</div>
	{/if}
</section>

<style>
	h2 {
		margin: 0 0 12px;
		font-size: 20px;
	}
	.summary {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: 12px 24px;
		padding: 16px;
	}
	.summary div {
		display: grid;
		gap: 2px;
	}
	.progress,
	.errbox {
		margin-top: 16px;
		padding: 16px;
		display: grid;
		gap: 10px;
	}
	.errbox {
		border-color: var(--red);
		background: var(--red-soft);
	}
	.between {
		justify-content: space-between;
	}
	.bar {
		height: 8px;
		border-radius: 999px;
		background: var(--surface-3);
		overflow: hidden;
	}
	.bar div {
		height: 100%;
		background: var(--accent);
		border-radius: 999px;
		transition: width 0.3s ease;
	}
	.bar.indeterminate div {
		animation: slide 1.2s infinite ease-in-out;
	}
	@keyframes slide {
		from {
			transform: translateX(-100%);
		}
		to {
			transform: translateX(340%);
		}
	}
	.actions {
		display: flex;
		justify-content: flex-end;
		align-items: center;
		gap: 12px;
		margin-top: 20px;
	}
	.done {
		padding: 32px;
		display: grid;
		justify-items: center;
		text-align: center;
		gap: 8px;
	}
	.done ol {
		text-align: left;
		line-height: 1.9;
	}
	.done a {
		color: var(--accent);
	}
	.big-check {
		width: 56px;
		height: 56px;
		border-radius: 50%;
		display: grid;
		place-items: center;
		font-size: 28px;
		background: var(--green-soft);
		color: var(--green);
	}
	code {
		font-size: 12px;
		padding: 1px 5px;
		border-radius: 5px;
		background: var(--surface-3);
	}
</style>
