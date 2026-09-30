// HTTP calls for camera mapping (F6), pixel counts (F7) and the receiver
// wizard (F9). WS4 keeps them here so the shared client stays untouched.
import { API_BASE, ApiError, REQUEST_HEADER, request } from '$lib/api/client';
import type { Id, Show } from '$lib/api/types';
import type { ApplyResult, CountStep, CvProposal, IdentifyAnswer, MapStart, StoredRun } from './types';

const post = <R>(p: string, b?: unknown) => request<R>('POST', p, b ?? {});

export interface MapScope {
	all?: boolean;
	nodeId?: Id;
	propIds?: Id[];
}

export const mappingApi = {
	start: (b: { scope: MapScope; bitMs?: number; level?: number; passes?: number; force?: boolean }) =>
		post<MapStart>('/mapping/runs', b),
	frame: (on: boolean, force = false) => post('/mapping/frame', { on, force }),
	stop: (id: string) => post(`/mapping/runs/${id}/stop`),
	list: () => request<StoredRun[]>('GET', '/mapping/runs'),
	get: (id: string) => request<StoredRun>('GET', `/mapping/runs/${id}`),
	remove: (id: string) => request('DELETE', `/mapping/runs/${id}`),
	results: (
		id: string,
		b: {
			detected: { k: number; pixels: [number, number, number, number][] }[];
			proposals: CvProposal[];
			stats?: unknown;
		}
	) => post<StoredRun>(`/mapping/runs/${id}/results`, b),
	apply: (id: string, proposalIds: string[]) =>
		post<ApplyResult>(`/mapping/runs/${id}/apply`, { proposalIds }),
	photoUrl: (id: string) => `${API_BASE}/mapping/runs/${id}/photo`,
	photoAsBackground: (id: string) => post(`/mapping/runs/${id}/photo/background`),
	/** Store the run's photo (a raw JPEG body). */
	async uploadPhoto(id: string, jpeg: Blob): Promise<void> {
		const res = await fetch(`${API_BASE}/mapping/runs/${id}/photo`, {
			method: 'PUT',
			credentials: 'same-origin',
			headers: { [REQUEST_HEADER]: '1', 'Content-Type': 'image/jpeg' },
			body: jpeg
		});
		if (!res.ok) throw new ApiError(res.status, 'photo', 'The photo could not be saved.');
	}
};

export const pixelCountApi = {
	startCamera: (nodeId: Id, output: number, bitMs?: number, force = false) =>
		post<MapStart>('/pixelcount/start', { nodeId, output, method: 'camera', bitMs, force }),
	startManual: (nodeId: Id, output: number, force = false) =>
		post<CountStep>('/pixelcount/start', { nodeId, output, method: 'manual', force }),
	answer: (session: string, seen: boolean) => post<CountStep>(`/pixelcount/${session}/answer`, { seen }),
	undo: (session: string) => post<CountStep>(`/pixelcount/${session}/undo`),
	stop: (id: string) => post(`/pixelcount/${id}/stop`),
	result: (runId: string, count: number, dead: number[], confidence?: number) =>
		post<{ ok: boolean; count: number; configured: number }>(`/pixelcount/${runId}/result`, {
			count,
			dead,
			confidence
		}),
	apply: (id: string, b: { updatePropCount: boolean; count?: number; dead?: number[] }) =>
		post<ApplyResult>(`/pixelcount/${id}/apply`, b)
};

export interface PortPlan {
	port: number;
	propIds: Id[];
	reverse: boolean;
	colorOrder?: string;
}

export const wizardApi = {
	identify: (nodeId: Id, force = false) =>
		post<IdentifyAnswer>('/wizard/receiver/identify-jack', { nodeId, force }),
	pick: (s: string, color: string, blinks: number) =>
		post<IdentifyAnswer>(`/wizard/receiver/${s}/pick`, { color, blinks }),
	probe: (s: string, jack: number) => post(`/wizard/receiver/${s}/probe`, { jack }),
	jack: (s: string, jack: number) => post(`/wizard/receiver/${s}/jack`, { jack }),
	light: (s: string, port: number, pattern: 'solid' | 'chase' | 'red' | 'green' | 'blue' | 'off') =>
		post(`/wizard/receiver/${s}/port/${port}/light`, { pattern }),
	colorOrder: (s: string, port: number, red: string, green: string) =>
		post<{ colorOrder: string; configured: string; changed: boolean }>(`/wizard/receiver/${s}/color-order`, {
			port,
			red,
			green
		}),
	finish: (
		s: string,
		b: {
			receiver: { name: string; kind: string; location?: string; fuseAmps?: number; mainFuseAmps?: number };
			ports: PortPlan[];
		}
	) => post<{ show: Show; receiver: { id: Id }; snapshotId: string }>(`/wizard/receiver/${s}/finish`, b),
	cancel: (s: string) => post(`/wizard/receiver/${s}/cancel`)
};
