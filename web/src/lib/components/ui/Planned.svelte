<!-- Placeholder body for a feature-wave page (created by WS0; replaced by the page's owner). -->
<script lang="ts">
	import type { Component } from 'svelte';
	import PageHeader from './PageHeader.svelte';
	import EmptyState from './EmptyState.svelte';

	let {
		title,
		subtitle,
		icon,
		message = 'This part of PixelPlus is being built. It will appear here in an update.',
		back
	}: {
		title: string;
		subtitle?: string;
		icon?: Component<any>;
		message?: string;
		back?: { href: string; label: string };
	} = $props();
</script>

<div class="page">
	<PageHeader {title} {subtitle} />
	<section class="card">
		<EmptyState {icon} title="Coming soon" {message}>
			{#if back}<a class="btn" href={back.href}>{back.label}</a>{/if}
		</EmptyState>
	</section>
</div>
