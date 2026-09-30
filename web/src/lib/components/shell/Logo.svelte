<script lang="ts">
	let { size = 32 }: { size?: number } = $props();
</script>

<svg width={size} height={size} viewBox="0 0 64 64" aria-hidden="true">
	<defs>
		<linearGradient id="pp-g" x1="0" y1="0" x2="1" y2="1">
			<stop offset="0" stop-color="#FFC45C" />
			<stop offset="1" stop-color="#F08A0C" />
		</linearGradient>
	</defs>
	<rect width="64" height="64" rx="16" fill="url(#pp-g)" />
	<g fill="#1d1300">
		<circle cx="20" cy="20" r="5.5" />
		<circle cx="44" cy="20" r="5.5" opacity=".45" />
		<circle cx="20" cy="44" r="5.5" opacity=".45" />
		<circle cx="44" cy="44" r="5.5" />
	</g>
	<path d="M32 23v18M23 32h18" stroke="#1d1300" stroke-width="5" stroke-linecap="round" />
</svg>
