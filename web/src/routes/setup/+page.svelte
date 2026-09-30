<script lang="ts">
	import { goto } from '$app/navigation';
	import { api, upload } from '$lib/api/client';
	import { MIN_PASSPHRASE } from '$lib/api/fleet';
	import type { BoardKind, NodeRole } from '$lib/api/types';
	import { app } from '$lib/stores/app.svelte';
	import { toasts } from '$lib/stores/toasts.svelte';
	import { BOARDS } from '$lib/util/boards';
	import { searchCities, timezones } from '$lib/util/cities';
	import Logo from '$lib/components/shell/Logo.svelte';
	import BoardDiagram from '$lib/components/viz/BoardDiagram.svelte';
	import FollowerScreen from '$lib/components/shell/FollowerScreen.svelte';
	import {
		Crown,
		Radio,
		ArrowRight,
		ArrowLeft,
		Check,
		MapPin,
		LocateFixed,
		LockKeyhole,
		FileUp,
		Music,
		ListMusic,
		CalendarClock,
		Sparkles,
		TriangleAlert,
		CircleCheck,
		ArchiveRestore,
		Layers,
		SlidersHorizontal
	} from '@lucide/svelte';
	import { ESSENTIALS, FEATURES, FEATURE_GROUPS, normalize, presetDisabled, setFeature } from '$lib/features';
	import type { FeatureId } from '$lib/api/types';
	import { fly } from 'svelte/transition';

	let step = $state(0);
	let dir = $state(1);
	let role = $state<NodeRole>('leader');
	let board = $state<BoardKind>('difftx');
	let rev = $state('E');
	let changingBoard = $state(false);
	let writeEeprom = $state(true);
	let showName = $state('');
	let cityQ = $state('');
	let loc = $state({ lat: 0, lon: 0, timezone: Intl.DateTimeFormat().resolvedOptions().timeZone, label: '' });
	let password = $state('');
	let password2 = $state('');
	let busy = $state(false);
	let followerDone = $state(false);
	// F10 (WS5): restore a show leader from a controller transfer file.
	let xferFile = $state<File | null>(null);
	let xferPass = $state('');
	let xferPct = $state<number | null>(null);
	let xferError = $state<string | null>(null);
	let restoreNotes = $state<string[]>([]);

	const detected = $derived(app.system?.detectedBoard ?? app.system?.board ?? null);
	$effect(() => {
		if (detected) board = detected;
		if (app.system?.boardRev) rev = app.system.boardRev;
	});
	$effect(() => {
		if (app.ready && app.system && !app.system.needsSetup && !followerDone && step === 0)
			goto('/', { replaceState: true });
	});

	const hits = $derived(searchCities(cityQ, 6));
	// Never show a blank time zone: add this browser's zone (e.g. "UTC") when the list lacks it.
	const tzs = (() => {
		const list = timezones();
		const mine = loc.timezone;
		return mine && !list.includes(mine) ? [mine, ...list] : list;
	})();
	const steps = ['Welcome', 'Role', 'Board', 'Your show', 'Features', 'Password', 'Done'];

	// "What will you use?" (Settings → Features): Essentials by default for a new show.
	let usePreset = $state<'essentials' | 'everything' | 'custom'>('essentials');
	let customOff = $state<string[]>(presetDisabled('essentials'));
	const chosenDisabled = $derived(usePreset === 'custom' ? normalize(customOff) : presetDisabled(usePreset));
	function toggleCustom(id: FeatureId) {
		customOff = setFeature(customOff, id, customOff.includes(id)).disabled;
	}

	function next() {
		dir = 1;
		step++;
	}
	function back() {
		dir = -1;
		step--;
	}

	function geolocate() {
		navigator.geolocation?.getCurrentPosition(
			(p) => {
				loc = {
					lat: +p.coords.latitude.toFixed(4),
					lon: +p.coords.longitude.toFixed(4),
					timezone: Intl.DateTimeFormat().resolvedOptions().timeZone,
					label: 'My location'
				};
			},
			() => toasts.error('Couldn’t get your location', 'Search for a nearby town instead.')
		);
	}

	async function finish(asRole: NodeRole) {
		busy = true;
		try {
			await api.setup(
				asRole === 'follower'
					? { role: 'follower' }
					: {
							role: 'leader',
							showName: showName.trim() || 'My Show',
							board,
							boardRev: rev || undefined,
							location: loc.lat || loc.lon ? { ...loc, label: loc.label || undefined } : undefined,
							timezone: loc.timezone,
							password: password || undefined,
							writeEeprom: !detected && writeEeprom,
							features: { disabled: chosenDisabled }
						}
			);
			try {
				sessionStorage.removeItem('pp-mock-setup');
			} catch {
				/* ignore */
			}
			await app.loadSystem();
			if (asRole === 'follower') followerDone = true;
			else {
				await app.reloadShow();
				dir = 1;
				step = 6;
			}
		} catch (e) {
			toasts.error('Setup didn’t finish', (e as Error).message);
		} finally {
			busy = false;
		}
	}

	async function restore() {
		if (!xferFile) return;
		busy = true;
		xferError = null;
		xferPct = 0;
		try {
			const form = new FormData();
			// The passphrase must come first: the file is decrypted while it uploads.
			form.append('passphrase', xferPass);
			form.append('transfer', xferFile, xferFile.name);
			const r = (await upload('/system/setup', form, (p) => (xferPct = Math.round(p * 100)))) as {
				notes?: string[];
				restored?: { showName: string; hostname: string; files: number };
			};
			restoreNotes = r?.notes ?? [];
			showName = r?.restored?.showName ?? showName;
			try {
				sessionStorage.removeItem('pp-mock-setup');
			} catch {
				/* ignore */
			}
			await app.loadSystem();
			await app.reloadShow();
			dir = 1;
			step = 6;
		} catch (e) {
			xferError = (e as Error).message;
		} finally {
			busy = false;
			xferPct = null;
		}
	}

	const boardChoices: BoardKind[] = ['difftxlarge', 'difftx', 'diffsmart', 'bare-pi', 'virtual'];
</script>

<svelte:head><title>Set up PixelPlus</title></svelte:head>

{#if followerDone}
	<FollowerScreen />
{:else}
	<div class="wiz">
		<div class="glow" aria-hidden="true"></div>
		<header class="top">
			<Logo size={30} />
			<span class="brand">PixelPlus</span>
			<span class="grow"></span>
			{#if step > 0 && step < 6}
				<ol class="dots" aria-label="Progress">
					{#each steps.slice(1, 6) as s, i (s)}<li
							class:on={step === i + 1}
							class:done={step > i + 1}
							aria-current={step === i + 1 ? 'step' : undefined}
						>
							<span class="sr-only">{s}</span>
						</li>{/each}
				</ol>
			{/if}
		</header>

		<main class="stage">
			{#key step}
				<section
					class="panel"
					in:fly={{ x: 40 * dir, duration: 280, delay: 60 }}
					out:fly={{ x: -40 * dir, duration: 180 }}
				>
					{#if step === 0}
						<div class="hero">
							<div class="bigmark"><Logo size={84} /></div>
							<h1>Welcome to PixelPlus</h1>
							<p class="lead">
								Let’s get your light show running. It takes about two minutes — no spreadsheets, no network
								math.
							</p>
							<button class="btn primary lg" onclick={next}>Get started <ArrowRight size={18} /></button>
							<p class="faint small">{app.system?.hostname ?? 'pixelplus'} · {app.system?.ips?.[0] ?? ''}</p>
						</div>
					{:else if step === 1}
						<h1>What does this controller do?</h1>
						<p class="lead">
							Every show has one leader. You set everything up there; other controllers just follow along.
						</p>
						<div class="roles">
							<button
								class="role"
								class:on={role === 'leader'}
								onclick={() => (role = 'leader')}
								aria-pressed={role === 'leader'}
							>
								<span class="ri accent"><Crown size={24} /></span>
								<strong>Make this the show leader</strong>
								<span class="muted small"
									>Runs the schedule, plays the music and tells the other controllers what to do. Pick this
									for your first controller.</span
								>
								{#if role === 'leader'}<span class="tick"><Check size={14} /></span>{/if}
							</button>
							<button
								class="role"
								class:on={role === 'follower'}
								onclick={() => (role = 'follower')}
								aria-pressed={role === 'follower'}
							>
								<span class="ri blue"><Radio size={24} /></span>
								<strong>This is a follower</strong>
								<span class="muted small"
									>Waits to be adopted by your leader, then receives its settings and sequences automatically.
									Nothing else to set up here.</span
								>
								{#if role === 'follower'}<span class="tick"><Check size={14} /></span>{/if}
							</button>
						</div>
						<button
							class="btn ghost sm restore-link"
							onclick={() => {
								dir = 1;
								step = 10;
							}}
						>
							<ArchiveRestore size={16} /> Replacing a show leader? Restore a show from a transfer file
						</button>
						<div class="nav">
							<button class="btn ghost" onclick={back}><ArrowLeft size={16} /> Back</button>
							{#if role === 'leader'}
								<button class="btn primary" onclick={next}>Continue <ArrowRight size={16} /></button>
							{:else}
								<button class="btn primary" onclick={() => finish('follower')} disabled={busy}
									>{busy ? 'Setting up…' : 'Wait to be adopted'} <ArrowRight size={16} /></button
								>
							{/if}
						</div>
					{:else if step === 2}
						{#if detected && !changingBoard}
							<div class="found">
								<CircleCheck size={16} />
								{app.system?.detectedBoard
									? 'Board detected'
									: board === 'virtual'
										? 'No PixelPlus board here — running on a computer'
										: 'Board set for this controller'}
							</div>
							<h1>{BOARDS[board].name}</h1>
							<p class="lead">{BOARDS[board].blurb}</p>
						{:else}
							<h1>{detected ? 'Choose your board' : 'Which board is this?'}</h1>
							<p class="lead">
								{detected
									? 'Pick the board this Raspberry Pi is plugged into.'
									: 'We couldn’t read the board’s ID chip — it may be blank. Pick the board you have.'}
							</p>
						{/if}
						<div class="boardpic"><BoardDiagram {board} {rev} compact={board !== 'difftxlarge'} /></div>
						{#if BOARDS[board].outputs}
							<div class="facts" aria-label="Board facts">
								{#if BOARDS[board].jacks > 1}<span><strong>{BOARDS[board].jacks}</strong> network jacks</span
									>{/if}
								<span><strong>{BOARDS[board].outputs}</strong> pixel outputs</span>
								{#if board === 'difftxlarge'}<span>Power &amp; temperature monitor</span>{/if}
							</div>
						{/if}
						{#if changingBoard || !detected}
							<div class="boards">
								{#each boardChoices as b (b)}
									<button
										class="bchoice"
										class:on={board === b}
										onclick={() => (board = b)}
										aria-pressed={board === b}
									>
										<strong>{BOARDS[b].name}</strong><span class="faint tiny"
											>{BOARDS[b].outputs ? `${BOARDS[b].outputs} outputs` : 'No outputs'}</span
										>
									</button>
								{/each}
							</div>
						{/if}
						{#if board === 'difftx'}
							<div class="revrow">
								<span class="small">Board revision</span>
								<div class="seg">
									{#each ['D', 'E'] as r (r)}<button
											class:on={rev === r}
											onclick={() => (rev = r)}
											aria-pressed={rev === r}>Rev {r}</button
										>{/each}
								</div>
							</div>
							{#if rev === 'D'}<div class="notice warn small">
									<TriangleAlert size={16} class="ico" /><span
										>Rev D boards need a short patch cable with pins 4 and 5 swapped on <strong>port 3</strong
										>. PixelPlus will remind you where it matters.</span
									>
								</div>{/if}
						{/if}
						{#if board === 'diffsmart'}<div class="notice info small">
								<Radio size={16} /><span
									>Set switch <strong>SW1</strong> on the board to <strong>PI</strong> so the Pi drives the outputs.</span
								>
							</div>{/if}
						{#if !detected && (board === 'difftx' || board === 'difftxlarge' || board === 'diffsmart')}
							<label class="chk"
								><input type="checkbox" class="check" bind:checked={writeEeprom} /> Save this on the board so it’s
								recognized automatically next time</label
							>
						{/if}
						<div class="nav">
							<button class="btn ghost" onclick={back}><ArrowLeft size={16} /> Back</button>
							{#if detected && !changingBoard}<button class="btn ghost" onclick={() => (changingBoard = true)}
									>That’s not right</button
								>{/if}
							<button class="btn primary" onclick={next}
								>{detected && !changingBoard ? 'Looks right' : 'Continue'} <ArrowRight size={16} /></button
							>
						</div>
					{:else if step === 3}
						<h1>Name your show</h1>
						<p class="lead">
							Visitors see this name on the song request page. Your location lets the show start at sunset.
						</p>
						<div class="col" style="gap:18px;text-align:left;width:100%">
							<label class="field"
								><span class="label">Show name</span><input
									class="input lg"
									placeholder="e.g. Chandler Family Lights"
									bind:value={showName}
								/></label
							>
							<div class="field">
								<span class="label">Where is your display?</span>
								<div class="row">
									<div class="input-group grow">
										<span class="prefix"><MapPin size={16} /></span><input
											class="input"
											placeholder="Search your town or city"
											bind:value={cityQ}
											aria-label="Search town or city"
										/>
									</div>
									<button class="btn" onclick={geolocate} aria-label="Use my location" title="Use my location"
										><LocateFixed size={16} /> <span class="hide-sm">Use my location</span></button
									>
								</div>
								{#if hits.length}
									<div class="hits">
										{#each hits as c (c.name + c.region)}
											<button
												class="hit"
												onclick={() => {
													loc = {
														lat: c.lat,
														lon: c.lon,
														timezone: c.tz,
														label: `${c.name}, ${c.region.split(',')[0]}`
													};
													cityQ = '';
												}}><strong>{c.name}</strong> <span class="faint small">{c.region}</span></button
											>
										{/each}
									</div>
								{/if}
								{#if loc.label}<div class="picked">
										<Check size={14} />
										{loc.label} <span class="faint small">({loc.lat.toFixed(2)}, {loc.lon.toFixed(2)})</span>
									</div>{/if}
							</div>
							<label class="field"
								><span class="label">Time zone</span><select class="select" bind:value={loc.timezone}
									>{#each tzs.includes(loc.timezone) ? tzs : [loc.timezone, ...tzs] as z (z)}<option value={z}
											>{z.replace(/_/g, ' ')}</option
										>{/each}</select
								></label
							>
						</div>
						<div class="nav">
							<button class="btn ghost" onclick={back}><ArrowLeft size={16} /> Back</button>
							<button class="btn primary" onclick={next} disabled={!showName.trim()}
								>Continue <ArrowRight size={16} /></button
							>
						</div>
					{:else if step === 4}
						<h1>What will you use?</h1>
						<p class="lead">
							PixelPlus can do a lot. Start with what most shows need and keep everything else out of your
							way.
						</p>
						<div class="roles presets" role="radiogroup" aria-label="Features">
							<button
								class="role"
								class:on={usePreset === 'essentials'}
								role="radio"
								aria-checked={usePreset === 'essentials'}
								onclick={() => (usePreset = 'essentials')}
							>
								<span class="ri accent"><Sparkles size={22} /></span>
								<strong>Essentials <span class="badge accent rec">Recommended</span></strong>
								<span class="muted small"
									>Props, controllers, sequences, playlists and schedule, plus the tools that help you build
									and fix your display.</span
								>
								{#if usePreset === 'essentials'}<span class="tick"><Check size={14} /></span>{/if}
							</button>
							<button
								class="role"
								class:on={usePreset === 'everything'}
								role="radio"
								aria-checked={usePreset === 'everything'}
								onclick={() => (usePreset = 'everything')}
							>
								<span class="ri blue"><Layers size={22} /></span>
								<strong>Everything</strong>
								<span class="muted small"
									>DJ voices, games, song requests, seasons, sensors, remote access and every other tool.</span
								>
								{#if usePreset === 'everything'}<span class="tick"><Check size={14} /></span>{/if}
							</button>
							<button
								class="role wide"
								class:on={usePreset === 'custom'}
								role="radio"
								aria-checked={usePreset === 'custom'}
								onclick={() => (usePreset = 'custom')}
							>
								<span class="row" style="gap:10px"
									><SlidersHorizontal size={18} /> <strong>Let me choose</strong></span
								>
								{#if usePreset === 'custom'}<span class="tick"><Check size={14} /></span>{/if}
							</button>
						</div>
						{#if usePreset === 'custom'}
							<div class="pick" aria-label="Choose features">
								{#each FEATURE_GROUPS as g (g.id)}
									<div class="pick-group">
										<span class="eyebrow">{g.label}</span>
										<div class="pick-chips">
											{#each FEATURES.filter((f) => f.group === g.id) as f (f.id)}
												<button
													class="chip"
													aria-pressed={!chosenDisabled.includes(f.id)}
													title={f.description}
													onclick={() => toggleCustom(f.id)}><f.icon size={14} /> {f.name}</button
												>
											{/each}
										</div>
									</div>
								{/each}
							</div>
						{/if}
						<p class="faint small">
							{FEATURES.length - chosenDisabled.length} of {FEATURES.length} features on
							{usePreset === 'essentials' ? `(${ESSENTIALS.length} tools)` : ''} · You can change this any time
							in
							<strong>Settings → Features</strong>.
						</p>
						<div class="nav">
							<button class="btn ghost" onclick={back}><ArrowLeft size={16} /> Back</button>
							<button class="btn primary" onclick={next}>Continue <ArrowRight size={16} /></button>
						</div>
					{:else if step === 5}
						<div class="ri accent big"><LockKeyhole size={28} /></div>
						<h1>Add a password?</h1>
						<p class="lead">
							Optional. Without one, anyone on your home network can open this page. The song request page for
							visitors is always open.
						</p>
						<div class="col" style="gap:12px;width:100%;max-width:360px">
							<input
								class="input lg"
								type="password"
								placeholder="Password (at least 6 characters)"
								bind:value={password}
								autocomplete="new-password"
								aria-label="Password"
							/>
							<input
								class="input lg"
								type="password"
								placeholder="Repeat password"
								bind:value={password2}
								autocomplete="new-password"
								aria-label="Repeat password"
							/>
							{#if password && password.length < 6}<span class="small" style="color:var(--red)"
									>Use at least 6 characters</span
								>{:else if password2 && password !== password2}<span class="small" style="color:var(--red)"
									>Passwords don’t match</span
								>{/if}
						</div>
						<div class="nav">
							<button class="btn ghost" onclick={back}><ArrowLeft size={16} /> Back</button>
							<button
								class="btn ghost"
								onclick={() => {
									password = password2 = '';
									finish('leader');
								}}
								disabled={busy}>Skip</button
							>
							<button
								class="btn primary"
								onclick={() => finish('leader')}
								disabled={busy || password.length < 6 || password !== password2}
								>{busy ? 'Finishing…' : 'Finish setup'} <ArrowRight size={16} /></button
							>
						</div>
					{:else if step === 10}
						<div class="ri accent big"><ArchiveRestore size={28} /></div>
						<h1>Restore a show</h1>
						<p class="lead">
							This controller takes over from the one the transfer file was made on: its show, sequences,
							music, followers and name. The old controller must stay switched off.
						</p>
						<div class="col" style="gap:12px;width:100%;max-width:420px">
							<label class="field">
								<span class="label">Transfer file (.ppxfer)</span>
								<input
									class="input"
									type="file"
									accept=".ppxfer"
									onchange={(e) => (xferFile = (e.currentTarget as HTMLInputElement).files?.[0] ?? null)}
								/>
							</label>
							<label class="field">
								<span class="label">Passphrase</span>
								<input
									class="input lg"
									type="password"
									autocomplete="off"
									bind:value={xferPass}
									placeholder="The passphrase you chose when downloading it"
								/>
							</label>
							{#if xferPct !== null}
								<div
									class="bar"
									role="progressbar"
									aria-valuenow={xferPct}
									aria-valuemin={0}
									aria-valuemax={100}
								>
									<span style="width:{xferPct}%"></span>
								</div>
								<span class="faint small"
									>{xferPct < 100
										? `Uploading and checking… ${xferPct}%`
										: 'Putting the show in place…'}</span
								>
							{/if}
							{#if xferError}
								<span class="small" style="color:var(--red)"><TriangleAlert size={14} /> {xferError}</span>
							{/if}
						</div>
						<div class="nav">
							<button
								class="btn ghost"
								onclick={() => {
									dir = -1;
									step = 1;
								}}
								disabled={busy}><ArrowLeft size={16} /> Back</button
							>
							<button
								class="btn primary"
								onclick={restore}
								disabled={busy || !xferFile || [...xferPass].length < MIN_PASSPHRASE}
								>{busy ? 'Restoring…' : 'Restore'} <ArrowRight size={16} /></button
							>
						</div>
					{:else}
						<div class="celebrate"><Sparkles size={34} /></div>
						<h1>{showName || 'Your show'} is ready</h1>
						{#if restoreNotes.length}
							<div class="notes">
								{#each restoreNotes as n (n)}<p class="small"><TriangleAlert size={14} /> {n}</p>{/each}
							</div>
						{/if}
						<p class="lead">Here’s what to do next. You can come back to any of these at any time.</p>
						<ol class="next">
							<li>
								<a href="/props"
									><span class="n">1</span><span class="ic"><FileUp size={18} /></span><span class="grow"
										><strong>Import your xLights layout</strong><span class="faint small"
											>Brings in every prop at once</span
										></span
									><ArrowRight size={16} /></a
								>
							</li>
							<li>
								<a href="/sequences"
									><span class="n">2</span><span class="ic"><Music size={18} /></span><span class="grow"
										><strong>Upload sequences & songs</strong><span class="faint small"
											>Your light sequences from xLights, with their music</span
										></span
									><ArrowRight size={16} /></a
								>
							</li>
							<li>
								<a href="/playlists"
									><span class="n">3</span><span class="ic"><ListMusic size={18} /></span><span class="grow"
										><strong>Build a playlist</strong><span class="faint small"
											>Your show’s running order</span
										></span
									><ArrowRight size={16} /></a
								>
							</li>
							<li>
								<a href="/schedule"
									><span class="n">4</span><span class="ic"><CalendarClock size={18} /></span><span
										class="grow"
										><strong>Schedule it</strong><span class="faint small">Start at sunset every night</span
										></span
									><ArrowRight size={16} /></a
								>
							</li>
						</ol>
						<a class="btn primary lg" href="/">Go to dashboard</a>
					{/if}
				</section>
			{/key}
		</main>
	</div>
{/if}

<style>
	.wiz {
		min-height: 100dvh;
		display: flex;
		flex-direction: column;
		position: relative;
		overflow: hidden;
	}
	.glow {
		position: absolute;
		inset: 0;
		background: radial-gradient(ellipse 55% 45% at 50% 22%, rgba(245, 165, 36, 0.13), transparent 100%);
		pointer-events: none;
	}
	.top {
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 20px 28px;
		position: relative;
	}
	.brand {
		font-weight: 650;
		letter-spacing: -0.01em;
	}
	.dots {
		display: flex;
		gap: 6px;
		list-style: none;
		margin: 0;
		padding: 0;
	}
	.dots li {
		width: 28px;
		height: 4px;
		border-radius: 4px;
		background: var(--surface-3);
		transition: background 250ms;
	}
	.dots li.done {
		background: var(--accent-line);
	}
	.dots li.on {
		background: var(--accent);
	}
	.stage {
		flex: 1;
		display: grid;
		place-items: center;
		padding: 24px 20px 48px;
		position: relative;
	}
	.panel {
		grid-area: 1 / 1;
		width: min(720px, 100%);
		display: flex;
		flex-direction: column;
		align-items: center;
		text-align: center;
		gap: 16px;
	}
	.panel h1 {
		font-size: 32px;
		letter-spacing: -0.03em;
	}
	.lead {
		color: var(--text-2);
		font-size: 16px;
		max-width: 540px;
		margin-bottom: 8px;
	}
	.hero {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 16px;
	}
	.hero h1 {
		font-size: 40px;
	}
	.bigmark {
		filter: drop-shadow(0 16px 50px rgba(245, 165, 36, 0.45));
		margin-bottom: 8px;
		animation: float 4s ease-in-out infinite;
	}
	@keyframes float {
		50% {
			transform: translateY(-6px);
		}
	}
	.restore-link {
		margin-top: 12px;
	}
	.bar {
		height: 6px;
		border-radius: 3px;
		background: var(--surface-3);
		overflow: hidden;
	}
	.bar span {
		display: block;
		height: 100%;
		background: var(--accent);
		transition: width 0.2s;
	}
	.notes {
		display: grid;
		gap: 4px;
		max-width: 520px;
		color: var(--text-2);
	}
	.notes p {
		margin: 0;
	}
	.roles {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: 16px;
		width: 100%;
	}
	.role {
		position: relative;
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: 10px;
		padding: 24px;
		border-radius: 20px;
		border: 1.5px solid var(--border-2);
		background: var(--surface);
		text-align: left;
		transition: all 180ms var(--ease);
	}
	.role:hover {
		border-color: var(--border-3);
		transform: translateY(-2px);
	}
	.role.on {
		border-color: var(--accent);
		background: linear-gradient(160deg, var(--accent-soft), transparent 70%), var(--surface);
		box-shadow: 0 10px 40px rgba(245, 165, 36, 0.12);
	}
	.role strong {
		font-size: 16px;
	}
	.presets .role.wide {
		grid-column: 1 / -1;
		flex-direction: row;
		align-items: center;
		padding: 16px 24px;
	}
	.rec {
		margin-left: 6px;
		vertical-align: 2px;
	}
	.pick {
		width: 100%;
		display: flex;
		flex-direction: column;
		gap: 14px;
		text-align: left;
	}
	.pick-group {
		display: flex;
		flex-direction: column;
		gap: 8px;
	}
	.pick-chips {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
	}
	.ri {
		width: 48px;
		height: 48px;
		border-radius: 14px;
		display: grid;
		place-items: center;
	}
	.ri.accent {
		background: var(--accent-soft);
		color: var(--accent-text);
	}
	.ri.blue {
		background: var(--blue-soft);
		color: var(--blue);
	}
	.ri.big {
		width: 64px;
		height: 64px;
		border-radius: 20px;
	}
	.tick {
		position: absolute;
		top: 16px;
		right: 16px;
		width: 24px;
		height: 24px;
		border-radius: 50%;
		display: grid;
		place-items: center;
		background: var(--accent);
		color: var(--accent-fg);
	}
	.nav {
		display: flex;
		gap: 8px;
		justify-content: center;
		margin-top: 16px;
		flex-wrap: wrap;
	}
	.found {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		color: var(--green);
		font-weight: 600;
		font-size: 13px;
		padding: 5px 12px;
		border-radius: 99px;
		background: var(--green-soft);
	}
	.boardpic {
		width: 100%;
		max-width: 620px;
		margin: 8px 0;
	}
	.facts {
		display: flex;
		flex-wrap: wrap;
		justify-content: center;
		gap: 8px;
		margin-bottom: 8px;
	}
	.facts span {
		padding: 6px 12px;
		border-radius: 99px;
		background: var(--surface-2);
		border: 1px solid var(--border);
		font-size: 13px;
		color: var(--text-2);
	}
	.facts strong {
		color: var(--text);
	}
	.boards {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(130px, 1fr));
		gap: 8px;
		width: 100%;
	}
	.bchoice {
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: 2px;
		padding: 12px;
		border-radius: 12px;
		border: 1.5px solid var(--border-2);
		background: var(--surface);
		text-align: left;
		font-size: 12.5px;
	}
	.bchoice.on {
		border-color: var(--accent);
		background: var(--accent-soft);
	}
	.revrow {
		display: flex;
		align-items: center;
		gap: 12px;
	}
	.seg {
		display: flex;
		gap: 2px;
		padding: 3px;
		border-radius: 10px;
		background: var(--surface-2);
		border: 1px solid var(--border);
	}
	.seg button {
		height: 30px;
		padding: 0 14px;
		border-radius: 7px;
		font-size: 13px;
		color: var(--text-2);
	}
	.seg button.on {
		background: var(--surface-3);
		color: var(--text);
	}
	.chk {
		display: flex;
		align-items: center;
		gap: 10px;
		font-size: 13px;
		color: var(--text-2);
		cursor: pointer;
	}
	.input.lg {
		height: 52px;
		font-size: 17px;
		border-radius: 12px;
	}
	.hits {
		display: flex;
		flex-direction: column;
		border: 1px solid var(--border-2);
		border-radius: 12px;
		overflow: hidden;
		background: var(--surface);
	}
	.hit {
		text-align: left;
		padding: 11px 14px;
		border-bottom: 1px solid var(--border);
	}
	.hit:last-child {
		border-bottom: 0;
	}
	.hit:hover {
		background: var(--accent-soft);
	}
	.picked {
		display: flex;
		align-items: center;
		gap: 6px;
		color: var(--green);
		font-size: 13.5px;
		font-weight: 560;
	}
	.celebrate {
		width: 76px;
		height: 76px;
		border-radius: 24px;
		display: grid;
		place-items: center;
		color: var(--accent-fg);
		background: linear-gradient(135deg, #ffc45c, #f08a0c);
		box-shadow: 0 16px 50px rgba(245, 165, 36, 0.4);
		animation: pop 600ms var(--ease-spring);
	}
	@keyframes pop {
		from {
			transform: scale(0.6);
			opacity: 0;
		}
	}
	.next {
		list-style: none;
		margin: 0 0 12px;
		padding: 0;
		width: 100%;
		max-width: 520px;
		display: flex;
		flex-direction: column;
		gap: 8px;
		text-align: left;
	}
	.next a {
		display: flex;
		align-items: center;
		gap: 14px;
		padding: 14px 16px;
		border-radius: 14px;
		border: 1px solid var(--border-2);
		background: var(--surface);
		transition: all 160ms var(--ease);
	}
	.next a:hover {
		border-color: var(--accent-line);
		transform: translateX(3px);
	}
	.next .grow {
		display: flex;
		flex-direction: column;
	}
	.n {
		width: 24px;
		height: 24px;
		border-radius: 50%;
		display: grid;
		place-items: center;
		font-size: 12px;
		font-weight: 700;
		background: var(--surface-3);
		color: var(--text-2);
	}
	.ic {
		color: var(--accent-text);
		display: flex;
	}
	@media (max-width: 640px) {
		.roles {
			grid-template-columns: 1fr;
		}
		.panel h1 {
			font-size: 26px;
		}
		.hero h1 {
			font-size: 30px;
		}
		.lead {
			font-size: 15px;
		}
		.top {
			padding: 16px;
		}
		.hide-sm {
			display: none;
		}
	}
</style>
