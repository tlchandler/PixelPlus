<!--
	F10 (WS5): replaced controllers that showed up on the network again ("New controllers found"
	lists them with `retired`). Their key was revoked when they were replaced; "Release it" tells
	one to forget this leader and start over as a new, unconfigured controller.
	WS4 embeds this under "New controllers found" on the Controllers page:
		<RetiredControllers discovered={found} onreleased={reload} />
-->
<script lang="ts">
	import { Archive } from '@lucide/svelte';
	import type { DiscoveredNode } from '$lib/api/types';
	import { fleetApi } from '$lib/api/fleet';
	import { toasts } from '$lib/stores/toasts.svelte';

	let { discovered, onreleased }: { discovered: DiscoveredNode[]; onreleased?: () => void } = $props();

	const retired = $derived(discovered.filter((d) => d.retired));
	let busy = $state<string | null>(null);

	const when = (s: string) =>
		new Date(s).toLocaleDateString(undefined, { month: 'short', day: 'numeric', year: 'numeric' });

	async function release(d: DiscoveredNode) {
		busy = d.id;
		try {
			await fleetApi.releaseRetired(d.id);
			toasts.success(`The old ${d.retired?.name ?? 'controller'} was released; it's a new controller now`);
			onreleased?.();
		} catch (e) {
			toasts.error("Couldn't release it", (e as Error).message);
		} finally {
			busy = null;
		}
	}
</script>

{#each retired as d (d.id + (d.ip ?? ''))}
	<div class="retired notice info">
		<Archive size={18} />
		<div class="grow">
			<strong>Retired controller “{d.retired?.name}”</strong>
			<span class="faint small">
				· replaced on {when(d.retired?.replacedAt ?? '')}{d.ip ? ` · ${d.ip}` : ''}</span
			>
			<div class="small muted">
				Its replacement runs the show part now; this old one can't take part any more. Release it to reuse it
				as a new controller.
			</div>
		</div>
		<button class="btn sm" disabled={busy === d.id} onclick={() => release(d)}>
			{busy === d.id ? 'Releasing…' : 'Release it'}
		</button>
	</div>
{/each}

<style>
	.retired {
		align-items: center;
		gap: 12px;
	}
	.grow {
		flex: 1 1 auto;
		min-width: 0;
	}
</style>
