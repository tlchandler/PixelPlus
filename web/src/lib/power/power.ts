// Power limiter helpers (F12, WS3): labels for limiter groups, live data merge,
// and the extended estimate shape (supply view + simulated limiting).
import type { PowerEstimate, PowerLive, Show } from '$lib/api/types';

/** `GET /power/estimate` with the F12 planning fields (only present when relevant). */
export interface PowerEstimateX extends PowerEstimate {
	perSupply?: {
		supplyId: string;
		name: string;
		volts: number;
		ratedAmps: number;
		peakAmps: number;
		avgAmps: number;
		peakWatts: number;
		status: 'ok' | 'warn' | 'over';
	}[];
	limited?: { groupId: string; nodeId: string; label: string; seconds: number; minScale: number }[];
}

/** `GET /power/live` as the daemon sends it (extra per-node fields; followers
 *  report no currents, so `amps` may be null). */
export type LiveNode = PowerLive['nodes'][number] & {
	mode?: 'off' | 'warn' | 'limit';
	limiting?: boolean;
	minScale?: number;
	online?: boolean;
	secondsLimited?: number;
};

/** People-facing name of a limiter group id. */
export function groupLabel(show: Show | null | undefined, id: string): string {
	const [kind, a = '', b = ''] = id.split(':');
	const receiver = () => show?.receivers.find((r) => r.id === a)?.name ?? 'Receiver';
	switch (kind) {
		case 'port':
			return `${receiver()} · port ${b}`;
		case 'bus':
			return `${receiver()} · main fuse`;
		case 'supply':
			return show?.powerSupplies?.find((s) => s.id === a)?.name ?? 'Power supply';
		case 'global':
			return 'Whole display (power cap)';
		default:
			return id;
	}
}

/** What kind of budget a group id is. */
export function groupKind(id: string): 'port' | 'bus' | 'supply' | 'global' | 'other' {
	const k = id.split(':')[0];
	return k === 'port' || k === 'bus' || k === 'supply' || k === 'global' ? k : 'other';
}

/** Share of the budget in use (0..∞), for bars. */
export function load(amps: number | null | undefined, budget: number): number {
	if (amps == null || !(budget > 0)) return 0;
	return amps / budget;
}

/** The node's name. */
export function nodeName(show: Show | null | undefined, id: string): string {
	return show?.nodes.find((n) => n.id === id)?.name ?? id;
}

/** Nodes limiting right now, with the group that limits most (dashboard badge). */
export function limitingSummary(
	show: Show | null | undefined,
	live: LiveNode[]
): { node: string; group: string; scale: number } | null {
	let best: { node: string; group: string; scale: number } | null = null;
	for (const n of live) {
		for (const g of n.groups) {
			if (g.scale < 0.995 && (!best || g.scale < best.scale))
				best = { node: nodeName(show, n.nodeId), group: groupLabel(show, g.id), scale: g.scale };
		}
	}
	return best;
}

/** Estimated full-white current of a prop (A) at `mApp` mA per pixel. */
export function propMaxAmps(pixels: number, mApp: number | undefined): number {
	return (pixels * (mApp ?? 60)) / 1000;
}
