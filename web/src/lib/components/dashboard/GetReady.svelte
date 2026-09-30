<script lang="ts">
	import type { Show } from '$lib/api/types';
	import { Check, ChevronRight, Cpu, FileUp, Music, ListMusic, CalendarClock, X, Rocket } from '@lucide/svelte';
	import { slide } from 'svelte/transition';

	let { show }: { show: Show } = $props();

	const KEY = $derived(`pp-getready-hidden:${show.name}`);
	let hidden = $state(false);
	$effect(() => {
		try {
			hidden = localStorage.getItem(KEY) === '1';
		} catch {
			hidden = false;
		}
	});
	function dismiss() {
		hidden = true;
		try {
			localStorage.setItem(KEY, '1');
		} catch {
			/* ignore */
		}
	}

	const steps = $derived([
		{
			done: show.nodes.some((n) => n.outputs.length > 0),
			title: 'Connect your controllers',
			hint: 'Adopt every PixelPlus box on your network',
			href: '/controllers',
			icon: Cpu
		},
		{
			done: show.props.length > 0,
			title: 'Import your xLights layout',
			hint: 'Brings in every prop and where it’s plugged in',
			href: '/props?import=1',
			icon: FileUp
		},
		{
			done: show.sequences.length > 0,
			title: 'Upload sequences and songs',
			hint: 'Your light sequences from xLights, with their music',
			href: '/sequences',
			icon: Music
		},
		{
			done: show.playlists.some((p) => p.items.length > 0),
			title: 'Build a playlist',
			hint: 'The running order for the night',
			href: '/playlists',
			icon: ListMusic
		},
		{
			done: show.schedule.enabled && show.schedule.entries.some((e) => e.enabled),
			title: 'Turn on the schedule',
			hint: 'Start the show at sunset, every night',
			href: '/schedule',
			icon: CalendarClock
		}
	]);
	const doneCount = $derived(steps.filter((s) => s.done).length);
	const nextIdx = $derived(steps.findIndex((s) => !s.done));
</script>

{#if !hidden && doneCount < steps.length}
	<section class="card ready" transition:slide={{ duration: 200 }} aria-labelledby="ready-title">
		<header>
			<span class="icon-tile accent"><Rocket size={20} /></span>
			<div class="grow">
				<h2 id="ready-title">Get your show ready</h2>
				<p class="muted small">{doneCount} of {steps.length} done · each step takes a few minutes</p>
			</div>
			<button class="btn ghost icon sm" onclick={dismiss} aria-label="Hide this checklist" title="Hide"
				><X size={16} /></button
			>
		</header>
		<div class="bar" aria-hidden="true"><span style:width="{(doneCount / steps.length) * 100}%"></span></div>
		<ol>
			{#each steps as s, i (s.title)}
				<li class:done={s.done} class:next={i === nextIdx}>
					<a href={s.href}>
						<span class="tick" aria-hidden="true">
							{#if s.done}<Check size={14} strokeWidth={3} />{:else}{i + 1}{/if}
						</span>
						<span class="ic" aria-hidden="true"><s.icon size={17} /></span>
						<span class="grow txt">
							<strong>{s.title}</strong>
							<span class="faint small">{s.done ? 'Done' : s.hint}</span>
						</span>
						<span class="sr-only">{s.done ? '(done)' : '(to do)'}</span>
						{#if !s.done}<ChevronRight size={16} class="chev" />{/if}
					</a>
				</li>
			{/each}
		</ol>
	</section>
{/if}

<style>
	.ready {
		margin-bottom: 16px;
		overflow: hidden;
		background:
			radial-gradient(900px 200px at 0% 0%, var(--accent-soft), transparent 70%),
			var(--surface);
		border-color: var(--accent-line);
	}
	header {
		display: flex;
		align-items: center;
		gap: 14px;
		padding: 18px 20px 12px;
	}
	h2 {
		font-size: 16px;
	}
	.bar {
		height: 3px;
		margin: 0 20px;
		border-radius: 3px;
		background: var(--surface-3);
		overflow: hidden;
	}
	.bar span {
		display: block;
		height: 100%;
		background: var(--accent);
		border-radius: inherit;
		transition: width 400ms var(--ease);
	}
	ol {
		list-style: none;
		margin: 0;
		padding: 8px 8px 10px;
		display: grid;
		grid-template-columns: repeat(5, minmax(0, 1fr));
		gap: 6px;
	}
	li a {
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: 8px;
		height: 100%;
		padding: 12px;
		border-radius: 12px;
		border: 1px solid transparent;
		transition:
			background var(--dur) var(--ease),
			border-color var(--dur) var(--ease);
	}
	li a:hover {
		background: var(--surface-2);
		border-color: var(--border-2);
	}
	li.next a {
		background: var(--surface-2);
		border-color: var(--accent-line);
	}
	.tick {
		display: grid;
		place-items: center;
		width: 24px;
		height: 24px;
		border-radius: 50%;
		font-size: 12px;
		font-weight: 650;
		color: var(--text-2);
		border: 1.5px solid var(--border-3);
		flex: 0 0 auto;
	}
	li.done .tick {
		background: var(--green);
		border-color: var(--green);
		color: #fff;
	}
	li.next .tick {
		border-color: var(--accent);
		color: var(--accent-text);
	}
	.ic {
		display: none;
	}
	.txt {
		display: flex;
		flex-direction: column;
		gap: 2px;
		min-width: 0;
	}
	.txt strong {
		font-size: 13.5px;
		font-weight: 600;
	}
	li.done .txt strong {
		color: var(--text-2);
	}
	li :global(.chev) {
		display: none;
	}
	@media (max-width: 1100px) {
		ol {
			grid-template-columns: minmax(0, 1fr);
			gap: 2px;
		}
		li a {
			flex-direction: row;
			align-items: center;
			gap: 12px;
			min-height: 56px;
			padding: 8px 12px;
		}
		.ic {
			display: grid;
			place-items: center;
			width: 32px;
			height: 32px;
			border-radius: 9px;
			background: var(--surface-3);
			color: var(--text-2);
			flex: 0 0 auto;
		}
		li.next .ic {
			background: var(--accent-soft);
			color: var(--accent-text);
		}
		li :global(.chev) {
			display: block;
			color: var(--text-3);
		}
	}
	@media (max-width: 760px) {
		header {
			padding: 14px 14px 10px;
		}
		.bar {
			margin: 0 14px;
		}
		ol {
			padding: 6px 4px 8px;
		}
	}
</style>
