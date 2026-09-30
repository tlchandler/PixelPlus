<!-- WS5 (F14) owns this page (placeholder created by WS0). -->
<script lang="ts">
	import Planned from '$lib/components/ui/Planned.svelte';
	import { Globe } from '@lucide/svelte';
</script>

<Planned
	title="Remote access"
	subtitle="Reach your show from anywhere, without port forwarding."
	icon={Globe}
	back={{ href: '/settings', label: 'Back to Settings' }}
/>
