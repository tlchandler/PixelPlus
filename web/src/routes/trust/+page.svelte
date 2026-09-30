<!-- WS1 (F1) owns this page (placeholder created by WS0). Reachable over plain HTTP without
     signing in: phones open it to install the show's certificate. -->
<script lang="ts">
	import Planned from '$lib/components/ui/Planned.svelte';
	import { ShieldCheck } from '@lucide/svelte';
</script>

<svelte:head><title>Trust this phone · PixelPlus</title></svelte:head>

<main class="trust">
	<Planned
		title="Make this phone trusted"
		subtitle="Install this show's certificate once so camera and microphone pages work on your phone."
		icon={ShieldCheck}
		back={{ href: '/', label: 'Back to PixelPlus' }}
	/>
</main>

<style>
	.trust {
		max-width: 720px;
		margin: 0 auto;
		padding: 24px 16px;
		min-height: 100vh;
		background: var(--bg);
	}
</style>
