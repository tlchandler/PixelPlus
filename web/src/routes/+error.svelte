<script lang="ts">
	import { page } from '$app/state';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import { Compass } from '@lucide/svelte';
</script>

<div class="page">
	<EmptyState icon={Compass} title={page.status === 404 ? 'Page not found' : 'Something went wrong'} message={page.error?.message}>
		<a class="btn primary" href="/">Back to dashboard</a>
	</EmptyState>
</div>
