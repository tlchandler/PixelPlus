<!-- WS1 (F1) owns this page (placeholder created by WS0). -->
<script lang="ts">
	import Planned from '$lib/components/ui/Planned.svelte';
	import { ShieldCheck } from '@lucide/svelte';
</script>

<Planned
	title="Secure connection"
	subtitle="HTTPS for camera and microphone pages on phones."
	icon={ShieldCheck}
	back={{ href: '/settings', label: 'Back to Settings' }}
/>
