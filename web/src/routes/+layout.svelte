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
		// Public pages (visitors, phones installing the certificate) need no admin session.
		if (!['/request', '/trust'].some((p) => page.url.pathname.startsWith(p))) app.boot();
	});
</script>

{@render children()}
<Toaster />
<ConfirmHost />
