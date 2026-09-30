<script lang="ts">
	import { page } from '$app/state';
	import { api } from '$lib/api/client';
	import type { PlaylistItem } from '$lib/api/types';
	import { app } from '$lib/stores/app.svelte';
	import { toasts } from '$lib/stores/toasts.svelte';
	import { fmtDuration } from '$lib/util/format';
	import { togglePlay, stopShow, setLightsOff, LIGHTS_OFF_HELP } from '$lib/player';
	import LayoutCanvas from '$lib/components/viz/LayoutCanvas.svelte';
	import {
		Play,
		Pause,
		Square,
		SkipForward,
		SkipBack,
		Volume2,
		VolumeX,
		SunMedium,
		Power,
		Mic,
		Music,
		WandSparkles,
		Clock,
		FlaskConical,
		ChevronDown,
		ListMusic
	} from '@lucide/svelte';
	import { fly, fade } from 'svelte/transition';

	const st = $derived(app.status);
	const playing = $derived(st?.state === 'playing');
	const active = $derived(!!st && st.state !== 'idle');
	const pct = $derived(st && st.durationMs ? Math.min(100, (st.posMs / st.durationMs) * 100) : 0);

	let volume = $state(80);
	let brightness = $state(100);
	let dragVol = false;
	let dragBri = false;
	let scrub = $state<number | null>(null);
	let expanded = $state(false);

	$effect(() => {
		if (st && !dragVol) volume = st.volume;
		if (st && !dragBri) brightness = st.brightness;
	});

	async function act(fn: () => Promise<unknown>) {
		try {
			await fn();
		} catch (e) {
			toasts.error('Player command failed', e instanceof Error ? e.message : undefined);
		}
	}

	function itemIcon(type?: string) {
		if (type === 'dj') return Mic;
		if (type === 'effect') return WandSparkles;
		if (type === 'pause') return Clock;
		if (type === 'test') return FlaskConical;
		return Music;
	}
	const ItemIcon = $derived(itemIcon(st?.item?.type));

	let volTimer: ReturnType<typeof setTimeout>;
	function onVol() {
		dragVol = true;
		clearTimeout(volTimer);
		volTimer = setTimeout(() => {
			act(() => api.setVolume(volume));
			dragVol = false;
		}, 120);
	}
	let briTimer: ReturnType<typeof setTimeout>;
	function onBri() {
		dragBri = true;
		clearTimeout(briTimer);
		briTimer = setTimeout(() => {
			act(() => api.setBrightness(brightness));
			dragBri = false;
		}, 120);
	}
	function lightsOff() {
		setLightsOff(!st?.blackout);
	}
	// The dashboard has its own big Now Playing card; on phones the mini player would repeat it.
	const onDashboard = $derived(page.url.pathname === '/');
	const hasProps = $derived(!!app.show?.props.length);

	/** The next few items of the running playlist (in order unless it's shuffled). */
	const upNext = $derived.by(() => {
		const show = app.show;
		const pl = st?.playlist && show?.playlists.find((p) => p.id === st.playlist!.id);
		if (!show || !pl || pl.shuffle) return st?.nextItem ? [st.nextItem.name] : [];
		return pl.items
			.slice(st!.playlist!.index + 1, st!.playlist!.index + 4)
			.map((it) => itemName(it))
			.filter(Boolean) as string[];
	});
	function itemName(it: PlaylistItem): string | undefined {
		const show = app.show!;
		switch (it.type) {
			case 'sequence':
				return show.sequences.find((x) => x.id === it.sequenceId)?.name;
			case 'dj':
				return show.djClips.find((x) => x.id === it.djClipId)?.name;
			case 'effect':
				return show.effects.find((x) => x.id === it.effectId)?.name;
			case 'media':
				return show.media.find((x) => x.id === it.mediaId)?.name;
			case 'pause':
				return 'Pause';
			default:
				return undefined;
		}
	}
</script>

{#snippet title()}
	<div class="np">
		<div class="art" class:live={playing}>
			<ItemIcon size={18} />
			{#if playing}<span class="eq" aria-hidden="true"><i></i><i></i><i></i></span>{/if}
		</div>
		<div class="grow meta">
			<div class="t ellipsis">
				{#if st?.item}{st.item.name}{:else if st?.nextShow}Next show: {st.nextShow.name}{:else}Nothing playing{/if}
			</div>
			<div class="s ellipsis">
				{#if st?.playlist}
					<ListMusic size={12} />
					{st.playlist.name} · {st.playlist.index + 1}/{st.playlist.count}{#if st.nextItem}&nbsp;· Next: {st
							.nextItem.name}{/if}
				{:else if st?.state === 'effect'}
					Live effect
				{:else if st?.state === 'testing'}
					Testing props
				{:else}
					Press play to start the show
				{/if}
			</div>
		</div>
	</div>
{/snippet}

{#snippet controls(big = false)}
	<div class="ctl" class:big>
		<button class="cb" onclick={() => act(api.previous)} disabled={!active} aria-label="Previous"
			><SkipBack size={big ? 22 : 18} /></button
		>
		<button class="cb main" onclick={togglePlay} aria-label={playing ? 'Pause' : 'Play'}>
			{#if playing}<Pause size={big ? 26 : 20} fill="currentColor" />{:else}<Play
					size={big ? 26 : 20}
					fill="currentColor"
				/>{/if}
		</button>
		<button class="cb" onclick={stopShow} disabled={!active} aria-label="Stop"
			><Square size={big ? 20 : 16} fill="currentColor" /></button
		>
		<button class="cb" onclick={() => act(api.next)} disabled={!active} aria-label="Next"
			><SkipForward size={big ? 22 : 18} /></button
		>
	</div>
{/snippet}

{#snippet progress()}
	<div class="prog">
		<span class="time num">{fmtDuration(scrub ?? st?.posMs ?? 0)}</span>
		<input
			type="range"
			class="range scrub"
			min="0"
			max={st?.durationMs || 1}
			step="1000"
			value={scrub ?? st?.posMs ?? 0}
			disabled={!st?.durationMs}
			style:--pct="{scrub != null && st?.durationMs ? (scrub / st.durationMs) * 100 : pct}%"
			aria-label="Position"
			oninput={(e) => (scrub = Number((e.target as HTMLInputElement).value))}
			onchange={() => {
				const v = scrub;
				scrub = null;
				if (v != null) act(() => api.seek(v));
			}}
		/>
		<span class="time num">{fmtDuration(st?.durationMs ?? 0)}</span>
	</div>
{/snippet}

{#snippet levels(labelled = false)}
	<div class="lv" class:labelled>
		{#if labelled}<span class="lvl">Volume</span>{/if}
		<button
			class="icon-ghost"
			onclick={() => {
				volume = volume ? 0 : 70;
				onVol();
			}}
			aria-label={volume ? 'Mute' : 'Unmute'}
		>
			{#if volume}<Volume2 size={17} />{:else}<VolumeX size={17} />{/if}
		</button>
		<input
			type="range"
			class="range"
			min="0"
			max="100"
			bind:value={volume}
			oninput={onVol}
			style:--pct="{volume}%"
			aria-label="Volume"
		/>
		{#if labelled}<span class="lvv num">{volume}%</span>{/if}
	</div>
	<div class="lv" class:labelled>
		{#if labelled}<span class="lvl">Brightness</span>{/if}
		<span class="icon-ghost" aria-hidden="true"><SunMedium size={17} /></span>
		<input
			type="range"
			class="range"
			min="0"
			max="100"
			bind:value={brightness}
			oninput={onBri}
			style:--pct="{brightness}%"
			aria-label="Brightness"
		/>
		{#if labelled}<span class="lvv num">{brightness}%</span>{/if}
	</div>
	<button
		class="bo"
		class:on={st?.blackout}
		onclick={lightsOff}
		aria-pressed={!!st?.blackout}
		title="{LIGHTS_OFF_HELP} (Shift+B)"
	>
		<Power size={15} /> <span>{st?.blackout ? 'Lights are off' : 'Lights off'}</span>
	</button>
{/snippet}

<div class="transport" class:home={onDashboard} role="region" aria-label="Player">
	<div class="line" style:width="{pct}%"></div>
	<div class="left">
		<button class="np-btn" onclick={() => (expanded = true)} aria-label="Open player"
			>{@render title()}</button
		>
	</div>
	<div class="center">
		{@render controls()}
		<div class="desk-only">{@render progress()}</div>
	</div>
	<div class="right desk-only">{@render levels()}</div>
	<div class="mobile-ctl">
		<button class="cb main sm" onclick={togglePlay} aria-label={playing ? 'Pause' : 'Play'}>
			{#if playing}<Pause size={18} fill="currentColor" />{:else}<Play size={18} fill="currentColor" />{/if}
		</button>
		<button class="cb" onclick={() => act(api.next)} disabled={!active} aria-label="Next"
			><SkipForward size={18} /></button
		>
	</div>
</div>

{#if expanded}
	<div
		class="scrim"
		transition:fade={{ duration: 150 }}
		onclick={() => (expanded = false)}
		aria-hidden="true"
	></div>
	<div class="sheet" role="dialog" aria-label="Player" transition:fly={{ y: 400, duration: 260, opacity: 1 }}>
		<button class="collapse" onclick={() => (expanded = false)} aria-label="Close player"
			><ChevronDown size={22} /></button
		>
		<div class="big-art" class:live={playing} class:stage={hasProps} class:dark={st?.blackout}>
			{#if hasProps && app.show}
				<LayoutCanvas props={app.show.props} height="100%" />
				{#if st?.blackout}<span class="art-badge off">LIGHTS OFF</span>{:else if playing}<span
						class="art-badge"><span class="dot live"></span> LIVE</span
					>{/if}
			{:else}
				<ItemIcon size={44} strokeWidth={1.5} />
			{/if}
		</div>
		<div class="sheet-title">{@render title()}</div>
		{@render progress()}
		{@render controls(true)}
		<div class="sheet-levels">{@render levels(true)}</div>
		{#if upNext.length}
			<div class="upnext">
				<div class="eyebrow">Up next</div>
				<ol>
					{#each upNext as n, i (i + n)}<li><span class="num faint">{i + 1}</span>{n}</li>{/each}
				</ol>
			</div>
		{/if}
	</div>
{/if}

<style>
	.transport {
		position: fixed;
		left: var(--sidebar-w);
		right: 0;
		bottom: 0;
		height: var(--transport-h);
		display: grid;
		grid-template-columns: minmax(200px, 1fr) minmax(320px, 1.3fr) minmax(200px, 1fr);
		align-items: center;
		gap: 24px;
		padding: 0 20px;
		background: var(--sidebar);
		backdrop-filter: blur(20px) saturate(1.5);
		-webkit-backdrop-filter: blur(20px) saturate(1.5);
		border-top: 1px solid var(--border);
		z-index: 50;
	}
	.line {
		display: none;
	}
	.left {
		min-width: 0;
	}
	.np-btn {
		display: block;
		width: 100%;
		text-align: left;
		cursor: default;
	}
	.np {
		display: flex;
		align-items: center;
		gap: 12px;
		min-width: 0;
	}
	.art {
		position: relative;
		width: 44px;
		height: 44px;
		border-radius: 11px;
		display: grid;
		place-items: center;
		color: var(--text-2);
		background: linear-gradient(135deg, var(--surface-3), var(--surface-2));
		border: 1px solid var(--border-2);
		flex: 0 0 auto;
		overflow: hidden;
	}
	.art.live {
		color: var(--accent-fg);
		background: linear-gradient(135deg, #ffc45c, #f08a0c);
		border-color: transparent;
		box-shadow: 0 4px 18px rgba(245, 165, 36, 0.3);
	}
	.art.live :global(svg) {
		opacity: 0;
	}
	.eq {
		position: absolute;
		inset: 0;
		display: flex;
		align-items: flex-end;
		justify-content: center;
		gap: 3px;
		padding-bottom: 12px;
	}
	.eq i {
		width: 4px;
		border-radius: 2px;
		background: var(--accent-fg);
		animation: eq 0.9s ease-in-out infinite;
		height: 10px;
	}
	.eq i:nth-child(2) {
		animation-delay: -0.3s;
	}
	.eq i:nth-child(3) {
		animation-delay: -0.6s;
	}
	@keyframes eq {
		0%,
		100% {
			height: 6px;
		}
		50% {
			height: 20px;
		}
	}
	.meta {
		min-width: 0;
	}
	.t {
		font-weight: 600;
		font-size: 13.5px;
		letter-spacing: -0.005em;
	}
	.s {
		color: var(--text-3);
		font-size: 12px;
		display: flex;
		align-items: center;
		gap: 4px;
	}
	.center {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 2px;
		min-width: 0;
	}
	.desk-only {
		width: 100%;
	}
	.ctl {
		display: flex;
		align-items: center;
		gap: 6px;
	}
	.cb {
		width: 36px;
		height: 36px;
		border-radius: 50%;
		display: grid;
		place-items: center;
		color: var(--text-2);
		transition: all 150ms var(--ease);
	}
	.cb:hover:not(:disabled) {
		color: var(--text);
		background: var(--surface-3);
	}
	.cb:disabled {
		opacity: 0.35;
		cursor: default;
	}
	.cb.main {
		width: 40px;
		height: 40px;
		background: var(--text);
		color: var(--bg);
	}
	.cb.main:hover {
		transform: scale(1.06);
		background: var(--text);
		color: var(--bg);
	}
	.cb.main:active {
		transform: scale(0.96);
	}
	.ctl.big {
		justify-content: center;
		gap: 18px;
		margin: 8px 0 16px;
	}
	.ctl.big .cb {
		width: 52px;
		height: 52px;
	}
	.ctl.big .cb.main {
		width: 68px;
		height: 68px;
	}
	.prog {
		display: flex;
		align-items: center;
		gap: 10px;
		width: 100%;
	}
	.time {
		font-size: 11px;
		color: var(--text-3);
		min-width: 34px;
		text-align: center;
	}
	.scrub {
		height: 16px;
	}
	.right {
		display: flex;
		align-items: center;
		justify-content: flex-end;
		gap: 14px;
	}
	.lv {
		display: flex;
		align-items: center;
		gap: 6px;
		width: 110px;
	}
	.icon-ghost {
		color: var(--text-3);
		display: grid;
		place-items: center;
		flex: 0 0 auto;
	}
	.bo {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		height: 32px;
		padding: 0 12px;
		border-radius: 99px;
		border: 1px solid var(--border-2);
		font-size: 12.5px;
		font-weight: 560;
		color: var(--text-2);
		transition: all 150ms var(--ease);
		white-space: nowrap;
	}
	.bo:hover {
		color: var(--red);
		border-color: color-mix(in srgb, var(--red) 50%, transparent);
	}
	.bo.on {
		background: var(--red);
		border-color: var(--red);
		color: #fff;
	}
	.mobile-ctl {
		display: none;
	}
	.scrim,
	.sheet {
		display: none;
	}

	@media (max-width: 1180px) {
		.transport {
			grid-template-columns: minmax(180px, 1fr) minmax(260px, 1.2fr) auto;
			gap: 16px;
		}
		.lv {
			width: 84px;
		}
		.bo span {
			display: none;
		}
	}
	@media (max-width: 760px) {
		.transport {
			left: 8px;
			right: 8px;
			bottom: calc(var(--tabbar-h) + env(safe-area-inset-bottom) + 8px);
			height: 60px;
			grid-template-columns: 1fr auto;
			gap: 8px;
			padding: 0 8px 0 8px;
			border: 1px solid var(--border-2);
			border-radius: 16px;
			background: var(--surface-2);
			box-shadow: var(--shadow-2);
			overflow: hidden;
		}
		.line {
			display: block;
			position: absolute;
			left: 0;
			bottom: 0;
			height: 2px;
			background: var(--accent);
			transition: width 250ms linear;
		}
		.np-btn {
			cursor: pointer;
		}
		.art {
			width: 40px;
			height: 40px;
		}
		.center,
		.desk-only {
			display: none;
		}
		.mobile-ctl {
			display: flex;
			gap: 2px;
			align-items: center;
		}
		.cb.main.sm {
			width: 40px;
			height: 40px;
		}
		.cb {
			width: 44px;
			height: 44px;
		}
		.scrim {
			display: block;
			position: fixed;
			inset: 0;
			background: var(--overlay);
			z-index: 70;
		}
		.transport.home {
			display: none;
		}
		.sheet {
			display: flex;
			flex-direction: column;
			position: fixed;
			left: 0;
			right: 0;
			bottom: 0;
			max-height: 94dvh;
			overflow-y: auto;
			overscroll-behavior: contain;
			padding: 12px 20px calc(24px + env(safe-area-inset-bottom));
			background: var(--surface);
			border-top-left-radius: 24px;
			border-top-right-radius: 24px;
			border: 1px solid var(--border-2);
			z-index: 71;
			gap: 12px;
		}
		.collapse {
			align-self: center;
			color: var(--text-3);
			width: 44px;
			height: 32px;
			display: grid;
			place-items: center;
		}
		.big-art {
			align-self: center;
			width: 180px;
			height: 180px;
			border-radius: 28px;
			display: grid;
			place-items: center;
			color: var(--text-3);
			background: linear-gradient(135deg, var(--surface-3), var(--surface-2));
			margin: 8px 0;
		}
		.big-art.live {
			color: var(--accent-fg);
			background: linear-gradient(135deg, #ffc45c, #f08a0c);
			box-shadow: 0 12px 40px rgba(245, 165, 36, 0.35);
		}
		/* The live layout is the "album art": the show itself, playing right now. */
		.big-art.stage {
			position: relative;
			width: 100%;
			height: auto;
			aspect-ratio: 16 / 10;
			max-height: 34dvh;
			border-radius: 20px;
			overflow: hidden;
			background: var(--canvas-bg);
			border: 1px solid var(--border-2);
			box-shadow: 0 14px 44px rgba(0, 0, 0, 0.35);
			margin: 4px 0;
		}
		.big-art.stage.live {
			background: var(--canvas-bg);
			box-shadow:
				0 14px 44px rgba(0, 0, 0, 0.35),
				0 0 0 1px rgba(245, 165, 36, 0.25);
		}
		.big-art.stage :global(canvas) {
			position: absolute;
			inset: 0;
			width: 100% !important;
			height: 100% !important;
		}
		.big-art.stage.dark :global(canvas) {
			opacity: 0.15;
		}
		.art-badge {
			position: absolute;
			top: 10px;
			left: 10px;
			display: inline-flex;
			align-items: center;
			gap: 6px;
			padding: 3px 9px;
			border-radius: 99px;
			background: rgba(0, 0, 0, 0.6);
			color: #ff8a8a;
			font-size: 10.5px;
			font-weight: 700;
			letter-spacing: 0.08em;
		}
		.art-badge.off {
			background: #d6363c;
			color: #fff;
		}
		.lv.labelled {
			display: grid;
			grid-template-columns: 80px auto 1fr 40px;
			align-items: center;
			gap: 8px;
		}
		.sheet-levels .icon-ghost {
			width: 44px;
			height: 44px;
		}
		.lvl {
			font-size: 12.5px;
			font-weight: 560;
			color: var(--text-2);
		}
		.lvv {
			font-size: 12px;
			color: var(--text-3);
			text-align: right;
		}
		.upnext {
			margin-top: 4px;
			padding-top: 12px;
			border-top: 1px solid var(--border);
		}
		.upnext ol {
			list-style: none;
			margin: 8px 0 0;
			padding: 0;
			display: flex;
			flex-direction: column;
			gap: 6px;
		}
		.upnext li {
			display: flex;
			gap: 12px;
			font-size: 14px;
			align-items: baseline;
		}
		.sheet-title .np {
			justify-content: center;
			text-align: center;
		}
		.sheet-title :global(.art) {
			display: none;
		}
		.sheet-title .t {
			font-size: 18px;
		}
		.sheet-title .s {
			justify-content: center;
		}
		.sheet-levels {
			display: flex;
			flex-direction: column;
			gap: 14px;
		}
		.sheet-levels .lv {
			width: 100%;
		}
		.sheet-levels .bo {
			height: 44px;
			justify-content: center;
		}
		.sheet-levels .bo span {
			display: inline;
		}
	}
</style>
