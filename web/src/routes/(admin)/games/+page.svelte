<script lang="ts">
	import { untrack } from 'svelte';
	import { api } from '$lib/api/client';
	import type { GameSettings, GamesStatus } from '$lib/api/types';
	import { app } from '$lib/stores/app.svelte';
	import { toasts, confirm } from '$lib/stores/toasts.svelte';
	import { fmtBytes } from '$lib/util/format';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import Switch from '$lib/components/ui/Switch.svelte';
	import Segmented from '$lib/components/ui/Segmented.svelte';
	import Skeleton from '$lib/components/ui/Skeleton.svelte';
	import QrCode from '$lib/components/viz/QrCode.svelte';
	import PropPreview from '$lib/components/viz/PropPreview.svelte';
	import {
		Gamepad2,
		Users,
		Timer,
		Megaphone,
		Square,
		Grid3x3,
		UploadCloud,
		Trash2,
		Joystick,
		TriangleAlert,
		Copy,
		Globe,
		Image,
		Info,
		Check
	} from '@lucide/svelte';

	const show = $derived(app.show);
	let g = $state<GameSettings | null>(null);
	let status = $state<GamesStatus | null>(null);
	let saveState = $state<'saved' | 'saving' | 'dirty'>('saved');
	let timer: ReturnType<typeof setTimeout>;
	let romInput: HTMLInputElement | undefined = $state();
	let romProgress = $state<number | null>(null);
	let levelsAll = $state(true);

	$effect(() => {
		const src = show?.settings.games;
		if (!src) return;
		untrack(() => {
			if (saveState !== 'saved' && g) return;
			const copy = structuredClone($state.snapshot(src) as GameSettings);
			g = copy;
			levelsAll = !copy.levels.trim() || copy.levels.trim() === 'all';
		});
	});
	async function poll() {
		status = await api.games.status().catch(() => null);
	}
	$effect(() => {
		poll();
		const t = setInterval(poll, 3000);
		return () => clearInterval(t);
	});

	function changed() {
		saveState = 'dirty';
		clearTimeout(timer);
		timer = setTimeout(save, 600);
	}
	async function save() {
		if (!g) return;
		saveState = 'saving';
		try {
			await api.saveSettings({ games: $state.snapshot(g) as GameSettings });
			await app.reloadShow();
			saveState = 'saved';
		} catch (e) {
			toasts.error('Could not save game settings', (e as Error).message);
			saveState = 'dirty';
		}
	}

	async function act(fn: () => Promise<unknown>, ok: string) {
		try {
			await fn();
			toasts.success(ok);
			poll();
		} catch (e) {
			toasts.error('That didn’t work', (e as Error).message);
		}
	}
	async function uploadRom(e: Event) {
		const f = (e.target as HTMLInputElement).files?.[0];
		(e.target as HTMLInputElement).value = '';
		if (!f) return;
		romProgress = 0;
		try {
			await api.games.uploadRom(f, (p) => (romProgress = p));
			toasts.success(`Added ${f.name}`);
			poll();
		} catch (err) {
			toasts.error('Upload failed', (err as Error).message);
		} finally {
			romProgress = null;
		}
	}
	async function removeRom(name: string) {
		if (!(await confirm({ title: `Remove ${name}?`, confirmLabel: 'Remove', danger: true }))) return;
		act(() => api.games.removeRom(name), 'Removed');
	}

	const matrices = $derived(show?.props.filter((p) => p.kind === 'matrix' || p.matrix) ?? []);
	const matrix = $derived(matrices.find((p) => p.id === g?.matrixPropId));
	const url = $derived(
		g?.publicUrl ? (g.publicUrl.startsWith('http') ? g.publicUrl : `https://${g.publicUrl}`) : ''
	);
	const localUrl = $derived(`http://${app.system?.ips?.[0] ?? 'pixelplus.local'}:${g?.port ?? 8088}`);
	const hasMario = $derived(status?.roms?.some((r) => /mario/i.test(r.name)) ?? false);
</script>

{#snippet num(
	label: string,
	key: keyof GameSettings,
	min: number,
	max: number,
	step: number,
	suffix: string,
	tip?: string
)}
	<div class="setting stack">
		<div class="text">
			<div class="title">{label}</div>
			{#if tip}<div class="desc">{tip}</div>{/if}
		</div>
		<div class="control">
			<div class="input-group" style="width:150px">
				<input
					class="input num"
					type="number"
					{min}
					{max}
					{step}
					value={g?.[key] as number}
					oninput={(e) => {
						if (g) {
							(g as any)[key] = Number((e.target as HTMLInputElement).value);
							changed();
						}
					}}
					aria-label={label}
				/>
				<span class="suffix">{suffix}</span>
			</div>
		</div>
	</div>
{/snippet}

{#snippet toggle(label: string, key: keyof GameSettings, tip?: string)}
	<div class="setting">
		<div class="text">
			<div class="title">{label}</div>
			{#if tip}<div class="desc">{tip}</div>{/if}
		</div>
		<div class="control">
			<Switch
				checked={!!g?.[key]}
				{label}
				onchange={(v) => {
					if (g) {
						(g as any)[key] = v;
						changed();
					}
				}}
			/>
		</div>
	</div>
{/snippet}

<div class="page">
	<PageHeader
		title="Games"
		subtitle="Let people walking by play Super Mario Bros. on your matrix — their phone is the controller."
	>
		{#snippet actions()}
			<span class="faint small save"
				>{saveState === 'saving'
					? 'Saving…'
					: saveState === 'dirty'
						? 'Unsaved'
						: ''}{#if saveState === 'saved'}<Check size={13} /> Saved{/if}</span
			>
		{/snippet}
	</PageHeader>

	{#if !show || !g}
		<div class="card card-pad"><Skeleton count={8} h={30} /></div>
	{:else}
		<section class="hero card" class:on={g.enabled}>
			<div class="hero-main">
				<span class="gp"><Gamepad2 size={30} /></span>
				<div class="grow">
					<h2>{g.enabled ? (g.arcadeMode ? 'Arcade is open' : 'Games are on') : 'Games are off'}</h2>
					<p class="muted small">
						{g.enabled
							? `Visitors reach the controller at ${g.publicUrl || localUrl}`
							: 'Turn on to open the phone controller page for visitors.'}
					</p>
				</div>
				<Switch bind:checked={g.enabled} label="Enable games" onchange={changed} />
			</div>
			<div class="stats">
				<div class="stat">
					<span class="faint tiny">Now</span><strong
						>{status
							? status.running
								? `Playing${status.player ? ` · ${status.player}` : ''}`
								: status.cooldownS > 0
									? 'Cooling down'
									: g.enabled
										? 'Ready'
										: 'Off'
							: '—'}</strong
					>
				</div>
				<div class="stat">
					<span class="faint tiny"><Users size={12} /> In line</span><strong class="num"
						>{status?.queueLength ?? 0}</strong
					>
				</div>
				<div class="stat">
					<span class="faint tiny"><Timer size={12} /> Cooldown</span><strong class="num"
						>{status?.cooldownS
							? `${Math.floor(status.cooldownS / 60)}:${String(status.cooldownS % 60).padStart(2, '0')}`
							: '—'}</strong
					>
				</div>
				<div class="stat actions">
					<button
						class="btn sm"
						onclick={() => act(api.games.invite, 'Invite is flashing on the matrix')}
						disabled={!g.enabled}><Megaphone size={14} /> Show invite</button
					>
					<button
						class="btn sm"
						onclick={() => act(api.games.stop, 'Game stopped')}
						disabled={!status?.running}><Square size={13} /> Stop game</button
					>
				</div>
			</div>
			{#if status?.lastError}<div class="notice danger small" style="margin:0 20px 16px">
					<TriangleAlert size={16} /><span>{status.lastError}</span>
				</div>{/if}
			{#if !hasMario && status}<div class="notice warn small" style="margin:0 20px 16px">
					<TriangleAlert size={16} class="ico" /><span
						>Upload your own Super Mario Bros. <strong>.nes</strong> file below — it isn’t included, and it’s never
						sent to visitors.</span
					>
				</div>{/if}
		</section>

		<div class="cols">
			<div class="col" style="gap:16px">
				<section class="card">
					<div class="card-head">
						<Grid3x3 size={18} />
						<h2 class="grow">Matrix</h2>
					</div>
					<div class="card-body">
						{#if !matrices.length}
							<p class="muted small">No matrix props yet. Import one from xLights on the Props page.</p>
						{:else}
							<div class="row wrap" style="gap:12px">
								<select
									class="select"
									style="max-width:280px"
									bind:value={g.matrixPropId}
									onchange={changed}
									aria-label="Matrix prop"
								>
									<option value={undefined}>Choose a matrix…</option>
									{#each matrices as m (m.id)}<option value={m.id}
											>{m.name}{m.matrix ? ` (${m.matrix.width}×${m.matrix.height})` : ''}</option
										>{/each}
								</select>
								<button
									class="btn"
									disabled={!g.matrixPropId}
									onclick={() =>
										g?.matrixPropId &&
										act(
											() => api.games.testPattern(g!.matrixPropId!),
											'Test pattern is showing on the matrix'
										)}><Image size={15} /> Test pattern on matrix</button
								>
							</div>
							{#if matrix}
								<div class="mpv"><PropPreview prop={matrix} height={150} /></div>
								<p class="faint small">
									The test pattern shows a blue border, a red top-left corner, a green top-right corner and
									the matrix size in the middle. If the corners are swapped, fix the matrix orientation in
									xLights and re-import.
								</p>
							{/if}
						{/if}
					</div>
				</section>

				<section class="card">
					<div class="card-head">
						<Gamepad2 size={18} />
						<h2 class="grow">Game</h2>
					</div>
					<div class="card-body tight">
						{@render num(
							'Game length',
							'gameSeconds',
							10,
							600,
							5,
							'seconds',
							'How long each visitor gets to play.'
						)}
						{@render num(
							'Cooldown after a game',
							'cooldownMinutes',
							0,
							1440,
							1,
							'min',
							'No invites and no new games for this long; phones show a countdown. 0 = none.'
						)}
						<div class="setting stack">
							<div class="text">
								<div class="title">Levels</div>
								<div class="desc">One is picked at random for each game.</div>
							</div>
							<div class="control" style="flex-wrap:wrap">
								<Segmented
									value={levelsAll ? 'all' : 'pick'}
									size="sm"
									label="Levels"
									options={[
										{ value: 'all', label: 'All levels' },
										{ value: 'pick', label: 'Only some' }
									]}
									onchange={(v) => {
										levelsAll = v === 'all';
										if (g) {
											g.levels = levelsAll ? '' : g.levels || '1-1,1-2,4-1';
											changed();
										}
									}}
								/>
								{#if !levelsAll}<input
										class="input sm"
										style="width:180px"
										placeholder="1-1,1-2,4-1"
										bind:value={g.levels}
										oninput={changed}
										aria-label="Levels"
									/>{/if}
							</div>
						</div>
						<div class="setting stack">
							<div class="text">
								<div class="title">Games allowed</div>
								<div class="desc">Only while the show plays keeps people from playing at 3 am.</div>
							</div>
							<div class="control">
								<Segmented
									bind:value={g.playWindow}
									size="sm"
									label="Games allowed"
									onchange={changed}
									options={[
										{ value: 'duringShow', label: 'During the show' },
										{ value: 'anytime', label: 'Any time' }
									]}
								/>
							</div>
						</div>
						{@render toggle(
							'Pause the show during a game',
							'pauseShow',
							'Pauses lights and music for the game and picks up exactly where it left off.'
						)}
						{@render toggle('Santa hat on Mario', 'santaHat')}
					</div>
				</section>

				<section class="card">
					<div class="card-head">
						<Joystick size={18} />
						<h2 class="grow">Arcade mode</h2>
						<Switch bind:checked={g.arcadeMode} label="Arcade mode" onchange={changed} />
					</div>
					<div class="card-body tight">
						<p class="muted small" style="padding-bottom:6px">
							A full-time NES arcade: the show stops, the matrix lists every game you’ve uploaded, and
							visitors take turns. Holding SELECT+START+B+A for 2 seconds returns to the list.
						</p>
						{#if g.arcadeMode}
							<div class="notice warn small">
								<TriangleAlert size={16} class="ico" /><span
									>While the arcade is open your playlist stays stopped, even if the schedule tries to start
									it.</span
								>
							</div>
						{/if}
						{@render num(
							'Turn length',
							'arcadeMinutes',
							0,
							1440,
							1,
							'min',
							'How long each visitor keeps the controller. 0 = until they leave or go idle.'
						)}
						{@render num(
							'End a turn after idle',
							'arcadeIdleSeconds',
							0,
							3600,
							5,
							'seconds',
							'The next person in line gets the controller. 0 = never.'
						)}
					</div>
				</section>

				<section class="card">
					<div class="card-head">
						<Image size={18} />
						<h2 class="grow">Picture & sound</h2>
					</div>
					<div class="card-body tight">
						<div class="setting stack">
							<div class="text">
								<div class="title">Picture fit</div>
								<div class="desc">
									Fit keeps Mario’s proportions with black bars at the sides. Stretch fills the whole matrix.
								</div>
							</div>
							<div class="control">
								<Segmented
									bind:value={g.scaleMode}
									size="sm"
									label="Picture fit"
									onchange={changed}
									options={[
										{ value: 'fit', label: 'Fit' },
										{ value: 'stretch', label: 'Stretch' }
									]}
								/>
							</div>
						</div>
						<div class="setting stack">
							<div class="text">
								<div class="title">Matrix frame rate</div>
								<div class="desc">The game always runs at 60 fps; this is how often the matrix updates.</div>
							</div>
							<div class="control">
								<Segmented
									bind:value={g.outputFps}
									size="sm"
									label="Frame rate"
									onchange={changed}
									options={[
										{ value: 40, label: '40 fps' },
										{ value: 20, label: '20 fps' }
									]}
								/>
							</div>
						</div>
						<div class="setting stack">
							<div class="text"><div class="title">Brightness</div></div>
							<div class="control slider-c">
								<input
									type="range"
									class="range"
									min="5"
									max="100"
									step="5"
									bind:value={g.brightness}
									oninput={changed}
									style:--pct="{g.brightness}%"
									aria-label="Game brightness"
								/><span class="num small" style="width:40px">{g.brightness}%</span>
							</div>
						</div>
						<div class="setting stack">
							<div class="text">
								<div class="title">Game volume</div>
								<div class="desc">Relative to the show volume.</div>
							</div>
							<div class="control slider-c">
								<input
									type="range"
									class="range"
									min="0"
									max="100"
									step="5"
									bind:value={g.volume}
									oninput={changed}
									style:--pct="{g.volume}%"
									aria-label="Game volume"
								/><span class="num small" style="width:40px">{g.volume}%</span>
							</div>
						</div>
						<details class="adv">
							<summary>Advanced · crop & port</summary>
							<p class="faint small">
								Which part of the 256×240 NES screen is shown in Mario mode. The defaults drop the score bar
								and the blank left column.
							</p>
							<div class="crop">
								{#each ['Left', 'Top', 'Right', 'Bottom'] as side, i (side)}
									<label class="field"
										><span class="label">{side}</span><input
											class="input sm num"
											type="number"
											min="0"
											max={i % 2 ? 240 : 256}
											value={g.crop[i]}
											oninput={(e) => {
												if (g) {
													g.crop[i] = Number((e.target as HTMLInputElement).value);
													changed();
												}
											}}
										/></label
									>
								{/each}
							</div>
							{@render num(
								'Controller port',
								'port',
								1024,
								65535,
								1,
								'',
								'Point your tunnel or port forward at this port only — never at PixelPlus’s own port 80.'
							)}
						</details>
					</div>
				</section>
			</div>

			<div class="col" style="gap:16px">
				<section class="card">
					<div class="card-head">
						<Megaphone size={18} />
						<h2 class="grow">Invite</h2>
					</div>
					<div class="card-body">
						<label class="field"
							><span class="label">Public URL</span>
							<div class="input-group">
								<span class="prefix"><Globe size={15} /></span><input
									class="input"
									placeholder="play.yourlights.com"
									bind:value={g.publicUrl}
									oninput={changed}
								/>
							</div>
							<span class="hint"
								>What visitors type or scan. It must reach the controller port (e.g. through a Cloudflare
								Tunnel). Short URLs read best — about 20 characters fit on an 80×40 matrix.</span
							>
						</label>
						{#if url}
							<div class="qr">
								<QrCode text={url} size={148} />
								<div class="col" style="gap:6px">
									<strong class="small">{g.publicUrl}</strong>
									<span class="faint tiny"
										>This is what flashes on the matrix when the invite style is “QR code”.</span
									>
									<button
										class="btn sm"
										onclick={() => {
											navigator.clipboard?.writeText(url);
											toasts.success('Link copied');
										}}><Copy size={13} /> Copy link</button
									>
								</div>
							</div>
						{/if}
						<div class="col" style="gap:0;margin-top:8px">
							{@render num(
								'Show the invite every',
								'inviteEveryMinutes',
								0,
								1440,
								1,
								'min',
								'0 = only when a playlist runs “Show game invite”.'
							)}
							<div class="setting stack">
								<div class="text"><div class="title">Invite style</div></div>
								<div class="control">
									<Segmented
										bind:value={g.inviteStyle}
										size="sm"
										label="Invite style"
										onchange={changed}
										options={[
											{ value: 'text', label: 'URL text' },
											{ value: 'qr', label: 'QR code' },
											{ value: 'alternate', label: 'Both' }
										]}
									/>
								</div>
							</div>
							{@render num('Flashes each time', 'inviteFlashes', 1, 10, 1, '')}
							<div class="setting">
								<div class="text"><div class="title">URL text color</div></div>
								<div class="control">
									<input
										type="color"
										bind:value={g.inviteColor}
										oninput={changed}
										aria-label="Invite color"
									/>
								</div>
							</div>
						</div>
					</div>
				</section>

				<section class="card">
					<div class="card-head">
						<UploadCloud size={18} />
						<h2 class="grow">Games (ROMs)</h2>
						<button class="btn sm" onclick={() => romInput?.click()}
							><UploadCloud size={14} /> Upload .nes</button
						>
					</div>
					<input bind:this={romInput} type="file" accept=".nes" class="sr-only" onchange={uploadRom} />
					{#if romProgress != null}<div class="card-body" style="padding-bottom:0">
							<div class="progress"><span style:width="{romProgress * 100}%"></span></div>
						</div>{/if}
					<div class="list">
						{#each status?.roms ?? [] as r (r.name)}
							<div class="list-row">
								<span class="icon-tile" style="width:34px;height:34px;border-radius:9px"
									><Gamepad2 size={16} /></span
								>
								<div class="grow">
									<div class="ellipsis small"><strong>{r.name}</strong></div>
									<div class="faint tiny">{fmtBytes(r.sizeBytes)}</div>
								</div>
								<button
									class="btn ghost icon sm"
									onclick={() => removeRom(r.name)}
									aria-label="Remove {r.name}"><Trash2 size={14} /></button
								>
							</div>
						{:else}
							<div class="card-body faint small">No games uploaded yet.</div>
						{/each}
					</div>
					<div class="card-body faint tiny" style="padding-top:8px">
						<Info size={12} /> Use backups of games you own. ROMs stay on the controller; phones only send button
						presses.
					</div>
				</section>
			</div>
		</div>
	{/if}
</div>

<style>
	.save {
		display: inline-flex;
		align-items: center;
		gap: 4px;
		align-self: center;
	}
	.hero {
		overflow: hidden;
		margin-bottom: 20px;
	}
	.hero.on {
		border-color: color-mix(in srgb, var(--green) 40%, transparent);
		background: linear-gradient(110deg, var(--green-soft), transparent 50%), var(--surface);
	}
	.hero-main {
		display: flex;
		align-items: center;
		gap: 16px;
		padding: 20px;
	}
	.gp {
		width: 56px;
		height: 56px;
		border-radius: 16px;
		display: grid;
		place-items: center;
		background: linear-gradient(135deg, #e7382b, #b3140a);
		color: #fff;
		box-shadow: 0 8px 24px rgba(231, 56, 43, 0.35);
		flex: 0 0 auto;
	}
	.hero h2 {
		font-size: 18px;
	}
	.stats {
		display: flex;
		gap: 24px;
		padding: 14px 20px;
		border-top: 1px solid var(--border);
		flex-wrap: wrap;
		align-items: center;
	}
	.stat {
		display: flex;
		flex-direction: column;
		gap: 2px;
	}
	.stat .faint {
		display: flex;
		align-items: center;
		gap: 4px;
	}
	.stat.actions {
		flex-direction: row;
		gap: 8px;
		margin-left: auto;
	}
	.cols {
		display: grid;
		grid-template-columns: minmax(0, 1.2fr) minmax(0, 1fr);
		gap: 16px;
		align-items: start;
	}
	.card-body.tight {
		padding-top: 4px;
		padding-bottom: 8px;
	}
	.mpv {
		margin: 14px 0 10px;
		border-radius: 12px;
		overflow: hidden;
	}
	.slider-c {
		width: 220px;
	}
	.qr {
		display: flex;
		gap: 16px;
		align-items: center;
		margin-top: 14px;
		padding: 14px;
		border-radius: 14px;
		background: var(--surface-2);
	}
	.qr :global(svg) {
		border-radius: 10px;
		flex: 0 0 auto;
	}
	.adv {
		padding: 12px 0 4px;
	}
	.adv summary {
		cursor: pointer;
		font-weight: 560;
		font-size: 13px;
		color: var(--text-2);
		margin-bottom: 8px;
	}
	.crop {
		display: grid;
		grid-template-columns: repeat(4, 1fr);
		gap: 8px;
		margin: 10px 0;
	}
	@media (max-width: 1100px) {
		.cols {
			grid-template-columns: 1fr;
		}
	}
	@media (max-width: 640px) {
		.slider-c {
			width: 100%;
		}
		.stat.actions {
			margin-left: 0;
			width: 100%;
		}
		.stat.actions .btn {
			flex: 1;
		}
		.qr {
			flex-direction: column;
			align-items: flex-start;
		}
	}
</style>
