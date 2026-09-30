<!--
	Dashboard chip "🎄 Christmas season" (F8). WS6 delivers it; the dashboard page embeds it
	with one line: `<SeasonChip />`. Hidden when the show has no seasons.
-->
<script lang="ts">
	import { app } from '$lib/stores/app.svelte';
	import { CalendarRange } from '@lucide/svelte';

	const show = $derived(app.show);
	const active = $derived(show?.profiles?.find((p) => p.id === show.activeProfileId));
	/** The next date-based switch (only when auto-switch is on). */
	const next = $derived.by(() => {
		if (!show?.profileAutoSwitch || !show.profiles?.length) return null;
		const today = new Date();
		for (let i = 1; i <= 366; i++) {
			const d = new Date(today.getFullYear(), today.getMonth(), today.getDate() + i);
			const md = `${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
			const hit = show.profiles
				.filter((p) => p.dateRange && inRange(md, p.dateRange.start, p.dateRange.end))
				.sort((a, b) => b.priority - a.priority)[0];
			if (hit && hit.id !== show.activeProfileId) return { name: hit.name, date: d };
			if (hit) return null;
		}
		return null;
	});
	function inRange(md: string, start: string, end: string) {
		return start <= end ? md >= start && md <= end : md >= start || md <= end;
	}
</script>

{#if show?.profiles?.length}
	<a
		class="season-chip"
		href="/settings/seasons"
		style:--season={active?.color ?? 'var(--accent)'}
		title={next ? `Switches to ${next.name} on ${next.date.toLocaleDateString()}` : 'Seasons'}
	>
		{#if active?.icon}<span class="ic" aria-hidden="true">{active.icon}</span>{:else}<CalendarRange
				size={14}
			/>{/if}
		<span>{active ? `${active.name} season` : 'No season active'}</span>
	</a>
{/if}

<style>
	.season-chip {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		height: 28px;
		padding: 0 12px 0 10px;
		border-radius: 999px;
		font-size: 13px;
		font-weight: 560;
		color: var(--text);
		background: var(--surface-2);
		border: 1px solid var(--border-2);
		box-shadow: inset 3px 0 0 var(--season);
		text-decoration: none;
		white-space: nowrap;
	}
	.season-chip:hover {
		background: var(--surface-hover);
	}
	.ic {
		font-size: 14px;
		line-height: 1;
	}
</style>
