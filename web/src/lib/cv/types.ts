// WS4 (F6/F7/F9) wire types beyond the frozen $lib/api/types (which they extend).
import type { Id, MappingProposal, MappingRun, Show } from '$lib/api/types';
import type { Plan, Schedule } from './mapcode';

/** A mapping target as the daemon describes it. */
export interface RunTarget {
	k: number;
	nodeId: Id;
	output: number;
	label: string;
	/** Props wired to the output, in string order. */
	propIds?: Id[];
	/** Configured pixels on the output (end of its last segment). */
	configured?: number;
}

/** `POST /mapping/runs` and camera `POST /pixelcount/start`. */
export interface MapStart {
	runId: string;
	kind: 'map' | 'pixelCount';
	startedAt: string;
	plan: Plan;
	schedule: Schedule;
	codebook: number[][];
	targets: RunTarget[];
	/** Pixel count (camera): configured length and probe length. */
	configured?: number;
	maxProbe?: number;
	limited?: boolean;
}

export type ProposalKind = MappingProposal['kind'] | 'info';

/** A proposal as the review screen shows it. */
export interface CvProposal {
	id: string;
	kind: ProposalKind;
	propId?: Id;
	message: string;
	/** Longer explanation under the message. */
	detail?: string;
	data?: Record<string, unknown>;
	/** Ticked by default. */
	selected: boolean;
}

export interface StoredRun extends Omit<MappingRun, 'targets' | 'results'> {
	kind?: 'map' | 'pixelCount';
	targets: RunTarget[];
	results?: {
		detected: { k: number; pixels: [number, number, number, number][] }[];
		proposals: CvProposal[];
		stats?: Record<string, unknown>;
	};
	appliedProposalIds?: string[];
	photo?: boolean;
}

/** `POST /mapping/runs/:id/apply` and `POST /pixelcount/:id/apply`. */
export interface ApplyResult {
	show: Show;
	snapshotId: string;
	applied?: string[];
	message?: string;
}

/** Manual pixel-count step (`/pixelcount/start|answer|undo`). */
export interface CountStep {
	session: string;
	nodeId: Id;
	output: number;
	configured: number;
	maxProbe: number;
	canUndo: boolean;
	limited?: boolean;
	step?: { litUntil: number; ask: string; number: number; maxRemaining: number };
	count?: number;
}

/** Receiver wizard jack identification. */
export interface JackSignal {
	jack: number;
	color: string;
	blinks: number;
}
export interface IdentifyAnswer {
	sessionId: string;
	method: 'identify' | 'sequential';
	round: 'first' | 'final';
	candidates: JackSignal[];
	probeJack?: number | null;
	done?: boolean;
	jack?: number;
}
