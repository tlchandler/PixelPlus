<script lang="ts">
	import { api, ApiError } from '$lib/api/client';
	import { initBackend } from '$lib/api/mode';
	import type { PublicRequests } from '$lib/api/types';
	import { fmtDuration } from '$lib/util/format';
	import { fmStation } from '$lib/util/visitors';
	import { Music, Check, Search, Radio, Sparkles, Clock } from '@lucide/svelte';
	import { fade, fly } from 'svelte/transition';

	let data = $state<PublicRequests | null>(null);
	let error = $state('');
	let q = $state('');
	let name = $state('');
	let sending = $state<string | null>(null);
	let sent = $state<{ name: string; position: number } | null>(null);
	let ready = $state(false);
	/** Song requests are turned off on this controller (Settings → Features). */
	let unavailable = $state(false);

	try {
		name = localStorage.getItem('pp-req-name') ?? '';
	} catch {
		/* ignore */
	}

	async function load() {
		try {
			data = await api.publicRequests();
			error = '';
			unavailable = false;
		} catch (e) {
			if (e instanceof ApiError && e.code === 'feature_disabled') {
				unavailable = true;
				data = null;
				error = '';
				return;
			}
			error = e instanceof Error ? e.message : 'Can’t reach the show right now';
		}
	}

	$effect(() => {
		let t: ReturnType<typeof setInterval>;
		initBackend().then(() => {
			ready = true;
			load();
			t = setInterval(load, 8000);
		});
		return () => clearInterval(t);
	});

	async function request(id: string, title: string) {
		sending = id;
		try {
			try {
				localStorage.setItem('pp-req-name', name);
			} catch {
				/* ignore */
			}
			const r = (await api.submitRequest(id, name.trim() || undefined)) as { position?: number } | undefined;
			sent = { name: title, position: r?.position ?? (data?.queue.length ?? 0) + 1 };
			await load();
		} catch (e) {
			error = e instanceof ApiError ? e.message : 'That didn’t work — please try again';
			setTimeout(() => (error = ''), 4000);
		} finally {
			sending = null;
		}
	}

	const songs = $derived(
		data?.songs.filter((s) => !q || s.name.toLowerCase().includes(q.toLowerCase())) ?? []
	);
	const queued = $derived(new Set(data?.queue.map((x) => x.sequenceId) ?? []));
	const full = $derived(!!data && data.queue.length >= data.maxQueue);
	const station = $derived(fmStation(data?.radioFrequency));
</script>

<svelte:head>
	<title>{data?.title ?? 'Request a song'}{data?.showName ? ` · ${data.showName}` : ''}</title>
	<meta name="theme-color" content="#0b1026" />
</svelte:head>

<div class="req">
	<div class="snow" aria-hidden="true">
		{#each Array(28) as _, i (i)}<i
				style:left="{(i * 37) % 100}%"
				style:animation-delay="{-(i * 1.3) % 12}s"
				style:animation-duration="{9 + (i % 5) * 2}s"
				style:opacity={0.3 + (i % 4) * 0.15}
			></i>{/each}
	</div>

	<header>
		<div class="star"><Sparkles size={22} /></div>
		<p class="show">{data?.showName ?? ' '}</p>
		<h1>{data?.title ?? 'Request a song'}</h1>
		{#if data?.message}<p class="msg">{data.message}</p>{/if}
		{#if station}
			<p class="tune"><Radio size={18} /> Tune your radio to <strong>{station}</strong></p>
		{/if}
	</header>

	{#if unavailable}
		<div class="closed">
			<Clock size={26} />
			<h2>Song requests aren’t available here</h2>
			<p>This light show doesn’t take requests. Enjoy the lights!</p>
		</div>
	{:else if !ready || (!data && !error)}
		<div class="loading" aria-busy="true">
			{#each Array(5) as _, i (i)}<div class="sk"></div>{/each}
		</div>
	{:else if data && !data.enabled}
		<div class="closed">
			<Clock size={26} />
			<h2>Requests are closed right now</h2>
			<p>Come back during the show — and enjoy the lights!</p>
		</div>
	{:else if data}
		{#if data.nowPlaying}
			<div class="now">
				<span class="eq" aria-hidden="true"><i></i><i></i><i></i><i></i></span>
				<div class="grow">
					<span class="lbl">Now playing</span>
					<strong>{data.nowPlaying.name}</strong>
				</div>
				<span class="time">{fmtDuration(data.nowPlaying.durationMs - data.nowPlaying.posMs)} left</span>
			</div>
		{/if}

		{#if data.queue.length}
			<section class="queue">
				<h2>Up next</h2>
				<ol>
					{#each data.queue as item, i (item.id)}
						<li in:fly={{ y: 8 }}>
							<span class="pos">{i + 1}</span><span class="grow">{item.name}</span>{#if item.requestedBy}<span
									class="by">for {item.requestedBy}</span
								>{/if}
						</li>
					{/each}
				</ol>
			</section>
		{/if}

		<section class="pick">
			<div class="pickhead">
				<h2>{data.title.toLowerCase().includes('pick a song') ? 'Songs' : 'Pick a song'}</h2>
				{#if full}<span class="full">Line-up is full — try again soon</span>{/if}
			</div>
			<div class="fields">
				<label class="search"
					><Search size={18} /><input
						placeholder="Search songs"
						bind:value={q}
						aria-label="Search songs"
					/></label
				>
				<input
					class="name"
					placeholder="Your first name (optional)"
					bind:value={name}
					maxlength="20"
					aria-label="Your first name"
				/>
			</div>
			<ul class="songs">
				{#each songs as s (s.sequenceId)}
					{@const isQ = queued.has(s.sequenceId)}
					<li>
						<span class="note"><Music size={18} /></span>
						<span class="grow"
							><strong>{s.name}</strong><span class="dur">{fmtDuration(s.durationMs)}</span></span
						>
						<button
							class="go"
							class:done={isQ}
							disabled={isQ || full || sending != null}
							onclick={() => request(s.sequenceId, s.name)}
						>
							{#if isQ}<Check size={16} /> Queued{:else if sending === s.sequenceId}…{:else}Request{/if}
						</button>
					</li>
				{:else}
					<li class="none">No songs match “{q}”.</li>
				{/each}
			</ul>
		</section>
		<p class="foot">
			{#if station}<Radio size={14} /> {station}&nbsp;·&nbsp;{/if}Please be kind to the neighbors
		</p>
	{/if}

	{#if error}
		<div class="toast err" transition:fly={{ y: 20 }} role="alert">{error}</div>
	{/if}
	{#if sent}
		<div class="sheet-bg" transition:fade onclick={() => (sent = null)} aria-hidden="true"></div>
		<div class="sheet" transition:fly={{ y: 300, duration: 300 }} role="dialog" aria-label="Request sent">
			<div class="burst"><Check size={34} /></div>
			<h2>You’re on the list!</h2>
			<p>
				<strong>{sent.name}</strong> is number {sent.position} in line. Keep watching — it’s coming up soon.
			</p>
			<button class="ok" onclick={() => (sent = null)}>Merry Christmas!</button>
		</div>
	{/if}
</div>

<style>
	:global(body:has(.req)) {
		background: #0b1026;
	}
	.req {
		--gold: #ffc85a;
		--red: #ff5a5f;
		--green: #3ddc97;
		min-height: 100dvh;
		color: #f4f1ea;
		background:
			radial-gradient(ellipse at 50% -10%, rgba(255, 200, 90, 0.18), transparent 50%),
			radial-gradient(ellipse at 100% 100%, rgba(255, 90, 95, 0.12), transparent 50%),
			linear-gradient(180deg, #0b1026, #0a0d1c 60%, #0b0a14);
		padding: 0 16px calc(40px + env(safe-area-inset-bottom));
		position: relative;
		overflow-x: hidden;
		font-size: 15px;
	}
	/* Snow falls behind the content: cards are frosted so flakes never cross the text. */
	.snow {
		position: fixed;
		inset: 0;
		pointer-events: none;
		overflow: hidden;
		z-index: 0;
	}
	header,
	.now,
	.queue,
	.pick,
	.closed,
	.loading,
	.foot {
		z-index: 1;
	}
	.now,
	.queue,
	.closed {
		background-color: rgba(11, 16, 38, 0.78);
		backdrop-filter: blur(10px);
		-webkit-backdrop-filter: blur(10px);
	}
	.tune {
		display: inline-flex;
		align-items: center;
		gap: 8px;
		margin-top: 14px;
		padding: 8px 16px;
		border-radius: 999px;
		background: rgba(61, 220, 151, 0.14);
		border: 1px solid rgba(61, 220, 151, 0.35);
		color: #d9fbe9;
		font-size: 15px;
	}
	.tune strong {
		color: #fff;
		font-weight: 750;
		letter-spacing: -0.01em;
	}
	.snow i {
		position: absolute;
		top: -10px;
		width: 5px;
		height: 5px;
		border-radius: 50%;
		background: #fff;
		filter: blur(0.5px);
		animation: fall linear infinite;
	}
	@keyframes fall {
		to {
			transform: translate3d(30px, 105vh, 0);
		}
	}
	header {
		text-align: center;
		padding: 40px 8px 20px;
		position: relative;
		max-width: 560px;
		margin: 0 auto;
	}
	.star {
		width: 52px;
		height: 52px;
		margin: 0 auto 14px;
		border-radius: 50%;
		display: grid;
		place-items: center;
		color: #2a1a00;
		background: radial-gradient(circle at 35% 30%, #fff1c4, var(--gold));
		box-shadow: 0 0 40px rgba(255, 200, 90, 0.55);
	}
	.show {
		font-size: 12px;
		letter-spacing: 0.18em;
		text-transform: uppercase;
		color: var(--gold);
		font-weight: 700;
		min-height: 18px;
	}
	h1 {
		font-size: 34px;
		letter-spacing: -0.03em;
		margin: 6px 0 8px;
		font-weight: 750;
	}
	.msg {
		color: rgba(244, 241, 234, 0.72);
		line-height: 1.5;
	}
	.loading,
	.now,
	.queue,
	.pick,
	.closed,
	.foot {
		max-width: 560px;
		margin-left: auto;
		margin-right: auto;
		position: relative;
	}
	.sk {
		height: 64px;
		border-radius: 16px;
		background: rgba(255, 255, 255, 0.06);
		margin-bottom: 10px;
		animation: pulse 1.4s infinite ease-in-out;
	}
	@keyframes pulse {
		50% {
			opacity: 0.5;
		}
	}
	.closed {
		text-align: center;
		padding: 40px 20px;
		border-radius: 20px;
		background: rgba(255, 255, 255, 0.05);
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 8px;
	}
	.now {
		display: flex;
		align-items: center;
		gap: 14px;
		padding: 14px 16px;
		border-radius: 18px;
		background: linear-gradient(120deg, rgba(255, 90, 95, 0.22), rgba(255, 200, 90, 0.14));
		border: 1px solid rgba(255, 200, 90, 0.25);
		margin-bottom: 16px;
	}
	.now .grow {
		display: flex;
		flex-direction: column;
		min-width: 0;
		flex: 1;
	}
	.lbl {
		font-size: 11px;
		letter-spacing: 0.12em;
		text-transform: uppercase;
		color: var(--gold);
		font-weight: 700;
	}
	.now strong {
		font-size: 16px;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.time {
		font-size: 12px;
		color: rgba(244, 241, 234, 0.7);
		font-variant-numeric: tabular-nums;
		white-space: nowrap;
	}
	.eq {
		display: flex;
		gap: 3px;
		align-items: flex-end;
		height: 22px;
	}
	.eq i {
		width: 4px;
		border-radius: 2px;
		background: var(--gold);
		animation: bar 0.9s ease-in-out infinite;
		height: 8px;
	}
	.eq i:nth-child(2) {
		animation-delay: -0.2s;
	}
	.eq i:nth-child(3) {
		animation-delay: -0.5s;
	}
	.eq i:nth-child(4) {
		animation-delay: -0.7s;
	}
	@keyframes bar {
		50% {
			height: 22px;
		}
	}
	h2 {
		font-size: 17px;
		letter-spacing: -0.01em;
		margin: 0;
	}
	.queue {
		margin-bottom: 18px;
		padding: 16px;
		border-radius: 18px;
		background: rgba(255, 255, 255, 0.05);
		border: 1px solid rgba(255, 255, 255, 0.08);
	}
	.queue ol {
		list-style: none;
		margin: 10px 0 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 8px;
	}
	.queue li {
		display: flex;
		align-items: center;
		gap: 10px;
	}
	.queue .grow {
		flex: 1;
	}
	.pos {
		width: 24px;
		height: 24px;
		border-radius: 50%;
		display: grid;
		place-items: center;
		font-size: 12px;
		font-weight: 700;
		background: rgba(255, 200, 90, 0.16);
		color: var(--gold);
	}
	.by {
		font-size: 12px;
		color: rgba(244, 241, 234, 0.6);
	}
	.pickhead {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		gap: 10px;
		margin-bottom: 10px;
	}
	.full {
		font-size: 12px;
		color: var(--red);
	}
	.fields {
		display: flex;
		flex-direction: column;
		gap: 8px;
		margin-bottom: 12px;
	}
	.search {
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 0 14px;
		height: 50px;
		border-radius: 14px;
		background: rgba(255, 255, 255, 0.08);
		color: rgba(244, 241, 234, 0.6);
	}
	.search input,
	.name {
		flex: 1;
		background: none;
		border: 0;
		color: #fff;
		font-size: 16px;
		height: 100%;
	}
	.name {
		flex: none;
		height: 50px;
		padding: 0 14px;
		border-radius: 14px;
		background: rgba(255, 255, 255, 0.05);
		border: 1px dashed rgba(255, 255, 255, 0.15);
	}
	input::placeholder {
		color: rgba(244, 241, 234, 0.45);
	}
	.search:focus-within,
	.name:focus {
		outline: 2px solid rgba(255, 200, 90, 0.6);
		outline-offset: 0;
	}
	.songs {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 8px;
	}
	.songs li {
		display: flex;
		align-items: center;
		gap: 12px;
		padding: 10px 10px 10px 12px;
		border-radius: 16px;
		background: rgba(25, 30, 54, 0.82);
		border: 1px solid rgba(255, 255, 255, 0.07);
	}
	.songs .grow {
		flex: 1;
		display: flex;
		flex-direction: column;
		min-width: 0;
	}
	.songs strong {
		font-weight: 620;
		line-height: 1.3;
	}
	.dur {
		font-size: 12px;
		color: rgba(244, 241, 234, 0.55);
	}
	.note {
		width: 40px;
		height: 40px;
		border-radius: 12px;
		display: grid;
		place-items: center;
		background: rgba(61, 220, 151, 0.12);
		color: var(--green);
		flex: 0 0 auto;
	}
	.go {
		height: 44px;
		padding: 0 18px;
		border-radius: 12px;
		font-weight: 700;
		font-size: 14px;
		color: #2a1a00;
		background: linear-gradient(180deg, #ffd67a, var(--gold));
		box-shadow: 0 4px 16px rgba(255, 200, 90, 0.25);
		display: inline-flex;
		align-items: center;
		gap: 6px;
		flex: 0 0 auto;
		transition: transform 120ms;
	}
	.go:active {
		transform: scale(0.95);
	}
	.go:disabled {
		opacity: 0.45;
		box-shadow: none;
	}
	.go.done {
		background: rgba(61, 220, 151, 0.18);
		color: var(--green);
		opacity: 1;
	}
	.none {
		justify-content: center;
		color: rgba(244, 241, 234, 0.6);
	}
	.foot {
		text-align: center;
		margin-top: 24px;
		font-size: 12.5px;
		color: rgba(244, 241, 234, 0.5);
		display: flex;
		align-items: center;
		justify-content: center;
		gap: 6px;
	}
	.toast {
		position: fixed;
		left: 16px;
		right: 16px;
		bottom: calc(20px + env(safe-area-inset-bottom));
		max-width: 520px;
		margin: 0 auto;
		padding: 14px 16px;
		border-radius: 14px;
		background: #3a1418;
		border: 1px solid rgba(255, 90, 95, 0.4);
		text-align: center;
		z-index: 20;
	}
	.sheet-bg {
		position: fixed;
		inset: 0;
		background: rgba(0, 0, 0, 0.55);
		z-index: 30;
	}
	.sheet {
		position: fixed;
		left: 0;
		right: 0;
		bottom: 0;
		max-width: 560px;
		margin: 0 auto;
		padding: 28px 24px calc(28px + env(safe-area-inset-bottom));
		border-radius: 26px 26px 0 0;
		background: #151a33;
		border: 1px solid rgba(255, 255, 255, 0.1);
		text-align: center;
		z-index: 31;
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 10px;
	}
	.sheet p {
		color: rgba(244, 241, 234, 0.75);
		line-height: 1.5;
	}
	.burst {
		width: 72px;
		height: 72px;
		border-radius: 50%;
		display: grid;
		place-items: center;
		color: #06281a;
		background: radial-gradient(circle at 35% 30%, #b8ffe0, var(--green));
		box-shadow: 0 0 50px rgba(61, 220, 151, 0.5);
		animation: pop 500ms cubic-bezier(0.34, 1.56, 0.64, 1);
	}
	@keyframes pop {
		from {
			transform: scale(0.4);
		}
	}
	.ok {
		margin-top: 8px;
		width: 100%;
		height: 52px;
		border-radius: 14px;
		font-weight: 700;
		font-size: 16px;
		color: #2a1a00;
		background: linear-gradient(180deg, #ffd67a, var(--gold));
	}
	@media (prefers-reduced-motion: reduce) {
		.snow {
			display: none;
		}
	}
</style>
