// Client-side mirror of the daemon's plan building (services/mapping.rs), used
// for time estimates on the scope picker and by the demo (mock) backend.
import type { Id, Show } from '$lib/api/types';
import { outputLabel as boardLabel } from '$lib/util/boards';
import { pixelBitsFor, PHASE_A, PHASE_B, schedule, type Plan, type Schedule } from './mapcode';
import type { RunTarget } from './types';

export interface Scope {
	all?: boolean;
	nodeId?: Id;
	propIds?: Id[];
}

/** Configured pixels on an output: the end of its last segment. */
export function outputLength(show: Show, nodeId: Id, output: number): number {
	let n = 0;
	for (const p of show.props)
		for (const s of p.segments)
			if (s.nodeId === nodeId && s.output === output) n = Math.max(n, s.startPixel + s.pixelCount);
	return n;
}

export function propsOnOutput(show: Show, nodeId: Id, output: number): Id[] {
	const v: [number, Id][] = [];
	for (const p of show.props)
		for (const s of p.segments) if (s.nodeId === nodeId && s.output === output) v.push([s.startPixel, p.id]);
	v.sort((a, b) => a[0] - b[0]);
	return [...new Set(v.map((x) => x[1]))];
}

export function outputName(show: Show, nodeId: Id, output: number): string {
	const n = show.nodes.find((x) => x.id === nodeId);
	if (!n) return `${nodeId} ${output}`;
	const label = n.outputs.find((o) => o.index === output)?.label || boardLabel(n.board, output);
	return `${n.name} ${label}`;
}

/** Wired outputs in a scope, ordered by (nodeId, output). */
export function scopeTargets(show: Show, scope: Scope): [Id, number][] {
	const set = new Map<string, [Id, number]>();
	for (const p of show.props) {
		if (scope.propIds?.length && !scope.propIds.includes(p.id)) continue;
		for (const s of p.segments) {
			if (scope.nodeId && s.nodeId !== scope.nodeId) continue;
			if (!show.nodes.some((n) => n.id === s.nodeId) || s.output < 1) continue;
			set.set(`${s.nodeId}\u0000${s.output}`, [s.nodeId, s.output]);
		}
	}
	return [...set.values()].sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : a[1] - b[1]));
}

/** `max(configured × 1.25, configured + 64)`, clamped. */
export const probeLen = (configured: number, limit?: number) =>
	Math.min(
		Math.max(Math.floor((configured * 5) / 4), configured + 64),
		limit && limit > 0 ? limit : Infinity,
		4096
	);

export function buildPlan(
	show: Show,
	outs: [Id, number][],
	opts: { bitMs?: number; level?: number; passes?: number; probeExtra?: boolean } = {},
	phases = PHASE_A | PHASE_B
): { plan: Plan; targets: RunTarget[]; schedule: Schedule } {
	const targets: RunTarget[] = outs.map(([nodeId, output], k) => ({
		k,
		nodeId,
		output,
		label: outputName(show, nodeId, output),
		propIds: propsOnOutput(show, nodeId, output),
		configured: outputLength(show, nodeId, output)
	}));
	const planTargets = targets.map((t) => ({
		nodeId: t.nodeId,
		output: t.output,
		maxPixels: opts.probeExtra ? probeLen(Math.max(1, t.configured ?? 1)) : Math.max(1, t.configured ?? 1)
	}));
	const most = Math.max(1, ...planTargets.map((t) => t.maxPixels));
	const plan: Plan = {
		seed: Math.floor(Math.random() * 2 ** 31),
		bitMs: opts.bitMs ?? 200,
		level: Math.min(127, opts.level ?? 77),
		passes: opts.passes ?? 3,
		phases,
		targets: planTargets,
		pixelBits: pixelBitsFor(most),
		startPosMs: 0
	};
	return { plan, targets, schedule: schedule(plan) };
}
