<script lang="ts">
	let { peaks = [], progress = 0, height = 40, color = 'var(--text-3)', active = 'var(--accent)' }: { peaks?: number[]; progress?: number; height?: number; color?: string; active?: string } =
		$props();
	const n = $derived(peaks.length || 1);
</script>

<svg viewBox="0 0 {n * 3} 100" preserveAspectRatio="none" style:height="{height}px" aria-hidden="true">
	{#each peaks as p, i (i)}
		<rect x={i * 3} y={50 - p * 48} width="2" height={Math.max(2, p * 96)} rx="1" fill={i / n < progress ? active : color} />
	{/each}
</svg>

<style>
	svg {
		width: 100%;
		display: block;
	}
</style>
