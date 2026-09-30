// Wiring actions shared by the prop drawer and the Controllers page.
import { api } from '$lib/api/client';
import type { Prop, PropSegment, Show } from '$lib/api/types';
import { app } from '$lib/stores/app.svelte';
import { toasts } from '$lib/stores/toasts.svelte';
import { moveItem } from '$lib/actions/sortable';
import { pixelsOnOutput, portName, propsOnOutput } from '$lib/util/boards';

type Op = { op: 'update'; id: string; patch: Partial<Prop> };

/** New start pixels for every prop on one output after moving chain item `from` to `to`. */
export function chainReorderOps(show: Show, nodeId: string, output: number, from: number, to: number): Op[] {
	const chain = moveItem(propsOnOutput(show, nodeId, output), from, to);
	let cursor = 0;
	const patched = new Map<string, Prop>();
	for (const { prop: p, seg: s } of chain) {
		cursor += s.nullPixels;
		const target = patched.get(p.id) ?? (structuredClone($stateSnapshot(p)) as Prop);
		const ts = target.segments.find(
			(x) => x.nodeId === s.nodeId && x.output === s.output && x.propOffset === s.propOffset
		);
		if (ts) ts.startPixel = cursor;
		patched.set(p.id, target);
		cursor += s.pixelCount;
	}
	return [...patched].map(([id, p]) => ({ op: 'update', id, patch: { segments: p.segments } }));
}

// Props may be reactive proxies; a JSON round-trip gives a plain copy in any context.
function $stateSnapshot<T>(v: T): T {
	return JSON.parse(JSON.stringify(v));
}

/** Move a prop along the daisy chain on one port, with an Undo toast. */
export async function reorderChain(show: Show, nodeId: string, output: number, from: number, to: number) {
	if (from === to) return;
	const chain = propsOnOutput(show, nodeId, output);
	const before: Op[] = chain.map(({ prop }) => ({
		op: 'update',
		id: prop.id,
		patch: { segments: $stateSnapshot(prop.segments) }
	}));
	const ops = chainReorderOps(show, nodeId, output, from, to);
	const moved = chain[from]?.prop.name ?? 'Prop';
	const node = show.nodes.find((n) => n.id === nodeId);
	const where = node ? portName(node.board, output) : `port ${output}`;
	const ok = await app.mutate(async () => {
		await api.props.bulk(ops);
		return true;
	});
	if (ok === undefined) return;
	toasts.success(`Moved ${moved} to position ${to + 1} on ${where}`, {
		label: 'Undo',
		run: () => app.mutate(() => api.props.bulk(before))
	});
}

/** Wire (the unwired part of) a prop onto the end of the chain on a port. */
export async function wirePropToPort(show: Show, propId: string, nodeId: string, output: number) {
	const p = show.props.find((x) => x.id === propId);
	if (!p) return;
	const before = $stateSnapshot(p.segments);
	const wired = p.segments.reduce((n, s) => n + s.pixelCount, 0);
	const seg: PropSegment = {
		nodeId,
		output,
		startPixel: pixelsOnOutput(show, nodeId, output),
		pixelCount: Math.max(1, p.pixelCount - wired),
		propOffset: wired,
		reverse: false,
		nullPixels: 0
	};
	const node = show.nodes.find((n) => n.id === nodeId);
	const ok = await app.mutate(async () => {
		await api.props.update(p.id, { ...p, segments: [...p.segments, seg] });
		return true;
	});
	if (ok === undefined) return;
	toasts.success(`${p.name} is plugged into ${node ? portName(node.board, output) : `port ${output}`}`, {
		label: 'Undo',
		run: () => app.mutate(() => api.props.update(p.id, { ...p, segments: before }))
	});
}

/** Light one port for a few seconds so you can find the physical cable. */
export async function flashPort(nodeId: string, output: number, seconds = 6) {
	try {
		await api.testStart({ mode: 'chase', color: '#ffffff', target: { nodeId, output } });
		toasts.push({
			kind: 'info',
			message: `Lighting this port for ${seconds} seconds`,
			action: { label: 'Stop', run: () => api.testStop().catch(() => {}) }
		});
		setTimeout(() => api.testStop().catch(() => {}), seconds * 1000);
	} catch (e) {
		toasts.error('Couldn’t light the port', (e as Error).message);
	}
}

/** Gamma presets shown as friendly "Color correction" steps. */
export const COLOR_CORRECTION = [
	{ gamma: 1, label: 'Off' },
	{ gamma: 1.8, label: 'Light' },
	{ gamma: 2.2, label: 'Normal' },
	{ gamma: 2.8, label: 'Strong' }
];
export function correctionIndex(gamma: number): number {
	let best = 0;
	for (let i = 1; i < COLOR_CORRECTION.length; i++)
		if (Math.abs(COLOR_CORRECTION[i].gamma - gamma) < Math.abs(COLOR_CORRECTION[best].gamma - gamma)) best = i;
	return best;
}
