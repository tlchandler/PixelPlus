<!-- WS5 (F15) owns this page (placeholder created by WS0). -->
<script lang="ts">
	import Planned from '$lib/components/ui/Planned.svelte';
	import { Download } from '@lucide/svelte';
</script>

<Planned
	title="Update everything"
	subtitle="Update channels, automatic updates and rollback for every controller."
	icon={Download}
	back={{ href: '/settings', label: 'Back to Settings' }}
/>
