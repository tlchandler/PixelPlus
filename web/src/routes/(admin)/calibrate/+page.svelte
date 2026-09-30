<!-- WS1 (F1) owns this page (placeholder created by WS0). -->
<script lang="ts">
	import Planned from '$lib/components/ui/Planned.svelte';
	import { AudioWaveform } from '@lucide/svelte';
</script>

<Planned
	title="Sync lights to sound"
	subtitle="Measure the sound delay with your phone's camera and microphone."
	icon={AudioWaveform}
	back={{ href: '/settings#audio', label: 'Adjust by hand' }}
/>
