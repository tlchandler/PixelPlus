<script lang="ts">
	import '../app.css';
	import { page } from '$app/state';
	import { app } from '$lib/stores/app.svelte';
	import { theme } from '$lib/stores/theme.svelte';
	import Toaster from '$lib/components/ui/Toaster.svelte';
	import ConfirmHost from '$lib/components/ui/ConfirmHost.svelte';

	let { children } = $props();

	$effect(() => {
		theme.init();
		if (!page.url.pathname.startsWith('/request')) app.boot();
	});
</script>

{@render children()}
<Toaster />
<ConfirmHost />
