<!-- WS3 (F12) owns this page (placeholder created by WS0). -->
<script lang="ts">
	import Planned from '$lib/components/ui/Planned.svelte';
	import { Zap } from '@lucide/svelte';
</script>

<Planned
	title="Power"
	subtitle="Power supplies, the brightness limiter and late-night dimming."
	icon={Zap}
	back={{ href: '/settings', label: 'Back to Settings' }}
/>
