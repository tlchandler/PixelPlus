<script lang="ts">
	import { wifiQuality } from '$lib/util/units';

	/** Wi-Fi strength as four bars (dBm stays out of sight). */
	let { dbm, showLabel = false }: { dbm: number | null | undefined; showLabel?: boolean } = $props();
	const q = $derived(wifiQuality(dbm));
</script>

<span class="sig" title="Signal: {q.label}" aria-label="Signal {q.label}" role="img">
	<svg width="16" height="14" viewBox="0 0 16 14" aria-hidden="true">
		{#each [0, 1, 2, 3] as i (i)}
			<rect x={i * 4.2} y={10 - i * 3.2} width="3" height={4 + i * 3.2} rx="1" class:on={i < q.bars} />
		{/each}
	</svg>
	{#if showLabel}<span class="lbl">{q.label}</span>{/if}
</span>

<style>
	.sig {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		color: var(--text-2);
		font-size: 12.5px;
	}
	svg {
		display: inline-block;
	}
	rect {
		fill: var(--border-3);
	}
	rect.on {
		fill: currentColor;
	}
</style>
