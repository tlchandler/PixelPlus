<script lang="ts">
	let {
		w = '100%',
		h = 16,
		r = 6,
		count = 1,
		gap = 8
	}: { w?: string | number; h?: number; r?: number; count?: number; gap?: number } = $props();
</script>

<div class="col" style:gap="{gap}px" aria-hidden="true">
	{#each Array(count) as _, i (i)}
		<div
			class="skeleton"
			style:width={typeof w === 'number' ? `${w}px` : w}
			style:height="{h}px"
			style:border-radius="{r}px"
		></div>
	{/each}
</div>
