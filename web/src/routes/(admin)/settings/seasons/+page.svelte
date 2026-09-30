<!-- WS6 (F8) owns this page (placeholder created by WS0). -->
<script lang="ts">
	import Planned from '$lib/components/ui/Planned.svelte';
	import { CalendarRange } from '@lucide/svelte';
</script>

<Planned
	title="Seasons"
	subtitle="Halloween, Christmas, July 4th: switch everything at once."
	icon={CalendarRange}
	back={{ href: '/settings', label: 'Back to Settings' }}
/>
