<!-- WS6 (F11) owns this page (placeholder created by WS0). -->
<script lang="ts">
	import Planned from '$lib/components/ui/Planned.svelte';
	import { ClipboardList } from '@lucide/svelte';
</script>

<Planned
	title="Nightly report"
	subtitle="When and how the morning report is sent."
	icon={ClipboardList}
	back={{ href: '/settings', label: 'Back to Settings' }}
/>
