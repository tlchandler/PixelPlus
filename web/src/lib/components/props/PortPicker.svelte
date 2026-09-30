<script lang="ts">
	import type { Show } from '$lib/api/types';
	import { jackOf, portOf, propsOnOutput, receiverFor, MAX_PIXELS_PER_OUTPUT } from '$lib/util/boards';

	/**
	 * Two-step port picker that mirrors the hardware: pick the network jack (J1–J15), then one of
	 * its four ports. Boards with a single jack go straight to the ports.
	 */
	let {
		show,
		nodeId,
		value,
		propId,
		onchange
	}: {
		show: Show;
		nodeId: string;
		value: number;
		/** The prop being wired: its own pixels don't count as "in use". */
		propId?: string;
		onchange: (output: number) => void;
	} = $props();

	const node = $derived(show.nodes.find((n) => n.id === nodeId));
	const multiJack = $derived(node?.board === 'difftxlarge');
	const jacks = $derived(
		multiJack ? Array.from({ length: Math.ceil((node?.outputs.length ?? 0) / 4) }, (_, i) => i + 1) : []
	);
	let jack = $state(1);
	$effect(() => {
		if (node) jack = jackOf(node.board, value) ?? 1;
	});
	const ports = $derived.by(() => {
		if (!node) return [];
		const outs = multiJack
			? [1, 2, 3, 4].map((p) => (jack - 1) * 4 + p)
			: node.outputs.map((o) => o.index);
		return outs
			.filter((o) => node.outputs.some((x) => x.index === o))
			.map((o) => {
				const others = propsOnOutput(show, node.id, o).filter((x) => x.prop.id !== propId);
				const px = others.reduce((m, x) => Math.max(m, x.seg.startPixel + x.seg.pixelCount), 0);
				return { output: o, port: multiJack ? portOf(o) : o, px, names: others.map((x) => x.prop.name) };
			});
	});
	const jackRx = $derived(
		node && multiJack ? receiverFor(show, node.id, (jack - 1) * 4 + 1) : undefined
	);
	function jackUse(j: number) {
		if (!node) return 0;
		return [1, 2, 3, 4].filter((p) =>
			propsOnOutput(show, node.id, (j - 1) * 4 + p).some((x) => x.prop.id !== propId)
		).length;
	}
</script>

{#if node}
	<div class="pp">
		{#if multiJack}
			<div class="step">
				<span class="lbl">1 · Network jack</span>
				<div class="jacks" role="radiogroup" aria-label="Network jack">
					{#each jacks as j (j)}
						{@const rx = show.receivers.find((r) => r.nodeId === node.id && r.jack === j)}
						{@const used = jackUse(j)}
						<button
							type="button"
							role="radio"
							aria-checked={jack === j}
							class="jack"
							class:on={jack === j}
							class:mine={jackOf(node.board, value) === j}
							onclick={() => (jack = j)}
							title={rx ? `${rx.name} receiver` : 'No receiver'}
						>
							<span class="jn">J{j}</span>
							<span class="jr ellipsis">{rx ? rx.name : used ? `${used}/4` : ''}</span>
						</button>
					{/each}
				</div>
			</div>
		{/if}
		<div class="step">
			<span class="lbl"
				>{multiJack ? '2 · Port' : 'Port'}{#if jackRx}<span class="faint">{` on the ${jackRx.name} receiver`}</span>{/if}</span
					>{/if}</span
			>
			<div class="ports" role="radiogroup" aria-label="Port">
				{#each ports as p (p.output)}
					<button
						type="button"
						role="radio"
						aria-checked={value === p.output}
						class="port"
						class:on={value === p.output}
						onclick={() => onchange(p.output)}
					>
						<span class="pn">Port {p.port}</span>
						<span class="pu ellipsis" class:over={p.px > MAX_PIXELS_PER_OUTPUT}
							>{p.px ? `${p.names.length === 1 ? p.names[0] : `${p.names.length} props`} · ${p.px} px` : 'Free'}</span
						>
					</button>
				{/each}
			</div>
		</div>
	</div>
{/if}

<style>
	.pp {
		display: flex;
		flex-direction: column;
		gap: 12px;
	}
	.step {
		display: flex;
		flex-direction: column;
		gap: 6px;
	}
	.lbl {
		font-size: 12.5px;
		font-weight: 550;
		color: var(--text-2);
	}
	.jacks {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(64px, 1fr));
		gap: 6px;
	}
	.jack,
	.port {
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		justify-content: center;
		gap: 1px;
		min-height: 44px;
		padding: 6px 10px;
		border-radius: 10px;
		border: 1px solid var(--border-2);
		background: var(--surface);
		text-align: left;
		min-width: 0;
		transition:
			border-color var(--dur) var(--ease),
			background var(--dur) var(--ease);
	}
	.jack:hover,
	.port:hover {
		border-color: var(--border-3);
	}
	.jack.mine:not(.on) {
		border-style: dashed;
		border-color: var(--accent-line);
	}
	.jack.on,
	.port.on {
		background: var(--accent-soft);
		border-color: var(--accent);
	}
	.jn,
	.pn {
		font-weight: 620;
		font-size: 13px;
	}
	.jr,
	.pu {
		max-width: 100%;
		font-size: 11px;
		color: var(--text-3);
	}
	.pu.over {
		color: var(--red);
	}
	.ports {
		display: grid;
		grid-template-columns: repeat(4, minmax(0, 1fr));
		gap: 6px;
	}
	@media (max-width: 520px) {
		.ports {
			grid-template-columns: repeat(2, minmax(0, 1fr));
		}
		.jacks {
			grid-template-columns: repeat(5, minmax(0, 1fr));
		}
		.jack {
			padding: 6px 6px;
		}
	}
</style>
